//! The aligned tree grid shared by gputree and cputree. Every row uses one grid:
//!
//! ```text
//! gutter(G)  bar(BAR) cell cell ...  name(NAME) tag(TAG)  pid
//! ```
//!
//! The gutter has one width for the whole screen; shallower rows extend their branch
//! with `─` so bars, numbers, names, chips and pids line up at every depth. Nothing
//! returned is wider than the terminal.

use crate::tags;
use crate::term::{Line, bar, level_sgr, meter, pad_left, width_of};

pub const BAR: usize = 12;
/// Widest a summary gauge gets, and the room kept after it for its detail text.
const METER_MAX: usize = 48;
const METER_REST: usize = 22;
pub const DIM: &str = "2";

/// A styled cell: (text, SGR).
pub type Cell = (String, &'static str);

pub fn cell(text: impl Into<String>, sgr: &'static str) -> Cell {
    (text.into(), sgr)
}

pub fn blank() -> Cell {
    (String::new(), "")
}

pub struct Row {
    /// ancestors' continuation marks ("│  " / "   "), 3 columns per level
    pub prefix: String,
    pub last: bool,
    pub bar: Option<f64>,
    /// right-aligned numeric columns, widths from the grid
    pub cells: Vec<Cell>,
    pub name: Line,
    /// shorter variants of `name`, tried in order when it does not fit its column
    pub alts: Vec<Line>,
    pub tag: Option<(&'static str, bool)>,
    pub pid: String,
}

impl Row {
    pub fn new(prefix: &str, name: Line) -> Row {
        Row { prefix: prefix.to_string(), last: false, bar: None, cells: vec![], name, alts: vec![], tag: None, pid: String::new() }
    }
}

pub enum Out {
    Free(Line),
    /// prose (headline, footnotes): word-wrapped to the width instead of clipped
    Wrap(Line),
    /// summary rows: the label sits in the gutter, then bar, cells and free text.
    /// With no bar, `rest` starts at the bar column (e.g. a tag rollup).
    Label { label: &'static str, bar: Option<f64>, cells: Vec<Cell>, rest: Line },
    /// a headline gauge: a wide bar, then `value` ("2632 / 8151 MiB"), a percentage
    /// and free text. Every gauge on a screen shares one bar width and number column.
    Meter { label: &'static str, frac: f64, value: String, pct: String, rest: Line },
    Row(Row),
}

/// Continuation for children of a node drawn with `prefix` / `last`.
pub fn child_prefix(prefix: &str, last: bool) -> String {
    format!("{prefix}{}", if last { "   " } else { "│  " })
}

pub fn chip(tag: &str, jev: bool) -> String {
    if jev { format!("[{tag}]*") } else { format!("[{tag}]") }
}

/// Lay rows out for `width` columns. `cols` are the widths of the numeric cells.
pub fn layout(out: Vec<Out>, width: usize, cols: &[usize]) -> Vec<Line> {
    let rows = || out.iter().filter_map(|o| if let Out::Row(r) = o { Some(r) } else { None });
    let max_depth = rows().map(|r| width_of(&r.prefix) / 3).max().unwrap_or(0);
    let gutter = (3 * (max_depth + 1)).max(6);
    let tag_w = rows().filter_map(|r| r.tag.map(|(t, j)| width_of(&chip(t, j)))).max().unwrap_or(0);
    let pid_w = rows().map(|r| width_of(&r.pid)).max().unwrap_or(0);
    // rows without chips/pids (engines, "+ N more", notes) may run into the chip column
    let name_max = rows().filter(|r| r.tag.is_some() || !r.pid.is_empty()).map(|r| r.name.width()).max().unwrap_or(0);
    let cells_w: usize = cols.iter().map(|c| c + 1).sum();
    let fixed = gutter + BAR + cells_w + 2;
    let tail = if tag_w > 0 { 1 + tag_w } else { 0 } + if pid_w > 0 { 2 + pid_w } else { 0 };
    let name_w = name_max.min(width.saturating_sub(fixed + tail)).max(10);

    let meters = || out.iter().filter_map(|o| if let Out::Meter { value, .. } = o { Some(width_of(value)) } else { None });
    let value_w = meters().max().unwrap_or(0);
    let meter_w = width.saturating_sub(gutter + 1 + value_w + 1 + 4 + 2 + METER_REST).clamp(BAR, METER_MAX);

    let push_cells = |l: &mut Line, cells: &[Cell]| {
        for (i, w) in cols.iter().enumerate() {
            let (t, s) = cells.get(i).cloned().unwrap_or_default();
            l.plain(" ");
            l.push(s, pad_left(&t, *w));
        }
        l.plain("  ");
    };

    let mut lines = vec![];
    for o in out {
        let mut l = Line::new();
        match o {
            Out::Free(x) => l = x,
            Out::Wrap(x) => {
                let mut wrapped = wrap(&x, width, 3);
                let last = wrapped.pop().unwrap_or_default();
                lines.extend(wrapped);
                l = last;
            }
            Out::Label { label, bar: b, cells, rest } => {
                l.push(DIM, format!(" {label}"));
                l.pad_to(gutter);
                match b {
                    Some(f) => {
                        l.append(bar(f, BAR));
                        push_cells(&mut l, &cells);
                        l.append(rest);
                    }
                    None if !cells.is_empty() => {
                        // a column header: cells without a bar
                        l.plain(" ".repeat(BAR));
                        push_cells(&mut l, &cells);
                        l.append(rest);
                    }
                    None => {
                        l.append(rest);
                    }
                }
            }
            Out::Meter { label, frac, value, pct, rest } => {
                l.push(DIM, format!(" {label}"));
                l.pad_to(gutter);
                l.append(meter(frac, meter_w));
                l.plain(" ");
                l.push("1", pad_left(&value, value_w));
                l.plain(" ");
                l.push(&level_sgr(frac, true), pad_left(&pct, 4));
                l.plain("  ");
                l.append(rest);
            }
            Out::Row(r) => {
                let pw = width_of(&r.prefix);
                let fill = gutter.saturating_sub(pw + 2);
                l.push(DIM, format!("{}{}{} ", r.prefix, if r.last { "└" } else { "├" }, "─".repeat(fill)));
                match r.bar {
                    Some(f) => {
                        l.append(bar(f, BAR));
                    }
                    None => {
                        l.plain(" ".repeat(BAR));
                    }
                }
                push_cells(&mut l, &r.cells);
                let has_tail = r.tag.is_some() || !r.pid.is_empty();
                let mut name = r.name;
                if has_tail && name.width() > name_w {
                    if let Some(a) = r.alts.into_iter().find(|a| a.width() <= name_w) {
                        name = a;
                    }
                    name.truncate(name_w);
                }
                let nw = name.width();
                l.append(name);
                if has_tail {
                    l.plain(" ".repeat(name_w.saturating_sub(nw)));
                }
                if tag_w > 0 && has_tail {
                    l.plain(" ");
                    match r.tag {
                        Some((t, j)) => {
                            let c = chip(t, j);
                            let cw = width_of(&c);
                            l.push(tags::color(t), c);
                            l.plain(" ".repeat(tag_w - cw));
                        }
                        None => {
                            l.plain(" ".repeat(tag_w));
                        }
                    }
                }
                if !r.pid.is_empty() {
                    l.push(DIM, format!("  {}", r.pid));
                }
            }
        }
        // trim trailing padding, then clip to the terminal
        while let Some(last) = l.segs.last_mut() {
            let t = last.text.trim_end().to_string();
            if t.is_empty() && last.sgr.is_empty() {
                l.segs.pop();
                continue;
            }
            if last.sgr.is_empty() {
                last.text = t;
            }
            break;
        }
        l.truncate(width);
        lines.push(l);
    }
    lines
}

/// Greedy word wrap that keeps each word's style; at most `max_lines` (the last is clipped).
pub fn wrap(x: &Line, width: usize, max_lines: usize) -> Vec<Line> {
    let mut words: Vec<(String, String)> = vec![]; // (sgr, word incl. leading spaces)
    for seg in &x.segs {
        let mut cur = String::new();
        for ch in seg.text.chars() {
            if ch == ' ' && !cur.trim().is_empty() {
                words.push((seg.sgr.clone(), std::mem::take(&mut cur)));
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            words.push((seg.sgr.clone(), cur));
        }
    }
    let mut out: Vec<Line> = vec![Line::new()];
    for (sgr, w) in words {
        let n = out.len();
        let cur = out.last_mut().unwrap();
        if cur.width() + width_of(&w) > width && cur.width() > 0 && n < max_lines {
            let mut nl = Line::new();
            nl.push(&sgr, w.trim_start().to_string());
            out.push(nl);
        } else {
            cur.push(&sgr, w);
        }
    }
    for l in &mut out {
        l.truncate(width);
    }
    out
}

/// Headline shown on top, with where it came from.
#[derive(Clone, Debug, Default)]
pub struct Headline {
    pub text: String,
    /// "" for local, "jev" when Jev picked it
    pub source: &'static str,
    pub jev_tags: usize,
    /// key phrases to colour, in the order they appear in `text`: (substring, SGR)
    pub emph: Vec<(String, &'static str)>,
}

impl Headline {
    pub fn line(&self) -> Line {
        let mut h = Line::new();
        if self.emph.is_empty() {
            h.push("1", self.text.clone());
        } else {
            // plain sentence, key phrases lit: the figure in bold, the workload in its tag colour
            let mut rest = self.text.as_str();
            for (needle, sgr) in &self.emph {
                if let Some(i) = rest.find(needle.as_str()).filter(|_| !needle.is_empty()) {
                    if i > 0 {
                        h.plain(rest[..i].to_string());
                    }
                    h.push(sgr, needle.clone());
                    rest = &rest[i + needle.len()..];
                }
            }
            if !rest.is_empty() {
                h.plain(rest.to_string());
            }
        }
        if self.source == "jev" {
            h.push(
                DIM,
                if self.jev_tags > 0 {
                    format!("  · jev (+{} tag{})", self.jev_tags, if self.jev_tags == 1 { "" } else { "s" })
                } else {
                    "  · jev".into()
                },
            );
        }
        h
    }
}
