//! Width-safe styled lines, smooth bars, and in-place redraw.

use std::io::{IsTerminal, Write};
use unicode_width::UnicodeWidthStr;

pub fn width_of(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate to `max` display columns, ending in `…` if anything was cut.
pub fn clip(s: &str, max: usize) -> String {
    if width_of(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

pub fn pad_left(s: &str, w: usize) -> String {
    let s = clip(s, w);
    let n = w.saturating_sub(width_of(&s));
    format!("{}{s}", " ".repeat(n))
}

/// A run of text with one SGR style ("" = plain).
#[derive(Clone, Debug)]
pub struct Seg {
    pub sgr: String,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub segs: Vec<Seg>,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, sgr: &str, text: impl Into<String>) -> &mut Self {
        let text = text.into();
        if !text.is_empty() {
            self.segs.push(Seg { sgr: sgr.to_string(), text });
        }
        self
    }
    pub fn plain(&mut self, text: impl Into<String>) -> &mut Self {
        self.push("", text)
    }
    pub fn append(&mut self, other: Line) -> &mut Self {
        self.segs.extend(other.segs);
        self
    }
    pub fn width(&self) -> usize {
        self.segs.iter().map(|s| width_of(&s.text)).sum()
    }
    /// Pad with spaces up to `w` columns (no-op if already wider).
    pub fn pad_to(&mut self, w: usize) -> &mut Self {
        let cur = self.width();
        if cur < w {
            self.plain(" ".repeat(w - cur));
        }
        self
    }
    /// Clip to `max` columns; the last visible column becomes `…` if anything was cut.
    pub fn truncate(&mut self, max: usize) {
        if self.width() <= max {
            return;
        }
        let budget = max.saturating_sub(1); // one column for the ellipsis
        let mut out: Vec<Seg> = vec![];
        let mut used = 0;
        'outer: for s in self.segs.drain(..) {
            let mut text = String::new();
            for ch in s.text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + cw > budget {
                    if !text.is_empty() {
                        out.push(Seg { sgr: s.sgr.clone(), text });
                    }
                    break 'outer;
                }
                used += cw;
                text.push(ch);
            }
            out.push(Seg { sgr: s.sgr, text });
        }
        if max > 0 {
            out.push(Seg { sgr: "2".into(), text: "…".into() });
        }
        self.segs = out;
    }
    pub fn render(&self, color: bool) -> String {
        let mut o = String::new();
        for s in &self.segs {
            if color && !s.sgr.is_empty() {
                o.push_str("\x1b[");
                o.push_str(&s.sgr);
                o.push('m');
                o.push_str(&s.text);
                o.push_str("\x1b[0m");
            } else {
                o.push_str(&s.text);
            }
        }
        o
    }
}

pub const BAR_BG: &str = "48;5;236";

/// Whether styled output is on (set once at startup). Bars draw their empty part as
/// `·` when it is off, since there is no background colour to show the track.
pub static COLOR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn color_on() -> bool {
    COLOR.load(std::sync::atomic::Ordering::Relaxed)
}

/// Smooth eighth-block bar of exactly `w` columns on a subtle dim background.
pub fn bar(frac: f64, w: usize) -> Line {
    let color = color_on();
    const PART: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let f = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    let mut units = (f * (w * 8) as f64).round() as usize;
    if units == 0 && f > 0.004 {
        units = 1;
    }
    let full = units / 8;
    let rem = units % 8;
    let mut s = "█".repeat(full);
    if full < w {
        if rem > 0 {
            s.push(PART[rem]);
        }
        let used = full + usize::from(rem > 0);
        s.push_str(&(if color { " " } else { "·" }).repeat(w - used));
    }
    let fg = if f >= 0.66 {
        "38;5;167"
    } else if f >= 0.33 {
        "38;5;179"
    } else {
        "38;5;108"
    };
    let mut l = Line::new();
    l.push(&format!("{fg};{BAR_BG}"), s);
    l
}

pub fn fmt_bytes(b: f64) -> String {
    const K: f64 = 1024.0;
    if b >= K * K * K {
        format!("{:.1} GiB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.0} MiB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KiB", b / K)
    } else {
        format!("{b:.0} B")
    }
}

/// Usable columns: explicit override, else the console width, else 100. One column is
/// kept free so the cursor never auto-wraps.
pub fn columns(over: Option<usize>) -> usize {
    if let Some(w) = over {
        return w.max(20);
    }
    if let Some(w) = std::env::var("COLUMNS").ok().and_then(|v| v.parse::<usize>().ok()) {
        return w.saturating_sub(1).max(20);
    }
    match crossterm::terminal::size() {
        Ok((w, _)) if w > 0 => (w as usize).saturating_sub(1).max(20),
        _ => 100,
    }
}

pub fn rows() -> usize {
    if let Some(h) = std::env::var("LINES").ok().and_then(|v| v.parse::<usize>().ok()) {
        return h.max(5);
    }
    crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(50)
}

pub fn stdout_is_tty() -> bool {
    // GPUTREE_FORCE_TTY=1 exercises the progressive redraw path through a pipe (tests)
    std::io::stdout().is_terminal() || std::env::var_os("GPUTREE_FORCE_TTY").is_some()
}

/// Turn on ANSI processing for classic conhost (Windows Terminal already has it).
pub fn enable_vt() {
    use windows_sys::Win32::System::Console::*;
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT);
        }
    }
}

/// Redraws a block of lines in place: moves up over what was drawn last time, rewrites
/// each line (clearing its tail), and clears anything left below.
pub struct Painter {
    pub color: bool,
    drawn: usize,
    /// watch mode: repaint from the top-left of the screen instead
    home: bool,
}

impl Painter {
    pub fn new(color: bool, home: bool) -> Self {
        Painter { color, drawn: 0, home }
    }
    pub fn paint(&mut self, lines: &[Line]) {
        let mut o = String::new();
        if self.home {
            o.push_str("\x1b[H");
        } else if self.drawn > 0 {
            o.push_str(&format!("\x1b[{}F", self.drawn));
        }
        for l in lines {
            o.push_str(&l.render(self.color));
            if self.color {
                o.push_str("\x1b[K");
            }
            o.push('\n');
        }
        if self.color {
            o.push_str("\x1b[J");
        }
        self.drawn = lines.len();
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(o.as_bytes());
        let _ = out.flush();
    }
    /// Plain one-shot print (piped output).
    pub fn print_once(lines: &[Line], color: bool) {
        let mut o = String::new();
        for l in lines {
            o.push_str(&l.render(color));
            o.push('\n');
        }
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(o.as_bytes());
        let _ = out.flush();
    }
}
