//! Parsers for Linux `/proc` and `/sys` text. Pure functions over strings, compiled on
//! every platform so the fixture tests (tests/fixtures) run everywhere; the Linux
//! backends do the reading.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// The fields of `/proc/<pid>/stat` that cputree uses.
#[derive(Clone, Debug, PartialEq)]
pub struct PidStat {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    /// clock ticks
    pub utime: u64,
    pub stime: u64,
    pub threads: u32,
    /// clock ticks since boot
    pub starttime: u64,
    /// resident pages (the same counter as VmRSS in /proc/<pid>/status)
    pub rss_pages: u64,
}

/// `/proc/<pid>/stat`. `comm` may contain spaces and parentheses, so the fields are
/// counted from the last `)`.
pub fn parse_pid_stat(s: &str) -> Option<PidStat> {
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = s[..open].trim().parse().ok()?;
    let comm = s[open + 1..close].to_string();
    // after ") ": field 3 (state) is f[0]
    let f: Vec<&str> = s[close + 1..].split_whitespace().collect();
    let n = |i: usize| -> Option<u64> { f.get(i)?.parse().ok() };
    Some(PidStat {
        pid,
        comm,
        state: f.first()?.chars().next()?,
        ppid: n(1)? as u32,
        utime: n(11)?,
        stime: n(12)?,
        threads: n(17)? as u32,
        starttime: n(19)?,
        rss_pages: n(21).unwrap_or(0),
    })
}

/// One `cpu` / `cpuN` line of `/proc/stat`, in clock ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// The Windows shape the model works with: (idle, kernel incl. idle, user).
    /// iowait counts as idle; irq, softirq and steal as kernel; nice as user
    /// (guest time is already inside user).
    pub fn triple(&self) -> (u64, u64, u64) {
        let idle = self.idle + self.iowait;
        let kernel = self.system + self.irq + self.softirq + self.steal + idle;
        (idle, kernel, self.user + self.nice)
    }
}

/// `/proc/stat` -> (the aggregate `cpu` line, one entry per `cpuN` line in order).
pub fn parse_proc_stat(s: &str) -> (Option<CpuTimes>, Vec<CpuTimes>) {
    let mut total = None;
    let mut cores = vec![];
    for l in s.lines() {
        let mut it = l.split_whitespace();
        let Some(k) = it.next() else { continue };
        if !k.starts_with("cpu") {
            continue;
        }
        let v: Vec<u64> = it.map(|x| x.parse().unwrap_or(0)).collect();
        let g = |i: usize| v.get(i).copied().unwrap_or(0);
        let t = CpuTimes {
            user: g(0),
            nice: g(1),
            system: g(2),
            idle: g(3),
            iowait: g(4),
            irq: g(5),
            softirq: g(6),
            steal: g(7),
        };
        if k == "cpu" {
            total = Some(t);
        } else if k[3..].chars().all(|c| c.is_ascii_digit()) {
            cores.push(t);
        }
    }
    (total, cores)
}

/// `/proc/meminfo` -> (MemTotal, MemAvailable) in bytes. Falls back to
/// MemFree + Buffers + Cached on kernels without MemAvailable.
pub fn parse_meminfo(s: &str) -> (u64, u64) {
    let mut get = std::collections::HashMap::new();
    for l in s.lines() {
        if let Some((k, v)) = l.split_once(':') {
            let kib: u64 = v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
            get.insert(k.trim(), kib * 1024);
        }
    }
    let total = get.get("MemTotal").copied().unwrap_or(0);
    let avail = get.get("MemAvailable").copied().unwrap_or_else(|| {
        ["MemFree", "Buffers", "Cached"].iter().map(|k| get.get(k).copied().unwrap_or(0)).sum()
    });
    (total, avail.min(total))
}

/// `VmRSS` from `/proc/<pid>/status`, bytes.
pub fn parse_status_rss(s: &str) -> Option<u64> {
    let l = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kib: u64 = l[6..].split_whitespace().next()?.parse().ok()?;
    Some(kib * 1024)
}

/// `/proc/<pid>/cmdline` (NUL-separated) -> one line.
pub fn cmdline(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    s.split('\0').filter(|a| !a.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The name a Linux process is shown and merged by. The equivalent of a Windows
/// image name: argv[0]'s basename when it is the program (comm is cut at 15 chars),
/// comm otherwise (kernel threads, `@dbus-daemon`, `-bash`), and for interpreters the
/// shortened script + arguments, like the WSL rows (`train bd1-walk-flat`).
pub fn display_name(comm: &str, cmd: &str) -> String {
    let Some(argv0) = cmd.split_whitespace().next() else {
        return comm.to_string();
    };
    if crate::wsl::is_interp(argv0) && cmd.split_whitespace().nth(1).is_some() {
        let (label, _) = crate::wsl::shorten(cmd, "");
        if !label.is_empty() && label != "?" {
            return label;
        }
    }
    let base = argv0.rsplit('/').next().unwrap_or(argv0).trim_end_matches(':');
    // a comm cut at 15 characters that merely extends the program name
    // ("init-systemd(Ub" for /init) reads better as the program name
    let cut = comm.len() >= 15 && !base.is_empty() && comm.starts_with(base);
    if (!comm.is_empty() && base.starts_with(comm)) || cut {
        base.to_string()
    } else if comm.is_empty() {
        base.to_string()
    } else {
        comm.to_string()
    }
}

/// A hwmon / thermal millidegree reading -> °C, None for nonsense values.
pub fn millideg(s: &str) -> Option<f64> {
    let v: f64 = s.trim().parse().ok()?;
    let c = v / 1000.0;
    (c > 0.0 && c < 150.0).then_some(c)
}

/// `uid` -> user name, from `/etc/passwd` text.
pub fn passwd_user(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2].parse::<u32>().ok() == Some(uid)).then(|| f[0].to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROC_STAT: &str = include_str!("../tests/fixtures/proc/stat");
    const MEMINFO: &str = include_str!("../tests/fixtures/proc/meminfo");
    const TRAIN_STAT: &str = include_str!("../tests/fixtures/proc/1194/stat");
    const TRAIN_STATUS: &str = include_str!("../tests/fixtures/proc/1194/status");
    const TRAIN_CMDLINE: &[u8] = include_bytes!("../tests/fixtures/proc/1194/cmdline");

    #[test]
    fn proc_stat_totals_and_cores() {
        let (t, cores) = parse_proc_stat(PROC_STAT);
        let t = t.unwrap();
        assert_eq!(cores.len(), 4);
        assert_eq!(t.user, 104_500);
        let (idle, kernel, user) = t.triple();
        assert_eq!(idle, 900_000 + 2_000);
        assert_eq!(user, 104_500 + 500);
        assert_eq!(kernel, 20_000 + 100 + 400 + 0 + idle);
        // busy share of the whole window, as the model computes it
        let busy = (kernel + user - idle) as f64 / (kernel + user) as f64;
        assert!((busy - 0.1223).abs() < 0.001, "{busy}");
    }

    #[test]
    fn pid_stat_with_spaces_and_parens_in_comm() {
        let s = parse_pid_stat(TRAIN_STAT).unwrap();
        assert_eq!(s.pid, 1194);
        assert_eq!(s.comm, "train");
        assert_eq!(s.ppid, 1193);
        assert_eq!((s.utime, s.stime), (812_345, 45_678));
        assert_eq!(s.threads, 57);
        assert_eq!(s.starttime, 1_529_000);
        assert_eq!(s.rss_pages, 717_379);
        let odd = "42 (a) b (c)) S 1 42 42 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 1 0 999 1000 25 18446744073709551615";
        let o = parse_pid_stat(odd).unwrap();
        assert_eq!(o.comm, "a) b (c)");
        assert_eq!((o.ppid, o.utime, o.stime, o.starttime, o.rss_pages), (1, 7, 3, 999, 25));
    }

    #[test]
    fn meminfo_and_status() {
        let (t, a) = parse_meminfo(MEMINFO);
        assert_eq!(t, 16_281_000 * 1024);
        assert_eq!(a, 11_534_336 * 1024);
        assert_eq!(parse_meminfo("MemTotal: 100 kB\nMemFree: 10 kB\nCached: 20 kB\n"), (102_400, 30 * 1024));
        assert_eq!(parse_status_rss(TRAIN_STATUS), Some(2_869_516 * 1024));
    }

    #[test]
    fn names_like_windows_image_names() {
        let cmd = cmdline(TRAIN_CMDLINE);
        assert!(cmd.starts_with("/home/grego/ll/microduck_rl/.venv/bin/python /home/grego/ll/microduck_rl/.venv/bin/train bd1-walk-flat"));
        assert_eq!(display_name("train", &cmd), "train bd1-walk-flat");
        assert_eq!(display_name("kworker/0:1-events", ""), "kworker/0:1-events");
        assert_eq!(display_name("dbus-daemon", "@dbus-daemon --system"), "dbus-daemon");
        assert_eq!(display_name("bash", "-bash"), "bash");
        assert_eq!(display_name("sshd", "sshd: grego@pts/0"), "sshd");
        // comm is cut at 15 characters; argv[0] has the whole name
        assert_eq!(display_name("gnome-terminal-", "/usr/libexec/gnome-terminal-server"), "gnome-terminal-server");
        assert_eq!(display_name("init-systemd(Ub", "/init"), "init");
        assert_eq!(display_name("Relay(6925)", "/init"), "Relay(6925)");
        assert_eq!(display_name("chrome", "/opt/google/chrome/chrome --type=renderer"), "chrome");
        assert_eq!(
            display_name("python3", "/usr/bin/python3 -m vllm.entrypoints.openai.api_server --model x"),
            "vllm.entrypoints.openai.api_server --model x"
        );
        assert_eq!(display_name("python3", "python3"), "python3");
    }

    #[test]
    fn small_readers() {
        assert_eq!(millideg("54000\n"), Some(54.0));
        assert_eq!(millideg("-273000"), None);
        assert_eq!(passwd_user("root:x:0:0::/root:/bin/bash\ngrego:x:1000:1000::/home/grego:/bin/bash\n", 1000).as_deref(), Some("grego"));
    }
}
