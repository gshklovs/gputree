import os
os.chdir(r"C:\Users\grego\repos\gputree")


def sub(p, a, b):
    s = open(p, encoding="utf-8").read()
    assert a in s, (p, a[:80])
    s = s.replace(a, b, 1)
    open(p, "w", encoding="utf-8", newline="\n").write(s)


R = "crates/treecore/src/gpu/render.rs"
sub(R, """/// detail from the bottom: later adapters' process rows first, then the footnotes,
/// then the first adapter's engine rows, then its processes. `--all` is never folded.""",
    """/// detail from the least busy adapters first (their process rows), then the footnotes,
/// then the busiest adapter's engine rows, then its processes. `--all` is never folded.""")
sub(R, """    for i in (1..b.tops.len()).rev() {
        while b.tops[i] > 0 {""", """    // fold order: least busy adapter first (ties: the one drawn lower first)
    let drawn = draw_order(s, a);
    let mut fold: Vec<usize> = (0..drawn.len()).collect();
    fold.sort_by(|&i, &j| drawn[i].util.total_cmp(&drawn[j].util).then(j.cmp(&i)));
    let keep = fold.pop().unwrap_or(0);
    for i in fold {
        while b.tops[i] > 0 {""")
sub(R, """    while b.tops.first().is_some_and(|t| *t > 0) {
        b.tops[0] -= 1;""", """    while b.tops.get(keep).is_some_and(|t| *t > 0) {
        b.tops[keep] -= 1;""")
sub(R, """fn render_with(s: &Snap, a: &Opts, head: &Headline, width: usize, budget: &Budget) -> Vec<Line> {""",
    """/// Adapters in drawing order: by memory (or util with --metric util), largest first.
fn draw_order<'a>(s: &'a Snap, a: &Opts) -> Vec<&'a Gpu> {
    let mut gpus: Vec<&Gpu> = s.gpus.iter().collect();
    gpus.sort_by(|x, y| if a.metric_util { y.util.total_cmp(&x.util) } else { y.mem.total_cmp(&x.mem) });
    gpus
}

fn render_with(s: &Snap, a: &Opts, head: &Headline, width: usize, budget: &Budget) -> Vec<Line> {""")
sub(R, """    let mut gpus: Vec<&Gpu> = s.gpus.iter().collect();
    gpus.sort_by(|x, y| if a.metric_util { y.util.total_cmp(&x.util) } else { y.mem.total_cmp(&x.mem) });
    for (i, g) in gpus.into_iter().enumerate() {""", """    for (i, g) in draw_order(s, a).into_iter().enumerate() {""")
sub(R, """        let mut all = args();
        all.all = true;""", """        // a busier adapter drawn lower keeps its rows; the idle one on top folds
        let mut s2 = s.clone();
        s2.gpus[0].util = 2.0;
        s2.gpus[1].mem = 5e9;
        s2.gpus[1].util = 90.0;
        let l = render_fit(&s2, &args(), &local_headline(&s2), 100, 20);
        let text: Vec<String> = l.iter().map(|l| l.render(false)).collect();
        assert!(text.iter().position(|t| t.contains("Intel(R) Graphics")) < text.iter().position(|t| t.contains("NVIDIA GeForce")), "{text:#?}");
        assert!(text.iter().any(|t| t.contains("app0")), "{text:#?}");
        assert!(!text.iter().any(|t| t.contains("vmwp")), "{text:#?}");

        let mut all = args();
        all.all = true;""")
print("ok")
