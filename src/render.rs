//! Aligned tree layout. Every row shares one column grid:
//!
//! ```text
//! gutter(G)  bar(BAR) mem(MEM) util(UTIL)  name(NAME) tag(TAG) pid
//! ```
//!
//! The gutter has a fixed width for the whole screen; shallower rows extend their
//! branch with `─` so bars, numbers, names, chips and pids line up at every depth.

use crate::args::Args;
use crate::headline;
use crate::model::{Gpu, Proc, Snap, by_tag, is_active, sort_procs};
use crate::tags;
use crate::term::{Line, bar, fmt_bytes, pad_left, width_of};

pub const BAR: usize = 12;
const MEM: usize = 9;
const UTIL: usize = 5;
const DIM: &str = "2";
const MAG: &str = "35";

struct Row {
    /// ancestors' continuation marks ("│  " / "   "), 3 columns per level
    prefix: String,
    last: bool,
    bar: Option<f64>,
    mem: (String, &'static str),
    util: (String, &'static str),
    name: Line,
    /// shorter variants of `name`, tried in order when it does not fit its column
    alts: Vec<Line>,
    tag: Option<(&'static str, bool)>,
    pid: String,
}

enum Out {
    Free(Line),
    /// prose (headline, footnotes): word-wrapped to the width instead of clipped
    Wrap(Line),
    /// adapter summary rows: label sits in the gutter, then the normal columns
    Label { label: &'static str, bar: Option<f64>, mem: (String, &'static str), util: (String, &'static str), rest: Line },
    Row(Row),
}

pub struct Headline {
    pub text: String,
    /// "" for local, "jev" when Jev picked it
    pub source: &'static str,
    pub jev_tags: usize,
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v.min(100.0))
}

fn eng_pct(v: f64) -> String {
    if v < 9.95 { format!("{v:.1}%") } else { format!("{:.0}%", v.min(100.0)) }
}

fn chip(tag: &str, jev: bool) -> String {
    if jev { format!("[{tag}]*") } else { format!("[{tag}]") }
}

struct Ctx<'a> {
    a: &'a Args,
    s: &'a Snap,
    out: Vec<Out>,
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
        } else if g.mem > 0.0 {
            Some(pm / g.mem)
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
            mem: (fmt_bytes(pm), ""),
            util: self.util_cell(p.util),
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
                    mem: (String::new(), ""),
                    util: (eng_pct(*v), DIM),
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
                        mem: (fmt_bytes(w.rss), DIM),
                        util: (String::new(), ""),
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
        if a.depth >= 3 && !g.integrated && p.shr >= 1048576.0 {
            let mut n = Line::new();
            n.push(DIM, "+ spilled to shared memory");
            kids.push(Row {
                prefix: kid_prefix.clone(),
                last: false,
                bar: None,
                mem: (fmt_bytes(p.shr), DIM),
                util: (String::new(), ""),
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
            mem: (if mem > 0.0 { fmt_bytes(mem) } else { String::new() }, DIM),
            util: (String::new(), ""),
            name: n,
            alts: vec![],
            tag: None,
            pid: String::new(),
        });
    }

    fn adapter(&mut self, g: &Gpu) {
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

        let mut rest = Line::new();
        if g.cap > 0.0 {
            rest.push(DIM, format!("of {}{}", fmt_bytes(g.cap), if g.integrated { " shared" } else { "" }));
        } else if g.npu {
            rest.push(DIM, "compute accelerator");
        }
        self.out.push(Out::Label {
            label: "mem",
            bar: Some(if g.cap > 0.0 { g.mem / g.cap } else { 0.0 }),
            mem: (fmt_bytes(g.mem), ""),
            util: (String::new(), ""),
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
        self.out.push(Out::Label {
            label: "util",
            bar: Some(if self.s.util_ready { g.util / 100.0 } else { 0.0 }),
            mem: (String::new(), ""),
            util: self.util_cell(g.util),
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
            self.out.push(Out::Label { label: "tags", bar: None, mem: (String::new(), ""), util: (String::new(), ""), rest: tl });
        }

        if a.group {
            let shown: Vec<_> = groups.iter().filter(|t| a.all || t.mem >= 1048576.0 || t.util >= 0.1).collect();
            let rest_n: usize = groups.iter().filter(|t| !(a.all || t.mem >= 1048576.0 || t.util >= 0.1)).map(|t| t.procs.len()).sum();
            for (i, t) in shown.iter().enumerate() {
                let last_t = i + 1 == shown.len() && rest_n == 0;
                let frac = if a.metric_util { t.util / 100.0 } else if g.mem > 0.0 { t.mem / g.mem } else { 0.0 };
                let mut n = Line::new();
                n.push(tags::color(t.tag), format!("[{}]", t.tag));
                let np = t.procs.len();
                self.row(Row {
                    prefix: String::new(),
                    last: last_t,
                    bar: Some(frac),
                    mem: (fmt_bytes(t.mem), ""),
                    util: self.util_cell(t.util),
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
                if !a.all && sub.len() > a.top {
                    sub.truncate(a.top);
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
            if !a.all && sorted.len() > a.top {
                sorted.truncate(a.top);
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
pub fn render(s: &Snap, a: &Args, head: &Headline, width: usize) -> Vec<Line> {
    let mut cx = Ctx { a, s, out: vec![] };

    // headline + title
    let mut h = Line::new();
    h.push("1", head.text.clone());
    if head.source == "jev" {
        h.push(DIM, if head.jev_tags > 0 { format!("  · jev (+{} tag{})", head.jev_tags, if head.jev_tags == 1 { "" } else { "s" }) } else { "  · jev".into() });
    }
    cx.out.push(Out::Wrap(h));
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

    let mut gpus: Vec<&Gpu> = s.gpus.iter().collect();
    gpus.sort_by(|x, y| if a.metric_util { y.util.total_cmp(&x.util) } else { y.mem.total_cmp(&x.mem) });
    for g in gpus {
        cx.adapter(g);
    }
    if s.wsl.as_ref().is_some_and(|w| !w.is_empty()) {
        cx.out.push(Out::Free(Line::new()));
        let mut f = Line::new();
        f.push(DIM, "WSL rows: Linux processes holding /dev/dxg (host RAM shown). They share one VM, so their GPU % is the VM's total.");
        cx.out.push(Out::Wrap(f));
    }
    if head.jev_tags > 0 {
        let mut f = Line::new();
        f.push(DIM, "* tag chosen by Jev");
        cx.out.push(Out::Free(f));
    }

    layout(cx.out, width)
}

fn layout(out: Vec<Out>, width: usize) -> Vec<Line> {
    // column widths
    let max_depth = out
        .iter()
        .filter_map(|o| if let Out::Row(r) = o { Some(width_of(&r.prefix) / 3) } else { None })
        .max()
        .unwrap_or(0);
    let gutter = (3 * (max_depth + 1)).max(6);
    let tag_w = out
        .iter()
        .filter_map(|o| if let Out::Row(Row { tag: Some((t, j)), .. }) = o { Some(width_of(&chip(t, *j))) } else { None })
        .max()
        .unwrap_or(0);
    let pid_w = out.iter().filter_map(|o| if let Out::Row(r) = o { Some(width_of(&r.pid)) } else { None }).max().unwrap_or(0);
    // rows without chips/pids (engines, "+ N more", spill notes) may run into the chip column
    let name_max = out
        .iter()
        .filter_map(|o| if let Out::Row(r) = o { (r.tag.is_some() || !r.pid.is_empty()).then(|| r.name.width()) } else { None })
        .max()
        .unwrap_or(0);
    let fixed = gutter + BAR + 1 + MEM + 1 + UTIL + 2;
    let tail = if tag_w > 0 { 1 + tag_w } else { 0 } + if pid_w > 0 { 2 + pid_w } else { 0 };
    let name_w = name_max.min(width.saturating_sub(fixed + tail)).max(10);

    let mut lines = vec![];
    for o in out {
        let mut l = Line::new();
        match o {
            Out::Free(x) => l = x,
            Out::Wrap(x) => {
                let mut wrapped = wrap(&x, width, 3);
                let last = wrapped.pop().unwrap_or_default();
                lines.extend(wrapped);
                l = last;
            }
            Out::Label { label, bar: b, mem, util, rest } => {
                l.push(DIM, format!(" {label}"));
                l.pad_to(gutter);
                match b {
                    Some(f) => {
                        l.append(bar(f, BAR));
                    }
                    None => {
                        // tags line: chips start at the bar column
                        l.append(rest);
                        l.truncate(width);
                        lines.push(l);
                        continue;
                    }
                }
                l.plain(" ");
                l.push(mem.1, pad_left(&mem.0, MEM));
                l.plain(" ");
                l.push(util.1, pad_left(&util.0, UTIL));
                l.plain("  ");
                l.append(rest);
            }
            Out::Row(r) => {
                let pw = width_of(&r.prefix);
                let fill = gutter.saturating_sub(pw + 2);
                l.push(DIM, format!("{}{}{} ", r.prefix, if r.last { "└" } else { "├" }, "─".repeat(fill)));
                match r.bar {
                    Some(f) => {
                        l.append(bar(f, BAR));
                    }
                    None => {
                        l.plain(" ".repeat(BAR));
                    }
                }
                l.plain(" ");
                l.push(r.mem.1, pad_left(&r.mem.0, MEM));
                l.plain(" ");
                l.push(r.util.1, pad_left(&r.util.0, UTIL));
                l.plain("  ");
                let has_tail = r.tag.is_some() || !r.pid.is_empty();
                let mut name = r.name;
                if has_tail && name.width() > name_w {
                    if let Some(a) = r.alts.into_iter().find(|a| a.width() <= name_w) {
                        name = a;
                    }
                    name.truncate(name_w);
                }
                let nw = name.width();
                l.append(name);
                if has_tail {
                    l.plain(" ".repeat(name_w - nw));
                }
                if tag_w > 0 && has_tail {
                    l.plain(" ");
                    match r.tag {
                        Some((t, j)) => {
                            let c = chip(t, j);
                            let cw = width_of(&c);
                            l.push(tags::color(t), c);
                            l.plain(" ".repeat(tag_w - cw));
                        }
                        None => {
                            l.plain(" ".repeat(tag_w));
                        }
                    }
                }
                if !r.pid.is_empty() {
                    l.push(DIM, format!("  {}", r.pid));
                }
            }
        }
        // trim trailing padding, then clip to the terminal
        while let Some(last) = l.segs.last_mut() {
            let t = last.text.trim_end().to_string();
            if t.is_empty() && last.sgr.is_empty() {
                l.segs.pop();
                continue;
            }
            if last.sgr.is_empty() {
                last.text = t;
            }
            break;
        }
        l.truncate(width);
        lines.push(l);
    }
    lines
}

/// Greedy word wrap that keeps each word's style; at most `max_lines` (the last is clipped).
fn wrap(x: &Line, width: usize, max_lines: usize) -> Vec<Line> {
    let mut words: Vec<(String, String)> = vec![]; // (sgr, word incl. leading spaces)
    for seg in &x.segs {
        let mut cur = String::new();
        for ch in seg.text.chars() {
            if ch == ' ' && !cur.trim().is_empty() {
                words.push((seg.sgr.clone(), std::mem::take(&mut cur)));
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            words.push((seg.sgr.clone(), cur));
        }
    }
    let mut out: Vec<Line> = vec![Line::new()];
    for (sgr, w) in words {
        let n = out.len();
        let cur = out.last_mut().unwrap();
        if cur.width() + width_of(&w) > width && cur.width() > 0 && n < max_lines {
            let mut nl = Line::new();
            nl.push(&sgr, w.trim_start().to_string());
            out.push(nl);
        } else {
            cur.push(&sgr, w);
        }
    }
    for l in &mut out {
        l.truncate(width);
    }
    out
}

/// The instant, local headline (first template), or a sampling note before util exists.
pub fn local_headline(s: &Snap) -> Headline {
    let text = headline::candidates(s)
        .into_iter()
        .next()
        .map(|c| c.text)
        .unwrap_or_else(|| "Sampling GPU load…".into());
    Headline { text, source: "", jev_tags: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Gpu, Proc, Snap, WslProc};
    use std::collections::BTreeMap;

    fn fixture() -> Snap {
        let mut eng = BTreeMap::new();
        eng.insert("3d".to_string(), 55.0);
        eng.insert("copy".to_string(), 1.2);
        let vm = Proc { luid: 1, pid: 36088, name: "vmwp".into(), ded: 3.2e9, shr: 8e7, eng: eng.clone(), util: 55.0, tag: "training", jev_tag: false };
        let sys = Proc { luid: 1, pid: 4, name: "System".into(), ded: 4e6, shr: 0.0, eng: BTreeMap::new(), util: 0.0, tag: "system", jev_tag: false };
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

    fn args() -> crate::args::Args {
        crate::args::Args {
            metric_util: false,
            depth: 3,
            watch: 0.0,
            top: 10,
            group: false,
            all: false,
            no_wsl: false,
            no_ai: true,
            width: None,
            no_color: true,
            timing: false,
        }
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
