//! The Windows backend. Thin, read-only Win32 helpers: GPU performance counters, the
//! DirectX / ComputeAccelerator registry keys, and the process list. Nothing here ever
//! opens a process for anything beyond PROCESS_QUERY_LIMITED_INFORMATION (image path lookup).

use super::{Adapter, Raw};
use regex::Regex;
use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::sync::LazyLock;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, SYSTEMTIME};
use windows_sys::core::GUID;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Performance::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide_buf(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

// ---------------------------------------------------------------- GPU counters

/// The same data as the PDH counters `\GPU Engine(*)\Utilization Percentage`,
/// `\GPU Process Memory(*)\{Dedicated,Shared} Usage` and `\GPU Adapter Memory(*)\...`,
/// read through the Perflib V2 consumer API (advapi32). PDH's first
/// PdhAddCounter costs ~130-170 ms of one-time init; this costs ~10 ms.
pub struct Counters {
    /// one query handle per counter: PerfQueryCounterData does not promise to return
    /// counters in the order they were added, and ded/shr instances look identical
    hs: Vec<HANDLE>,
    prev: Option<Sample>,
    cur: Option<Sample>,
    collections: u32,
}

struct Sample {
    t100ns: i64,
    /// one list per added counter, in the order added: (instance name, raw value)
    sets: Vec<Vec<(String, u64)>>,
}

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> GUID {
    GUID { data1: d1, data2: d2, data3: d3, data4: d4 }
}
// Counter sets registered by dxgkrnl (stable across Windows 10/11 builds).
const GPU_ENGINE: GUID = guid(0x978c167d, 0x4764, 0x4d9c, [0x98, 0x24, 0x14, 0x74, 0x73, 0x51, 0xdc, 0x81]);
const GPU_PROCESS_MEMORY: GUID = guid(0xf802502b, 0x77b4, 0x4713, [0x81, 0xb3, 0x3b, 0xe0, 0x57, 0x59, 0xda, 0x5d]);
const GPU_ADAPTER_MEMORY: GUID = guid(0xbe2139c7, 0xab81, 0x424d, [0xb1, 0x07, 0xd8, 0x7f, 0x7c, 0x93, 0x22, 0xac]);

/// (set, English set name, counter id, English counter name) in query order.
const WANTED: [(GUID, &str, u32, &str); 5] = [
    (GPU_ENGINE, "GPU Engine", 2, "Utilization Percentage"),
    (GPU_PROCESS_MEMORY, "GPU Process Memory", 4, "Dedicated Usage"),
    (GPU_PROCESS_MEMORY, "GPU Process Memory", 5, "Shared Usage"),
    (GPU_ADAPTER_MEMORY, "GPU Adapter Memory", 2, "Dedicated Usage"),
    (GPU_ADAPTER_MEMORY, "GPU Adapter Memory", 3, "Shared Usage"),
];

static RX_PROC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^pid_(\d+)_luid_(0x[0-9a-f]+)_(0x[0-9a-f]+)_phys_\d+(?:_eng_\d+_engtype_(.*))?$").unwrap()
});
static RX_ADAPTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^luid_(0x[0-9a-f]+)_(0x[0-9a-f]+)").unwrap());
static RX_DUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#\d+$").unwrap());
static RX_ENGNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_\d+$").unwrap());

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

unsafe fn wstr_at(p: *const u8, max_bytes: usize) -> String {
    let w = p as *const u16;
    let mut n = 0;
    while n * 2 < max_bytes && unsafe { w.add(n).read_unaligned() } != 0 {
        n += 1;
    }
    let v: Vec<u16> = (0..n).map(|i| unsafe { w.add(i).read_unaligned() }).collect();
    String::from_utf16_lossy(&v)
}

unsafe fn reg_info(set: &GUID, code: i32) -> Vec<u8> {
    let mut need = 0u32;
    unsafe {
        PerfQueryCounterSetRegistrationInfo(null(), set, code, 0x409, null_mut(), 0, &mut need);
        let mut b = vec![0u8; need as usize];
        if need == 0 || PerfQueryCounterSetRegistrationInfo(null(), set, code, 0x409, b.as_mut_ptr(), need, &mut need) != 0 {
            return vec![];
        }
        b
    }
}

/// Fallback when a hard-coded GUID/id is rejected: look the set and counter up by English name.
fn resolve(set_name: &str, counter_name: &str) -> Option<(GUID, u32)> {
    unsafe {
        let mut n = 0u32;
        PerfEnumerateCounterSet(null(), null_mut(), 0, &mut n);
        let mut ids = vec![std::mem::zeroed::<GUID>(); n as usize];
        if PerfEnumerateCounterSet(null(), ids.as_mut_ptr(), n, &mut n) != 0 {
            return None;
        }
        for g in ids {
            let name = reg_info(&g, PERF_REG_COUNTERSET_ENGLISH_NAME);
            if name.is_empty() || wstr_at(name.as_ptr(), name.len()) != set_name {
                continue;
            }
            // PERF_STRING_BUFFER_HEADER { dwSize, dwCounters } + { dwCounterId, dwOffset }[]
            let names = reg_info(&g, PERF_REG_COUNTER_ENGLISH_NAMES);
            if names.len() < 8 {
                return None;
            }
            let cnt = u32::from_le_bytes(names[4..8].try_into().ok()?) as usize;
            for i in 0..cnt {
                let o = 8 + i * 8;
                let id = u32::from_le_bytes(names.get(o..o + 4)?.try_into().ok()?);
                let off = u32::from_le_bytes(names.get(o + 4..o + 8)?.try_into().ok()?) as usize;
                if off < names.len() && wstr_at(names.as_ptr().add(off), names.len() - off) == counter_name {
                    return Some((g, id));
                }
            }
        }
        None
    }
}

unsafe fn add_counter(h: HANDLE, set: GUID, id: u32) -> bool {
    // PERF_COUNTER_IDENTIFIER followed by the instance name "*" (8-byte padded)
    let head = std::mem::size_of::<PERF_COUNTER_IDENTIFIER>();
    let size = head + 8;
    let mut buf = vec![0u64; size.div_ceil(8)];
    unsafe {
        let ci = buf.as_mut_ptr() as *mut PERF_COUNTER_IDENTIFIER;
        (*ci).CounterSetGuid = set;
        (*ci).Size = size as u32;
        (*ci).CounterId = id;
        (*ci).InstanceId = u32::MAX;
        let name = (ci as *mut u8).add(head) as *mut u16;
        *name = '*' as u16;
        PerfAddCounters(h, ci, size as u32) == 0 && (*ci).Status == 0
    }
}

impl Counters {
    /// Opens the query, adds the five wildcard counters and takes the first sample.
    pub fn open() -> Option<Counters> {
        unsafe {
            let mut hs = vec![];
            for (set, set_name, id, ctr_name) in WANTED {
                let mut h: HANDLE = null_mut();
                if PerfOpenQueryHandle(null(), &mut h) != 0 {
                    break;
                }
                hs.push(h);
                if !add_counter(h, set, id) {
                    let ok = resolve(set_name, ctr_name).is_some_and(|(g, i)| add_counter(h, g, i));
                    if !ok {
                        break;
                    }
                }
            }
            if hs.len() < WANTED.len() {
                for h in hs {
                    PerfCloseQueryHandle(h);
                }
                return None;
            }
            let mut c = Counters { hs, prev: None, cur: None, collections: 0 };
            c.collect();
            Some(c)
        }
    }

    pub fn collections(&self) -> u32 {
        self.collections
    }

    pub fn collect(&mut self) {
        let s = self.query();
        self.prev = self.cur.take();
        self.cur = s;
        self.collections += 1;
    }

    fn query(&self) -> Option<Sample> {
        let mut sample = Sample { t100ns: 0, sets: vec![] };
        for (i, &h) in self.hs.iter().enumerate() {
            let (t, list) = Self::query_one(h)?;
            if i == 0 {
                sample.t100ns = t;
            }
            sample.sets.push(list);
        }
        Some(sample)
    }

    /// (timestamp in 100 ns, instances) for a single-counter query.
    fn query_one(h: HANDLE) -> Option<(i64, Vec<(String, u64)>)> {
        unsafe {
            let mut need = 0u32;
            PerfQueryCounterData(h, null_mut(), 0, &mut need);
            if need == 0 {
                return None;
            }
            // headroom in case instances appear between the two calls; retry once
            let mut buf = vec![0u64; need as usize / 8 + 512];
            let mut cap = (buf.len() * 8) as u32;
            let mut st = PerfQueryCounterData(h, buf.as_mut_ptr().cast(), cap, &mut need);
            if st != 0 {
                buf = vec![0u64; need as usize / 8 + 4096];
                cap = (buf.len() * 8) as u32;
                st = PerfQueryCounterData(h, buf.as_mut_ptr().cast(), cap, &mut need);
                if st != 0 {
                    return None;
                }
            }
            let p = buf.as_ptr() as *const u8;
            let total = (need as usize).min(cap as usize);
            let hdr = &*(p as *const PERF_DATA_HEADER);
            let off = std::mem::size_of::<PERF_DATA_HEADER>();
            let mut list = vec![];
            if hdr.dwNumCounters >= 1 && off + 16 <= total {
                let ch = &*(p.add(off) as *const PERF_COUNTER_HEADER);
                if ch.dwStatus == 0 && ch.dwType == PERF_MULTIPLE_INSTANCES {
                    let mi = &*(p.add(off + 16) as *const PERF_MULTI_INSTANCES);
                    let mut io = off + 16 + std::mem::size_of::<PERF_MULTI_INSTANCES>();
                    let end = (off + ch.dwSize as usize).min(total);
                    for _ in 0..mi.dwInstances {
                        if io + 8 > end {
                            break;
                        }
                        let ih = &*(p.add(io) as *const PERF_INSTANCE_HEADER);
                        let isz = ih.Size as usize;
                        if isz < 8 || io + isz + 8 > end {
                            break;
                        }
                        let name = wstr_at(p.add(io + 8), isz - 8);
                        let cd = &*(p.add(io + isz) as *const PERF_COUNTER_DATA);
                        let vp = p.add(io + isz + std::mem::size_of::<PERF_COUNTER_DATA>());
                        let v = match cd.dwDataSize {
                            4 => (vp as *const u32).read_unaligned() as u64,
                            _ => (vp as *const u64).read_unaligned(),
                        };
                        list.push((name, v));
                        io += isz + (cd.dwSize as usize).max(8);
                    }
                }
            }
            Some((hdr.PerfTime100NSec, list))
        }
    }

    /// Everything from the latest sample. Utilisation is a rate (100 ns busy time over
    /// wall time), so it needs two samples.
    pub fn read(&self) -> Raw {
        let mut raw = Raw::default();
        let Some(cur) = &self.cur else { return raw };
        let norm = |s: &str| RX_DUP.replace(&s.to_lowercase(), "").into_owned();
        let set = |i: usize| cur.sets.get(i).map(|v| v.as_slice()).unwrap_or(&[]);

        for (i, is_ded) in [(3, true), (4, false)] {
            for (name, v) in set(i) {
                let inst = norm(name);
                if let Some(m) = RX_ADAPTER.captures(&inst) {
                    let k = (hex(&m[1]) << 32) | hex(&m[2]);
                    let e = raw.adapter_mem.entry(k).or_default();
                    if is_ded { e.0 += *v as f64 } else { e.1 += *v as f64 }
                }
            }
        }
        let proc_of = |raw: &mut Raw, inst: &str| -> Option<(u64, u32, Option<String>)> {
            let m = RX_PROC.captures(inst)?;
            let pid: u32 = m[1].parse().ok()?;
            let luid = (hex(&m[2]) << 32) | hex(&m[3]);
            raw.procs.entry((luid, pid)).or_default();
            Some((luid, pid, m.get(4).map(|e| RX_ENGNUM.replace(e.as_str(), "").into_owned())))
        };
        for (i, is_ded) in [(1, true), (2, false)] {
            for (name, v) in set(i) {
                if let Some((luid, pid, _)) = proc_of(&mut raw, &norm(name)) {
                    let r = raw.procs.get_mut(&(luid, pid)).unwrap();
                    if is_ded { r.ded = r.ded.max(*v as f64) } else { r.shr = r.shr.max(*v as f64) }
                }
            }
        }
        // engines: delta of busy time between samples, matched by name (+ occurrence)
        let mut prev: HashMap<(&str, usize), u64> = HashMap::new();
        let dt = self.prev.as_ref().map(|p| (cur.t100ns - p.t100ns) as f64).unwrap_or(0.0);
        if let Some(p) = &self.prev {
            let mut seen: HashMap<&str, usize> = HashMap::new();
            for (name, v) in p.sets.first().map(|v| v.as_slice()).unwrap_or(&[]) {
                let k = seen.entry(name.as_str()).or_insert(0);
                prev.insert((name.as_str(), *k), *v);
                *k += 1;
            }
        }
        raw.util_ready = self.prev.is_some() && dt > 0.0;
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (name, v) in set(0) {
            let k = seen.entry(name.as_str()).or_insert(0);
            let before = prev.get(&(name.as_str(), *k)).copied();
            *k += 1;
            let Some((luid, pid, eng)) = proc_of(&mut raw, &norm(name)) else { continue };
            if !raw.util_ready {
                continue;
            }
            let Some(before) = before else { continue };
            let pct = (v.saturating_sub(before) as f64 / dt * 100.0).clamp(0.0, 100.0);
            let r = raw.procs.get_mut(&(luid, pid)).unwrap();
            *r.eng.entry(eng.unwrap_or_else(|| "?".into())).or_insert(0.0) += pct;
        }
        raw
    }
}

impl Drop for Counters {
    fn drop(&mut self) {
        unsafe {
            for &h in &self.hs {
                PerfCloseQueryHandle(h);
            }
        }
    }
}

// ---------------------------------------------------------------- registry

fn reg_bytes(root: HKEY, sub: &str, value: &str) -> Option<Vec<u8>> {
    unsafe {
        let (s, v) = (wide(sub), wide(value));
        let mut size = 0u32;
        let mut ty = 0u32;
        if RegGetValueW(root, s.as_ptr(), v.as_ptr(), RRF_RT_ANY, &mut ty, null_mut(), &mut size) != 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize + 2];
        if RegGetValueW(root, s.as_ptr(), v.as_ptr(), RRF_RT_ANY, &mut ty, buf.as_mut_ptr().cast(), &mut size) != 0 {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

fn reg_str(root: HKEY, sub: &str, value: &str) -> Option<String> {
    let b = reg_bytes(root, sub, value)?;
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = from_wide_buf(&w);
    (!s.is_empty()).then_some(s)
}

fn reg_u64(root: HKEY, sub: &str, value: &str) -> Option<u64> {
    let b = reg_bytes(root, sub, value)?;
    let mut a = [0u8; 8];
    for (i, x) in b.iter().take(8).enumerate() {
        a[i] = *x;
    }
    Some(u64::from_le_bytes(a))
}

fn subkeys(root: HKEY, path: &str) -> Vec<String> {
    let mut out = vec![];
    unsafe {
        let mut h: HKEY = null_mut();
        let p = wide(path);
        if RegOpenKeyExW(root, p.as_ptr(), 0, KEY_READ, &mut h) != 0 {
            return out;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(h, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) != 0 {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
        RegCloseKey(h);
    }
    out
}

/// luid -> adapter, from HKLM\SOFTWARE\Microsoft\DirectX\{guid}.
pub fn adapters() -> HashMap<u64, Adapter> {
    let base = r"SOFTWARE\Microsoft\DirectX";
    let mut map = HashMap::new();
    for k in subkeys(HKEY_LOCAL_MACHINE, base) {
        let sub = format!(r"{base}\{k}");
        let (Some(name), Some(luid)) =
            (reg_str(HKEY_LOCAL_MACHINE, &sub, "Description"), reg_u64(HKEY_LOCAL_MACHINE, &sub, "AdapterLuid"))
        else {
            continue;
        };
        if luid == 0 {
            continue;
        }
        let total = reg_u64(HKEY_LOCAL_MACHINE, &sub, "DedicatedVideoMemory").unwrap_or(0) as f64;
        let shared = reg_u64(HKEY_LOCAL_MACHINE, &sub, "SharedSystemMemory").unwrap_or(0) as f64;
        map.insert(luid, Adapter { name, total, shared });
    }
    map
}

/// Name of the first compute accelerator (NPU), from its device-class key. Much
/// faster than PnP enumeration.
pub fn npu_name() -> Option<String> {
    let base = r"SYSTEM\CurrentControlSet\Control\Class\{f01a9d53-3ff6-48d2-9f97-c8a7004be10c}";
    let mut keys = subkeys(HKEY_LOCAL_MACHINE, base);
    keys.sort();
    keys.iter()
        .filter(|k| k.chars().all(|c| c.is_ascii_digit()))
        .find_map(|k| reg_str(HKEY_LOCAL_MACHINE, &format!(r"{base}\{k}"), "DriverDesc"))
}

// ---------------------------------------------------------------- processes

/// pid -> process name without ".exe" (like Get-Process ProcessName).
pub fn process_names() -> HashMap<u32, String> {
    let mut m = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return m;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let mut name = from_wide_buf(&e.szExeFile);
            if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
                name.truncate(name.len() - 4);
            }
            let name = match e.th32ProcessID {
                0 => "Idle".to_string(),
                _ => name,
            };
            m.insert(e.th32ProcessID, name);
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    m
}

/// Full image path via a query-limited handle (read-only). None for protected processes.
pub fn process_path(pid: u32) -> Option<String> {
    if pid <= 4 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Command lines of other processes are not read on Windows (it would need
/// PROCESS_VM_READ); the tags use the name and image path.
pub fn process_cmd(_pid: u32) -> Option<String> {
    None
}

pub fn local_time() -> String {
    unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
    }
}
