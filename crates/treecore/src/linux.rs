//! Small Linux helpers shared by the GPU and CPU backends: /proc listing, clock and
//! page sizes, local time, WSL detection, and a (pid, start time) -> name / command
//! cache so each process's cmdline is read once. Everything here only reads.

use crate::procfs;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

pub fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// USER_HZ (100 on every mainstream architecture, but asked rather than assumed).
pub fn clk_tck() -> u64 {
    static V: LazyLock<u64> = LazyLock::new(|| {
        let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if v > 0 { v as u64 } else { 100 }
    });
    *V
}

pub fn page_size() -> u64 {
    static V: LazyLock<u64> = LazyLock::new(|| {
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 { v as u64 } else { 4096 }
    });
    *V
}

/// Every numeric entry of /proc.
pub fn pids() -> Vec<u32> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    let mut v: Vec<u32> = rd.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect();
    v.sort_unstable();
    v
}

pub fn local_time() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

/// Inside WSL2: the paravirtual GPU device exists (and there is no real DRM GPU).
pub fn is_wsl() -> bool {
    static V: LazyLock<bool> = LazyLock::new(|| {
        Path::new("/dev/dxg").exists()
            || read("/proc/sys/kernel/osrelease").is_some_and(|r| r.to_ascii_lowercase().contains("microsoft"))
    });
    *V
}

/// The distro name inside WSL (`WSL_DISTRO_NAME`, else /etc/os-release, else "WSL").
pub fn distro() -> String {
    if let Ok(d) = std::env::var("WSL_DISTRO_NAME") {
        if !d.is_empty() {
            return d;
        }
    }
    read("/etc/os-release")
        .and_then(|t| {
            t.lines().find_map(|l| l.strip_prefix("NAME=").map(|v| v.trim_matches('"').split_whitespace().next().unwrap_or("").to_string()))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "WSL".into())
}

/// (display name, command line) of a process, read once per (pid, start time).
#[derive(Clone, Debug, Default)]
pub struct Ident {
    pub name: String,
    pub cmd: String,
}

static IDENT: LazyLock<Mutex<HashMap<(u32, u64), Ident>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn ident(pid: u32, starttime: u64, comm: &str) -> Ident {
    let mut m = IDENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = m.get(&(pid, starttime)) {
        return i.clone();
    }
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).map(|b| procfs::cmdline(&b)).unwrap_or_default();
    let i = Ident { name: procfs::display_name(comm, &cmd), cmd };
    // forget the dead now and then
    if m.len() > 8192 {
        m.clear();
    }
    m.insert((pid, starttime), i.clone());
    i
}

/// Name and command line of a live pid (reads its stat for the start time).
pub fn ident_of(pid: u32) -> Option<Ident> {
    let st = procfs::parse_pid_stat(&read(format!("/proc/{pid}/stat"))?)?;
    Some(ident(pid, st.starttime, &st.comm))
}

pub fn user_of(uid: u32) -> String {
    static PASSWD: LazyLock<String> = LazyLock::new(|| read("/etc/passwd").unwrap_or_default());
    procfs::passwd_user(&PASSWD, uid).unwrap_or_else(|| uid.to_string())
}

/// Owner uid of /proc/<pid>.
pub fn uid_of(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(format!("/proc/{pid}")).ok().map(|m| m.uid())
}

/// Restores the terminal (colours, cursor) when Ctrl+C ends a --watch.
pub fn on_interrupt_restore_terminal() {
    extern "C" fn handler(_: libc::c_int) {
        let s = b"\x1b[0m\x1b[?25h\n";
        unsafe {
            libc::write(1, s.as_ptr().cast(), s.len());
            libc::_exit(130);
        }
    }
    unsafe {
        libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t);
    }
}
