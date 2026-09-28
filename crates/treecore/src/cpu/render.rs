//! cputree's screen: totals, per-core bars, then the process tree on the shared grid
//! (`crate::layout`) with cells [Σcpu, own cpu, Σmem].

use super::Opts;
use super::headline;
use super::model::{Group, LinuxProc, Snap, by_tag, groups};
use crate::layout::{Cell, DIM, Headline, Out, Row, blank, cell, child_prefix, layout};
use crate::tags;
use crate::term::{Line, color_on, fmt_bytes, fmt_pair};

const PCT: usize = 5;
const MEM: usize = 9;

fn pct(v: f64) -> String {
    if v > 0.0 && v < 0.95 { format!("{v:.1}%") } else { format!("{:.0}%", v.min(100.0)) }
}

enum Child<'a> {
    Linux(&'a LinuxProc),
    Win(Group),
    More { n: usize, cpu: f64, mem: f64, what: &'static str },
}

struct Ctx<'a> {
    o: &'a Opts,
    s: &'a Snap,
    out: Vec<Out>,
    /// the VM host and its ancestors
    vm_path: Vec<usize>,
}

impl<'a> Ctx<'a> {
    fn cpu_cell(&self, v: f64, sgr: &'static str) -> Cell {
        if self.s.cpu_ready { cell(pct(v), sgr) } else { cell("…", DIM) }
    }

    fn frac(&self, cpu: f64, mem: f64) -> f64 {
        if self.o.metric_mem {
            if self.s.mem_total > 0.0 { mem / self.s.mem_total } else { 0.0 }
        } else if self.s.cpu_ready {
            cpu / 100.0
        } else {
            0.0
        }
    }

    /// Children of one level: Linux processes first under the VM, then Windows groups,
    /// then a "+ N more" summary.
    fn children(&self, ids: &[usize], vm: bool) -> Vec<Child<'a>> {
        let mut v = vec![];
        if vm {
            let s: &'a Snap = self.s;
            if let Some(w) = &s.wsl {
                let n = if self.o.all { w.len() } else { self.o.top.min(w.len()) };
                v.extend(w.iter().take(n).map(Child::Linux));
            }
        }
        let gs = groups(self.s, ids, !self.o.all, self.o.metric_mem);
        let keep = if self.o.all { gs.len() } else { self.o.top.min(gs.len()) };
        let (shown, hidden) = gs.split_at(keep);
        v.extend(shown.iter().cloned().map(Child::Win));
        if !hidden.is_empty() {
            let n: usize = hidden.iter().map(|g| g.members.len()).sum();
            v.push(Child::More {
                n,
                cpu: hidden.iter().map(|g| g.sub_cpu).sum(),
                mem: hidden.iter().map(|g| g.sub_mem).sum(),
                what: "process",
            });
        }
        v
    }

    fn emit(&mut self, c: Child<'a>, prefix: &str, last: bool, level: u8, max_depth: u8) {
        match c {
            Child::Linux(l) => {
                let mut n = Line::new();
                n.push("1", l.label.clone());
                n.push(DIM, format!("  {}", l.context()));
                let mut a1 = Line::new();
                a1.push("1", l.label.clone());
                if let Some(p) = &l.project {
                    a1.push(DIM, format!("  ({p})"));
                }
                let mut a2 = Line::new();
                a2.push("1", l.label.clone());
                let mut r = Row::new(prefix, n);
                r.last = last;
                r.bar = Some(self.frac(l.cpu, l.rss));
                r.cells = vec![cell(pct(l.cpu), ""), cell(pct(l.cpu), DIM), cell(fmt_bytes(l.rss), DIM)];
                r.alts = vec![a1, a2];
                r.tag = (!self.o.group).then_some((l.tag, false));
                r.pid = format!("pid {}", l.pid);
                self.out.push(Out::Row(r));
            }
            Child::More { n, cpu, mem, what } => {
                let mut name = Line::new();
                name.push(DIM, format!("+ {n} more {what}{} (--all to list)", if n == 1 { "" } else { "es" }));
                let mut r = Row::new(prefix, name);
                r.last = last;
                r.cells = vec![self.cpu_cell(cpu, DIM), blank(), cell(fmt_bytes(mem), DIM)];
                self.out.push(Out::Row(r));
            }
            Child::Win(g) => {
                let many = g.members.len() > 1;
                let mut name = Line::new();
                name.plain(g.name.clone());
                if many {
                    name.push(DIM, format!(" ×{}", g.members.len()));
                }
                if g.is_vm() {
                    name.push("35", " (WSL2 VM)");
                }
                let mut r = Row::new(prefix, name);
                r.last = last;
                r.bar = Some(self.frac(g.sub_cpu, g.sub_mem));
                r.cells = vec![self.cpu_cell(g.sub_cpu, ""), self.cpu_cell(g.own_cpu, DIM), cell(fmt_bytes(g.sub_mem), "")];
                r.tag = (!self.o.group).then_some((g.tag, g.jev_tag));
                r.pid = if many { format!("{} procs", g.members.len()) } else { format!("pid {}", self.s.procs[g.members[0]].pid) };
                self.out.push(Out::Row(r));
                // the VM always drills in (its Linux processes are the point), whatever
                // --depth says, and so does the chain of parents leading to it
                let on_path = !self.o.group && (g.is_vm() || g.members.iter().any(|m| self.vm_path.contains(m)));
                let max_depth = if on_path { max_depth.max(level + 2) } else { max_depth };
                if level + 1 < max_depth {
                    let kids = self.children(&g.kids(self.s), g.hosts_linux(self.s));
                    let pre = child_prefix(prefix, last);
                    let n = kids.len();
                    for (i, k) in kids.into_iter().enumerate() {
                        self.emit(k, &pre, i + 1 == n, level + 1, max_depth);
                    }
                }
            }
        }
    }
}

/// Per-core usage as one character each, wrapped over as many lines as needed.
fn core_lines(s: &Snap, width: usize, gutter: usize) -> Vec<Out> {
    const V: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let per = width.saturating_sub(gutter + 2).max(8);
    let mut out = vec![];
    for (i, chunk) in s.cores.chunks(per).enumerate() {
        let mut l = Line::new();
        for &c in chunk {
            let k = ((c / 100.0) * 8.0).ceil().clamp(1.0, 8.0) as usize - 1;
            let fg = if c >= 66.0 {
                "38;5;167"
            } else if c >= 33.0 {
                "38;5;179"
            } else {
                "38;5;108"
            };
            let sgr = if color_on() { format!("{fg};{}", crate::term::BAR_BG) } else { String::new() };
            l.push(&sgr, V[k].to_string());
        }
        out.push(Out::Label { label: if i == 0 { "cores" } else { "" }, bar: None, cells: vec![], rest: l });
    }
    out
}

/// Like `render`, but at most `max_lines` tall (for a terminal screen): fewer children
/// per parent first (the path to the busiest processes is always kept), then fewer
/// levels. `--all` is never folded.
pub fn render_fit(s: &Snap, o: &Opts, head: &Headline, width: usize, max_lines: usize) -> Vec<Line> {
    let full = render(s, o, head, width);
    if o.all || full.len() <= max_lines {
        return full;
    }
    let mut f = o.clone();
    loop {
        if f.top > 1 {
            f.top -= 1;
        } else if f.depth > 1 {
            f.depth -= 1;
        } else {
            let mut l = render(s, &f, head, width);
            l.truncate(max_lines);
            return l;
        }
        let l = render(s, &f, head, width);
        if l.len() <= max_lines {
            return l;
        }
    }
}

pub fn render(s: &Snap, o: &Opts, head: &Headline, width: usize) -> Vec<Line> {
    let mut vm_path = vec![];
    let mut at = s.vm_host;
    while let Some(i) = at {
        if vm_path.contains(&i) {
            break;
        }
        vm_path.push(i);
        at = s.procs[i].parent;
    }
    let mut cx = Ctx { o, s, out: vec![], vm_path };
    cx.out.push(Out::Wrap(head.line()));
    let mut t = Line::new();
    t.push("1", "cputree");
    t.push(
        DIM,
        format!(
            "  {} logical CPUs · {} RAM · ranked by {} · {}",
            s.ncpu,
            fmt_bytes(s.mem_total),
            if o.metric_mem { "memory" } else { "cpu" },
            s.time
        ),
    );
    cx.out.push(Out::Free(t));
    cx.out.push(Out::Free(Line::new()));

    let mut rest = Line::new();
    if s.cpu_ready {
        rest.push(DIM, format!("user {} · kernel {}", pct(s.user), pct(s.kernel)));
    } else {
        rest.push(DIM, "sampling…");
    }
    cx.out.push(Out::Meter {
        label: "cpu",
        frac: if s.cpu_ready { s.total / 100.0 } else { 0.0 },
        value: String::new(),
        pct: if s.cpu_ready { pct(s.total) } else { "…".into() },
        rest,
    });
    if s.cpu_ready && !s.cores.is_empty() {
        cx.out.extend(core_lines(s, width, 6));
    }
    let frac = if s.mem_total > 0.0 { s.mem_used / s.mem_total } else { 0.0 };
    let mut rest = Line::new();
    if frac >= 0.9 {
        rest.push("1;38;5;167", "near full");
    }
    cx.out.push(Out::Meter { label: "mem", frac, value: fmt_pair(s.mem_used, s.mem_total), pct: pct(frac * 100.0), rest });
    let tagroll = by_tag(s, o.metric_mem);
    let mut tl = Line::new();
    for (tag, c, m, _) in tagroll.iter().filter(|t| t.1 >= 0.5 || t.2 >= 256e6).take(6) {
        if !tl.segs.is_empty() {
            tl.plain("  ");
        }
        tl.push(tags::color(tag), format!("[{tag}]"));
        let cc = if s.cpu_ready { format!(" {}", pct(*c)) } else { String::new() };
        tl.push(DIM, format!("{cc} {}", fmt_bytes(*m)));
    }
    if !tl.segs.is_empty() {
        cx.out.push(Out::Label { label: "tags", bar: None, cells: vec![], rest: tl });
    }
    cx.out.push(Out::Free(Line::new()));
    let mut hdr = Line::new();
    hdr.push(DIM, if o.group { "tag / process" } else { "process tree" });
    cx.out.push(Out::Label { label: "", bar: None, cells: vec![cell("Σcpu", DIM), cell("own", DIM), cell("Σmem", DIM)], rest: hdr });

    if o.group {
        let n_tags = tagroll.len();
        for (i, (tag, c, m, ids)) in tagroll.iter().enumerate() {
            let last = i + 1 == n_tags;
            let mut name = Line::new();
            name.push(tags::color(tag), format!("[{tag}]"));
            let mut r = Row::new("", name);
            r.last = last;
            r.bar = Some(cx.frac(*c, *m));
            r.cells = vec![cx.cpu_cell(*c, ""), blank(), cell(fmt_bytes(*m), "")];
            r.pid = format!("{} process{}", ids.len(), if ids.len() == 1 { "" } else { "es" });
            cx.out.push(Out::Row(r));
            if o.depth >= 2 {
                // flat within a tag: no nesting, so own numbers are the ones that add up
                let mut gs = groups(s, ids, !o.all, o.metric_mem);
                for g in gs.iter_mut() {
                    g.sub_cpu = g.own_cpu;
                    g.sub_mem = g.own_mem;
                }
                super::model::sort_groups(&mut gs, o.metric_mem);
                let keep = if o.all { gs.len() } else { o.top.min(gs.len()) };
                let hidden: Vec<Group> = gs.split_off(keep);
                let mut kids: Vec<Child> = vec![];
                for g in gs {
                    let vm = g.hosts_linux(s);
                    kids.push(Child::Win(g));
                    if vm && o.depth >= 3 {
                        if let Some(w) = &s.wsl {
                            kids.extend(w.iter().take(o.top).map(Child::Linux));
                        }
                    }
                }
                if !hidden.is_empty() {
                    kids.push(Child::More {
                        n: hidden.iter().map(|g| g.members.len()).sum(),
                        cpu: hidden.iter().map(|g| g.own_cpu).sum(),
                        mem: hidden.iter().map(|g| g.own_mem).sum(),
                        what: "process",
                    });
                }
                let pre = child_prefix("", last);
                let n = kids.len();
                // inside a tag the list is flat: no process-tree children
                for (k, ch) in kids.into_iter().enumerate() {
                    cx.emit(ch, &pre, k + 1 == n, 0, 1);
                }
            }
        }
    } else {
        let roots = s.roots.clone();
        let kids = cx.children(&roots, false);
        let n = kids.len();
        for (i, k) in kids.into_iter().enumerate() {
            cx.emit(k, "", i + 1 == n, 0, o.depth);
        }
    }

    if s.wsl.as_ref().is_some_and(|w| !w.is_empty()) {
        cx.out.push(Out::Free(Line::new()));
        let mut f = Line::new();
        f.push(DIM, "WSL rows: Linux processes by CPU over 0.3 s inside the VM (share of the whole machine, RSS); they are part of the VM's own CPU.");
        cx.out.push(Out::Wrap(f));
    }
    if head.jev_tags > 0 {
        let mut f = Line::new();
        f.push(DIM, "* tag chosen by Jev");
        cx.out.push(Out::Free(f));
    }
    layout(cx.out, width, &[PCT, PCT, MEM])
}

/// The instant, local headline (first template), or a note while CPU is being sampled.
pub fn local_headline(s: &Snap) -> Headline {
    let text = headline::candidates(s).into_iter().next().map(|c| c.text).unwrap_or_else(|| "Sampling CPU load…".into());
    Headline { text, source: "", jev_tags: 0, emph: vec![] }
}
