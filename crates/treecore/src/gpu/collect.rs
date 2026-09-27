//! Gathering for gputree's slower inputs: Linux processes on the GPU inside WSL, and
//! the Jev request (compact state + questions).

use super::headline;
use super::model::{Proc, Snap, WslProc, is_active, sort_procs};
use crate::jev::{self, JevJob};
use crate::term::fmt_bytes;
use crate::{tags, wsl};
use std::collections::HashMap;

const WSL_SCRIPT: &str = r#"for p in /proc/[0-9]*; do
  ls -l $p/fd 2>/dev/null | grep -q /dev/dxg || continue
  pid=${p#/proc/}
  rss=$(awk '/^VmRSS/{print $2}' $p/status 2>/dev/null)
  user=$(stat -c %U $p 2>/dev/null)
  cwd=$(readlink $p/cwd 2>/dev/null)
  cmd=$(tr '\0' ' ' < $p/cmdline 2>/dev/null)
  echo "$pid|${rss:-0}|$user|$cwd|$cmd"
done
"#;

/// Linux processes inside every running WSL distro that hold the paravirtual GPU.
pub fn wsl_gpu_procs() -> Vec<WslProc> {
    let mut out = vec![];
    for (distro, text) in wsl::run_everywhere(WSL_SCRIPT) {
        for l in text.lines() {
            let f: Vec<&str> = l.splitn(5, '|').collect();
            if f.len() < 5 {
                continue;
            }
            let cmd = f[4].trim().to_string();
            if cmd.is_empty() {
                continue;
            }
            let first = cmd.split_whitespace().next().unwrap_or("");
            let base = first.rsplit('/').next().unwrap_or(first);
            let (label, project) = wsl::shorten(&cmd, f[3]);
            out.push(WslProc {
                distro: distro.clone(),
                pid: f[0].parse().unwrap_or(0),
                rss: f[1].trim().parse::<f64>().unwrap_or(0.0) * 1024.0,
                user: f[2].to_string(),
                tag: tags::tag(base, "", &cmd, None),
                cmd,
                label,
                project,
            });
        }
    }
    out
}

/// Compact, privacy-safe text state for Jev: names, tags, numbers and short commands only.
pub fn jev_state(s: &Snap) -> String {
    let mut o = String::new();
    for g in &s.gpus {
        let eng: Vec<String> = g.eng.iter().filter(|(_, v)| **v >= 0.5).map(|(k, v)| format!("{k} {v:.0}%")).collect();
        o.push_str(&format!(
            "- {}{}: util {:.0}% ({}), memory {} of {}",
            g.short,
            if g.npu { " (NPU)" } else if g.integrated { " (integrated)" } else { "" },
            g.util,
            if eng.is_empty() { "idle".into() } else { eng.join(", ") },
            fmt_bytes(g.mem),
            if g.cap > 0.0 { fmt_bytes(g.cap) } else { "?".into() },
        ));
        if let Some(n) = &g.nv {
            if let Some(t) = n.temp {
                o.push_str(&format!(", {t}C"));
            }
        }
        o.push('\n');
        let mut ps: Vec<&Proc> = g.procs.iter().filter(|p| is_active(p, false)).collect();
        sort_procs(g, &mut ps, true);
        for p in ps.iter().take(6) {
            o.push_str(&format!("  - {} [{}] util {:.0}% mem {}", p.name, p.tag, p.util, fmt_bytes(g.pmem(p))));
            if p.name.eq_ignore_ascii_case("vmwp") {
                o.push_str(" (the WSL2 virtual machine)");
                if let Some(w) = &s.wsl {
                    let inner: Vec<String> = w.iter().take(4).map(|w| format!("{} {} [{}]", w.label, w.context(), w.tag)).collect();
                    if !inner.is_empty() {
                        o.push_str(&format!("; Linux processes using the GPU: {}", inner.join("; ")));
                    }
                }
            }
            o.push('\n');
        }
    }
    o
}

fn jev_questions(s: &Snap, cands: &[headline::Cand], known: &HashMap<String, &'static str>) -> (Vec<jev::Question>, HashMap<String, String>) {
    let mut qs = vec![];
    if cands.len() >= 2 {
        qs.push(jev::Question {
            key: "headline".into(),
            instructions: "Pick the one sentence that most accurately and helpfully tells a non-expert what their GPUs are doing right now, given the state. Prefer the sentence that names the workload that actually dominates GPU load.".into(),
            criteria: cands.iter().map(|c| (c.kind.to_string(), c.text.clone())).collect(),
        });
    }
    // unknown processes worth asking about, once per name
    let mut asked: HashMap<String, String> = HashMap::new();
    for g in &s.gpus {
        let mut ps: Vec<&Proc> = g.procs.iter().filter(|p| matches!(p.tag, "other" | "game?") && !p.jev_tag && is_active(p, false)).collect();
        sort_procs(g, &mut ps, true);
        for p in ps {
            let lname = p.name.to_lowercase();
            if p.name.starts_with('<') || known.contains_key(&lname) || asked.values().any(|v| *v == lname) || asked.len() >= 6 {
                continue;
            }
            let key = format!("tag_{}", asked.len());
            let eng: Vec<String> = p.eng.iter().filter(|(_, v)| **v >= 0.5).map(|(k, v)| format!("{k} {v:.0}%")).collect();
            qs.push(jev::tag_question(
                key.clone(),
                format!(
                    "Windows process '{}' is using the {} ({} of GPU memory, engines: {}). Which category best describes what this program is?",
                    p.name,
                    g.short,
                    fmt_bytes(g.pmem(p)),
                    if eng.is_empty() { "idle".into() } else { eng.join(", ") }
                ),
            ));
            asked.insert(key, lname);
        }
    }
    (qs, asked)
}


/// Everything Jev needs for one call, or None when there is nothing to ask.
pub fn jev_job(s: &Snap, known: &HashMap<String, &'static str>) -> Option<JevJob> {
    let cands = headline::candidates(s);
    let (questions, asked) = jev_questions(s, &cands, known);
    (!questions.is_empty()).then(|| JevJob { state: jev_state(s), questions, asked })
}
