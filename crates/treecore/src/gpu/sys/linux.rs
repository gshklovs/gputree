//! The Linux backend.
//!
//! - **DRM fdinfo** (the generic path: amdgpu, i915, xe, nouveau, panfrost, ...): every
//!   fd of every process that points at `/dev/dri/card*` or `renderD*`, its
//!   `/proc/<pid>/fdinfo/<fd>` read twice; engine busy time over the window is the
//!   utilisation, clients are de-duplicated by (drm-pdev, drm-client-id).
//! - **NVML** (the proprietary NVIDIA driver, which has no fdinfo engines): per-process
//!   used memory and utilisation samples, adapter memory, from `libnvidia-ml.so.1`
//!   loaded at run time. Only asked when an NVIDIA card is awake (asking wakes a
//!   runtime-suspended laptop GPU), or in WSL.
//! - **sysfs** for the adapters: `/sys/class/drm/card*/device` (vendor, device, uevent,
//!   amdgpu's product_name and mem_info_*), names from NVML or `pci.ids`.
//! - **WSL** has no DRM: the GPU is `/dev/dxg`. The adapter comes from NVML (else
//!   nvidia-smi), and one `/dev/dxg` row carries the whole GPU's numbers; the Linux
//!   processes holding /dev/dxg hang under it (see `gpu::collect::wsl_gpu_procs`).
//!
//! Other users' fds are only readable as root, so without root the tree shows your own
//! processes (as nvtop and intel_gpu_top do). Nothing here writes or signals anything.

use super::{Adapter, Raw};
use crate::gpu::drm::{self, DrmSample, NvProc, NvUtil};
use crate::gpu::model::DXG_HOST;
use crate::gpu::nvidia;
use crate::linux;
use nvml_wrapper::enums::device::UsedGpuMemory;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- sysfs

#[derive(Clone, Debug)]
struct Card {
    pdev: String,
    vendor: u16,
    device: u16,
    driver: String,
    /// /sys/class/drm/cardN/device, resolved
    dev: PathBuf,
}

impl Card {
    fn key(&self) -> u64 {
        drm::pci_key(&self.pdev)
    }
    fn file(&self, f: &str) -> Option<String> {
        linux::read(self.dev.join(f)).map(|s| s.trim().to_string())
    }
    fn num(&self, f: &str) -> Option<u64> {
        self.file(f)?.parse().ok()
    }
    /// runtime PM: a suspended dGPU stays asleep unless something asks it
    fn awake(&self) -> bool {
        self.file("power/runtime_status").is_none_or(|s| s != "suspended")
    }
}

fn hex16(s: Option<String>) -> u16 {
    s.and_then(|s| u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()).unwrap_or(0)
}

/// GPUs with a DRM node, one per device (card and render node share it), plus
/// device-node path -> PCI address for fds on kernels that do not print drm-pdev.
struct Drm {
    cards: Vec<Card>,
    node_pdev: HashMap<String, String>,
}

static DRM: LazyLock<Drm> = LazyLock::new(|| {
    let mut cards: Vec<Card> = vec![];
    let mut node_pdev = HashMap::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/drm") else { return Drm { cards, node_pdev } };
    let mut names: Vec<String> = rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    names.sort();
    for n in names {
        let is_node = n.strip_prefix("card").or_else(|| n.strip_prefix("renderD")).is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
        if !is_node {
            continue; // connectors: card0-DP-1, ...
        }
        let Ok(dev) = std::fs::canonicalize(format!("/sys/class/drm/{n}/device")) else { continue };
        let pdev = dev.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let ue = drm::parse_uevent(&linux::read(dev.join("uevent")).unwrap_or_default());
        let driver = ue.get("DRIVER").cloned().unwrap_or_default();
        if matches!(driver.as_str(), "vgem" | "vkms") {
            continue; // virtual, no hardware behind them
        }
        node_pdev.insert(format!("/dev/dri/{n}"), pdev.clone());
        if cards.iter().any(|c| c.pdev == pdev) {
            continue;
        }
        cards.push(Card {
            vendor: hex16(linux::read(dev.join("vendor"))),
            device: hex16(linux::read(dev.join("device"))),
            pdev,
            driver,
            dev,
        });
    }
    Drm { cards, node_pdev }
});

/// WSL: the GPU is /dev/dxg and there is no real DRM device.
fn wsl_gpu() -> bool {
    linux::is_wsl() && std::path::Path::new("/dev/dxg").exists() && DRM.cards.is_empty()
}

/// NVML is worth asking: an NVIDIA card is awake, or WSL (whose NVML is the host's).
fn want_nvml() -> bool {
    wsl_gpu() || DRM.cards.iter().any(|c| c.vendor == 0x10de && c.awake())
}

fn pci_ids() -> Option<String> {
    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"].iter().find_map(|p| linux::read(p))
}

fn mem_total() -> f64 {
    crate::procfs::parse_meminfo(&linux::read("/proc/meminfo").unwrap_or_default()).0 as f64
}

// ---------------------------------------------------------------- NVIDIA

/// One NVIDIA device as NVML (or nvidia-smi) sees it right now.
#[derive(Clone, Debug, Default)]
struct NvDev {
    key: u64,
    name: String,
    used: f64,
    total: f64,
    util: f64,
    procs: Vec<NvProc>,
    samples: Vec<NvUtil>,
}

fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

fn nv_devices(since_us: Option<u64>, per_process: bool) -> Vec<NvDev> {
    if let Some(nvml) = nvidia::nvml() {
        let n = nvml.device_count().unwrap_or(0);
        let mut out = vec![];
        for i in 0..n {
            let Ok(d) = nvml.device_by_index(i) else { continue };
            let Ok(pci) = d.pci_info() else { continue };
            let mem = d.memory_info().ok();
            let mut dev = NvDev {
                key: drm::pci_key(&pci.bus_id),
                name: d.name().unwrap_or_else(|_| "NVIDIA GPU".into()),
                used: mem.as_ref().map(|m| m.used as f64).unwrap_or(0.0),
                total: mem.as_ref().map(|m| m.total as f64).unwrap_or(0.0),
                util: d.utilization_rates().map(|u| u.gpu as f64).unwrap_or(0.0),
                ..Default::default()
            };
            if per_process {
                let used = |u: UsedGpuMemory| match u {
                    UsedGpuMemory::Used(b) => Some(b),
                    UsedGpuMemory::Unavailable => None,
                };
                for p in d.running_compute_processes().unwrap_or_default() {
                    dev.procs.push(NvProc { pid: p.pid, used: used(p.used_gpu_memory), graphics: false });
                }
                for p in d.running_graphics_processes().unwrap_or_default() {
                    match dev.procs.iter_mut().find(|q| q.pid == p.pid) {
                        Some(q) => q.graphics = true,
                        None => dev.procs.push(NvProc { pid: p.pid, used: used(p.used_gpu_memory), graphics: true }),
                    }
                }
                if let Some(ts) = since_us {
                    dev.samples = d
                        .process_utilization_stats(ts)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|s| NvUtil { pid: s.pid, ts: s.timestamp, sm: s.sm_util, enc: s.enc_util, dec: s.dec_util })
                        .collect();
                }
            }
            out.push(dev);
        }
        return out;
    }
    // no NVML library: nvidia-smi, if it runs at all
    let Some(text) = nvidia::smi(&["--query-gpu=pci.bus_id,name,memory.used,memory.total,utilization.gpu", "--format=csv,noheader,nounits"]) else {
        return vec![];
    };
    let apps = if per_process {
        nvidia::smi(&["--query-compute-apps=gpu_bus_id,pid,used_memory", "--format=csv,noheader,nounits"]).unwrap_or_default()
    } else {
        String::new()
    };
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 5 {
                return None;
            }
            let mib = |s: &str| s.parse::<f64>().unwrap_or(0.0) * 1048576.0;
            let bus = f[0].to_string();
            let procs = apps
                .lines()
                .filter_map(|a| a.split_once(','))
                .filter(|(b, _)| b.trim() == bus)
                .flat_map(|(_, rest)| drm::parse_compute_apps(rest))
                .collect();
            Some(NvDev {
                key: drm::pci_key(&bus),
                name: f[1].to_string(),
                used: mib(f[2]),
                total: mib(f[3]),
                util: f[4].parse().unwrap_or(0.0),
                procs,
                samples: vec![],
            })
        })
        .collect()
}

// ---------------------------------------------------------------- counters

struct Sample {
    at: Instant,
    wall_us: u64,
    drm: DrmSample,
    nv: Vec<NvDev>,
    /// amdgpu: adapter key -> (vram used, gtt used) from sysfs
    sysfs_mem: HashMap<u64, (f64, f64)>,
}

/// Same shape as the Windows counters: `open` takes the first sample, `collect` the
/// next, and `read` turns the latest two into per-process numbers.
pub struct Counters {
    prev: Option<Sample>,
    cur: Option<Sample>,
    collections: u32,
    wsl: bool,
}

/// Every DRM fd in /proc: (pid, device-node PCI address, fdinfo).
fn scan_fdinfo() -> DrmSample {
    let mut s = DrmSample::default();
    for pid in linux::pids() {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for e in rd.flatten() {
            let Ok(target) = std::fs::read_link(e.path()) else { continue };
            let t = target.to_string_lossy();
            if !t.starts_with("/dev/dri/") {
                continue;
            }
            let fd = e.file_name();
            let Some(text) = linux::read(format!("/proc/{pid}/fdinfo/{}", fd.to_string_lossy())) else { continue };
            if let Some(info) = drm::parse_fdinfo(&text) {
                s.add(pid, DRM.node_pdev.get(t.as_ref()).map(String::as_str), info);
            }
        }
    }
    s
}

impl Counters {
    pub fn open() -> Option<Counters> {
        let mut c = Counters { prev: None, cur: None, collections: 0, wsl: wsl_gpu() };
        c.collect();
        Some(c)
    }

    pub fn collections(&self) -> u32 {
        self.collections
    }

    pub fn collect(&mut self) {
        let since = self.cur.as_ref().map(|c| c.wall_us);
        let wall_us = now_us();
        let nv = if want_nvml() { nv_devices(since, !self.wsl) } else { vec![] };
        let drm = if self.wsl { DrmSample::default() } else { scan_fdinfo() };
        let mut sysfs_mem = HashMap::new();
        for c in DRM.cards.iter().filter(|c| c.driver == "amdgpu") {
            if let Some(v) = c.num("mem_info_vram_used") {
                sysfs_mem.insert(c.key(), (v as f64, c.num("mem_info_gtt_used").unwrap_or(0) as f64));
            }
        }
        let s = Sample { at: Instant::now(), wall_us, drm, nv, sysfs_mem };
        self.prev = self.cur.replace(s);
        self.collections += 1;
    }

    pub fn read(&self) -> Raw {
        let Some(cur) = &self.cur else { return Raw::default() };
        let dt_ns = self.prev.as_ref().map(|p| cur.at.duration_since(p.at).as_nanos() as f64).unwrap_or(0.0);
        let mut raw = drm::to_raw(self.prev.as_ref().map(|p| &p.drm), &cur.drm, dt_ns);
        for (k, v) in &cur.sysfs_mem {
            raw.adapter_mem.insert(*k, *v);
        }
        for d in &cur.nv {
            if self.wsl {
                // no per-process view: one row for the whole GPU, the Linux processes
                // holding /dev/dxg go under it
                let r = raw.procs.entry((d.key, 0)).or_default();
                r.ded = d.used;
                if raw.util_ready {
                    // NVML's figure is a short recent average; the two ends of the
                    // window together describe it better than either alone
                    let before = self.prev.as_ref().and_then(|p| p.nv.iter().find(|x| x.key == d.key)).map(|x| x.util);
                    r.eng.insert("gpu".into(), before.map_or(d.util, |b| (b + d.util) / 2.0));
                }
            } else {
                drm::nv_into_raw(&mut raw, d.key, &d.procs, &d.samples);
            }
            raw.adapter_mem.insert(d.key, (d.used, 0.0));
        }
        raw
    }
}

// ---------------------------------------------------------------- adapters, processes

/// adapter key -> adapter. NVIDIA names come from NVML when it is awake, everything
/// else from amdgpu's product_name or pci.ids. An integrated GPU (no VRAM of its own)
/// is measured against half of RAM, like Windows' shared GPU memory.
pub fn adapters() -> HashMap<u64, Adapter> {
    let mut map = HashMap::new();
    let nv = if want_nvml() { nv_devices(None, false) } else { vec![] };
    let ids = if DRM.cards.iter().any(|c| !(c.vendor == 0x10de && nv.iter().any(|d| d.key == c.key()))) { pci_ids() } else { None };
    let half_ram = mem_total() / 2.0;
    for c in &DRM.cards {
        let key = c.key();
        if let Some(d) = nv.iter().find(|d| d.key == key) {
            map.insert(key, Adapter { name: d.name.clone(), total: d.total, shared: half_ram });
            continue;
        }
        let product = c.file("product_name");
        let lookup = ids.as_deref().and_then(|t| drm::pci_ids_lookup(t, c.vendor, c.device));
        let name = drm::adapter_name(c.vendor, c.device, product.as_deref(), lookup, &c.driver);
        let total = c.num("mem_info_vram_total").unwrap_or(0) as f64;
        let shared = c.num("mem_info_gtt_total").map(|v| v as f64).unwrap_or(half_ram);
        map.insert(key, Adapter { name, total, shared });
    }
    for d in nv {
        map.entry(d.key).or_insert(Adapter { name: d.name, total: d.total, shared: half_ram });
    }
    map
}

/// NPUs (/dev/accel) are not read yet.
pub fn npu_name() -> Option<String> {
    None
}

/// pid -> display name for every process; in WSL, pid 0 is the /dev/dxg row.
pub fn process_names() -> HashMap<u32, String> {
    let mut m: HashMap<u32, String> = linux::pids().into_iter().filter_map(|p| Some((p, linux::ident_of(p)?.name))).collect();
    if wsl_gpu() {
        m.insert(0, DXG_HOST.to_string());
    }
    m
}

/// The executable (readable for your own processes, or as root).
pub fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    std::fs::read_link(format!("/proc/{pid}/exe")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// The command line, which the tags read (python + train, steamapps/common, ...).
pub fn process_cmd(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    linux::ident_of(pid).map(|i| i.cmd).filter(|c| !c.is_empty())
}

pub fn local_time() -> String {
    linux::local_time()
}
