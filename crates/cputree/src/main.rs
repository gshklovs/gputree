//! cputree - a disktree-style tree of what is using your CPU: the real process tree,
//! with CPU and memory rolled up to parents, and the Linux processes inside WSL.
//! Read-only: it samples system information and reads /proc, and never touches any
//! process.

mod args;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use treecore::cpu::model::{self, Inputs, LinuxProc, Snap};
use treecore::cpu::{collect, headline, render};
use treecore::jev::{self, JevResult};
use treecore::layout::Headline;
use treecore::term::{self, Line, Painter};

/// Second sample this long after the first (CPU % is a rate).
const CPU_WINDOW: Duration = Duration::from_millis(300);
/// Everything async (WSL, Jev) must land within this, measured from frame start.
const DEADLINE: Duration = Duration::from_millis(2200);
/// Start Jev without WSL names if WSL has not answered by then.
const JEV_LATEST_START: Duration = Duration::from_millis(1100);

enum Msg {
    Wsl(Vec<LinuxProc>),
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

struct Session {
    a: args::Args,
    opts: treecore::cpu::Opts,
    key: Option<String>,
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
            let n = s.procs.iter().filter(|p| p.jev_tag).count();
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
        render::render(&s, &self.opts, &h, width)
    }

    fn has_vm(&self) -> bool {
        self.inp.cur.as_ref().is_some_and(|c| c.procs.iter().any(|p| model::is_vm(&p.name)))
    }

    fn frame(&mut self, painter: &mut Option<Painter>, watch_footer: Option<&str>) {
        let t0 = Instant::now();
        let mark = |s: &mut Self, what: &str| s.timing.push((what.to_string(), s.start.elapsed()));
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();

        // the first frame reuses the sample taken at startup; later frames take a new
        // one, whose CPU % then covers the whole --watch interval
        if self.frames > 0 {
            self.inp.sample();
        }
        self.frames += 1;
        self.inp.fill_paths();
        self.inp.wsl = None;
        mark(self, "phase1 data");

        let mut pending = 0usize;
        let mut wsl_started = false;
        if !self.a.no_wsl && self.has_vm() {
            let tx = tx.clone();
            let n = treecore::cpu::sys::ncpu();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Wsl(collect::wsl_cpu_procs(n)));
            });
            wsl_started = true;
            pending += 1;
        }

        let rows = term::rows();
        let draw = |s: &Self, p: &mut Option<Painter>, final_: bool| {
            let width = term::columns(s.a.width);
            let mut lines = s.lines(width);
            if let Some(f) = watch_footer {
                lines.truncate(rows.saturating_sub(2).max(5));
                let mut l = Line::new();
                l.push("2", f.to_string());
                lines.push(l);
            }
            match p {
                Some(p) => {
                    if final_ || watch_footer.is_some() || lines.len() < rows {
                        p.paint(&lines);
                    }
                }
                None if final_ => Painter::print_once(&lines, term::color_on()),
                None => {}
            }
        };

        if self.inp.prev.is_none() {
            draw(self, painter, false);
            mark(self, "phase1 drawn");
            let since = self.inp.cur.as_ref().map(|c| c.at.elapsed()).unwrap_or_default();
            std::thread::sleep(CPU_WINDOW.saturating_sub(since));
            self.inp.sample();
            self.inp.fill_paths();
        }
        draw(self, painter, false);
        mark(self, "phase2 drawn");

        let jev_enabled = !self.a.no_ai && self.key.is_some();
        let mut jev_started = false;
        let mut wsl_done = !wsl_started;
        let maybe_jev = |s: &mut Self, started: &mut bool, pending: &mut usize| {
            if *started || !jev_enabled {
                return;
            }
            *started = true;
            let snap = model::build(&s.inp);
            let sig: String = headline::candidates(&snap).iter().map(|c| c.kind).collect::<Vec<_>>().join(",")
                + model::by_tag(&snap, false).first().map(|t| t.0).unwrap_or("");
            let fresh = s.jev_at.is_some_and(|t| t.elapsed() < Duration::from_secs(120)) && sig == s.jev_sig;
            if fresh {
                return;
            }
            let left = DEADLINE.saturating_sub(t0.elapsed()).max(Duration::from_millis(600));
            let known = s.inp.jev_tags.clone();
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
                Ok(Msg::Wsl(v)) => {
                    self.inp.wsl = Some(v);
                    wsl_done = true;
                    mark(self, "wsl");
                }
                Ok(Msg::Jev(r, took)) => match r {
                    Ok(j) => {
                        if j.kind.is_some() {
                            self.jev_kind = j.kind;
                        }
                        self.inp.jev_tags.extend(j.tags);
                        self.timing.push((format!("jev ok ({} ms call)", took.as_millis()), self.start.elapsed()));
                    }
                    Err(e) => self.timing.push((format!("jev failed: {e}"), self.start.elapsed())),
                },
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
            eprintln!("cputree: {e}");
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

    let mut inp = Inputs::default();
    inp.sample();
    let t_sample = t_start.elapsed();
    let mut sess = Session {
        key: jev::api_key(),
        opts: treecore::cpu::Opts { metric_mem: a.metric_mem, depth: a.depth, top: a.top, group: a.group, all: a.all },
        a: a.clone(),
        inp,
        jev_kind: None,
        jev_sig: String::new(),
        jev_at: None,
        timing: vec![("first sample".into(), t_sample)],
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
        // CPUTREE_WATCH_FRAMES=N stops after N frames (tests; avoids having to signal it)
        let max_frames: Option<u32> = std::env::var("CPUTREE_WATCH_FRAMES").ok().and_then(|v| v.parse().ok());
        let mut n = 0;
        loop {
            if max_frames.is_some_and(|m| n >= m) {
                print!("\x1b[?25h");
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
