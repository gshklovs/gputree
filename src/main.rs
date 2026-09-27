//! gputree - a disktree-style tree of what is using your GPUs.
//! Read-only: it samples counters and reads /proc, and never touches any process.

mod args;
mod headline;
mod jev;
mod model;
mod nvidia;
mod render;
mod tags;
mod term;
mod win;
mod wsl;

use model::{Inputs, Snap, WslProc};
use render::Headline;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use term::{Line, Painter};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Second PDH sample this long after the first (utilisation is a rate).
const UTIL_WINDOW: Duration = Duration::from_millis(280);
/// Everything async (NVML, WSL, Jev) must land within this, measured from start.
const DEADLINE: Duration = Duration::from_millis(2200);
/// Start Jev without WSL names if WSL has not answered by then.
const JEV_LATEST_START: Duration = Duration::from_millis(1100);

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

enum Msg {
    Nv(Vec<nvidia::NvStats>),
    Wsl(Vec<WslProc>),
    Jev(Result<JevResult, String>, Duration),
}

struct JevResult {
    kind: Option<String>,
    tags: HashMap<String, &'static str>,
}

/// Linux processes inside every running WSL distro that hold the paravirtual GPU.
fn wsl_gpu_procs() -> Vec<WslProc> {
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
fn jev_state(s: &Snap) -> String {
    let mut o = String::new();
    for g in &s.gpus {
        let eng: Vec<String> = g.eng.iter().filter(|(_, v)| **v >= 0.5).map(|(k, v)| format!("{k} {v:.0}%")).collect();
        o.push_str(&format!(
            "- {}{}: util {:.0}% ({}), memory {} of {}",
            g.short,
            if g.npu { " (NPU)" } else if g.integrated { " (integrated)" } else { "" },
            g.util,
            if eng.is_empty() { "idle".into() } else { eng.join(", ") },
            term::fmt_bytes(g.mem),
            if g.cap > 0.0 { term::fmt_bytes(g.cap) } else { "?".into() },
        ));
        if let Some(n) = &g.nv {
            if let Some(t) = n.temp {
                o.push_str(&format!(", {t}C"));
            }
        }
        o.push('\n');
        let mut ps: Vec<&model::Proc> = g.procs.iter().filter(|p| model::is_active(p, false)).collect();
        model::sort_procs(g, &mut ps, true);
        for p in ps.iter().take(6) {
            o.push_str(&format!("  - {} [{}] util {:.0}% mem {}", p.name, p.tag, p.util, term::fmt_bytes(g.pmem(p))));
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
        let mut ps: Vec<&model::Proc> = g.procs.iter().filter(|p| matches!(p.tag, "other" | "game?") && !p.jev_tag && model::is_active(p, false)).collect();
        model::sort_procs(g, &mut ps, true);
        for p in ps {
            let lname = p.name.to_lowercase();
            if p.name.starts_with('<') || known.contains_key(&lname) || asked.values().any(|v| *v == lname) || asked.len() >= 6 {
                continue;
            }
            let key = format!("tag_{}", asked.len());
            let eng: Vec<String> = p.eng.iter().filter(|(_, v)| **v >= 0.5).map(|(k, v)| format!("{k} {v:.0}%")).collect();
            qs.push(jev::Question {
                key: key.clone(),
                instructions: format!(
                    "Windows process '{}' is using the {} ({} of GPU memory, engines: {}). Which category best describes what this program is?",
                    p.name,
                    g.short,
                    term::fmt_bytes(g.pmem(p)),
                    if eng.is_empty() { "idle".into() } else { eng.join(", ") }
                ),
                criteria: tags::ALL.iter().map(|t| (t.to_string(), tags::describe(t).to_string())).collect(),
            });
            asked.insert(key, lname);
        }
    }
    (qs, asked)
}

fn spawn_jev(tx: &Sender<Msg>, key: String, s: &Snap, known: &HashMap<String, &'static str>, timeout: Duration) -> bool {
    let cands = headline::candidates(s);
    let (qs, asked) = jev_questions(s, &cands, known);
    if qs.is_empty() {
        return false;
    }
    let state = jev_state(s);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let t = Instant::now();
        let r = jev::ask(&key, &state, &qs, timeout).map(|ans| {
            let mut out = JevResult { kind: None, tags: HashMap::new() };
            for (k, a) in ans {
                if k == "headline" {
                    out.kind = Some(a.choice);
                } else if let (Some(name), Some(tag)) = (asked.get(&k), tags::intern(&a.choice)) {
                    if tag != "other" && a.confidence >= 0.5 {
                        out.tags.insert(name.clone(), tag);
                    }
                }
            }
            out
        });
        let _ = tx.send(Msg::Jev(r, t.elapsed()));
    });
    true
}

/// State that survives between --watch frames.
struct Session {
    a: args::Args,
    key: Option<String>,
    pdh: win::Counters,
    inp: Inputs,
    jev_kind: Option<String>,
    jev_sig: String,
    jev_at: Option<Instant>,
    timing: Vec<(String, Duration)>,
    start: Instant,
    frames: u32,
}

impl Session {
    fn headline(&self, s: &Snap) -> Headline {
        let mut h = render::local_headline(s);
        if let Some(k) = &self.jev_kind {
            if let Some(c) = headline::candidates(s).into_iter().find(|c| c.kind == k) {
                h.text = c.text;
                h.source = "jev";
            }
        }
        if !self.inp.jev_tags.is_empty() {
            let n = s.gpus.iter().flat_map(|g| &g.procs).filter(|p| p.jev_tag).count();
            if n > 0 {
                h.source = "jev";
                h.jev_tags = n;
            }
        }
        h
    }

    fn lines(&self, width: usize) -> Vec<Line> {
        let s = model::build(&self.inp);
        let h = self.headline(&s);
        render::render(&s, &self.a, &h, width)
    }

    fn refresh_names(&mut self) {
        self.inp.names = win::process_names();
        let pids: Vec<u32> = self.inp.raw.procs.keys().map(|k| k.1).collect();
        for pid in pids {
            self.inp.paths.entry(pid).or_insert_with(|| win::process_path(pid));
        }
    }

    fn nvidia_awake(&self) -> bool {
        self.inp.adapters.iter().any(|(luid, a)| {
            a.name.contains("NVIDIA")
                && (self.inp.raw.adapter_mem.get(luid).is_some_and(|m| m.0 > 0.0)
                    || self.inp.raw.procs.iter().any(|(k, r)| k.0 == *luid && r.ded > 0.0))
        })
    }

    fn vm_busy(&self) -> bool {
        self.inp.raw.procs.iter().any(|(&(_, pid), r)| {
            self.inp.names.get(&pid).is_some_and(|n| n.eq_ignore_ascii_case("vmwp"))
                && (r.ded > 0.0 || r.eng.values().any(|v| *v > 0.0))
        })
    }

    /// One complete, progressive frame. `painter` is None when output is piped.
    fn frame(&mut self, painter: &mut Option<Painter>, watch_footer: Option<&str>) {
        let t0 = Instant::now();
        let mark = |s: &mut Self, what: &str| s.timing.push((what.to_string(), s.start.elapsed()));
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();

        // the first frame reuses the sample taken at open(); later frames take a new one
        // (whose util then covers the whole --watch interval)
        if self.frames > 0 {
            self.pdh.collect();
        }
        self.frames += 1;
        let t_sample = Instant::now();
        self.inp.raw = self.pdh.read();
        self.inp.time = win::local_time();
        self.refresh_names();
        self.inp.wsl = None;
        mark(self, "phase1 data");

        let mut pending = 0;
        // NVML / nvidia-smi wake a sleeping laptop dGPU (and take ~2 s doing it), so
        // only ask when the counters say the NVIDIA adapter is actually in use.
        if self.nvidia_awake() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Nv(nvidia::query()));
            });
            pending += 1;
        } else {
            self.inp.nv = None;
        }
        let mut wsl_started = false;
        let start_wsl = |tx: &Sender<Msg>| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Wsl(wsl_gpu_procs()));
            });
        };
        if !self.a.no_wsl && self.vm_busy() {
            start_wsl(&tx);
            wsl_started = true;
            pending += 1;
        }

        let rows = term::rows();
        let draw = |s: &Self, p: &mut Option<Painter>, final_: bool| {
            let width = term::columns(s.a.width);
            let mut lines = s.lines(width);
            if let Some(f) = watch_footer {
                let max = rows.saturating_sub(2).max(5);
                lines.truncate(max);
                let mut l = Line::new();
                l.push("2", f.to_string());
                lines.push(l);
            }
            match p {
                Some(p) => {
                    // a block taller than the screen cannot be redrawn in place
                    if final_ || watch_footer.is_some() || lines.len() < rows {
                        p.paint(&lines);
                    }
                }
                None if final_ => Painter::print_once(&lines, term::color_on()),
                None => {}
            }
        };

        if !self.pdh_util_ready() {
            draw(self, painter, false);
            mark(self, "phase1 drawn");
            let window = std::env::var("GPUTREE_WINDOW_MS").ok().and_then(|v| v.parse().ok()).map(Duration::from_millis).unwrap_or(UTIL_WINDOW);
            let wait = window.saturating_sub(t_sample.elapsed());
            std::thread::sleep(wait);
            self.pdh.collect();
            self.inp.raw = self.pdh.read();
            self.refresh_names();
        }
        draw(self, painter, false);
        mark(self, "phase2 drawn");
        if !wsl_started && !self.a.no_wsl && self.vm_busy() {
            start_wsl(&tx);
            wsl_started = true;
            pending += 1;
        }

        let jev_enabled = !self.a.no_ai && self.key.is_some();
        let mut jev_started = false;
        let mut wsl_done = !wsl_started;
        let maybe_jev = |s: &mut Self, started: &mut bool, pending: &mut usize| {
            if *started || !jev_enabled {
                return;
            }
            *started = true;
            let snap = model::build(&s.inp);
            // in --watch, only re-ask when the story changes or every 2 minutes
            let sig: String = headline::candidates(&snap).iter().map(|c| c.kind).collect::<Vec<_>>().join(",")
                + &snap.gpus.iter().map(|g| model::by_tag(g, true).first().map(|t| t.tag).unwrap_or("")).collect::<String>();
            let fresh = s.jev_at.is_some_and(|t| t.elapsed() < Duration::from_secs(120)) && sig == s.jev_sig;
            let has_unknown = snap.gpus.iter().flat_map(|g| &g.procs).any(|p| {
                matches!(p.tag, "other" | "game?") && !p.jev_tag && model::is_active(p, false) && !p.name.starts_with('<')
            });
            if fresh && !has_unknown {
                return;
            }
            let left = DEADLINE.saturating_sub(t0.elapsed()).max(Duration::from_millis(600));
            let known: HashMap<String, &'static str> = s.inp.jev_tags.clone();
            if spawn_jev(&tx, s.key.clone().unwrap(), &snap, &known, left) {
                s.jev_sig = sig;
                s.jev_at = Some(Instant::now());
                *pending += 1;
            }
        };
        if wsl_done {
            maybe_jev(self, &mut jev_started, &mut pending);
        }

        while pending > 0 {
            let now = t0.elapsed();
            if now >= DEADLINE {
                break;
            }
            let mut wait = DEADLINE - now;
            if !jev_started && jev_enabled {
                wait = wait.min(JEV_LATEST_START.saturating_sub(now).max(Duration::from_millis(1)));
            }
            match rx.recv_timeout(wait) {
                Ok(Msg::Nv(v)) => {
                    self.inp.nv = Some(v);
                    mark(self, "nvml");
                }
                Ok(Msg::Wsl(v)) => {
                    self.inp.wsl = Some(v);
                    wsl_done = true;
                    mark(self, "wsl");
                }
                Ok(Msg::Jev(r, took)) => {
                    match r {
                        Ok(j) => {
                            if j.kind.is_some() {
                                self.jev_kind = j.kind;
                            }
                            self.inp.jev_tags.extend(j.tags);
                            self.timing.push((format!("jev ok ({} ms call)", took.as_millis()), self.start.elapsed()));
                        }
                        Err(e) => {
                            self.timing.push((format!("jev failed: {e}"), self.start.elapsed()));
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if !jev_started && t0.elapsed() >= JEV_LATEST_START {
                        maybe_jev(self, &mut jev_started, &mut pending);
                    }
                    continue;
                }
                Err(_) => break,
            }
            pending -= 1;
            if wsl_done {
                maybe_jev(self, &mut jev_started, &mut pending);
            }
            if pending > 0 {
                draw(self, painter, false);
            }
        }
        draw(self, painter, true);
        mark(self, "final drawn");
    }

    fn pdh_util_ready(&self) -> bool {
        self.pdh.collections() >= 2
    }
}

unsafe extern "system" fn on_ctrl(_: u32) -> windows_sys::core::BOOL {
    use std::io::Write;
    let mut o = std::io::stdout();
    let _ = o.write_all(b"\x1b[0m\x1b[?25h\n");
    let _ = o.flush();
    0 // let the default handler end the process
}

fn main() {
    let a = match args::parse() {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            print!("{}", args::HELP);
            return;
        }
        Err(e) => {
            eprintln!("gputree: {e}");
            std::process::exit(2);
        }
    };
    let t_start = Instant::now();
    let tty = term::stdout_is_tty();
    let color = tty && !a.no_color;
    term::COLOR.store(color, std::sync::atomic::Ordering::Relaxed);
    if tty {
        term::enable_vt();
    }

    let Some(pdh) = win::Counters::open() else {
        eprintln!("gputree: could not open the GPU performance counters");
        std::process::exit(1);
    };
    let t_pdh = t_start.elapsed();
    let inp = Inputs { adapters: win::adapters(), npu: win::npu_name(), ..Default::default() };
    let mut sess = Session {
        key: jev::api_key(),
        a: a.clone(),
        pdh,
        inp,
        jev_kind: None,
        jev_sig: String::new(),
        jev_at: None,
        timing: vec![("counters open + first sample".into(), t_pdh), ("registry".into(), t_start.elapsed())],
        start: t_start,
        frames: 0,
    };

    if a.watch > 0.0 {
        let mut painter = Some(Painter::new(color, true));
        if tty {
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), 1);
            }
            print!("\x1b[?25l\x1b[H\x1b[2J");
        } else {
            painter = None;
        }
        let footer = format!("refresh {}s · Ctrl+C to quit", a.watch);
        // GPUTREE_WATCH_FRAMES=N stops after N frames (tests; avoids having to signal it)
        let max_frames: Option<u32> = std::env::var("GPUTREE_WATCH_FRAMES").ok().and_then(|v| v.parse().ok());
        let mut n = 0;
        loop {
            if max_frames.is_some_and(|m| n >= m) {
                print!("[?25h");
                return;
            }
            n += 1;
            let t = Instant::now();
            sess.frame(&mut painter, Some(&footer));
            if !tty {
                println!();
            }
            std::thread::sleep(Duration::from_secs_f64(a.watch).saturating_sub(t.elapsed()));
        }
    }

    let mut painter = tty.then(|| Painter::new(color, false));
    sess.frame(&mut painter, None);
    if a.timing {
        eprintln!("terminal {:?}, tty {tty}", crossterm::terminal::size().ok());
        for (what, t) in &sess.timing {
            eprintln!("{:>7.1} ms  {what}", t.as_secs_f64() * 1000.0);
        }
        eprintln!("{:>7.1} ms  total", t_start.elapsed().as_secs_f64() * 1000.0);
    }
}
