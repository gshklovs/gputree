import os
os.chdir(r"C:\Users\grego\repos\gputree")


def sub(p, a, b, count=1):
    s = open(p, encoding="utf-8").read()
    assert s.count(a) >= count, (p, a[:80])
    s = s.replace(a, b, count)
    open(p, "w", encoding="utf-8", newline="\n").write(s)


R = "crates/treecore/src/gpu/render.rs"
sub(R, """struct Ctx<'a> {
    a: &'a Opts,
    s: &'a Snap,
    out: Vec<Out>,
}""", """struct Ctx<'a> {
    a: &'a Opts,
    s: &'a Snap,
    out: Vec<Out>,
}

/// How much of the screen to draw: processes per adapter (in drawing order) and
/// whether the footnotes are shown. `render_fit` shrinks these to fit a terminal.
struct Budget {
    tops: Vec<usize>,
    notes: bool,
}""")
sub(R, """    fn adapter(&mut self, g: &Gpu) {
        let a = self.a;""", """    fn adapter(&mut self, g: &Gpu, top: usize) {
        let a = self.a;""")
sub(R, """                if !a.all && sub.len() > a.top {
                    sub.truncate(a.top);""", """                if !a.all && sub.len() > top {
                    sub.truncate(top);""")
sub(R, """            if !a.all && sorted.len() > a.top {
                sorted.truncate(a.top);""", """            if !a.all && sorted.len() > top {
                sorted.truncate(top);""")
sub(R, """/// Lay everything out for a terminal `width` columns wide. No returned line is wider.
pub fn render(s: &Snap, a: &Opts, head: &Headline, width: usize) -> Vec<Line> {
    let mut cx = Ctx { a, s, out: vec![] };""", """/// Lay everything out for a terminal `width` columns wide. No returned line is wider.
pub fn render(s: &Snap, a: &Opts, head: &Headline, width: usize) -> Vec<Line> {
    render_with(s, a, head, width, &Budget { tops: vec![a.top; s.gpus.len()], notes: true })
}

/// Like `render`, but at most `max_lines` tall (for a terminal screen), so the
/// progressive redraw can happen in place. It keeps the headline, every adapter's
/// summary and the busiest rows, and folds the rest into "+ N more" rows, giving up
/// detail from the bottom: later adapters' process rows first, then the footnotes,
/// then the first adapter's engine rows, then its processes. `--all` is never folded.
pub fn render_fit(s: &Snap, a: &Opts, head: &Headline, width: usize, max_lines: usize) -> Vec<Line> {
    let full = render(s, a, head, width);
    if a.all || full.len() <= max_lines {
        return full;
    }
    let mut b = Budget { tops: vec![a.top; s.gpus.len()], notes: true };
    let mut o = a.clone();
    let fits = |o: &Opts, b: &Budget| {
        let l = render_with(s, o, head, width, b);
        (l.len() <= max_lines).then_some(l)
    };
    for i in (1..b.tops.len()).rev() {
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
    if o.depth > 2 {
        o.depth = 2;
        if let Some(l) = fits(&o, &b) {
            return l;
        }
    }
    while b.tops.first().is_some_and(|t| *t > 0) {
        b.tops[0] -= 1;
        if let Some(l) = fits(&o, &b) {
            return l;
        }
    }
    let mut l = render_with(s, &o, head, width, &b);
    l.truncate(max_lines);
    l
}

fn render_with(s: &Snap, a: &Opts, head: &Headline, width: usize, budget: &Budget) -> Vec<Line> {
    let mut cx = Ctx { a, s, out: vec![] };""")
sub(R, """    for g in gpus {
        cx.adapter(g);
    }
    if s.wsl.as_ref().is_some_and(|w| !w.is_empty()) {""", """    for (i, g) in gpus.into_iter().enumerate() {
        cx.adapter(g, budget.tops.get(i).copied().unwrap_or(a.top));
    }
    if budget.notes && s.wsl.as_ref().is_some_and(|w| !w.is_empty()) {""")
sub(R, """    if s.gpus.iter().any(|g| g.integrated && g.procs.iter().any(|p| p.adjusted)) {""",
    """    if budget.notes && s.gpus.iter().any(|g| g.integrated && g.procs.iter().any(|p| p.adjusted)) {""")
sub(R, """    if head.jev_tags > 0 {
        let mut f = Line::new();""", """    if budget.notes && head.jev_tags > 0 {
        let mut f = Line::new();""")

# test
sub(R, """    #[test]
    fn headline_lights_the_figure_and_the_workload() {""", """    #[test]
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
            assert!(text.iter().any(|t| t.contains("Intel(R) Graphics")), "{max}: {text:#?}");
            assert!(l.iter().all(|x| x.width() <= 100));
            if max < full.len() {
                assert!(text.iter().any(|t| t.contains("more process")), "{max}: {text:#?}");
            }
            if max >= 16 {
                // the discrete GPU's rows survive; the iGPU folds first
                assert!(text.iter().any(|t| t.contains("vmwp")), "{max}: {text:#?}");
            }
        }
        let mut all = args();
        all.all = true;
        assert!(render_fit(&s, &all, &h, 100, 9).len() > 9, "--all is never folded");
    }

    #[test]
    fn headline_lights_the_figure_and_the_workload() {""")

M = "crates/gputree/src/main.rs"
sub(M, """    fn lines(&self, width: usize) -> Vec<Line> {
        let s = model::build(&self.inp);
        let h = self.headline(&s);
        render::render(&s, &self.opts, &h, width)
    }""", """    /// `max_lines`: fit a terminal screen (TTY); None = everything (piped).
    fn lines(&self, width: usize, max_lines: Option<usize>) -> Vec<Line> {
        let s = model::build(&self.inp);
        let h = self.headline(&s);
        match max_lines {
            Some(m) => render::render_fit(&s, &self.opts, &h, width, m),
            None => render::render(&s, &self.opts, &h, width),
        }
    }""")
sub(M, """            let width = term::columns(s.a.width);
            let mut lines = s.lines(width);
            if let Some(f) = watch_footer {
                let max = rows.saturating_sub(2).max(5);
                lines.truncate(max);""", """            let width = term::columns(s.a.width);
            // on a terminal, fold the tree to the screen so every phase can redraw in
            // place (one row is kept for the cursor, one more for the watch footer)
            let fit = p.is_some().then(|| rows.saturating_sub(if watch_footer.is_some() { 2 } else { 1 }).max(5));
            let mut lines = s.lines(width, fit);
            if let Some(f) = watch_footer {
                let max = rows.saturating_sub(2).max(5);
                lines.truncate(max);""")
print("ok")
