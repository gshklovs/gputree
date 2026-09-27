//! cputree's slower inputs: per-process Linux CPU inside WSL, and the Jev request.

use super::headline;
use super::model::{LinuxProc, Snap, groups};
use crate::jev::{self, JevJob};
use crate::term::fmt_bytes;
use crate::{tags, wsl};
use std::collections::HashMap;

/// Samples /proc/*/stat twice ~300 ms apart and prints the busiest processes.
/// Only reads /proc; writes nothing.
const WSL_CPU_SCRIPT: &str = r#"CLK=$(getconf CLK_TCK 2>/dev/null || echo 100)
PG=$(getconf PAGESIZE 2>/dev/null || echo 4096)
snap() { for f in /proc/[0-9]*/stat; do read -r l < "$f" 2>/dev/null && echo "$l"; done; }
u0=$(cut -d' ' -f1 /proc/uptime); a=$(snap)
sleep 0.3
u1=$(cut -d' ' -f1 /proc/uptime); b=$(snap)
echo "H|$u0|$u1|$CLK|$(nproc)"
{ echo "$a" | sed 's/^/A /'; echo "$b" | sed 's/^/B /'; } | awk -v pg="$PG" '
{ k = substr($0, 1, 1); line = substr($0, 3)
  i = length(line); while (i > 0 && substr(line, i, 1) != ")") i--
  if (i == 0) next
  pid = substr(line, 1, index(line, " ") - 1)
  n = split(substr(line, i + 2), f, " ")
  t = f[12] + f[13]
  if (k == "A") t0[pid] = t; else { t1[pid] = t; rss[pid] = f[22] * pg }
}
END { for (p in t1) { d = (p in t0) ? t1[p] - t0[p] : 0; if (d > 0 || rss[p] > 50000000) print d "|" p "|" rss[p] } }' \
| sort -t'|' -k1,1nr | head -12 | while IFS='|' read -r d pid rss; do
  cmd=$(tr '\0' ' ' < /proc/$pid/cmdline 2>/dev/null)
  [ -z "$cmd" ] && continue
  cwd=$(readlink /proc/$pid/cwd 2>/dev/null)
  user=$(stat -c %U /proc/$pid 2>/dev/null)
  echo "P|$pid|$d|$rss|$user|$cwd|$cmd"
done
"#;

/// Busiest Linux processes in every running distro. CPU is a share of all `ncpu`
/// Windows logical processors, so it compares directly with Windows processes.
pub fn wsl_cpu_procs(ncpu: usize) -> Vec<LinuxProc> {
    let mut out = vec![];
    for (distro, text) in wsl::run_everywhere(WSL_CPU_SCRIPT) {
        let (mut secs, mut clk) = (0.3f64, 100f64);
        for l in text.lines() {
            let f: Vec<&str> = l.splitn(7, '|').collect();
            if f.first() == Some(&"H") && f.len() >= 4 {
                let u0: f64 = f[1].parse().unwrap_or(0.0);
                let u1: f64 = f[2].parse().unwrap_or(0.0);
                if u1 > u0 {
                    secs = u1 - u0;
                }
                clk = f[3].trim().parse().unwrap_or(100.0);
                continue;
            }
            if f.first() != Some(&"P") || f.len() < 7 {
                continue;
            }
            let ticks: f64 = f[2].parse().unwrap_or(0.0);
            let cmd = f[6].trim().to_string();
            let first = cmd.split_whitespace().next().unwrap_or("");
            let base = first.rsplit('/').next().unwrap_or(first);
            let (label, project) = wsl::shorten(&cmd, f[5]);
            out.push(LinuxProc {
                distro: distro.clone(),
                pid: f[1].parse().unwrap_or(0),
                cpu: (ticks / clk / secs / ncpu.max(1) as f64 * 100.0).clamp(0.0, 100.0),
                rss: f[3].parse().unwrap_or(0.0),
                user: f[4].to_string(),
                tag: tags::tag(base, "", &cmd, None),
                cmd,
                label,
                project,
            });
        }
    }
    out.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.rss.total_cmp(&a.rss)));
    out
}

/// Compact, privacy-safe state: totals, then the top workloads (names, tags, numbers,
/// shortened WSL commands only).
pub fn jev_state(s: &Snap) -> String {
    let mut o = format!(
        "CPU: {:.0}% busy of {} logical processors (user {:.0}%, kernel {:.0}%). RAM: {} of {} used.\n",
        s.total,
        s.ncpu,
        s.user,
        s.kernel,
        fmt_bytes(s.mem_used),
        fmt_bytes(s.mem_total)
    );
    let all: Vec<usize> = (0..s.procs.len()).collect();
    let mut g = groups(s, &all, true, false);
    g.truncate(8);
    o.push_str("Top processes by CPU (merged by name, share of the whole machine):\n");
    for x in &g {
        o.push_str(&format!(
            "- {}{} [{}] cpu {:.1}% mem {}{}\n",
            x.name,
            if x.members.len() > 1 { format!(" x{}", x.members.len()) } else { String::new() },
            x.tag,
            x.own_cpu,
            fmt_bytes(x.own_mem),
            if x.is_vm() { " (the WSL2 virtual machine)" } else { "" }
        ));
    }
    if let Some(w) = &s.wsl {
        if !w.is_empty() {
            o.push_str("Inside WSL:\n");
            for l in w.iter().take(5) {
                o.push_str(&format!("- {} {} [{}] cpu {:.1}% rss {}\n", l.label, l.context(), l.tag, l.cpu, fmt_bytes(l.rss)));
            }
        }
    }
    o
}

pub fn jev_job(s: &Snap, known: &HashMap<String, &'static str>) -> Option<JevJob> {
    let cands = headline::candidates(s);
    let mut qs = vec![];
    if cands.len() >= 2 {
        qs.push(jev::Question {
            key: "headline".into(),
            instructions: "Pick the one sentence that most accurately and helpfully tells a non-expert what is using their CPU right now, given the state. Prefer the sentence that names the workloads that actually dominate CPU load.".into(),
            criteria: cands.iter().map(|c| (c.kind.to_string(), c.text.clone())).collect(),
        });
    }
    let all: Vec<usize> = (0..s.procs.len()).collect();
    let mut asked: HashMap<String, String> = HashMap::new();
    for g in groups(s, &all, true, false).iter().filter(|g| g.tag == "other" && !g.jev_tag).take(12) {
        let lname = g.name.to_lowercase();
        if known.contains_key(&lname) || asked.len() >= 6 || (g.own_cpu < 0.5 && g.own_mem < 100e6) {
            continue;
        }
        let key = format!("tag_{}", asked.len());
        qs.push(jev::tag_question(
            key.clone(),
            format!(
                "Windows process '{}'{} is using {:.1}% of the CPU and {} of memory. Which category best describes what this program is?",
                g.name,
                if g.members.len() > 1 { format!(" ({} instances)", g.members.len()) } else { String::new() },
                g.own_cpu,
                fmt_bytes(g.own_mem)
            ),
        ));
        asked.insert(key, lname);
    }
    (!qs.is_empty()).then(|| JevJob { state: jev_state(s), questions: qs, asked })
}
