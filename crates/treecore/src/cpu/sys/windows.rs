//! The Windows backend. One read-only NtQuerySystemInformation call gives every process
//! with its parent, start time, CPU time and memory; GetSystemTimes / per-core times
//! give the totals.

use super::PInfo;
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation, SystemProcessorPerformanceInformation,
};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetSystemTimes};

fn rd<T: Copy>(b: &[u8], off: usize) -> T {
    assert!(off + std::mem::size_of::<T>() <= b.len());
    unsafe { (b.as_ptr().add(off) as *const T).read_unaligned() }
}

/// Every process. Layout of SYSTEM_PROCESS_INFORMATION on x64 (stable since Vista):
/// 0 NextEntryOffset, 4 NumberOfThreads, 8 WorkingSetPrivateSize, 32 CreateTime,
/// 40 UserTime, 48 KernelTime, 56 ImageName (UNICODE_STRING), 80 UniqueProcessId,
/// 88 InheritedFromUniqueProcessId.
pub fn processes() -> Vec<PInfo> {
    let mut size: u32 = 1 << 20;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        let mut need = 0u32;
        let st = unsafe { NtQuerySystemInformation(SystemProcessInformation, buf.as_mut_ptr().cast(), size, &mut need) };
        if st == 0 {
            break;
        }
        // STATUS_INFO_LENGTH_MISMATCH: grow and retry
        if st as u32 == 0xC000_0004 && size < (64 << 20) {
            size = need.max(size * 2) + (64 << 10);
            continue;
        }
        return vec![];
    }
    let mut out = vec![];
    let mut off = 0usize;
    loop {
        if off + 96 > buf.len() {
            break;
        }
        let next: u32 = rd(&buf, off);
        let threads: u32 = rd(&buf, off + 4);
        let ws_private: i64 = rd(&buf, off + 8);
        let create: i64 = rd(&buf, off + 32);
        let user: i64 = rd(&buf, off + 40);
        let kernel: i64 = rd(&buf, off + 48);
        let name_len: u16 = rd(&buf, off + 56);
        let name_ptr: usize = rd(&buf, off + 64);
        let pid: usize = rd(&buf, off + 80);
        let ppid: usize = rd(&buf, off + 88);
        let mut name = if name_ptr != 0 && name_len > 0 {
            let s = unsafe { std::slice::from_raw_parts(name_ptr as *const u16, name_len as usize / 2) };
            String::from_utf16_lossy(s)
        } else if pid == 0 {
            "Idle".into()
        } else {
            "?".into()
        };
        if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
            name.truncate(name.len() - 4);
        }
        out.push(PInfo {
            pid: pid as u32,
            ppid: ppid as u32,
            name,
            create,
            cpu: user + kernel,
            mem: ws_private.max(0) as u64,
            threads,
            cmd: String::new(),
        });
        if next == 0 {
            break;
        }
        off += next as usize;
    }
    out
}

fn ft(f: FILETIME) -> i64 {
    ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64
}

/// (idle, kernel incl. idle, user) in 100 ns ticks, whole machine.
pub fn system_times() -> (i64, i64, i64) {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut i, mut k, mut u) = (z, z, z);
    unsafe {
        GetSystemTimes(&mut i, &mut k, &mut u);
    }
    (ft(i), ft(k), ft(u))
}

/// Per logical processor (of the current group): (idle, kernel incl. idle, user).
pub fn core_times() -> Vec<(i64, i64, i64)> {
    const SZ: usize = 48; // SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION
    let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.clamp(1, 1024) as usize;
    let mut buf = vec![0u8; SZ * n];
    let mut need = 0u32;
    let st = unsafe {
        NtQuerySystemInformation(SystemProcessorPerformanceInformation, buf.as_mut_ptr().cast(), buf.len() as u32, &mut need)
    };
    if st != 0 {
        return vec![];
    }
    let got = (need as usize / SZ).min(n);
    (0..got).map(|c| (rd(&buf, c * SZ), rd(&buf, c * SZ + 8), rd(&buf, c * SZ + 16))).collect()
}

pub fn ncpu() -> usize {
    unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1) as usize
}

/// Windows only exposes CPU temperature through WMI/ACPI, which is neither cheap nor
/// reliable, so it is not shown.
pub fn cpu_temp() -> Option<f64> {
    None
}

/// (total, available) physical memory, bytes.
pub fn memory() -> (u64, u64) {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    unsafe {
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return (0, 0);
        }
    }
    (m.ullTotalPhys, m.ullAvailPhys)
}
