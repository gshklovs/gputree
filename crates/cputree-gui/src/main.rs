//! cputree-gui: what is using your CPU, as a disktree-style treemap of the real
//! process tree, sized by rolled-up CPU (or memory). Same-name siblings are
//! merged, and the WSL2 VM opens into the Linux processes inside it.
//! Read-only: there is no action anywhere that ends or changes a process.

#![windows_subsystem = "windows"]

use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use treecore::cpu::model::{self, Group, Inputs, LinuxProc, Snap, groups};
use treecore::cpu::{collect, headline, sys};
use treecore::jev::{self, JevResult};
use treecore::term::fmt_bytes;
use treeview::{Config, Frame, Metric, VNode};

const REFRESH: Duration = Duration::from_millis(1500);
const CPU_WINDOW: Duration = Duration::from_millis(300);

const USAGE: &str = "\
cputree-gui - what is using your CPU, as a treemap of the process tree (read-only)

usage: cputree-gui [--metric cpu|mem] [--depth 1-6] [--no-wsl] [--no-ai]

  t switches CPU / memory, enter zooms in, backspace goes up, ? lists every key.
";

struct Opts {
    mem: bool,
    depth: u32,
    no_wsl: bool,
    no_ai: bool,
}

fn parse() -> Result<Opts, String> {
    let mut o = Opts { mem: false, depth: 3, no_wsl: false, no_ai: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.trim_start_matches('-').to_ascii_lowercase().replace('-', "").as_str() {
            "h" | "help" => return Err(USAGE.into()),
            "m" | "metric" => o.mem = args.next().is_some_and(|v| v.to_ascii_lowercase().starts_with("mem")),
            "d" | "depth" => o.depth = args.next().and_then(|v| v.parse().ok()).filter(|d| (1..=6).contains(d)).ok_or("--depth needs 1-6")?,
            "nowsl" => o.no_wsl = true,
            "noai" => o.no_ai = true,
            _ => return Err(format!("unknown option {a}\n\n{USAGE}")),
        }
    }
    Ok(o)
}

fn pct(v: f64) -> String {
    if v > 0.0 && v < 9.95 { format!("{v:.1}%") } else { format!("{v:.0}%") }
}

enum Ev {
    Wsl(Vec<LinuxProc>),
    Jev(Result<JevResult, String>),
}

fn linux_node(parent: &str, l: &LinuxProc) -> VNode {
    let mut n = VNode::new(format!("{parent}/lx:{}:{}", l.distro, l.pid), l.label.clone(), l.tag, [l.cpu, l.rss])
        .detail("pid", format!("{} in {} (user {})", l.pid, l.distro, l.user))
        .detail("command", l.cmd.clone())
        .detail("CPU", format!("{} of the whole machine, measured inside the VM", pct(l.cpu)))
        .detail("RSS", fmt_bytes(l.rss));
    n.sub = l.context();
    n
}

/// One merged group and everything under it, recursively.
fn group_node(s: &Snap, parent: &str, g: &Group) -> VNode {
    let id = format!("{parent}/{}", g.name.to_lowercase());
    let first = &s.procs[g.members[0]];
    let mut n = VNode::new(id.clone(), g.name.clone(), g.tag, [g.sub_cpu, g.sub_mem]);
    n.jev_tag = g.jev_tag;
    if g.members.len() > 1 {
        n.sub = format!("×{}", g.members.len());
    } else if g.is_vm() {
        n.sub = "(WSL2 VM)".into();
    }
    let pids: Vec<String> = g.members.iter().take(8).map(|&m| s.procs[m].pid.to_string()).collect();
    n = n.detail(
        if g.members.len() > 1 { "pids" } else { "pid" },
        if g.members.len() > 8 { format!("{} +{}", pids.join(", "), g.members.len() - 8) } else { pids.join(", ") },
    );
    if !first.path.is_empty() {
        n = n.detail("path", first.path.clone());
    }
    let threads: u32 = g.members.iter().map(|&m| s.procs[m].threads).sum();
    n = n
        .detail("CPU", format!("{} with everything under it · {} itself", pct(g.sub_cpu), pct(g.own_cpu)))
        .detail("memory", format!("{} with everything under it · {} itself", fmt_bytes(g.sub_mem), fmt_bytes(g.own_mem)))
        .detail("threads", threads.to_string());

    let kids = g.kids(s);
    let hosts = g.hosts_linux(s);
    if !kids.is_empty() || hosts {
        // the process's own share, beside its children, so areas add up
        if g.own_cpu > 0.0 || g.own_mem > 0.0 {
            let mut me = VNode::new(format!("{id}/~self"), format!("{} itself", g.name), g.tag, [g.own_cpu, g.own_mem]);
            me.details = n.details.clone();
            n.children.push(me);
        }
        if hosts {
            if let Some(w) = &s.wsl {
                n = n.detail("inside", w.iter().take(3).map(|l| format!("{} {}", l.label, l.context())).collect::<Vec<_>>().join("; "));
                for l in w {
                    n.children.push(linux_node(&id, l));
                }
            }
        }
        for c in groups(s, &kids, true, false) {
            n.children.push(group_node(s, &id, &c));
        }
    }
    n
}

fn sparkline(cores: &[f64]) -> String {
    const V: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    cores.iter().map(|c| V[((c / 100.0) * 8.0).ceil().clamp(1.0, 8.0) as usize - 1]).collect()
}

fn to_frame(inp: &Inputs, jev_kind: &Option<String>, phase: u8) -> Frame {
    let s = model::build(inp);
    let mut headline = headline::candidates(&s).into_iter().next().map(|c| c.text).unwrap_or_else(|| "Sampling CPU load…".into());
    let mut source = "";
    if let Some(k) = jev_kind {
        if let Some(c) = headline::candidates(&s).into_iter().find(|c| c.kind == k) {
            headline = c.text;
            source = "jev";
        }
    }
    let mut root = VNode::new("pc", "This PC", "other", [s.total, s.mem_used])
        .detail("load", format!("{} busy · user {} · kernel {} · {} logical CPUs", pct(s.total), pct(s.user), pct(s.kernel), s.ncpu))
        .detail("RAM", format!("{} of {} in use", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)));
    for g in groups(&s, &s.roots, true, false) {
        root.children.push(group_node(&s, "pc", &g));
    }
    let mut summary = vec![
        ("CPU".to_string(), if s.cpu_ready { pct(s.total) } else { "…".into() }),
        ("RAM".to_string(), format!("{} of {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total))),
    ];
    if !s.cores.is_empty() {
        summary.push(("cores".to_string(), sparkline(&s.cores)));
    }
    let note = if s.wsl.as_ref().is_some_and(|w| !w.is_empty()) { "WSL rows: CPU measured inside the VM".to_string() } else { String::new() };
    Frame { root: Some(root), headline, headline_source: source, summary, phase, time: s.time, note }
}

fn collector(tx: Sender<Frame>, o: Opts) {
    let mut inp = Inputs::default();
    inp.sample();
    inp.fill_paths();
    if tx.send(to_frame(&inp, &None, 1)).is_err() {
        return;
    }
    let key = jev::api_key().filter(|_| !o.no_ai);
    let (etx, erx) = channel::<Ev>();
    let (mut wsl_busy, mut jev_busy) = (false, false);
    let mut wsl_at: Option<Instant> = None;
    let mut jev_at: Option<Instant> = None;
    let mut jev_kind: Option<String> = None;
    let mut awaiting = 0usize;
    let mut phase3 = false;
    let mut first = true;
    let started = Instant::now();
    std::thread::sleep(CPU_WINDOW);
    loop {
        inp.sample();
        inp.fill_paths();
        let has_vm = inp.cur.as_ref().is_some_and(|c| c.procs.iter().any(|p| model::is_vm(&p.name)));
        if !o.no_wsl && has_vm && !wsl_busy && wsl_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            wsl_busy = true;
            awaiting += usize::from(first);
            let etx = etx.clone();
            let n = sys::ncpu();
            std::thread::spawn(move || {
                let _ = etx.send(Ev::Wsl(collect::wsl_cpu_procs(n)));
            });
        }
        let want_jev = |wsl_busy: bool, jev_busy: bool, jev_at: Option<Instant>| {
            key.is_some() && !wsl_busy && !jev_busy && jev_at.is_none_or(|t| t.elapsed() > Duration::from_secs(120))
        };
        let spawn_jev = |inp: &Inputs| -> bool {
            let snap = model::build(inp);
            let known: HashMap<String, &'static str> = inp.jev_tags.clone();
            let Some(job) = collect::jev_job(&snap, &known) else { return false };
            let (etx, key) = (etx.clone(), key.clone().unwrap());
            std::thread::spawn(move || {
                let _ = etx.send(Ev::Jev(job.run(&key, Duration::from_secs(3))));
            });
            true
        };
        if want_jev(wsl_busy, jev_busy, jev_at) && spawn_jev(&inp) {
            jev_busy = true;
            jev_at = Some(Instant::now());
            awaiting += usize::from(first);
        }
        if first && awaiting == 0 {
            phase3 = true;
        }
        first = false;
        if tx.send(to_frame(&inp, &jev_kind, if phase3 { 3 } else { 2 })).is_err() {
            return;
        }
        let tick = Instant::now() + REFRESH;
        while let Some(left) = tick.checked_duration_since(Instant::now()) {
            let Ok(ev) = erx.recv_timeout(left) else { break };
            match ev {
                Ev::Wsl(v) => {
                    wsl_busy = false;
                    wsl_at = Some(Instant::now());
                    inp.wsl = Some(v);
                }
                Ev::Jev(r) => {
                    jev_busy = false;
                    if let Ok(j) = r {
                        if j.kind.is_some() {
                            jev_kind = j.kind;
                        }
                        inp.jev_tags.extend(j.tags);
                    }
                }
            }
            if !phase3 {
                awaiting = awaiting.saturating_sub(1);
                phase3 = awaiting == 0 || started.elapsed() > Duration::from_millis(2500);
            }
            if want_jev(wsl_busy, jev_busy, jev_at) && spawn_jev(&inp) {
                jev_busy = true;
                jev_at = Some(Instant::now());
                if !phase3 {
                    awaiting += 1;
                }
            }
            if tx.send(to_frame(&inp, &jev_kind, if phase3 { 3 } else { 2 })).is_err() {
                return;
            }
        }
        if started.elapsed() > Duration::from_millis(2500) {
            phase3 = true;
        }
    }
}

fn main() {
    let opts = match parse() {
        Ok(o) => o,
        Err(e) => {
            treeview::console::attach();
            eprintln!("{e}");
            treeview::console::detach();
            std::process::exit(if e == USAGE { 0 } else { 2 });
        }
    };
    let cfg = Config {
        app_name: "cputree",
        metrics: [Metric { name: "CPU", format: pct }, Metric { name: "Memory", format: fmt_bytes }],
        depth: opts.depth,
        metric: usize::from(opts.mem),
        help: vec![
            ("CPU", "The process tree by CPU, rolled up: a tile is the process and everything it started"),
            ("Memory", "The same tree by private working set"),
        ],
        root_label: "This PC",
        refresh: REFRESH,
    };
    treeview::run(cfg, move |tx| {
        std::thread::spawn(move || collector(tx, opts));
    });
}
