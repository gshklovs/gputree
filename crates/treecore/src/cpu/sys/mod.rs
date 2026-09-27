//! The platform layer under cputree. Each backend provides the same functions:
//! `processes()`, `system_times()`, `core_times()`, `ncpu()`, `memory()` and
//! `cpu_temp()`. Times are in 100 ns ticks on both, so the model computes the same
//! rates from either.

/// One process from the platform's process list.
#[derive(Clone, Debug)]
pub struct PInfo {
    pub pid: u32,
    pub ppid: u32,
    /// Windows: image name without ".exe". Linux: the program name (argv[0]'s
    /// basename, or comm), or for an interpreter the shortened script + arguments
    /// ("train bd1-walk-flat").
    pub name: String,
    /// start time in 100 ns ticks (Windows: FILETIME since 1601; Linux: since boot).
    /// Only compared, never shown: it guards against pid reuse.
    pub create: i64,
    /// user + kernel, 100 ns ticks
    pub cpu: i64,
    /// Windows: private working set (Task Manager's "Memory"). Linux: VmRSS. Bytes.
    pub mem: u64,
    pub threads: u32,
    /// Linux: the command line (NULs as spaces), empty for kernel threads. Windows: empty.
    pub cmd: String,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
