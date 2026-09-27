//! One plain-English sentence at the top. Built locally from templates; Jev (optional)
//! only picks which template reads best and re-tags unknown processes.

use super::model::{Gpu, Proc, Snap, by_tag};

pub fn tag_phrase(tag: &str) -> &'static str {
    match tag {
        "training" => "a training run",
        "ai inference" => "AI inference",
        "compute" => "a compute job",
        "video render" => "video rendering",
        "recording" => "screen recording",
        "video playback" => "video playback",
        "game" => "a game",
        "game?" => "what looks like a game",
        "launcher" => "a game launcher",
        "3d / cad" => "3D / CAD work",
        "game dev" => "a game editor",
        "browser" => "the browser",
        "app" => "desktop apps",
        "terminal" => "the terminal",
        "desktop" => "the desktop",
        "system" => "Windows",
        "vm" => "a virtual machine",
        "audio" => "audio",
        _ => "other apps",
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn gib(b: f64) -> String {
    crate::term::fmt_bytes(b)
}

/// What an adapter is mostly doing: (tag, share of its load, the top process of that tag).
struct Busy<'a> {
    g: &'a Gpu,
    tag: &'static str,
    share: f64,
    top: Option<&'a Proc>,
}

fn busy<'a>(g: &'a Gpu, by_util: bool) -> Option<Busy<'a>> {
    let groups = by_tag(g, by_util);
    let t = groups.first()?;
    let total: f64 = if by_util { groups.iter().map(|t| t.util).sum() } else { groups.iter().map(|t| t.mem).sum() };
    let part = if by_util { t.util } else { t.mem };
    let share = if total > 0.0 { (part / total).min(1.0) } else { 0.0 };
    let top = t.procs.iter().copied().max_by(|a, b| {
        if by_util { a.util.total_cmp(&b.util) } else { g.pmem(a).total_cmp(&g.pmem(b)) }
    });
    Some(Busy { g, tag: t.tag, share, top })
}

/// "train bd1-walk-flat, WSL" for the VM, else the process name.
fn who(s: &Snap, p: &Proc) -> String {
    if p.name.eq_ignore_ascii_case("vmwp") {
        if let Some(w) = s.wsl.as_ref().and_then(|w| {
            w.iter().filter(|w| w.tag == p.tag).max_by(|a, b| a.rss.total_cmp(&b.rss)).or_else(|| w.iter().max_by(|a, b| a.rss.total_cmp(&b.rss)))
        }) {
            return format!("{}, WSL", w.label);
        }
        return "WSL".into();
    }
    p.name.clone()
}

fn share_words(share: f64) -> &'static str {
    if share >= 0.8 {
        "almost all of it is"
    } else if share >= 0.5 {
        "mostly"
    } else {
        "the biggest share is"
    }
}

pub struct Cand {
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
}

/// Every template that applies right now. The first is the local pick.
pub fn candidates(s: &Snap) -> Vec<Cand> {
    let mut out = vec![];
    if !s.util_ready {
        return out;
    }
    let mut ranked: Vec<&Gpu> = s.gpus.iter().filter(|g| !g.procs.is_empty()).collect();
    ranked.sort_by(|a, b| b.util.total_cmp(&a.util));
    let Some(top) = ranked.first() else {
        out.push(Cand { kind: "idle", text: "No GPU activity right now.".into(), emph: vec![] });
        return out;
    };
    let busy_now = top.util >= 15.0;
    let b = busy(top, top.util >= 1.0);

    if let Some(b) = &b {
        let whom = b.top.map(|p| who(s, p));
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
        let tagline_emph = vec![(cap(tag_phrase(b.tag)), tc), (pct_words, "1")];
        let idle = {
            // the adapter most in use when nothing is really busy: prefer the iGPU's story
            let ig = s.gpus.iter().filter(|g| g.integrated && !g.npu).max_by(|a, b| a.util.total_cmp(&b.util)).copied_or(top);
            let ib = busy(ig, ig.util >= 1.0);
            let what = ib.as_ref().map(|x| tag_phrase(x.tag)).unwrap_or("nothing");
            let c = ib.as_ref().map(|x| emph_color(x.tag)).unwrap_or("1");
            (format!("GPUs are mostly idle; {what} is using the {}.", ig.short), vec![(what.to_string(), c)])
        };
        if busy_now {
            out.push(Cand { kind: "detail", text: detail, emph: detail_emph });
            out.push(Cand { kind: "tag", text: tagline, emph: tagline_emph });
        } else {
            out.push(Cand { kind: "idle", text: idle.0, emph: idle.1 });
            out.push(Cand { kind: "detail", text: detail, emph: detail_emph });
        }
    }

    // two adapters doing different things
    if ranked.len() >= 2 {
        let (a, c) = (ranked[0], ranked[1]);
        if let (Some(ba), Some(bc)) = (busy(a, a.util >= 1.0), busy(c, c.util >= 1.0)) {
            if c.util >= 1.0 || c.mem > 0.0 {
                let (pa, pc) = (format!("({:.0}%)", a.util), format!("({:.0}%)", c.util));
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
                });
            }
        }
    }

    // memory angle: who holds the most VRAM on a discrete GPU
    if let Some(d) = s.gpus.iter().filter(|g| !g.integrated && g.cap > 0.0).max_by(|a, b| a.mem.total_cmp(&b.mem)) {
        if let Some(bm) = busy(d, false) {
            if let Some(p) = bm.top {
                let whom = cap(&who(s, p));
                let busy_words = format!("{:.0}% busy", d.util);
                out.push(Cand {
                    kind: "mem",
                    text: format!("{whom} holds {} of the {}'s {} VRAM, which is {busy_words}.", gib(d.pmem(p)), d.short, gib(d.cap)),
                    emph: vec![(whom.clone(), emph_color(p.tag)), (busy_words, "1")],
                });
            }
        }
    }
    out
}

trait CopiedOr<'a> {
    fn copied_or(self, d: &'a Gpu) -> &'a Gpu;
}
impl<'a> CopiedOr<'a> for Option<&'a Gpu> {
    fn copied_or(self, d: &'a Gpu) -> &'a Gpu {
        self.unwrap_or(d)
    }
}
