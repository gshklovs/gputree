//! gputree's screen: adapter summaries, then processes -> engines / WSL processes,
//! on the shared grid (`crate::layout`) with cells [mem, util].

use super::Opts;
use super::headline;
use super::model::{Gpu, Proc, Snap, by_tag, is_active, sort_procs};
use crate::layout::{BAR, DIM, Headline, Out, Row, layout};
use crate::tags;
use crate::term::{Line, bar, fmt_bytes, fmt_pair};

const MEM: usize = 9;
const UTIL: usize = 5;
const MAG: &str = "35";

fn pct(v: f64) -> String {
    format!("{:.0}%", v.min(100.0))
}

fn eng_pct(v: f64) -> String {
    if v < 9.95 { format!("{v:.1}%") } else { format!("{:.0}%", v.min(100.0)) }
}

struct Ctx<'a> {
    a: &'a Opts,
    s: &'a Snap,
    out: Vec<Out>,
    spill: bool,
}

/// How much of the screen to draw: processes per adapter (in drawing order) and
/// whether the footnotes are shown. `render_fit` shrinks these to fit a terminal.
struct Budget {
    tops: Vec<usize>,
    notes: bool,
    /// the "+ spilled to shared memory" rows
    spill: bool,
}

impl Ctx<'_> {
    fn row(&mut self, r: Row) {
        self.out.push(Out::Row(r));
    }

    fn util_cell(&self, v: f64) -> (String, &'static str) {
        if self.s.util_ready { (pct(v), "") } else { ("…".into(), DIM) }
    }

    fn emit_proc(&mut self, g: &Gpu, p: &Proc, prefix: &str, last: bool) {
        let a = self.a;
        let pm = g.pmem(p);
        let frac = if a.metric_util {
            if self.s.util_ready { Some(p.util / 100.0) } else { Some(0.0) }
        } else if g.scale() > 0.0 {
            // bar width = the adapter's capacity, fill = what this process uses
            Some(pm / g.scale())
        } else {
            Some(0.0)
        };
        let vm = p.name.eq_ignore_ascii_case("vmwp");
        let mut name = Line::new();
        name.plain(p.name.clone());
        if vm {
            name.push(MAG, " (WSL2 VM)");
        }
        self.row(Row {
            prefix: prefix.to_string(),
            last,
            bar: frac,
            cells: vec![(if g.integrated && p.adjusted { format!("~{}", fmt_bytes(pm)) } else { fmt_bytes(pm) }, ""), self.util_cell(p.util)],
            name,
            alts: vec![],
            tag: (!a.group).then_some((p.tag, p.jev_tag)),
            pid: format!("pid {}", p.pid),
        });

        // children: engines, WSL processes, spill note
        let kid_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
        let mut kids: Vec<Row> = vec![];
        if a.depth >= 3 && self.s.util_ready {
            let mut e: Vec<(&String, &f64)> = p.eng.iter().filter(|(_, v)| **v > 0.05).collect();
            e.sort_by(|x, y| y.1.total_cmp(x.1));
            for (k, v) in e {
                let mut n = Line::new();
                n.push(DIM, k.clone());
                kids.push(Row {
                    prefix: kid_prefix.clone(),
                    last: false,
                    bar: Some(v / 100.0),
                    cells: vec![(String::new(), ""), (eng_pct(*v), DIM)],
                    name: n,
                    alts: vec![],
                    tag: None,
                    pid: String::new(),
                });
            }
        }
        if a.depth >= 2 && vm && (p.util >= 0.5 || p.ded >= 64.0 * 1048576.0) {
            if let Some(wsl) = &self.s.wsl {
                let mut w: Vec<_> = wsl.iter().collect();
                w.sort_by(|x, y| y.rss.total_cmp(&x.rss));
                for w in w {
                    let mut n = Line::new();
                    n.push("1", w.label.clone());
                    n.push(DIM, format!("  {}", w.context()));
                    kids.push(Row {
                        prefix: kid_prefix.clone(),
                        last: false,
                        bar: None,
                        cells: vec![(fmt_bytes(w.rss), DIM), (String::new(), "")],
                        name: n,
                        alts: {
                            let mut a1 = Line::new();
                            a1.push("1", w.label.clone());
                            if let Some(p) = &w.project {
                                a1.push(DIM, format!("  ({p})"));
                            }
                            let mut a2 = Line::new();
                            a2.push("1", w.label.clone());
                            vec![a1, a2]
                        },
                        tag: Some((w.tag, false)),
                        pid: format!("pid {}", w.pid),
                    });
                }
            }
        }
        if self.spill && a.depth >= 3 && !g.integrated && p.shr >= 1048576.0 {
            let mut n = Line::new();
            n.push(DIM, "+ spilled to shared memory");
            kids.push(Row {
                prefix: kid_prefix.clone(),
                last: false,
                bar: None,
                cells: vec![(fmt_bytes(p.shr), DIM), (String::new(), "")],
                name: n,
                alts: vec![],
                tag: None,
                pid: String::new(),
            });
        }
        let n = kids.len();
        for (i, mut k) in kids.into_iter().enumerate() {
            k.last = i + 1 == n;
            self.row(k);
        }
    }

    fn more_row(&mut self, prefix: &str, mem: f64, text: String) {
        let mut n = Line::new();
        n.push(DIM, text);
        self.row(Row {
            prefix: prefix.to_string(),
            last: true,
            bar: None,
            cells: vec![(if mem > 0.0 { fmt_bytes(mem) } else { String::new() }, DIM), (String::new(), "")],
            name: n,
            alts: vec![],
            tag: None,
            pid: String::new(),
        });
    }

    /// An idle iGPU / NPU next to a discrete GPU, in one line.
    fn adapter_line(&mut self, g: &Gpu) {
        let mut l = Line::new();
        l.push("36", g.name.clone());
        l.plain("  ");
        l.append(bar(if g.cap > 0.0 { g.mem / g.cap } else { 0.0 }, BAR));
        let mut bits = vec![format!("{}{}", mem_value(g), if g.integrated && !g.npu { " shared" } else { "" })];
        if self.s.util_ready {
            bits.push(format!("util {}", pct(g.util)));
        }
        let n = g.procs.len();
        bits.push(format!("{n} process{} (--all)", if n == 1 { "" } else { "es" }));
        l.push(DIM, format!("  {}", bits.join(" · ")));
        self.out.push(Out::Free(l));
    }

    fn adapter(&mut self, g: &Gpu, top: usize) {
        let a = self.a;
        self.out.push(Out::Free(Line::new()));
        let mut head = Line::new();
        head.push("1;36", g.name.clone());
        if let Some(nv) = &g.nv {
            let mut bits = vec![];
            if let Some(t) = nv.temp {
                bits.push(format!("{t}°C"));
            }
            if let Some(p) = nv.power_w {
                bits.push(format!("{p:.0} W"));
            }
            if !bits.is_empty() {
                head.push(DIM, format!("  {}", bits.join(" · ")));
            }
        }
        self.out.push(Out::Free(head));

        let frac = if g.cap > 0.0 { g.mem / g.cap } else { 0.0 };
        let mut rest = Line::new();
        if g.integrated {
            rest.push(DIM, "shared memory");
        } else if g.npu {
            rest.push(DIM, "compute accelerator");
        } else if frac >= 0.9 {
            rest.push("1;38;5;167", "near full · OOM risk");
        }
        self.out.push(Out::Meter {
            label: if g.integrated || g.npu { "mem" } else { "vram" },
            frac,
            value: mem_value(g),
            pct: if g.cap > 0.0 { pct(frac * 100.0) } else { String::new() },
            rest,
        });
        let mut rest = Line::new();
        if self.s.util_ready {
            let mut e: Vec<(&String, &f64)> = g.eng.iter().filter(|(_, v)| **v > 0.05).collect();
            e.sort_by(|x, y| y.1.total_cmp(x.1));
            let txt: Vec<String> = e.iter().map(|(k, v)| format!("{k} {}", pct(**v))).collect();
            rest.push(DIM, txt.join(" · "));
        } else {
            rest.push(DIM, "sampling…");
        }
        self.out.push(Out::Meter {
            label: "util",
            frac: if self.s.util_ready { g.util / 100.0 } else { 0.0 },
            value: String::new(),
            pct: if self.s.util_ready { pct(g.util) } else { "…".into() },
            rest,
        });

        let groups = by_tag(g, a.metric_util);
        let mut tl = Line::new();
        for t in groups.iter().filter(|t| t.mem >= 1048576.0 || t.util >= 0.5).take(6) {
            if !tl.segs.is_empty() {
                tl.plain("  ");
            }
            tl.push(tags::color(t.tag), format!("[{}]", t.tag));
            let u = if self.s.util_ready { format!(" {}", pct(t.util)) } else { String::new() };
            tl.push(DIM, format!(" {}{u}", fmt_bytes(t.mem)));
        }
        if !tl.segs.is_empty() {
            self.out.push(Out::Label { label: "tags", bar: None, cells: vec![], rest: tl });
        }

        if a.group {
            let shown: Vec<_> = groups.iter().filter(|t| a.all || t.mem >= 1048576.0 || t.util >= 0.1).collect();
            let rest_n: usize = groups.iter().filter(|t| !(a.all || t.mem >= 1048576.0 || t.util >= 0.1)).map(|t| t.procs.len()).sum();
            for (i, t) in shown.iter().enumerate() {
                let last_t = i + 1 == shown.len() && rest_n == 0;
                let frac = if a.metric_util { t.util / 100.0 } else if g.scale() > 0.0 { t.mem / g.scale() } else { 0.0 };
                let mut n = Line::new();
                n.push(tags::color(t.tag), format!("[{}]", t.tag));
                let np = t.procs.len();
                self.row(Row {
                    prefix: String::new(),
                    last: last_t,
                    bar: Some(frac),
                    cells: vec![(fmt_bytes(t.mem), ""), self.util_cell(t.util)],
                    name: n,
                    alts: vec![],
                    tag: None,
                    pid: format!("{np} process{}", if np == 1 { "" } else { "es" }),
                });
                if a.depth < 2 {
                    continue;
                }
                let mut sub: Vec<&Proc> = t.procs.iter().copied().filter(|p| is_active(p, a.all)).collect();
                sort_procs(g, &mut sub, a.metric_util);
                if !a.all && sub.len() > top {
                    sub.truncate(top);
                }
                let more = t.procs.len() - sub.len();
                let pre = if last_t { "   " } else { "│  " };
                for (k, p) in sub.iter().enumerate() {
                    self.emit_proc(g, p, pre, k + 1 == sub.len() && more == 0);
                }
                if more > 0 {
                    self.more_row(pre, 0.0, format!("+ {more} more"));
                }
            }
            if rest_n > 0 {
                self.more_row("", 0.0, format!("+ {rest_n} idle process{} (--all to list)", if rest_n == 1 { "" } else { "es" }));
            }
        } else {
            let mut sorted: Vec<&Proc> = g.procs.iter().filter(|p| is_active(p, a.all)).collect();
            sort_procs(g, &mut sorted, a.metric_util);
            if !a.all && sorted.len() > top {
                sorted.truncate(top);
            }
            let hidden = g.procs.len() - sorted.len();
            let hidden_mem: f64 = g.procs.iter().filter(|p| !sorted.iter().any(|s| std::ptr::eq(*s, *p))).map(|p| g.pmem(p)).sum();
            let n = sorted.len();
            for (i, p) in sorted.into_iter().enumerate() {
                self.emit_proc(g, p, "", i + 1 == n && hidden == 0);
            }
            if hidden > 0 {
                self.more_row(
                    "",
                    hidden_mem,
                    format!("+ {hidden} more process{} (--all to list)", if hidden == 1 { "" } else { "es" }),
                );
            }
        }
    }
}

/// Lay everything out for a terminal `width` columns wide. No returned line is wider.
pub fn render(s: &Snap, a: &Opts, head: &Headline, width: usize) -> Vec<Line> {
    render_with(s, a, head, width, &Budget { tops: vec![a.top; s.gpus.len()], notes: true, spill: true })
}

/// Like `render`, but at most `max_lines` tall (for a terminal screen), so the
/// progressive redraw can happen in place. It keeps the headline, every adapter's
/// summary and the busiest rows, and folds the rest into "+ N more" rows, giving up
/// detail from the least busy adapters first (their process rows), then the footnotes,
/// then the spill notes, then the busiest adapter's engine rows, then its processes.
/// `--all` is never folded.
pub fn render_fit(s: &Snap, a: &Opts, head: &Headline, width: usize, max_lines: usize) -> Vec<Line> {
    let full = render(s, a, head, width);
    if a.all || full.len() <= max_lines {
        return full;
    }
    let mut b = Budget { tops: vec![a.top; s.gpus.len()], notes: true, spill: true };
    let mut o = a.clone();
    let fits = |o: &Opts, b: &Budget| {
        let l = render_with(s, o, head, width, b);
        (l.len() <= max_lines).then_some(l)
    };
    // fold order: least busy adapter first (ties: the one drawn lower first)
    let drawn = draw_order(s, a);
    let mut fold: Vec<usize> = (0..drawn.len()).collect();
    // (before util is sampled everything ties at 0: fold integrated adapters before discrete ones)
    fold.sort_by(|&i, &j| {
        drawn[i].util.total_cmp(&drawn[j].util).then(drawn[j].integrated.cmp(&drawn[i].integrated)).then(j.cmp(&i))
    });
    let keep = fold.pop().unwrap_or(0);
    for i in fold {
        while b.tops[i] > 0 {
            b.tops[i] -= 1;
            if let Some(l) = fits(&o, &b) {
                return l;
            }
        }
    }
    b.notes = false;
    if let Some(l) = fits(&o, &b) {
        return l;
    }
    b.spill = false;
    if let Some(l) = fits(&o, &b) {
        return l;
    }
    if o.depth > 2 {
        o.depth = 2;
        if let Some(l) = fits(&o, &b) {
            return l;
        }
    }
    while b.tops.get(keep).is_some_and(|t| *t > 0) {
        b.tops[keep] -= 1;
        if let Some(l) = fits(&o, &b) {
            return l;
        }
    }
    let mut l = render_with(s, &o, head, width, &b);
    l.truncate(max_lines);
    l
}

/// Adapters in drawing order: discrete GPUs first (they're what people come to look
/// at), then by memory (or util with --metric util), largest first.
fn draw_order<'a>(s: &'a Snap, a: &Opts) -> Vec<&'a Gpu> {
    let mut gpus: Vec<&Gpu> = s.gpus.iter().collect();
    gpus.sort_by(|x, y| {
        let by = if a.metric_util { y.util.total_cmp(&x.util) } else { y.mem.total_cmp(&x.mem) };
        secondary(x).cmp(&secondary(y)).then(by)
    });
    gpus
}

fn secondary(g: &Gpu) -> bool {
    g.integrated || g.npu
}

/// Below this an iGPU / NPU is drawn as one line when there is a discrete GPU.
const IDLE_UTIL: f64 = 50.0;

fn collapsed(s: &Snap, a: &Opts, g: &Gpu) -> bool {
    !a.all && secondary(g) && g.util < IDLE_UTIL && s.gpus.iter().any(|d| !secondary(d))
}

/// "2632 / 8151 MiB" for NVIDIA (nvidia-smi's own numbers), "2.9 / 17.9 GiB" otherwise.
fn mem_value(g: &Gpu) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if g.cap <= 0.0 {
        fmt_bytes(g.mem)
    } else if g.nv.is_some() || g.name.starts_with("NVIDIA") {
        // MiB from the first frame, so the column doesn't change unit once NVML answers
        format!("{} / {} MiB", (g.mem / MIB) as u64, (g.cap / MIB) as u64)
    } else {
        fmt_pair(g.mem, g.cap)
    }
}

fn render_with(s: &Snap, a: &Opts, head: &Headline, width: usize, budget: &Budget) -> Vec<Line> {
    let mut cx = Ctx { a, s, out: vec![], spill: budget.spill };

    // headline + title
    cx.out.push(Out::Wrap(head.line()));
    let mut t = Line::new();
    t.push("1", "gputree");
    t.push(
        DIM,
        format!(
            "  {} adapter{} · ranked by {} · {}",
            s.gpus.len(),
            if s.gpus.len() == 1 { "" } else { "s" },
            if a.metric_util { "util" } else { "vram" },
            s.time
        ),
    );
    cx.out.push(Out::Free(t));

    let order = draw_order(s, a);
    let mut first_line = true;
    for (i, g) in order.into_iter().enumerate() {
        if collapsed(s, a, g) {
            if first_line {
                cx.out.push(Out::Free(Line::new()));
                first_line = false;
            }
            cx.adapter_line(g);
        } else {
            cx.adapter(g, budget.tops.get(i).copied().unwrap_or(a.top));
        }
    }
    if budget.notes && s.wsl.as_ref().is_some_and(|w| !w.is_empty()) {
        cx.out.push(Out::Free(Line::new()));
        let mut f = Line::new();
        f.push(DIM, "WSL rows: Linux processes holding /dev/dxg (host RAM shown). They share one VM, so their GPU % is the VM's total.");
        cx.out.push(Out::Wrap(f));
    }
    if budget.notes && s.gpus.iter().any(|g| g.integrated && g.procs.iter().any(|p| p.adjusted)) {
        let mut f = Line::new();
        f.push(DIM, "~ shared memory scaled to fit the adapter: Windows counts pages several processes map once per process.");
        cx.out.push(Out::Wrap(f));
    }
    if budget.notes && head.jev_tags > 0 {
        let mut f = Line::new();
        f.push(DIM, "* tag chosen by Jev");
        cx.out.push(Out::Free(f));
    }

    layout(cx.out, width, &[MEM, UTIL])
}

/// The instant, local headline (first template), or a sampling note before util exists.
pub fn local_headline(s: &Snap) -> Headline {
    match headline::candidates(s).into_iter().next() {
        Some(c) => Headline { text: c.text, source: "", jev_tags: 0, emph: c.emph },
        None => Headline { text: "Sampling GPU load…".into(), source: "", jev_tags: 0, emph: vec![] },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::{Gpu, Proc, Snap, WslProc};
    use crate::term::width_of;
    use std::collections::BTreeMap;

    fn fixture() -> Snap {
        let mut eng = BTreeMap::new();
        eng.insert("3d".to_string(), 55.0);
        eng.insert("copy".to_string(), 1.2);
        let vm = Proc { luid: 1, pid: 36088, name: "vmwp".into(), ded: 3.2e9, shr: 8e7, eng: eng.clone(), util: 55.0, tag: "training", jev_tag: false, shr_raw: 0.0, adjusted: false };
        let sys = Proc { luid: 1, pid: 4, name: "System".into(), ded: 4e6, shr: 0.0, eng: BTreeMap::new(), util: 0.0, tag: "system", jev_tag: false, shr_raw: 0.0, adjusted: false };
        let g = Gpu {
            luid: 1,
            name: "NVIDIA GeForce RTX 5060 Laptop GPU".into(),
            short: "NVIDIA RTX 5060".into(),
            integrated: false,
            npu: false,
            mem: 3.3e9,
            cap: 8.2e9,
            util: 55.0,
            eng,
            procs: vec![vm, sys],
            nv: None,
        };
        let w = WslProc {
            distro: "Ubuntu-22.04".into(),
            pid: 4934,
            rss: 2.8e9,
            user: "grego".into(),
            cmd: String::new(),
            label: "train bd1-walk-flat".into(),
            project: Some("microduck_rl".into()),
            tag: "training",
        };
        Snap { gpus: vec![g], util_ready: true, wsl: Some(vec![w]), time: "12:00:00".into() }
    }

    fn args() -> Opts {
        Opts { metric_util: false, depth: 3, top: 10, group: false, all: false }
    }

    #[test]
    fn fit_keeps_the_summary_and_folds_the_rest() {
        let mut s = fixture();
        let mut ig = s.gpus[0].clone();
        ig.luid = 2;
        ig.name = "Intel(R) Graphics".into();
        ig.short = "Intel iGPU".into();
        ig.integrated = true;
        ig.mem = 1e9;
        ig.procs = (0..8)
            .map(|i| Proc { luid: 2, pid: 100 + i, name: format!("app{i}"), ded: 0.0, shr: 5e7, eng: BTreeMap::new(), util: 1.0, tag: "app", jev_tag: false, shr_raw: 0.0, adjusted: false })
            .collect();
        s.gpus.push(ig);
        let h = local_headline(&s);
        let full = render(&s, &args(), &h, 100);
        for max in [full.len(), 20, 16, 12, 9] {
            let l = render_fit(&s, &args(), &h, 100, max);
            let text: Vec<String> = l.iter().map(|l| l.render(false)).collect();
            assert!(l.len() <= max, "{max}: {text:#?}");
            assert!(text[0].contains("busy"), "{text:#?}");
            assert!(l.iter().all(|x| x.width() <= 100));
            if max >= 16 {
                // every adapter keeps its summary while there is room for it
                assert!(text.iter().any(|t| t.contains("Intel(R) Graphics")), "{max}: {text:#?}");
            }
            if max < full.len() {
                assert!(text.iter().any(|t| t.contains("more process")), "{max}: {text:#?}");
            }
            if max >= 16 {
                // the discrete GPU's rows survive; the iGPU folds first
                assert!(text.iter().any(|t| t.contains("vmwp")), "{max}: {text:#?}");
            }
        }
        // a busier adapter drawn lower keeps its rows; the idle one on top folds
        let mut s2 = s.clone();
        s2.gpus[0].util = 2.0;
        s2.gpus[1].mem = 5e9;
        s2.gpus[1].util = 90.0;
        let l = render_fit(&s2, &args(), &local_headline(&s2), 100, 20);
        let text: Vec<String> = l.iter().map(|l| l.render(false)).collect();
        // the discrete GPU is drawn first even when the iGPU holds more memory
        assert!(text.iter().position(|t| t.contains("NVIDIA GeForce")) < text.iter().position(|t| t.contains("Intel(R) Graphics")), "{text:#?}");
        assert!(text.iter().any(|t| t.contains("app0")), "{text:#?}");
        assert!(!text.iter().any(|t| t.contains("vmwp")), "{text:#?}");

        let mut all = args();
        all.all = true;
        assert!(render_fit(&s, &all, &h, 100, 9).len() > 9, "--all is never folded");
    }

    #[test]
    fn headline_lights_the_figure_and_the_workload() {
        let s = fixture();
        let h = local_headline(&s);
        let l = h.line();
        assert_eq!(l.render(false), h.text);
        let seg = |needle: &str| l.segs.iter().find(|g| g.text.contains(needle)).map(|g| g.sgr.clone()).unwrap();
        assert_eq!(seg("55% busy"), "1");
        assert_eq!(seg("a training run (train bd1-walk-flat, WSL)"), crate::tags::color("training"));
        assert_eq!(seg("Your"), "");
    }

    #[test]
    fn never_wider_than_the_terminal_and_columns_align() {
        let s = fixture();
        let h = local_headline(&s);
        assert!(h.text.contains("training run"), "{}", h.text);
        for group in [false, true] {
            for w in [30, 44, 60, 80, 120] {
                let mut a = args();
                a.group = group;
                let lines = render(&s, &a, &h, w);
                if std::env::var_os("GPUTREE_PRINT_FIXTURE").is_some() && (w == 80 || w == 120) {
                    for l in &lines {
                        println!("{}", l.render(false));
                    }
                }
                for l in &lines {
                    assert!(l.width() <= w, "{w}: {:?}", l.render(false));
                }
                if w >= 80 && !group {
                    // bars start in the same column at every depth
                    let text: Vec<String> = lines.iter().map(|l| l.render(false)).collect();
                    let col = |needle: &str| {
                        let l = text.iter().find(|l| l.contains(needle) && l.contains("pid")).unwrap();
                        width_of(&l[..l.find(needle).unwrap()])
                    };
                    assert_eq!(col("3.0 GiB"), col("2.6 GiB") , "{text:#?}");
                    assert_eq!(col("pid 36088"), col("pid 4934"), "{text:#?}");
                }
            }
        }
    }
}
