//! Gathering for gputree's slower inputs: Linux processes on the GPU inside WSL, and
//! the Jev request (compact state + questions).

use super::headline;
use super::model::{Proc, Snap, WslProc, is_active, is_vm_host, sort_procs};
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

/// One `pid|rss KiB|user|cwd|cmd` line (the script's output) -> a row.
fn wsl_row(distro: &str, l: &str) -> Option<WslProc> {
    let f: Vec<&str> = l.splitn(5, '|').collect();
    if f.len() < 5 {
        return None;
    }
    let cmd = f[4].trim().to_string();
    if cmd.is_empty() {
        return None;
    }
    let first = cmd.split_whitespace().next().unwrap_or("");
    let base = first.rsplit('/').next().unwrap_or(first);
    let (label, project) = wsl::shorten(&cmd, f[3]);
    Some(WslProc {
        distro: distro.to_string(),
        pid: f[0].parse().unwrap_or(0),
        rss: f[1].trim().parse::<f64>().unwrap_or(0.0) * 1024.0,
        user: f[2].to_string(),
        tag: tags::tag(base, "", &cmd, None),
        cmd,
        label,
        project,
    })
}

/// Linux processes inside every running WSL distro that hold the paravirtual GPU.
#[cfg(windows)]
pub fn wsl_gpu_procs() -> Vec<WslProc> {
    let mut out = vec![];
    for (distro, text) in wsl::run_everywhere(WSL_SCRIPT) {
        out.extend(text.lines().filter_map(|l| wsl_row(&distro, l)));
    }
    out
}

/// Inside WSL itself: the processes holding /dev/dxg, read directly from /proc (the
/// same thing the Windows side's script prints). Other users' fds need root.
#[cfg(target_os = "linux")]
pub fn wsl_gpu_procs() -> Vec<WslProc> {
    let _ = WSL_SCRIPT;
    let distro = crate::linux::distro();
    let mut out = vec![];
    let me = std::process::id(); // gputree holds /dev/dxg itself once NVML is loaded
    for pid in crate::linux::pids().into_iter().filter(|&p| p != me) {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        let holds = rd.flatten().any(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.as_os_str() == "/dev/dxg"));
        if !holds {
            continue;
        }
        let rss = crate::linux::read(format!("/proc/{pid}/status")).and_then(|s| crate::procfs::parse_status_rss(&s)).unwrap_or(0);
        let user = crate::linux::uid_of(pid).map(crate::linux::user_of).unwrap_or_default();
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let cmd = crate::linux::ident_of(pid).map(|i| i.cmd).unwrap_or_default();
        out.extend(wsl_row(&distro, &format!("{pid}|{}|{user}|{cwd}|{cmd}", rss / 1024)));
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
            if is_vm_host(&p.name) {
                o.push_str(if p.name == super::model::DXG_HOST {
                    " (the whole GPU as seen from inside WSL; per-process GPU use is not visible there)"
                } else {
                    " (the WSL2 virtual machine)"
                });
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
                    "{} process '{}' is using the {} ({} of GPU memory, engines: {}). Which category best describes what this program is?",
                    crate::OS,
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
