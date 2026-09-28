//! gputree-gui: what is using your GPUs, as a disktree-style treemap.
//! Adapters -> processes -> engines (util) or the Linux processes inside the
//! WSL2 VM (VRAM). Read-only: it samples counters and reads /proc; there is no
//! action anywhere that ends or changes a process.
//!
//! Windows only. On other systems this builds a stub that points at `gputree`.

#![windows_subsystem = "windows"]

#[cfg(windows)]
mod gui;

#[cfg(windows)]
fn main() {
    gui::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("gputree-gui is Windows-only (GPUI, MSVC toolchain); use the gputree command here");
    std::process::exit(1);
}
