//! gputree-gui: what is using your GPUs, as a disktree-style treemap.
//! Adapters -> processes -> engines (util) or the Linux processes inside the
//! WSL2 VM (VRAM). Read-only: it samples counters and reads /proc; there is no
//! action anywhere that ends or changes a process.

#![windows_subsystem = "windows"]

use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use treecore::gpu::model::{self, Inputs, Snap, WslProc};
use treecore::gpu::{collect, headline, nvidia, sys};
use treecore::jev::{self, JevResult};
use treecore::term::fmt_bytes;
use treeview::{Config, Frame, Metric, VNode};

const REFRESH: Duration = Duration::from_millis(1500);
const UTIL_WINDOW: Duration = Duration::from_millis(280);

const USAGE: &str = "\
gputree-gui - what is using your GPUs, as a treemap (read-only)

usage: gputree-gui [--metric vram|util] [--depth 1-6] [--no-wsl] [--no-ai]

  t switches VRAM / util, enter zooms in, backspace goes up, ? lists every key.
";

struct Opts {
    util: bool,
    depth: u32,
    no_wsl: bool,
    no_ai: bool,
}

fn parse() -> Result<Opts, String> {
    let mut o = Opts { util: false, depth: 3, no_wsl: false, no_ai: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.trim_start_matches('-').to_ascii_lowercase().replace('-', "").as_str() {
            "h" | "help" => return Err(USAGE.into()),
            "m" | "metric" => o.util = args.next().is_some_and(|v| v.eq_ignore_ascii_case("util")),
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
    Nv(Vec<nvidia::NvStats>),
    Wsl(Vec<WslProc>),
    Jev(Result<JevResult, String>),
}

fn refresh_names(inp: &mut Inputs) {
    inp.names = sys::process_names();
    let pids: Vec<u32> = inp.raw.procs.keys().map(|k| k.1).collect();
    for pid in pids {
        inp.paths.entry(pid).or_insert_with(|| sys::process_path(pid));
    }
}

fn nvidia_awake(inp: &Inputs) -> bool {
    inp.adapters.iter().any(|(luid, a)| {
        a.name.contains("NVIDIA")
            && (inp.raw.adapter_mem.get(luid).is_some_and(|m| m.0 > 0.0) || inp.raw.procs.iter().any(|(k, r)| k.0 == *luid && r.ded > 0.0))
    })
}

fn vm_busy(inp: &Inputs) -> bool {
    inp.raw.procs.iter().any(|(&(_, pid), r)| {
        inp.names.get(&pid).is_some_and(|n| n.eq_ignore_ascii_case("vmwp")) && (r.ded > 0.0 || r.eng.values().any(|v| *v > 0.0))
    })
}

fn engines(eng: &std::collections::BTreeMap<String, f64>) -> String {
    let mut e: Vec<(&String, &f64)> = eng.iter().filter(|(_, v)| **v > 0.05).collect();
    e.sort_by(|a, b| b.1.total_cmp(a.1));
    if e.is_empty() { "idle".into() } else { e.iter().map(|(k, v)| format!("{k} {}", pct(**v))).collect::<Vec<_>>().join(" · ") }
}

/// Snapshot -> the window's tree.
fn to_frame(inp: &Inputs, jev_kind: &Option<String>, phase: u8) -> Frame {
    let s: Snap = model::build(inp);
    let mut headline = headline::candidates(&s).into_iter().next().map(|c| c.text).unwrap_or_else(|| "Sampling GPU load…".into());
    let mut source = "";
    if let Some(k) = jev_kind {
        if let Some(c) = headline::candidates(&s).into_iter().find(|c| c.kind == k) {
            headline = c.text;
            source = "jev";
        }
    }
    let mut root = VNode::new("gpus", "GPUs", "other", [0.0, 0.0]);
    let mut summary = vec![];
    let mut gpus: Vec<&model::Gpu> = s.gpus.iter().collect();
    gpus.sort_by(|a, b| b.mem.total_cmp(&a.mem));
    for g in gpus {
        let gid = format!("gpus/a{:x}", g.luid);
        let tag = model::by_tag(g, s.util_ready && g.util >= 1.0).first().map_or("other", |t| t.tag);
        let util = if s.util_ready { g.util } else { 0.0 };
        let mut a = VNode::new(gid.clone(), g.name.clone(), tag, [g.mem, util])
            .detail("memory", if g.cap > 0.0 { format!("{} of {}{}", fmt_bytes(g.mem), fmt_bytes(g.cap), if g.integrated { " shared" } else { "" }) } else { fmt_bytes(g.mem) })
            .detail("engines", if s.util_ready { engines(&g.eng) } else { "sampling…".into() });
        if let Some(nv) = &g.nv {
            let mut bits = vec![];
            if let Some(t) = nv.temp {
                bits.push(format!("{t} °C"));
            }
            if let Some(p) = nv.power_w {
                bits.push(format!("{p:.0} W"));
            }
            a = a.detail("temperature · power", bits.join(" · "));
        }
        a = a.detail("processes", format!("{} with a GPU handle", g.procs.len()));
        summary.push((g.short.clone(), if s.util_ready { format!("{} · {}", pct(g.util), fmt_bytes(g.mem)) } else { fmt_bytes(g.mem) }));
        for p in &g.procs {
            let pm = g.pmem(p);
            let pu = if s.util_ready { p.util } else { 0.0 };
            let pid_id = format!("{gid}/p{}", p.pid);
            let vm = p.name.eq_ignore_ascii_case("vmwp");
            let mut n = VNode::new(pid_id.clone(), p.name.clone(), p.tag, [pm, pu]);
            n.jev_tag = p.jev_tag;
            if vm {
                n.sub = "(WSL2 VM)".into();
            }
            n = n.detail("pid", p.pid.to_string());
            if let Some(Some(path)) = inp.paths.get(&p.pid) {
                n = n.detail("path", path.clone());
            }
            n = n
                .detail("GPU memory", format!("{} dedicated · {} shared", fmt_bytes(p.ded), fmt_bytes(p.shr)))
                .detail("engines", if s.util_ready { engines(&p.eng) } else { "sampling…".into() });
            // util mode: engines inside; VRAM mode: the Linux processes inside the VM
            for (k, v) in p.eng.iter().filter(|(_, v)| **v > 0.05 && s.util_ready) {
                n.children.push(VNode::new(format!("{pid_id}/e:{k}"), k.clone(), p.tag, [0.0, *v]).detail("engine", format!("{k} {}", pct(*v))));
            }
            if vm {
                if let Some(w) = &s.wsl {
                    let total: f64 = w.iter().map(|x| x.rss).sum::<f64>().max(1.0);
                    let inside: Vec<String> = w.iter().take(3).map(|x| format!("{} {}", x.label, x.context())).collect();
                    if !inside.is_empty() {
                        n = n.detail("inside", inside.join("; "));
                    }
                    for x in w {
                        let mut c = VNode::new(format!("{pid_id}/w:{}:{}", x.distro, x.pid), x.label.clone(), x.tag, [pm * x.rss / total, 0.0])
                            .detail("pid", format!("{} in {} (user {})", x.pid, x.distro, x.user))
                            .detail("command", x.cmd.clone())
                            .detail("RSS", fmt_bytes(x.rss))
                            .detail("sized by", "the VM's VRAM split by RSS (per-process GPU use is not visible from Windows)");
                        c.sub = x.context();
                        n.children.push(c);
                    }
                }
            }
            a.children.push(n);
        }
        if g.integrated && !g.npu {
            a = a.detail("note", "per-process shared usage counts pages several processes map, so it can add up to more than the adapter");
        }
        root.values[0] += g.mem;
        // the busiest adapter, not a sum: 100% means one GPU is flat out
        root.values[1] = root.values[1].max(util);
        root.children.push(a);
    }
    let note = if s.wsl.as_ref().is_some_and(|w| !w.is_empty()) { "WSL: VRAM split by RSS; GPU % is the VM's".to_string() } else { String::new() };
    Frame { root: Some(root), headline, headline_source: source, summary, phase, time: s.time, note }
}

fn collector(tx: Sender<Frame>, o: Opts) {
    let Some(mut counters) = sys::Counters::open() else {
        let _ = tx.send(Frame { headline: "Could not open the GPU performance counters.".into(), phase: 3, ..Frame::default() });
        return;
    };
    let mut inp = Inputs { adapters: sys::adapters(), npu: sys::npu_name(), ..Default::default() };
    let key = jev::api_key().filter(|_| !o.no_ai);
    let (etx, erx) = channel::<Ev>();
    let (mut nv_busy, mut wsl_busy, mut jev_busy) = (false, false, false);
    let mut wsl_at: Option<Instant> = None;
    let mut jev_at: Option<Instant> = None;
    let mut jev_kind: Option<String> = None;
    let mut first = true;
    // phase 3 once the slow inputs asked for on the first round have landed
    let mut awaiting: usize = 0;
    let mut phase3 = false;
    let started = Instant::now();
    loop {
        if !first {
            counters.collect();
        }
        inp.raw = counters.read();
        inp.time = sys::local_time();
        refresh_names(&mut inp);
        if !inp.raw.util_ready {
            if tx.send(to_frame(&inp, &jev_kind, 1)).is_err() {
                return;
            }
            std::thread::sleep(UTIL_WINDOW);
            counters.collect();
            inp.raw = counters.read();
            refresh_names(&mut inp);
        }
        if nvidia_awake(&inp) {
            if !nv_busy {
                nv_busy = true;
                awaiting += usize::from(first);
                let etx = etx.clone();
                std::thread::spawn(move || {
                    let _ = etx.send(Ev::Nv(nvidia::query()));
                });
            }
        } else {
            inp.nv = None;
        }
        if !o.no_wsl && vm_busy(&inp) && !wsl_busy && wsl_at.is_none_or(|t| t.elapsed() > Duration::from_secs(4)) {
            wsl_busy = true;
            awaiting += usize::from(first);
            let etx = etx.clone();
            std::thread::spawn(move || {
                let _ = etx.send(Ev::Wsl(collect::wsl_gpu_procs()));
            });
        }
        let want_jev = |wsl_busy: bool, jev_busy: bool, jev_at: Option<Instant>| {
            key.is_some() && !wsl_busy && !jev_busy && jev_at.is_none_or(|t| t.elapsed() > Duration::from_secs(120))
        };
        let spawn_jev = |inp: &Inputs| -> bool {
            let snap = model::build(inp);
            let Some(job) = collect::jev_job(&snap, &inp.jev_tags) else { return false };
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
        let phase = if phase3 { 3 } else { 2 };
        if tx.send(to_frame(&inp, &jev_kind, phase)).is_err() {
            return;
        }
        let tick = Instant::now() + REFRESH;
        while let Some(left) = tick.checked_duration_since(Instant::now()) {
            let Ok(ev) = erx.recv_timeout(left) else { break };
            match ev {
                Ev::Nv(v) => {
                    nv_busy = false;
                    inp.nv = Some(v);
                }
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
        app_name: "gputree",
        metrics: [Metric { name: "VRAM", format: fmt_bytes }, Metric { name: "Util", format: pct }],
        depth: opts.depth,
        metric: usize::from(opts.util),
        help: vec![("VRAM", "Adapters by memory in use; the WSL VM opens into its Linux processes (split by RSS)"), ("Util", "Adapters and processes by GPU time; processes open into their engines")],
        root_label: "GPUs",
        refresh: REFRESH,
    };
    treeview::run(cfg, move |tx| {
        std::thread::spawn(move || collector(tx, opts));
    });
}
