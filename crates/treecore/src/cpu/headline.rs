//! cputree's plain-English sentence, from templates. Jev (optional) only picks one.

use super::model::{Snap, groups};
use crate::gpu::headline::tag_phrase;
use crate::pretty_name;
use crate::term::fmt_bytes;

pub struct Cand {
    pub kind: &'static str,
    pub text: String,
}

/// A named contributor: ("your training run in WSL", 22.0, tag).
struct Who {
    label: String,
    cpu: f64,
    mem: f64,
    tag: &'static str,
}

fn contributors(s: &Snap) -> Vec<Who> {
    let all: Vec<usize> = (0..s.procs.len()).collect();
    let mut v: Vec<Who> = vec![];
    for g in groups(s, &all, true, false) {
        if g.is_vm() && s.vm_host.is_some() && !g.hosts_linux(s) {
            continue; // vmwp / vmcompute: the host process carries the VM's CPU
        }
        if g.is_vm() {
            // attribute the VM to what runs inside it, when we can see that
            if let Some(w) = s.wsl.as_ref().filter(|w| !w.is_empty()) {
                // the busiest process names the workload; the rest of the VM is folded in
                let top = w.iter().max_by(|a, b| a.cpu.total_cmp(&b.cpu)).unwrap();
                let label = match top.tag {
                    "training" => "your training run in WSL".to_string(),
                    "ai inference" => "AI inference in WSL".to_string(),
                    _ => format!("{} in WSL", top.label.split_whitespace().next().unwrap_or("Linux")),
                };
                let cpu = g.sub_cpu.max(w.iter().map(|l| l.cpu).sum());
                v.push(Who { label, cpu, mem: g.sub_mem.max(w.iter().map(|l| l.rss).sum()), tag: top.tag });
                continue;
            }
            v.push(Who { label: "the WSL VM".into(), cpu: g.own_cpu, mem: g.own_mem, tag: "vm" });
            continue;
        }
        let label = if g.name.eq_ignore_ascii_case("system") { "Windows (System)".into() } else { pretty_name(&g.name) };
        v.push(Who { label, cpu: g.own_cpu, mem: g.own_mem, tag: g.tag });
    }
    v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    v
}

fn cap(s: &str) -> String {
    pretty_name(s)
}

pub fn candidates(s: &Snap) -> Vec<Cand> {
    let mut out = vec![];
    if !s.cpu_ready {
        return out;
    }
    let who = contributors(s);
    let t = s.total;
    let mem_pct = if s.mem_total > 0.0 { s.mem_used / s.mem_total * 100.0 } else { 0.0 };
    let two = |w: &[Who]| -> String {
        match w {
            [a, b, ..] if b.cpu >= 1.0 => format!("{} ({:.0}%) and {} ({:.0}%)", a.label, a.cpu, b.label, b.cpu),
            [a, ..] => format!("{} ({:.0}%)", a.label, a.cpu),
            [] => "nothing in particular".into(),
        }
    };
    let top2: f64 = who.iter().take(2).map(|w| w.cpu).sum();
    let detail = format!(
        "CPU is {t:.0}% busy — {} {}.",
        if top2 >= t * 0.5 { "mostly" } else { "the biggest users are" },
        two(&who)
    );
    let idle = match who.first() {
        Some(w) if w.cpu >= 0.5 => format!("CPU is mostly idle ({t:.0}%); the biggest user is {} ({:.0}%).", w.label, w.cpu),
        _ => format!("CPU is idle ({t:.0}%)."),
    };
    if t >= 15.0 {
        out.push(Cand { kind: "detail", text: detail });
    } else {
        out.push(Cand { kind: "idle", text: idle });
        out.push(Cand { kind: "detail", text: detail });
    }
    // tag-first
    let mut tags: Vec<(&'static str, f64)> = vec![];
    for w in &who {
        match tags.iter_mut().find(|x| x.0 == w.tag) {
            Some(x) => x.1 += w.cpu,
            None => tags.push((w.tag, w.cpu)),
        }
    }
    tags.sort_by(|a, b| b.1.total_cmp(&a.1));
    if let Some((tag, c)) = tags.first().filter(|x| x.1 >= 1.0) {
        out.push(Cand {
            kind: "tag",
            text: format!("{} is using the most CPU ({c:.0}% of the {t:.0}% in use).", cap(tag_phrase(tag))),
        });
    }
    // memory angle
    let mut by_mem: Vec<&Who> = who.iter().collect();
    by_mem.sort_by(|a, b| b.mem.total_cmp(&a.mem));
    if let Some(m) = by_mem.first() {
        out.push(Cand {
            kind: "mem",
            text: format!(
                "RAM is {mem_pct:.0}% used ({} of {}); {} holds the most ({}), and CPU is {t:.0}% busy.",
                fmt_bytes(s.mem_used),
                fmt_bytes(s.mem_total),
                m.label,
                fmt_bytes(m.mem)
            ),
        });
    }
    out
}
