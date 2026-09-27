//! One plain-English sentence at the top. Built locally from templates; Jev (optional)
//! only picks which template reads best and re-tags unknown processes.

use super::model::{Gpu, Proc, Snap, by_tag, is_vm_host};

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
        "system" => crate::SYSTEM_PHRASE,
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
    if is_vm_host(&p.name) {
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
        out.push(Cand { kind: "idle", text: "No GPU activity right now.".into() });
        return out;
    };
    let busy_now = top.util >= 15.0;
    let b = busy(top, top.util >= 1.0);

    if let Some(b) = &b {
        let whom = b.top.map(|p| who(s, p));
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
        let tagline = format!("{} is using the {} ({:.0}%).", cap(tag_phrase(b.tag)), b.g.short, b.g.util);
        let idle = {
            // the adapter most in use when nothing is really busy: prefer the iGPU's story
            let ig = s.gpus.iter().filter(|g| g.integrated && !g.npu).max_by(|a, b| a.util.total_cmp(&b.util)).copied_or(top);
            let ib = busy(ig, ig.util >= 1.0);
            let what = ib.as_ref().map(|x| tag_phrase(x.tag)).unwrap_or("nothing");
            format!("GPUs are mostly idle; {what} is using the {}.", ig.short)
        };
        if busy_now {
            out.push(Cand { kind: "detail", text: detail });
            out.push(Cand { kind: "tag", text: tagline });
        } else {
            out.push(Cand { kind: "idle", text: idle });
            out.push(Cand { kind: "detail", text: detail });
        }
    }

    // two adapters doing different things
    if ranked.len() >= 2 {
        let (a, c) = (ranked[0], ranked[1]);
        if let (Some(ba), Some(bc)) = (busy(a, a.util >= 1.0), busy(c, c.util >= 1.0)) {
            if c.util >= 1.0 || c.mem > 0.0 {
                out.push(Cand {
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
                });
            }
        }
    }

    // memory angle: who holds the most VRAM on a discrete GPU
    if let Some(d) = s.gpus.iter().filter(|g| !g.integrated && g.cap > 0.0).max_by(|a, b| a.mem.total_cmp(&b.mem)) {
        if let Some(bm) = busy(d, false) {
            if let Some(p) = bm.top {
                out.push(Cand {
                    kind: "mem",
                    text: format!(
                        "{} holds {} of the {}'s {} VRAM, which is {:.0}% busy.",
                        cap(&who(s, p)),
                        gib(d.pmem(p)),
                        d.short,
                        gib(d.cap),
                        d.util
                    ),
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
