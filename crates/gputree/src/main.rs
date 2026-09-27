//! gputree - a disktree-style tree of what is using your GPUs.
//! Read-only: it samples counters and reads /proc, and never touches any process.

mod args;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use treecore::gpu::model::{self, Inputs, Snap, WslProc};
use treecore::gpu::{collect, headline, nvidia, render, sys as win};
use treecore::jev::{self, JevResult};
use treecore::layout::Headline;
use treecore::term::{self, Line, Painter};

/// Second PDH sample this long after the first (utilisation is a rate).
const UTIL_WINDOW: Duration = Duration::from_millis(280);
/// Everything async (NVML, WSL, Jev) must land within this, measured from start.
const DEADLINE: Duration = Duration::from_millis(2200);
/// Start Jev without WSL names if WSL has not answered by then.
const JEV_LATEST_START: Duration = Duration::from_millis(1100);
/// In --watch, re-ask WSL at most this often; frames in between reuse the last answer.
const WSL_REFRESH: Duration = Duration::from_secs(2);

enum Msg {
    Nv(Vec<nvidia::NvStats>),
    Wsl(Vec<WslProc>),
    Jev(Result<JevResult, String>, Duration),
}

fn spawn_jev(tx: &Sender<Msg>, key: String, s: &Snap, known: &HashMap<String, &'static str>, timeout: Duration) -> bool {
    let Some(job) = collect::jev_job(s, known) else { return false };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let t = Instant::now();
        let r = job.run(&key, timeout);
        let _ = tx.send(Msg::Jev(r, t.elapsed()));
    });
    true
}

/// State that survives between --watch frames.
struct Session {
    a: args::Args,
    opts: treecore::gpu::Opts,
    key: Option<String>,
    pdh: win::Counters,
    inp: Inputs,
    jev_kind: Option<String>,
    jev_sig: String,
    jev_at: Option<Instant>,
    wsl_at: Option<Instant>,
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
                h.emph = c.emph;
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

    /// `max_lines`: fit a terminal screen (TTY); None = everything (piped).
    fn lines(&self, width: usize, max_lines: Option<usize>) -> Vec<Line> {
        let s = model::build(&self.inp);
        let h = self.headline(&s);
        match max_lines {
            Some(m) => render::render_fit(&s, &self.opts, &h, width, m),
            None => render::render(&s, &self.opts, &h, width),
        }
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
        // later --watch frames draw over the previous frame's WSL rows instead of
        // blanking them until wsl.exe answers (that blank-then-refill was the jitter)
        if self.frames == 1 || !self.vm_busy() {
            self.inp.wsl = None;
        }
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
        let wsl_due = self.wsl_at.is_none_or(|t| t.elapsed() >= WSL_REFRESH);
        let start_wsl = |tx: &Sender<Msg>| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Wsl(collect::wsl_gpu_procs()));
            });
        };
        if !self.a.no_wsl && wsl_due && self.vm_busy() {
            start_wsl(&tx);
            wsl_started = true;
            self.wsl_at = Some(Instant::now());
            pending += 1;
        }

        let rows = term::rows();
        let draw = |s: &Self, p: &mut Option<Painter>, final_: bool| {
            let width = term::columns(s.a.width);
            // on a terminal, fold the tree to the screen so every phase can redraw in
            // place (one row is kept for the cursor, one more for the watch footer)
            let fit = p.is_some().then(|| rows.saturating_sub(if watch_footer.is_some() { 2 } else { 1 }).max(5));
            let mut lines = s.lines(width, fit);
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
        if !wsl_started && !self.a.no_wsl && wsl_due && self.vm_busy() {
            start_wsl(&tx);
            wsl_started = true;
            self.wsl_at = Some(Instant::now());
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
        opts: treecore::gpu::Opts { metric_util: a.metric_util, depth: a.depth, top: a.top, group: a.group, all: a.all },
        a: a.clone(),
        pdh,
        inp,
        jev_kind: None,
        jev_sig: String::new(),
        jev_at: None,
        wsl_at: None,
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
