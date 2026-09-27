//! The platform layer under gputree. Each backend provides the same items with the same
//! signatures, so the model, render, tags and Jev code above it is shared:
//!
//! - `Counters::open() -> Option<Counters>`, `.collect()`, `.collections()`, `.read() -> Raw`
//! - `adapters() -> HashMap<u64, Adapter>`, `npu_name() -> Option<String>`
//! - `process_names()`, `process_path(pid)`, `process_cmd(pid)`, `local_time()`
//!
//! Windows reads the GPU performance counters (Perflib V2) and the registry; Linux
//! reads DRM fdinfo, sysfs and NVML, or in WSL the processes holding `/dev/dxg`.
//! Adapters are keyed by a `u64`: the LUID on Windows, the PCI address on Linux.

use std::collections::{BTreeMap, HashMap};

/// Per (adapter key, pid) numbers.
#[derive(Clone, Debug, Default)]
pub struct ProcRaw {
    pub ded: f64,
    pub shr: f64,
    /// engine type (lowercase, "_N" stripped) -> summed utilisation %
    pub eng: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Raw {
    pub procs: HashMap<(u64, u32), ProcRaw>,
    /// adapter key -> (dedicated, shared) bytes in use
    pub adapter_mem: HashMap<u64, (f64, f64)>,
    pub util_ready: bool,
}

#[derive(Clone, Debug)]
pub struct Adapter {
    pub name: String,
    pub total: f64,
    pub shared: f64,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
