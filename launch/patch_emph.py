import os
os.chdir(r"C:\Users\grego\repos\gputree")


def sub(p, a, b):
    s = open(p, encoding="utf-8").read()
    assert a in s, (p, a[:80])
    s = s.replace(a, b, 1)
    open(p, "w", encoding="utf-8", newline="\n").write(s)


L = "crates/treecore/src/layout.rs"
sub(L, """    pub jev_tags: usize,
}

impl Headline {
    pub fn line(&self) -> Line {
        let mut h = Line::new();
        h.push("1", self.text.clone());""", """    pub jev_tags: usize,
    /// key phrases to colour, in the order they appear in `text`: (substring, SGR)
    pub emph: Vec<(String, &'static str)>,
}

impl Headline {
    pub fn line(&self) -> Line {
        let mut h = Line::new();
        if self.emph.is_empty() {
            h.push("1", self.text.clone());
        } else {
            // plain sentence, key phrases lit: the figure in bold, the workload in its tag colour
            let mut rest = self.text.as_str();
            for (needle, sgr) in &self.emph {
                if let Some(i) = rest.find(needle.as_str()).filter(|_| !needle.is_empty()) {
                    if i > 0 {
                        h.plain(rest[..i].to_string());
                    }
                    h.push(sgr, needle.clone());
                    rest = &rest[i + needle.len()..];
                }
            }
            if !rest.is_empty() {
                h.plain(rest.to_string());
            }
        }""")

sub("crates/treecore/src/cpu/render.rs", 'Headline { text, source: "", jev_tags: 0 }', 'Headline { text, source: "", jev_tags: 0, emph: vec![] }')

sub("crates/treecore/src/gpu/render.rs", """    let text = headline::candidates(s)
        .into_iter()
        .next()
        .map(|c| c.text)
        .unwrap_or_else(|| "Sampling GPU load…".into());
    Headline { text, source: "", jev_tags: 0 }""", """    match headline::candidates(s).into_iter().next() {
        Some(c) => Headline { text: c.text, source: "", jev_tags: 0, emph: c.emph },
        None => Headline { text: "Sampling GPU load…".into(), source: "", jev_tags: 0, emph: vec![] },
    }""")

H = "crates/treecore/src/gpu/headline.rs"
sub(H, """pub struct Cand {
    pub kind: &'static str,
    pub text: String,
}""", """pub struct Cand {
    pub kind: &'static str,
    pub text: String,
    /// key phrases in text order, for the terminal to colour: (substring, SGR)
    pub emph: Vec<(String, &'static str)>,
}

/// Bold version of a tag's chip colour (dim tags fall back to plain bold).
fn emph_color(tag: &str) -> &'static str {
    match crate::tags::color(tag) {
        "2" => "1",
        "35" => "1;35",
        "33" => "1;33",
        "32" => "1;32",
        "36" => "1;36",
        "37" => "1;37",
        c => c,
    }
}""")
sub(H, """        out.push(Cand { kind: "idle", text: "No GPU activity right now.".into() });""",
    """        out.push(Cand { kind: "idle", text: "No GPU activity right now.".into(), emph: vec![] });""")
sub(H, """        let whom = b.top.map(|p| who(s, p));
        let detail = match &whom {
            Some(w) => format!(
                "Your {} is {:.0}% busy — {} {} ({w}).",
                b.g.short,
                b.g.util,
                share_words(b.share),
                tag_phrase(b.tag)
            ),
            None => format!("Your {} is {:.0}% busy.", b.g.short, b.g.util),
        };
        let tagline = format!("{} is using the {} ({:.0}%).", cap(tag_phrase(b.tag)), b.g.short, b.g.util);""",
    """        let whom = b.top.map(|p| who(s, p));
        let busy_words = format!("{:.0}% busy", b.g.util);
        let tc = emph_color(b.tag);
        let (detail, detail_emph) = match &whom {
            Some(w) => {
                let what = format!("{} ({w})", tag_phrase(b.tag));
                (
                    format!("Your {} is {busy_words} — {} {what}.", b.g.short, share_words(b.share)),
                    vec![(busy_words.clone(), "1"), (what, tc)],
                )
            }
            None => (format!("Your {} is {busy_words}.", b.g.short), vec![(busy_words.clone(), "1")]),
        };
        let pct_words = format!("({:.0}%)", b.g.util);
        let tagline = format!("{} is using the {} {pct_words}.", cap(tag_phrase(b.tag)), b.g.short);
        let tagline_emph = vec![(cap(tag_phrase(b.tag)), tc), (pct_words, "1")];""")
sub(H, """            let what = ib.as_ref().map(|x| tag_phrase(x.tag)).unwrap_or("nothing");
            format!("GPUs are mostly idle; {what} is using the {}.", ig.short)
        };
        if busy_now {
            out.push(Cand { kind: "detail", text: detail });
            out.push(Cand { kind: "tag", text: tagline });
        } else {
            out.push(Cand { kind: "idle", text: idle });
            out.push(Cand { kind: "detail", text: detail });
        }""", """            let what = ib.as_ref().map(|x| tag_phrase(x.tag)).unwrap_or("nothing");
            let c = ib.as_ref().map(|x| emph_color(x.tag)).unwrap_or("1");
            (format!("GPUs are mostly idle; {what} is using the {}.", ig.short), vec![(what.to_string(), c)])
        };
        if busy_now {
            out.push(Cand { kind: "detail", text: detail, emph: detail_emph });
            out.push(Cand { kind: "tag", text: tagline, emph: tagline_emph });
        } else {
            out.push(Cand { kind: "idle", text: idle.0, emph: idle.1 });
            out.push(Cand { kind: "detail", text: detail, emph: detail_emph });
        }""")
sub(H, """                out.push(Cand {
                    kind: "split",
                    text: format!(
                        "The {} is busy with {} ({:.0}%) while the {} handles {} ({:.0}%).",
                        a.short,
                        tag_phrase(ba.tag),
                        a.util,
                        c.short,
                        tag_phrase(bc.tag),
                        c.util
                    ),
                });""", """                let (pa, pc) = (format!("({:.0}%)", a.util), format!("({:.0}%)", c.util));
                out.push(Cand {
                    kind: "split",
                    text: format!(
                        "The {} is busy with {} {pa} while the {} handles {} {pc}.",
                        a.short,
                        tag_phrase(ba.tag),
                        c.short,
                        tag_phrase(bc.tag),
                    ),
                    emph: vec![
                        (tag_phrase(ba.tag).to_string(), emph_color(ba.tag)),
                        (pa, "1"),
                        (tag_phrase(bc.tag).to_string(), emph_color(bc.tag)),
                        (pc, "1"),
                    ],
                });""")
sub(H, """                out.push(Cand {
                    kind: "mem",
                    text: format!(
                        "{} holds {} of the {}'s {} VRAM, which is {:.0}% busy.",
                        cap(&who(s, p)),
                        gib(d.pmem(p)),
                        d.short,
                        gib(d.cap),
                        d.util
                    ),
                });""", """                let whom = cap(&who(s, p));
                let busy_words = format!("{:.0}% busy", d.util);
                out.push(Cand {
                    kind: "mem",
                    text: format!("{whom} holds {} of the {}'s {} VRAM, which is {busy_words}.", gib(d.pmem(p)), d.short, gib(d.cap)),
                    emph: vec![(whom.clone(), emph_color(p.tag)), (busy_words, "1")],
                });""")

sub("crates/gputree/src/main.rs", """                h.text = c.text;
                h.source = "jev";""", """                h.text = c.text;
                h.emph = c.emph;
                h.source = "jev";""")

# test: the headline lights the % and the workload
sub("crates/treecore/src/gpu/render.rs", """    #[test]
    fn never_wider_than_the_terminal_and_columns_align() {""", """    #[test]
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
    fn never_wider_than_the_terminal_and_columns_align() {""")
print("patched")
