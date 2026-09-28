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
    let mut l = Line::new();
    l.push(&format!("{};{BAR_BG}", level_sgr(f, false)), s);
    l
}

/// Green / amber / red by how full something is; `strong` also bolds it from 90%.
pub fn level_sgr(frac: f64, strong: bool) -> String {
    let fg = if frac >= 0.66 {
        "38;5;167"
    } else if frac >= 0.33 {
        "38;5;179"
    } else {
        "38;5;108"
    };
    if strong && frac >= 0.9 { format!("1;{fg}") } else { fg.to_string() }
}

/// A summary gauge: `bar`, bold once it is 90% full.
pub fn meter(frac: f64, w: usize) -> Line {
    let mut l = bar(frac, w);
    if frac >= 0.9 {
        for s in &mut l.segs {
            s.sgr = format!("1;{}", s.sgr);
        }
    }
    l
}

/// A gauge split into coloured parts (fraction of the whole, 256-colour fill), left to
/// right, on the dim track. Where two parts meet inside one cell, the eighth-block is
/// drawn in the left part's colour over the right part's, so small parts stay visible.
pub fn stacked(parts: &[(f64, u8)], w: usize) -> Line {
    let total: f64 = parts.iter().map(|p| p.0.max(0.0)).sum();
    if !color_on() || parts.is_empty() {
        return bar(total, w);
    }
    const PART: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let units = w * 8;
    // [start, end) in eighths for each part; anything visible gets at least one eighth
    let mut spans: Vec<(usize, usize, u8)> = vec![];
    let mut at = 0usize;
    let mut acc = 0.0;
    for &(f, c) in parts {
        if !(f > 0.0) {
            continue;
        }
        acc += f;
        let mut end = ((acc.min(1.0)) * units as f64).round() as usize;
        if end <= at && f > 0.004 {
            end = at + 1;
        }
        let end = end.min(units);
        if end > at {
            spans.push((at, end, c));
            at = end;
        }
    }
    let color_at = |u: usize| spans.iter().find(|s| s.0 <= u && u < s.1).map(|s| s.2);
    let mut l = Line::new();
    for cell in 0..w {
        let (a, b) = (cell * 8, cell * 8 + 8);
        let left = color_at(a);
        // how far the left colour runs inside this cell
        let run = (a..b).take_while(|&u| color_at(u) == left).count();
        let (ch, fg, bg) = match left {
            Some(c) if run == 8 && c == crate::tags::FILL_REST => ('▒', c, None),
            Some(c) if run == 8 => ('█', c, None),
            Some(c) => (PART[run], c, color_at(a + run)),
            None => match color_at(a + run).filter(|_| run < 8) {
                // track, then a part starting mid-cell: draw the part from the right
                Some(c) => (PART[run], 236, Some(c)),
                None => (' ', 236, None),
            },
        };
        let bg = bg.map_or(BAR_BG.to_string(), |c| format!("48;5;{c}"));
        l.push(&format!("38;5;{fg};{bg}"), ch.to_string());
    }
    // merge runs of the same style
    let mut out = Line::new();
    for s in l.segs {
        match out.segs.last_mut() {
            Some(p) if p.sgr == s.sgr => p.text.push_str(&s.text),
            _ => out.segs.push(s),
        }
    }
    out
}

/// The fill colour `bar` uses at this level (green / amber / red).
pub fn level_fill(frac: f64) -> u8 {
    if frac >= 0.66 {
        167
    } else if frac >= 0.33 {
        179
    } else {
        108
    }
}

/// A darker shade of the same hue: a secondary part next to its primary (children
/// next to a process's own share, a second process of the same tag).
pub fn shade(c: u8) -> u8 {
    match c {
        167 => 131,
        179 => 136,
        108 => 65,
        170 => 133,
        133 => 96,
        143 => 101,
        71 => 28,
        65 => 22,
        68 => 25,
        73 => 30,
        250 => 245,
        110 => 67,
        139 => 96,
        246 => 241,
        103 => 60,
        c => c,
    }
}

/// Samples a --watch trail keeps.
pub const TRAIL: usize = 12;

/// A --watch trail: the last `TRAIL` samples (percent, oldest first) as a sparkline,
/// always `TRAIL` columns (empty slots on the left while it fills, so nothing after it
/// moves). The older half is drawn faint so it reads as history fading out.
pub fn trail(hist: &[f64]) -> Line {
    const V: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let h = &hist[hist.len().saturating_sub(TRAIL)..];
    let mut l = Line::new();
    l.plain(" ".repeat(TRAIL - h.len()));
    for (i, &v) in h.iter().enumerate() {
        let k = ((v / 100.0) * 8.0).ceil().clamp(1.0, 8.0) as usize - 1;
        let age = h.len() - 1 - i; // 0 = newest
        let sgr = if !color_on() {
            String::new()
        } else if age >= TRAIL / 2 {
            format!("2;{}", level_sgr(v / 100.0, false))
        } else {
            level_sgr(v / 100.0, false)
        };
        l.push(&sgr, V[k].to_string());
    }
    // merge same-style runs
    let mut out = Line::new();
    for s in l.segs {
        match out.segs.last_mut() {
            Some(p) if p.sgr == s.sgr => p.text.push_str(&s.text),
            _ => out.segs.push(s),
        }
    }
    out
}

/// "2.9 / 17.9 GiB": used and total in the total's unit.
pub fn fmt_pair(used: f64, total: f64) -> String {
    const K: f64 = 1024.0;
    let (d, u) = if total >= K * K * K { (K * K * K, "GiB") } else { (K * K, "MiB") };
    if u == "GiB" { format!("{:.1} / {:.1} {u}", used / d, total / d) } else { format!("{:.0} / {:.0} {u}", used / d, total / d) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_is_always_the_same_width() {
        for n in [0, 1, 5, TRAIL, TRAIL + 7] {
            let h: Vec<f64> = (0..n).map(|i| (i * 9 % 101) as f64).collect();
            assert_eq!(trail(&h).width(), TRAIL, "{n} samples");
        }
    }

    #[test]
    fn stacked_parts_fill_exactly_the_bar() {
        for w in [12, 30, 48] {
            let l = stacked(&[(0.31, 167), (0.004, 71), (0.2, crate::tags::FILL_REST)], w);
            assert_eq!(l.width(), w);
        }
    }
}
