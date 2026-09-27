//! cputree-gui: what is using your CPU, as a disktree-style treemap of the real
//! process tree, sized by rolled-up CPU (or memory). Same-name siblings are
//! merged, and the WSL2 VM opens into the Linux processes inside it.
//! Read-only: there is no action anywhere that ends or changes a process.
//!
//! Windows only. On other systems this builds a stub that points at `cputree`.

#![windows_subsystem = "windows"]

#[cfg(windows)]
mod gui;

#[cfg(windows)]
fn main() {
    gui::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("cputree-gui is Windows-only (GPUI, MSVC toolchain); use the cputree command here");
    std::process::exit(1);
}
