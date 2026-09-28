//! The Linux backend: `/proc/<pid>/stat` for every process (parent, start time, CPU
//! time, RSS, threads), `/proc/stat` for the machine and per-core totals,
//! `/proc/meminfo`, and hwmon / thermal zones for the CPU temperature. Only reads.

use super::PInfo;
use crate::linux::{self, clk_tck, page_size};
use crate::procfs;

/// clock ticks -> 100 ns ticks (the unit the model shares with Windows)
fn ticks(t: u64) -> i64 {
    (t as u128 * 10_000_000 / clk_tck() as u128) as i64
}

/// Every process. Kernel threads are included (under kthreadd) with no command line.
pub fn processes() -> Vec<PInfo> {
    let page = page_size();
    let mut out = vec![];
    for pid in linux::pids() {
        let Some(st) = linux::read(format!("/proc/{pid}/stat")).and_then(|s| procfs::parse_pid_stat(&s)) else { continue };
        if st.state == 'Z' {
            continue; // zombies have no CPU or memory left to show
        }
        let id = linux::ident(pid, st.starttime, &st.comm);
        out.push(PInfo {
            pid,
            ppid: st.ppid,
            name: id.name,
            create: ticks(st.starttime),
            cpu: ticks(st.utime + st.stime),
            mem: st.rss_pages * page,
            threads: st.threads,
            cmd: id.cmd,
        });
    }
    out
}

fn stat() -> (Option<procfs::CpuTimes>, Vec<procfs::CpuTimes>) {
    procfs::parse_proc_stat(&linux::read("/proc/stat").unwrap_or_default())
}

fn triple(t: &procfs::CpuTimes) -> (i64, i64, i64) {
    let (i, k, u) = t.triple();
    (ticks(i), ticks(k), ticks(u))
}

/// (idle, kernel incl. idle, user) in 100 ns ticks, whole machine.
pub fn system_times() -> (i64, i64, i64) {
    stat().0.map(|t| triple(&t)).unwrap_or_default()
}

/// Per logical processor: (idle, kernel incl. idle, user).
pub fn core_times() -> Vec<(i64, i64, i64)> {
    stat().1.iter().map(triple).collect()
}

pub fn ncpu() -> usize {
    let n = stat().1.len();
    if n > 0 { n } else { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) }
}

/// (total, available) physical memory, bytes.
pub fn memory() -> (u64, u64) {
    procfs::parse_meminfo(&linux::read("/proc/meminfo").unwrap_or_default())
}

/// Package / die temperature in °C: hwmon's coretemp, k10temp, zenpower or
/// cpu_thermal, else the x86_pkg_temp / cpu thermal zone. None when there is none
/// (VMs, WSL).
pub fn cpu_temp() -> Option<f64> {
    if let Ok(rd) = std::fs::read_dir("/sys/class/hwmon") {
        let mut dirs: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            let name = linux::read(d.join("name")).unwrap_or_default();
            if matches!(name.trim(), "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" | "soc_thermal") {
                if let Some(t) = linux::read(d.join("temp1_input")).and_then(|s| procfs::millideg(&s)) {
                    return Some(t);
                }
            }
        }
    }
    let rd = std::fs::read_dir("/sys/class/thermal").ok()?;
    let mut zones: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.to_string_lossy().contains("thermal_zone")).collect();
    zones.sort();
    for z in zones {
        let ty = linux::read(z.join("type")).unwrap_or_default();
        let ty = ty.trim();
        if ty == "x86_pkg_temp" || ty.starts_with("cpu") || ty == "soc_thermal" {
            if let Some(t) = linux::read(z.join("temp")).and_then(|s| procfs::millideg(&s)) {
                return Some(t);
            }
        }
    }
    None
}
