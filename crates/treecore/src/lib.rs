//! Shared pieces of gputree and cputree: data collection (GPU counters, the process
//! tree, WSL, NVML), workload tags, the Jev headline call, and the width-safe,
//! aligned terminal tree. Everything here is read-only with respect to processes.
//!
//! The platform code lives behind `gpu::sys` and `cpu::sys` (one backend per OS with
//! the same items); the model, render, tags and Jev code above them is shared.

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("gputree / cputree support Windows and Linux");

pub mod cpu;
pub mod gpu;
pub mod jev;
pub mod layout;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod procfs;
pub mod tags;
pub mod term;
pub mod wsl;

/// Child processes (wsl.exe, curl.exe, nvidia-smi) get no console window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The OS the processes belong to, for prose sent to Jev ("Windows process 'x'").
#[cfg(windows)]
pub const OS: &str = "Windows";
#[cfg(not(windows))]
pub const OS: &str = "Linux";

/// What the `system` tag is called in a headline.
#[cfg(windows)]
pub const SYSTEM_PHRASE: &str = "Windows";
#[cfg(not(windows))]
pub const SYSTEM_PHRASE: &str = "the system";

/// Keeps a spawned helper from flashing a console window (Windows); no-op elsewhere.
pub fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

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
