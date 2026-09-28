//! Linux GPU data as text: DRM fdinfo (the kernel's drm-usage-stats), sysfs `uevent`,
//! `pci.ids`, and NVML-shaped per-process samples, turned into the same `Raw` the
//! Windows counters produce. Pure functions over strings and structs, compiled on every
//! platform so the fixture tests prove them anywhere; `sys/linux.rs` does the reading.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use super::sys::Raw;
use std::collections::{BTreeMap, HashMap};

/// One `/proc/<pid>/fdinfo/<fd>` of a DRM file (render node or card).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FdInfo {
    pub driver: String,
    /// "0000:03:00.0"
    pub pdev: Option<String>,
    pub client_id: Option<u64>,
    /// engine name as the driver spells it -> busy time, ns
    pub engines: BTreeMap<String, u64>,
    /// engine -> number of instances (`drm-engine-capacity-*`, default 1)
    pub capacity: BTreeMap<String, u64>,
    /// engine -> (busy cycles, total cycles) (`drm-cycles-*`, `drm-total-cycles-*`: xe)
    pub cycles: BTreeMap<String, (u64, u64)>,
    /// device-local memory (vram*, local*), bytes
    pub dedicated: u64,
    /// system memory mapped for the GPU (gtt, system*), bytes
    pub shared: u64,
}

/// "123 KiB" / "5 MiB" / "1 GiB" / "4096" -> bytes.
fn bytes(v: &str) -> Option<u64> {
    let mut it = v.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    let mul = match it.next().unwrap_or("") {
        "" | "B" => 1,
        "KiB" | "kB" | "KB" => 1 << 10,
        "MiB" | "MB" => 1 << 20,
        "GiB" | "GB" => 1 << 30,
        _ => return None,
    };
    Some(n * mul)
}

/// Some(true): device memory; Some(false): system memory the GPU maps; None: neither
/// (amdgpu's "cpu" region, i915's stolen memory).
fn region_is_dedicated(region: &str) -> Option<bool> {
    let r = region.to_ascii_lowercase();
    if r.starts_with("vram") || r.starts_with("local") {
        Some(true)
    } else if r == "gtt" || r.starts_with("system") {
        Some(false)
    } else {
        None
    }
}

/// Parses one fdinfo file. None unless it is a DRM file (`drm-driver:` present).
pub fn parse_fdinfo(text: &str) -> Option<FdInfo> {
    let mut f = FdInfo::default();
    // region -> (rank, bytes): resident beats the legacy drm-memory beats total
    let mut mem: HashMap<String, (u8, u64)> = HashMap::new();
    for l in text.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k == "drm-driver" {
            f.driver = v.to_string();
        } else if k == "drm-pdev" {
            f.pdev = Some(v.to_string());
        } else if k == "drm-client-id" {
            f.client_id = v.parse().ok();
        } else if let Some(e) = k.strip_prefix("drm-engine-capacity-") {
            if let Ok(n) = v.parse() {
                f.capacity.insert(e.to_string(), n);
            }
        } else if let Some(e) = k.strip_prefix("drm-engine-") {
            if let Some(ns) = v.split_whitespace().next().and_then(|x| x.parse().ok()) {
                f.engines.insert(e.to_string(), ns);
            }
        } else if let Some(e) = k.strip_prefix("drm-total-cycles-") {
            f.cycles.entry(e.to_string()).or_default().1 = v.parse().unwrap_or(0);
        } else if let Some(e) = k.strip_prefix("drm-cycles-") {
            f.cycles.entry(e.to_string()).or_default().0 = v.parse().unwrap_or(0);
        } else {
            for (rank, pre) in [(0u8, "drm-resident-"), (1, "drm-memory-"), (2, "drm-total-")] {
                if let Some(region) = k.strip_prefix(pre) {
                    if let Some(b) = bytes(v) {
                        let e = mem.entry(region.to_string()).or_insert((u8::MAX, 0));
                        if rank < e.0 {
                            *e = (rank, b);
                        }
                    }
                }
            }
        }
    }
    if f.driver.is_empty() {
        return None;
    }
    for (region, (_, b)) in mem {
        match region_is_dedicated(&region) {
            Some(true) => f.dedicated += b,
            Some(false) => f.shared += b,
            None => {}
        }
    }
    Some(f)
}

/// Driver engine names -> the engine types gputree shows (the Windows names, so the
/// tag rules' videoencode / videodecode / compute / 3d hints apply unchanged).
pub fn engine_type(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    let base = n.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches(['_', '-']);
    match base {
        "gfx" | "render" | "rcs" | "3d" | "gr" | "fragment" | "vertex-tiler" | "graphics" => "3d".into(),
        "compute" | "ccs" | "comp" => "compute".into(),
        "dec" | "video" | "vcs" | "vcn" | "jpeg" | "nvdec" | "uvd" => "videodecode".into(),
        "enc" | "nvenc" | "vce" => "videoencode".into(),
        "video-enhance" | "vecs" => "videoprocessing".into(),
        "copy" | "bcs" | "dma" | "sdma" | "ce" => "copy".into(),
        _ => base.to_string(),
    }
}

/// A stable adapter key from a PCI address: "0000:03:00.0" and NVML's
/// "00000000:03:00.0" give the same key. Anything else is hashed.
pub fn pci_key(addr: &str) -> u64 {
    let parse = || -> Option<u64> {
        let (dom, rest) = addr.trim().rsplit_once(':').and_then(|(a, df)| {
            let (d, b) = a.split_once(':')?;
            Some((u64::from_str_radix(d, 16).ok()?, (b, df)))
        })?;
        let bus = u64::from_str_radix(rest.0, 16).ok()?;
        let (dev, func) = rest.1.split_once('.')?;
        let dev = u64::from_str_radix(dev, 16).ok()?;
        let func = u64::from_str_radix(func, 16).ok()?;
        Some((1 << 40) | (dom << 16) | (bus << 8) | (dev << 3) | func)
    };
    parse().unwrap_or_else(|| {
        // FNV-1a
        addr.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
    })
}

/// A DRM client: (PCI address, drm-client-id). A client whose fd was dup'ed or
/// inherited shows up in several fds and pids; it is counted once.
pub type ClientKey = (String, u64);

#[derive(Clone, Debug, Default)]
pub struct DrmSample {
    /// client -> (the lowest pid holding it, its fdinfo)
    pub clients: HashMap<ClientKey, (u32, FdInfo)>,
}

impl DrmSample {
    /// Adds one fd's fdinfo seen in `pid`. `pdev_hint` is the PCI address of the device
    /// node the fd points at, for kernels that do not print drm-pdev.
    pub fn add(&mut self, pid: u32, pdev_hint: Option<&str>, info: FdInfo) {
        let pdev = info.pdev.clone().or_else(|| pdev_hint.map(str::to_string)).unwrap_or_else(|| "?".into());
        // without a client id the fds of one process are taken to be one client
        let id = info.client_id.unwrap_or((1 << 63) | pid as u64);
        match self.clients.get(&(pdev.clone(), id)) {
            Some((p, _)) if *p <= pid => {}
            _ => {
                self.clients.insert((pdev, id), (pid, info));
            }
        }
    }
}

/// Two samples `dt_ns` apart -> per (adapter, pid) memory and engine utilisation.
/// Utilisation is the busy-time delta over the window, divided by the engine's
/// capacity, summed per engine type over a process's clients; memory is summed.
pub fn to_raw(prev: Option<&DrmSample>, cur: &DrmSample, dt_ns: f64) -> Raw {
    let mut raw = Raw { util_ready: prev.is_some() && dt_ns > 0.0, ..Default::default() };
    for (key, (pid, info)) in &cur.clients {
        let luid = pci_key(&key.0);
        let r = raw.procs.entry((luid, *pid)).or_default();
        r.ded += info.dedicated as f64;
        r.shr += info.shared as f64;
        let m = raw.adapter_mem.entry(luid).or_default();
        m.0 += info.dedicated as f64;
        m.1 += info.shared as f64;
        if !raw.util_ready {
            continue;
        }
        let Some((_, before)) = prev.and_then(|p| p.clients.get(key)) else { continue };
        let cap = |e: &str| info.capacity.get(e).copied().unwrap_or(1).max(1) as f64;
        for (e, ns) in &info.engines {
            let Some(b) = before.engines.get(e) else { continue };
            let pct = (ns.saturating_sub(*b) as f64 / dt_ns * 100.0 / cap(e)).clamp(0.0, 100.0);
            *r.eng.entry(engine_type(e)).or_insert(0.0) += pct;
        }
        for (e, (c, t)) in &info.cycles {
            let Some((c0, t0)) = before.cycles.get(e) else { continue };
            let dt = t.saturating_sub(*t0);
            if dt == 0 {
                continue;
            }
            let pct = (c.saturating_sub(*c0) as f64 / dt as f64 * 100.0 / cap(e)).clamp(0.0, 100.0);
            *r.eng.entry(engine_type(e)).or_insert(0.0) += pct;
        }
    }
    raw
}

// ---------------------------------------------------------------- NVIDIA per process

/// A process NVML lists on a device (nvmlProcessInfo_t, or nvidia-smi's compute apps).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NvProc {
    pub pid: u32,
    /// None where the driver cannot say (WDDM / WSL)
    pub used: Option<u64>,
    pub graphics: bool,
}

/// One nvmlProcessUtilizationSample_t.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NvUtil {
    pub pid: u32,
    /// µs
    pub ts: u64,
    pub sm: u32,
    pub enc: u32,
    pub dec: u32,
}

/// Adds NVML's per-process view of one device (adapter key `key`) to `raw`: memory as
/// reported, and the samples' average SM / encoder / decoder load. SM time is "3d"
/// for graphics clients and "compute" for CUDA ones, like the Windows engines.
pub fn nv_into_raw(raw: &mut Raw, key: u64, procs: &[NvProc], util: &[NvUtil]) {
    for p in procs {
        let r = raw.procs.entry((key, p.pid)).or_default();
        if let Some(u) = p.used {
            r.ded = r.ded.max(u as f64);
        }
    }
    if !raw.util_ready {
        return;
    }
    let mut by_pid: BTreeMap<u32, (f64, f64, f64, f64)> = BTreeMap::new();
    for s in util {
        let e = by_pid.entry(s.pid).or_default();
        e.0 += s.sm as f64;
        e.1 += s.enc as f64;
        e.2 += s.dec as f64;
        e.3 += 1.0;
    }
    for (pid, (sm, enc, dec, n)) in by_pid {
        let graphics = procs.iter().any(|p| p.pid == pid && p.graphics);
        let r = raw.procs.entry((key, pid)).or_default();
        for (k, v) in [(if graphics { "3d" } else { "compute" }, sm / n), ("videoencode", enc / n), ("videodecode", dec / n)] {
            if v > 0.0 {
                let e = r.eng.entry(k.to_string()).or_insert(0.0);
                *e = e.max(v.min(100.0));
            }
        }
    }
}

/// `nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits`.
pub fn parse_compute_apps(csv: &str) -> Vec<NvProc> {
    csv.lines()
        .filter_map(|l| {
            let mut f = l.split(',').map(str::trim);
            let pid = f.next()?.parse().ok()?;
            let used = f.next().and_then(|m| m.parse::<u64>().ok()).map(|mib| mib << 20);
            Some(NvProc { pid, used, graphics: false })
        })
        .collect()
}

// ---------------------------------------------------------------- adapter names

/// `KEY=value` lines (sysfs `uevent`).
pub fn parse_uevent(text: &str) -> HashMap<String, String> {
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}

/// (vendor name, device name) from `pci.ids` text.
pub fn pci_ids_lookup(ids: &str, vendor: u16, device: u16) -> Option<(String, Option<String>)> {
    let (v, d) = (format!("{vendor:04x}"), format!("{device:04x}"));
    let mut vendor_name = None;
    for l in ids.lines() {
        if l.starts_with('#') || l.is_empty() {
            continue;
        }
        match &vendor_name {
            None => {
                if !l.starts_with('\t') && l.len() > 6 && l[..4].eq_ignore_ascii_case(&v) {
                    vendor_name = Some(l[4..].trim().to_string());
                }
            }
            Some(vn) => {
                if !l.starts_with('\t') {
                    return Some((vn.clone(), None)); // next vendor: device not listed
                }
                if !l.starts_with("\t\t") && l.len() > 6 && l[1..5].eq_ignore_ascii_case(&d) {
                    return Some((vn.clone(), Some(l[5..].trim().to_string())));
                }
            }
        }
    }
    vendor_name.map(|v| (v, None))
}

pub fn vendor_short(vendor: u16) -> Option<&'static str> {
    match vendor {
        0x10de => Some("NVIDIA"),
        0x1002 | 0x1022 => Some("AMD"),
        0x8086 => Some("Intel"),
        0x1af4 => Some("Virtio"),
        0x15ad => Some("VMware"),
        0x1234 => Some("QEMU"),
        _ => None,
    }
}

/// A Task-Manager-like adapter name: "NVIDIA GeForce RTX 3060", "AMD Radeon RX 6800",
/// "Intel Iris Xe Graphics". `product` is amdgpu's `product_name` when present.
pub fn adapter_name(vendor: u16, device: u16, product: Option<&str>, ids: Option<(String, Option<String>)>, driver: &str) -> String {
    let short = vendor_short(vendor).map(str::to_string);
    let vname = short.clone().or_else(|| ids.as_ref().map(|i| i.0.split_whitespace().next().unwrap_or("").to_string()));
    let prefix = |s: &str| match &vname {
        Some(v) if !s.to_ascii_lowercase().starts_with(&v.to_ascii_lowercase()) => format!("{v} {s}"),
        _ => s.to_string(),
    };
    if let Some(p) = product.map(str::trim).filter(|p| !p.is_empty()) {
        return prefix(p);
    }
    if let Some(dn) = ids.and_then(|i| i.1) {
        // "GA106 [GeForce RTX 3060]" -> the marketing name in brackets
        let inner = match (dn.find('['), dn.rfind(']')) {
            (Some(a), Some(b)) if b > a => dn[a + 1..b].to_string(),
            _ => dn,
        };
        return prefix(&inner);
    }
    format!("{} GPU {device:04x} ({driver})", vname.unwrap_or_else(|| format!("{vendor:04x}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn amdgpu_fdinfo() {
        let a = parse_fdinfo(&fx("drm/amdgpu-a.fdinfo")).unwrap();
        assert_eq!(a.driver, "amdgpu");
        assert_eq!(a.pdev.as_deref(), Some("0000:03:00.0"));
        assert_eq!(a.client_id, Some(17));
        // drm-resident-vram wins over the legacy drm-memory-vram and drm-total-vram
        assert_eq!(a.dedicated, 1_000 << 20);
        assert_eq!(a.shared, 20_480 << 10);
        assert_eq!(a.engines.get("gfx"), Some(&1_000_000_000));
        assert_eq!(engine_type("gfx"), "3d");
        assert_eq!(engine_type("enc_1"), "videoencode");
        assert_eq!(engine_type("dec"), "videodecode");
    }

    #[test]
    fn not_drm() {
        assert_eq!(parse_fdinfo(&fx("drm/regular-file.fdinfo")), None);
    }

    #[test]
    fn utilisation_from_two_samples_with_dedup() {
        // pid 3100 (a game) and its forked helper 3101 hold the same client (dup'ed fd);
        // the Intel iGPU client is on another adapter with 2 video engines
        let mut s0 = DrmSample::default();
        let mut s1 = DrmSample::default();
        for (s, sfx) in [(&mut s0, "a"), (&mut s1, "b")] {
            let amd = parse_fdinfo(&fx(&format!("drm/amdgpu-{sfx}.fdinfo"))).unwrap();
            s.add(3101, None, amd.clone());
            s.add(3100, None, amd.clone());
            s.add(3100, None, amd);
            s.add(2200, Some("0000:00:02.0"), parse_fdinfo(&fx(&format!("drm/i915-{sfx}.fdinfo"))).unwrap());
        }
        assert_eq!(s1.clients.len(), 2);
        let first = to_raw(None, &s0, 0.0);
        assert!(!first.util_ready);
        let raw = to_raw(Some(&s0), &s1, 500e6);
        assert!(raw.util_ready);
        let amd = pci_key("0000:03:00.0");
        let intel = pci_key("0000:00:02.0");
        assert_eq!(raw.procs.len(), 2, "{:?}", raw.procs.keys());
        let g = &raw.procs[&(amd, 3100)];
        assert!((g.eng["3d"] - 60.0).abs() < 1e-6, "{:?}", g.eng);
        assert!((g.eng["videoencode"] - 4.0).abs() < 1e-6, "{:?}", g.eng);
        assert_eq!(g.ded, (1_000u64 << 20) as f64);
        let i = &raw.procs[&(intel, 2200)];
        assert!((i.eng["3d"] - 50.0).abs() < 1e-6, "{:?}", i.eng);
        // 200 ms busy over 500 ms on a class with 2 engines
        assert!((i.eng["videodecode"] - 20.0).abs() < 1e-6, "{:?}", i.eng);
        assert_eq!(i.shr, (80u64 << 20) as f64);
        assert_eq!(i.ded, 0.0);
        assert_eq!(raw.adapter_mem[&intel], (0.0, (80u64 << 20) as f64));
    }

    #[test]
    fn xe_cycles() {
        let a = parse_fdinfo(&fx("drm/xe-a.fdinfo")).unwrap();
        let b = parse_fdinfo(&fx("drm/xe-b.fdinfo")).unwrap();
        assert_eq!(a.dedicated, 500 << 20);
        assert_eq!(a.shared, 10 << 20);
        let (mut s0, mut s1) = (DrmSample::default(), DrmSample::default());
        s0.add(7, None, a);
        s1.add(7, None, b);
        let raw = to_raw(Some(&s0), &s1, 250e6);
        let p = &raw.procs[&(pci_key("0000:03:00.0"), 7)];
        // rcs: 250k of 1M cycles; ccs: 400k of 1M over 4 engines
        assert!((p.eng["3d"] - 25.0).abs() < 1e-6, "{:?}", p.eng);
        assert!((p.eng["compute"] - 10.0).abs() < 1e-6, "{:?}", p.eng);
    }

    #[test]
    fn pci_keys_match_between_sysfs_and_nvml() {
        assert_eq!(pci_key("0000:01:00.0"), pci_key("00000000:01:00.0"));
        assert_ne!(pci_key("0000:01:00.0"), pci_key("0000:02:00.0"));
        assert_ne!(pci_key("?"), 0);
    }

    fn nvml_fixture(text: &str) -> (Vec<NvProc>, Vec<NvUtil>) {
        let (mut procs, mut util) = (vec![], vec![]);
        for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let f: Vec<&str> = l.split_whitespace().collect();
            let n = |i: usize| f[i].parse::<u64>().unwrap();
            match f[0] {
                "compute" | "graphics" => procs.push(NvProc {
                    pid: n(1) as u32,
                    used: (f[2] != "n/a").then(|| n(2)),
                    graphics: f[0] == "graphics",
                }),
                "util" => util.push(NvUtil { pid: n(1) as u32, ts: n(2), sm: n(3) as u32, enc: n(5) as u32, dec: n(6) as u32 }),
                _ => panic!("bad fixture line {l}"),
            }
        }
        (procs, util)
    }

    #[test]
    fn nvml_processes() {
        let (procs, util) = nvml_fixture(&fx("nvml/device0.txt"));
        let key = pci_key("00000000:01:00.0");
        let mut raw = Raw { util_ready: true, ..Default::default() };
        nv_into_raw(&mut raw, key, &procs, &util);
        let train = &raw.procs[&(key, 1194)];
        assert_eq!(train.ded, 2_952_790_016.0);
        assert!((train.eng["compute"] - 92.0).abs() < 1e-6, "{:?}", train.eng);
        let obs = &raw.procs[&(key, 2210)];
        assert_eq!(obs.eng["3d"], 4.0);
        assert_eq!(obs.eng["videoencode"], 12.0);
        // listed without a memory figure (WDDM-style "n/a") but still present
        assert_eq!(raw.procs[&(key, 2300)].ded, 0.0);
        // before util is ready only memory is taken
        let mut r0 = Raw::default();
        nv_into_raw(&mut r0, key, &procs, &util);
        assert!(r0.procs[&(key, 1194)].eng.is_empty());
    }

    #[test]
    fn nvidia_smi_compute_apps() {
        let v = parse_compute_apps(&fx("nvml/compute-apps.csv"));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], NvProc { pid: 1194, used: Some(2816 << 20), graphics: false });
        assert_eq!(v[1].used, None);
    }

    #[test]
    fn names_from_sysfs_and_pci_ids() {
        let ue = parse_uevent(&fx("sys/uevent-amdgpu"));
        assert_eq!(ue["DRIVER"], "amdgpu");
        assert_eq!(ue["PCI_SLOT_NAME"], "0000:03:00.0");
        let ids = fx("sys/pci.ids");
        let nv = pci_ids_lookup(&ids, 0x10de, 0x2504);
        assert_eq!(adapter_name(0x10de, 0x2504, None, nv, "nvidia"), "NVIDIA GeForce RTX 3060 Lite Hash Rate");
        let amd = pci_ids_lookup(&ids, 0x1002, 0x73bf);
        assert_eq!(adapter_name(0x1002, 0x73bf, None, amd.clone(), "amdgpu"), "AMD Radeon RX 6800/6800 XT / 6900 XT");
        assert_eq!(adapter_name(0x1002, 0x73bf, Some("AMD Radeon RX 6800 XT"), amd, "amdgpu"), "AMD Radeon RX 6800 XT");
        let intel = pci_ids_lookup(&ids, 0x8086, 0x9a49);
        assert_eq!(adapter_name(0x8086, 0x9a49, None, intel, "i915"), "Intel Iris Xe Graphics");
        // a device the file does not list, and a vendor it does not list
        assert_eq!(adapter_name(0x8086, 0x1234, None, pci_ids_lookup(&ids, 0x8086, 0x1234), "xe"), "Intel GPU 1234 (xe)");
        assert_eq!(adapter_name(0xabcd, 0x0001, None, pci_ids_lookup(&ids, 0xabcd, 1), "x"), "abcd GPU 0001 (x)");
    }
}
