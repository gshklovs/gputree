//! gputree's screen: adapter summaries, then processes -> engines / WSL processes,
//! on the shared grid (`crate::layout`) with cells [mem, util].

use super::Opts;
use super::headline;
use super::model::{Gpu, Proc, Snap, by_tag, is_active, sort_procs};
use crate::layout::{DIM, Headline, Out, Row, layout};
use crate::tags;
use crate::term::{Line, fmt_bytes};

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
        if a.depth >= 3 && !g.integrated && p.shr >= 1048576.0 {
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
            cells: vec![(fmt_bytes(g.mem), ""), (String::new(), "")],
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
            cells: vec![(String::new(), ""), self.util_cell(g.util)],
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
pub fn render(s: &Snap, a: &Opts, head: &Headline, width: usize) -> Vec<Line> {
    let mut cx = Ctx { a, s, out: vec![] };

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
    if s.gpus.iter().any(|g| g.integrated && g.procs.iter().any(|p| p.adjusted)) {
        let mut f = Line::new();
        f.push(DIM, "~ shared memory scaled to fit the adapter: Windows counts pages several processes map once per process.");
        cx.out.push(Out::Wrap(f));
    }
    if head.jev_tags > 0 {
        let mut f = Line::new();
        f.push(DIM, "* tag chosen by Jev");
        cx.out.push(Out::Free(f));
    }

    layout(cx.out, width, &[MEM, UTIL])
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
