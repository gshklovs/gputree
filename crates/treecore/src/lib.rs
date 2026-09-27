//! Shared pieces of gputree and cputree: data collection (GPU counters, the process
//! tree, WSL, NVML), workload tags, the Jev headline call, and the width-safe,
//! aligned terminal tree. Everything here is read-only with respect to processes.

pub mod cpu;
pub mod gpu;
pub mod jev;
pub mod layout;
pub mod tags;
pub mod term;
pub mod wsl;

/// Child processes (wsl.exe, curl.exe, nvidia-smi) get no console window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Lower-case, word-safe process-name capitalisation for prose ("chrome" -> "Chrome").
pub fn pretty_name(n: &str) -> String {
    if n.chars().any(|c| c.is_uppercase()) {
        return n.to_string();
    }
    let mut c = n.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
