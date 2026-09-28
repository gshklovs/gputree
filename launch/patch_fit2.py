import os
os.chdir(r"C:\Users\grego\repos\gputree")


def sub(p, a, b):
    s = open(p, encoding="utf-8").read()
    assert a in s, (p, a[:80])
    s = s.replace(a, b, 1)
    open(p, "w", encoding="utf-8", newline="\n").write(s)


R = "crates/treecore/src/gpu/render.rs"
# spill rows become their own fold step
sub(R, """struct Budget {
    tops: Vec<usize>,
    notes: bool,
}""", """struct Budget {
    tops: Vec<usize>,
    notes: bool,
    /// the "+ spilled to shared memory" rows
    spill: bool,
}""")
sub(R, """struct Ctx<'a> {
    a: &'a Opts,
    s: &'a Snap,
    out: Vec<Out>,
}""", """struct Ctx<'a> {
    a: &'a Opts,
    s: &'a Snap,
    out: Vec<Out>,
    spill: bool,
}""")
sub(R, """        if a.depth >= 3 && !g.integrated && p.shr >= 1048576.0 {""", """        if self.spill && a.depth >= 3 && !g.integrated && p.shr >= 1048576.0 {""")
sub(R, """    render_with(s, a, head, width, &Budget { tops: vec![a.top; s.gpus.len()], notes: true })""",
    """    render_with(s, a, head, width, &Budget { tops: vec![a.top; s.gpus.len()], notes: true, spill: true })""")
sub(R, """    let mut b = Budget { tops: vec![a.top; s.gpus.len()], notes: true };""",
    """    let mut b = Budget { tops: vec![a.top; s.gpus.len()], notes: true, spill: true };""")
sub(R, """    fold.sort_by(|&i, &j| drawn[i].util.total_cmp(&drawn[j].util).then(j.cmp(&i)));""",
    """    // (before util is sampled everything ties at 0: fold integrated adapters before discrete ones)
    fold.sort_by(|&i, &j| {
        drawn[i].util.total_cmp(&drawn[j].util).then(drawn[j].integrated.cmp(&drawn[i].integrated)).then(j.cmp(&i))
    });""")
sub(R, """    b.notes = false;
    if let Some(l) = fits(&o, &b) {
        return l;
    }""", """    b.notes = false;
    if let Some(l) = fits(&o, &b) {
        return l;
    }
    b.spill = false;
    if let Some(l) = fits(&o, &b) {
        return l;
    }""")
sub(R, """    let mut cx = Ctx { a, s, out: vec![] };""", """    let mut cx = Ctx { a, s, out: vec![], spill: budget.spill };""")
sub(R, """/// detail from the least busy adapters first (their process rows), then the footnotes,
/// then the busiest adapter's engine rows, then its processes. `--all` is never folded.""",
    """/// detail from the least busy adapters first (their process rows), then the footnotes,
/// then the spill notes, then the busiest adapter's engine rows, then its processes.
/// `--all` is never folded.""")

# cputree: same idea, simpler knobs
C = "crates/treecore/src/cpu/render.rs"
sub(C, """pub fn render(s: &Snap, o: &Opts, head: &Headline, width: usize) -> Vec<Line> {""",
    """/// Like `render`, but at most `max_lines` tall (for a terminal screen): fewer children
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

pub fn render(s: &Snap, o: &Opts, head: &Headline, width: usize) -> Vec<Line> {""")

M = "crates/cputree/src/main.rs"
s = open(M, encoding="utf-8").read()
print([l for l in s.splitlines() if "fn lines" in l or "render::render(" in l])
