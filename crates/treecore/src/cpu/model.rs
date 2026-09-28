//! Samples -> the real parent/child process tree with own and rolled-up CPU and memory.

use super::sys::{self, PInfo};
use crate::tags;
use std::collections::HashMap;
use std::time::Instant;

pub struct Sample {
    pub at: Instant,
    pub procs: Vec<PInfo>,
    pub sys: (i64, i64, i64),
    pub cores: Vec<(i64, i64, i64)>,
}

impl Sample {
    pub fn take() -> Sample {
        Sample { at: Instant::now(), procs: sys::processes(), sys: sys::system_times(), cores: sys::core_times() }
    }
}

/// A Linux process inside WSL, with CPU measured over ~300 ms inside the distro.
#[derive(Clone, Debug)]
pub struct LinuxProc {
    pub distro: String,
    pub pid: u32,
    /// percent of the whole machine (all Windows logical CPUs)
    pub cpu: f64,
    pub rss: f64,
    pub user: String,
    pub cmd: String,
    pub label: String,
    pub project: Option<String>,
    pub tag: &'static str,
}

impl LinuxProc {
    pub fn context(&self) -> String {
        match &self.project {
            Some(p) => format!("({p} · {})", self.distro),
            None => format!("({})", self.distro),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub path: String,
    pub parent: Option<usize>,
    pub kids: Vec<usize>,
    pub own_cpu: f64,
    pub own_mem: f64,
    pub sub_cpu: f64,
    pub sub_mem: f64,
    pub threads: u32,
    pub tag: &'static str,
    pub jev_tag: bool,
}

pub struct Snap {
    pub procs: Vec<Proc>,
    pub roots: Vec<usize>,
    pub cpu_ready: bool,
    /// whole machine, percent
    pub total: f64,
    pub user: f64,
    pub kernel: f64,
    pub cores: Vec<f64>,
    /// per core, the kernel-mode part of `cores` (percent of that core)
    pub cores_kernel: Vec<f64>,
    pub mem_total: f64,
    pub mem_used: f64,
    pub ncpu: usize,
    pub wsl: Option<Vec<LinuxProc>>,
    /// the VM process whose CPU is the Linux side's (vmmemWSL, else vmmem, else vmwp);
    /// the Linux rows hang under it
    pub vm_host: Option<usize>,
    pub time: String,
}

#[derive(Default)]
pub struct Inputs {
    pub prev: Option<Sample>,
    pub cur: Option<Sample>,
    /// (pid, create time) -> image path
    pub paths: HashMap<(u32, i64), String>,
    pub wsl: Option<Vec<LinuxProc>>,
    pub jev_tags: HashMap<String, &'static str>,
    pub time: String,
}

pub fn is_vm(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "vmmem" || n == "vmmemwsl" || n == "vmwp"
}

/// Windows components the GPU rules call "other" but are plainly the system.
fn system_ish(name: &str, path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    if p.starts_with(r"c:\windows\") && !p.contains(r"\windowsapps\") {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "smss" | "wininit" | "winlogon" | "services" | "lsass" | "svchost" | "fontdrvhost" | "lsaiso" | "memcompression"
            | "secure system" | "spoolsv" | "searchindexer" | "msmpeng" | "wudfhost" | "dashost" | "sihost" | "ctfmon"
            | "taskhostw" | "runtimebroker" | "dllhost" | "conhost" | "securityhealthservice" | "wmiprvse"
    )
}

pub fn cpu_tag(name: &str, path: &str) -> &'static str {
    // shells live in System32 but are not the system
    if matches!(name.to_ascii_lowercase().as_str(), "powershell" | "pwsh" | "cmd" | "bash" | "wsl" | "wslhost" | "wslrelay") {
        return "terminal";
    }
    let t = tags::tag(name, path, "", None);
    if t == "other" && system_ish(name, path) { "system" } else { t }
}

impl Inputs {
    /// Takes a new sample (the previous one becomes the baseline for CPU %).
    pub fn sample(&mut self) {
        let s = Sample::take();
        self.prev = self.cur.replace(s);
        self.time = crate::gpu::sys::local_time();
    }

    /// Image paths for processes not seen before (query-limited handles, read-only).
    pub fn fill_paths(&mut self) {
        let Some(cur) = &self.cur else { return };
        for p in &cur.procs {
            self.paths.entry((p.pid, p.create)).or_insert_with(|| crate::gpu::sys::process_path(p.pid).unwrap_or_default());
        }
    }
}

pub fn build(inp: &Inputs) -> Snap {
    let ncpu = sys::ncpu();
    let (mt, ma) = sys::memory();
    let mut snap = Snap {
        procs: vec![],
        roots: vec![],
        cpu_ready: false,
        total: 0.0,
        user: 0.0,
        kernel: 0.0,
        cores: vec![],
        cores_kernel: vec![],
        mem_total: mt as f64,
        mem_used: mt.saturating_sub(ma) as f64,
        ncpu,
        wsl: inp.wsl.clone(),
        vm_host: None,
        time: inp.time.clone(),
    };
    let Some(cur) = &inp.cur else { return snap };

    // CPU % per process over the window, as a share of the whole machine
    let mut before: HashMap<(u32, i64), i64> = HashMap::new();
    let mut wall = 0.0;
    if let Some(prev) = &inp.prev {
        for p in &prev.procs {
            before.insert((p.pid, p.create), p.cpu);
        }
        wall = cur.at.duration_since(prev.at).as_secs_f64() * 1e7; // 100 ns ticks
        let (di, dk, du) = (cur.sys.0 - prev.sys.0, cur.sys.1 - prev.sys.1, cur.sys.2 - prev.sys.2);
        let busy = (dk + du - di) as f64;
        let all = (dk + du) as f64;
        if all > 0.0 {
            snap.cpu_ready = true;
            snap.total = (busy / all * 100.0).clamp(0.0, 100.0);
            snap.user = (du as f64 / all * 100.0).clamp(0.0, 100.0);
            snap.kernel = (snap.total - snap.user).max(0.0);
        }
        snap.cores = cur
            .cores
            .iter()
            .zip(prev.cores.iter())
            .map(|(c, p)| {
                let (di, dk, du) = (c.0 - p.0, c.1 - p.1, c.2 - p.2);
                let all = (dk + du) as f64;
                if all > 0.0 { ((all - di as f64) / all * 100.0).clamp(0.0, 100.0) } else { 0.0 }
            })
            .collect();
        // kernel time includes idle; what is left over is time spent in kernel mode
        snap.cores_kernel = cur
            .cores
            .iter()
            .zip(prev.cores.iter())
            .map(|(c, p)| {
                let (di, dk, du) = (c.0 - p.0, c.1 - p.1, c.2 - p.2);
                let all = (dk + du) as f64;
                if all > 0.0 { ((dk - di) as f64 / all * 100.0).clamp(0.0, 100.0) } else { 0.0 }
            })
            .collect();
    }

    let list: Vec<&PInfo> = cur.procs.iter().filter(|p| p.pid != 0).collect();
    let idx: HashMap<u32, usize> = list.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    for p in &list {
        let own_cpu = match before.get(&(p.pid, p.create)) {
            Some(b) if wall > 0.0 => ((p.cpu - b).max(0) as f64 / (wall * ncpu as f64) * 100.0).min(100.0),
            _ => 0.0,
        };
        let path = inp.paths.get(&(p.pid, p.create)).cloned().unwrap_or_default();
        let mut tag = cpu_tag(&p.name, &path);
        let mut jev_tag = false;
        if tag == "other" {
            if let Some(t) = inp.jev_tags.get(&p.name.to_lowercase()) {
                tag = t;
                jev_tag = true;
            }
        }
        snap.procs.push(Proc {
            pid: p.pid,
            ppid: p.ppid,
            name: p.name.clone(),
            path,
            parent: None,
            kids: vec![],
            own_cpu,
            own_mem: p.mem as f64,
            sub_cpu: 0.0,
            sub_mem: 0.0,
            threads: p.threads,
            tag,
            jev_tag,
        });
    }
    // parents: must exist and have started no later than the child (pid reuse)
    for i in 0..snap.procs.len() {
        let (ppid, pid) = (snap.procs[i].ppid, snap.procs[i].pid);
        let parent = idx.get(&ppid).copied().filter(|&j| j != i && ppid != pid && list[j].create <= list[i].create);
        snap.procs[i].parent = parent;
        match parent {
            Some(j) => snap.procs[j].kids.push(i),
            None => snap.roots.push(i),
        }
    }
    // roll up (iterative post-order; the create-time rule rules out cycles)
    let mut order = vec![];
    let mut stack: Vec<usize> = snap.roots.clone();
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(snap.procs[i].kids.iter().copied());
    }
    for &i in order.iter().rev() {
        let (mut c, mut m) = (snap.procs[i].own_cpu, snap.procs[i].own_mem);
        for &k in &snap.procs[i].kids {
            c += snap.procs[k].sub_cpu;
            m += snap.procs[k].sub_mem;
        }
        snap.procs[i].sub_cpu = c;
        snap.procs[i].sub_mem = m;
    }
    let rank = |n: &str| match n.to_ascii_lowercase().as_str() {
        "vmmemwsl" => 3,
        "vmmem" => 2,
        "vmwp" => 1,
        _ => 0,
    };
    snap.vm_host = (0..snap.procs.len())
        .filter(|&i| rank(&snap.procs[i].name) > 0)
        .max_by(|&a, &b| {
            let (pa, pb) = (&snap.procs[a], &snap.procs[b]);
            rank(&pa.name).cmp(&rank(&pb.name)).then(pa.own_cpu.total_cmp(&pb.own_cpu))
        });
    // the VM does whatever its busiest Linux workload does
    if let Some(w) = &snap.wsl {
        if let Some(top) = w.iter().filter(|l| l.tag != "other").max_by(|a, b| a.cpu.total_cmp(&b.cpu).then(a.rss.total_cmp(&b.rss))) {
            for p in snap.procs.iter_mut().filter(|p| is_vm(&p.name)) {
                p.tag = top.tag;
            }
        }
    }
    snap
}

/// Same-name siblings shown as one row ("chrome ×40") unless `--all`.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub members: Vec<usize>,
    pub own_cpu: f64,
    pub own_mem: f64,
    pub sub_cpu: f64,
    pub sub_mem: f64,
    pub tag: &'static str,
    pub jev_tag: bool,
}

impl Group {
    pub fn kids(&self, s: &Snap) -> Vec<usize> {
        self.members.iter().flat_map(|&m| s.procs[m].kids.iter().copied()).collect()
    }
    pub fn is_vm(&self) -> bool {
        is_vm(&self.name)
    }
    /// Whether the Linux rows belong under this row.
    pub fn hosts_linux(&self, s: &Snap) -> bool {
        s.vm_host.is_some_and(|h| self.members.contains(&h))
    }
}

pub fn groups(s: &Snap, ids: &[usize], merge: bool, metric_mem: bool) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    for &i in ids {
        let p = &s.procs[i];
        let key = if merge { p.name.to_lowercase() } else { format!("{}#{}", p.name.to_lowercase(), p.pid) };
        match at.get(&key) {
            Some(&g) => {
                let g = &mut out[g];
                g.members.push(i);
                g.own_cpu += p.own_cpu;
                g.own_mem += p.own_mem;
                g.sub_cpu += p.sub_cpu;
                g.sub_mem += p.sub_mem;
            }
            None => {
                at.insert(key, out.len());
                out.push(Group {
                    name: p.name.clone(),
                    members: vec![i],
                    own_cpu: p.own_cpu,
                    own_mem: p.own_mem,
                    sub_cpu: p.sub_cpu,
                    sub_mem: p.sub_mem,
                    tag: p.tag,
                    jev_tag: p.jev_tag,
                });
            }
        }
    }
    sort_groups(&mut out, metric_mem);
    out
}

pub fn sort_groups(v: &mut [Group], metric_mem: bool) {
    v.sort_by(|a, b| {
        let k = |g: &Group| if metric_mem { (g.sub_mem, g.sub_cpu) } else { (g.sub_cpu, g.sub_mem) };
        let (ka, kb) = (k(a), k(b));
        kb.0.total_cmp(&ka.0).then(kb.1.total_cmp(&ka.1)).then(a.name.cmp(&b.name))
    });
}

/// Rollup by tag over all processes (own numbers, so nothing is counted twice).
pub fn by_tag(s: &Snap, metric_mem: bool) -> Vec<(&'static str, f64, f64, Vec<usize>)> {
    let mut m: Vec<(&'static str, f64, f64, Vec<usize>)> = vec![];
    for (i, p) in s.procs.iter().enumerate() {
        match m.iter_mut().find(|t| t.0 == p.tag) {
            Some(t) => {
                t.1 += p.own_cpu;
                t.2 += p.own_mem;
                t.3.push(i);
            }
            None => m.push((p.tag, p.own_cpu, p.own_mem, vec![i])),
        }
    }
    m.sort_by(|a, b| if metric_mem { b.2.total_cmp(&a.2) } else { b.1.total_cmp(&a.1).then(b.2.total_cmp(&a.2)) });
    m
}
