//! The window: a top bar with the headline, the trail, the mosaic, a side
//! panel for the selection, and the key bar. Strictly a viewer: nothing here
//! can end, signal or reprioritize a process.
//!
//! The screen structure, the mosaic painting and the label placement follow
//! disktree's `views.rs` and `treemap_view.rs` (https://github.com/tobi/disktree),
//! Copyright (c) 2026 Tobi Lütke, MIT License. disktree's marking, review and
//! removal flow is deliberately left out.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Bounds, ContentMask, Context, Corners, Div, Edges, ElementId, FocusHandle, Font, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    NavigationDirection, ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    Size, StatefulInteractiveElement as _, Styled, TextAlign, TextRun, Window, canvas, div, px, quad, relative,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, ChoiceItem, Theme, button, button_group, with_tooltip};

use crate::layout::{self, LayoutOptions, Rect, Tile};
use crate::model::{Frame, Metric, VNode};
use crate::palette;
use crate::ui::{PANEL_REMS, PANEL_SHOWN_REMS, size, space, text};

/// What a tool tells the window about itself.
pub struct Config {
    pub app_name: &'static str,
    pub metrics: [Metric; 2],
    /// Levels drawn at first.
    pub depth: u32,
    /// Metric shown at first (0 or 1).
    pub metric: usize,
    /// Extra help rows specific to the tool.
    pub help: Vec<(&'static str, &'static str)>,
    /// What the root tile set is called in the trail ("GPUs", "This PC").
    pub root_label: &'static str,
    pub refresh: Duration,
}

/// When the first frames arrived, for the report (`TREEVIEW_TIMING=path`).
#[derive(Default)]
struct Timing {
    start: Option<Instant>,
    first_paint: Option<Duration>,
    phases: [Option<Duration>; 3],
    written: bool,
}

const LABEL_MIN_W_REMS: f32 = 3.375;
const LABEL_MIN_H_REMS: f32 = 0.9375;
const MAX_LABELS: usize = 180;
const ANIM: Duration = Duration::from_millis(320);

#[derive(Clone, Debug)]
struct Label {
    text: String,
    size_text: String,
    rect: Rect,
    header: Option<Rect>,
    depth: u32,
}

#[derive(Clone, Debug)]
struct Deco {
    rect: Rect,
    depth: u32,
    tag: &'static str,
    tail: bool,
    hovered: bool,
    selected: bool,
}

pub struct TreeApp {
    cfg: Config,
    pub focus: FocusHandle,
    rx: Receiver<Frame>,
    frame: Frame,
    metric: usize,
    depth: u32,
    /// ids from the root to the node currently filling the mosaic
    zoom: Vec<String>,
    selected: Option<String>,
    hovered: Option<String>,
    pointer: Option<Point<Pixels>>,
    pointer_active: bool,
    show_help: bool,
    tiles: Vec<Tile>,
    origin: Rc<Cell<Point<Pixels>>>,
    measured: Rc<Cell<Size<Pixels>>>,
    anim_from: HashMap<String, Rect>,
    anim_start: Option<Instant>,
    drawn: HashMap<String, Rect>,
    rem: f32,
    timing: Timing,
    title: String,
}

fn tile_key(t: &Tile) -> String {
    t.id.clone().unwrap_or_else(|| format!("{}\u{1}tail", t.owner))
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl TreeApp {
    pub fn new(cfg: Config, rx: Receiver<Frame>, started: Instant, cx: &mut Context<'_, Self>) -> Self {
        let depth = cfg.depth;
        let metric = cfg.metric.min(1);
        let app = TreeApp {
            cfg,
            focus: cx.focus_handle(),
            rx,
            frame: Frame::default(),
            metric,
            depth,
            zoom: vec![],
            selected: None,
            hovered: None,
            pointer: None,
            pointer_active: false,
            show_help: false,
            tiles: vec![],
            origin: Rc::new(Cell::new(Point::default())),
            measured: Rc::new(Cell::new(Size::default())),
            anim_from: HashMap::new(),
            anim_start: None,
            drawn: HashMap::new(),
            rem: 16.0,
            timing: Timing { start: Some(started), ..Timing::default() },
            title: String::new(),
        };
        Self::start_polling(cx);
        app
    }

    /// Frames come from the collector thread; pick them up on the UI thread.
    fn start_polling(cx: &Context<'_, Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(25)).await;
                let ok = this
                    .update(cx, |this, cx| {
                        let mut latest = None;
                        while let Ok(f) = this.rx.try_recv() {
                            latest = Some(f);
                        }
                        if let Some(f) = latest {
                            this.apply(f);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply(&mut self, f: Frame) {
        if let (Some(start), p @ 1..=3) = (self.timing.start, f.phase) {
            for i in 0..usize::from(p) {
                self.timing.phases[i].get_or_insert(start.elapsed());
            }
        }
        self.frame = f;
        // keep the zoom and the selection when their nodes still exist
        if let Some(root) = &self.frame.root {
            if self.zoom.is_empty() {
                self.zoom = vec![root.id.clone()];
            }
            while self.zoom.len() > 1 && root.find(self.zoom.last().unwrap()).is_none() {
                self.zoom.pop();
            }
            if self.zoom.first() != Some(&root.id) {
                self.zoom = vec![root.id.clone()];
            }
            if self.selected.as_ref().is_some_and(|s| root.find(s).is_none()) {
                self.selected = None;
            }
        }
        self.animate();
        self.write_timing();
    }

    fn write_timing(&mut self) {
        if self.timing.written || self.timing.phases[2].is_none() {
            return;
        }
        let Some(path) = std::env::var_os("TREEVIEW_TIMING") else { return };
        self.timing.written = true;
        let ms = |d: Option<Duration>| d.map_or("-".into(), |d| format!("{:.0} ms", d.as_secs_f64() * 1000.0));
        let text = format!(
            "{}: first paint {}, data (phase 1) {}, rates (phase 2) {}, complete (phase 3) {}\n",
            self.cfg.app_name,
            ms(self.timing.first_paint),
            ms(self.timing.phases[0]),
            ms(self.timing.phases[1]),
            ms(self.timing.phases[2])
        );
        let _ = std::fs::write(path, text);
    }

    /// Start moving every tile from where it is drawn now to its new place.
    fn animate(&mut self) {
        self.anim_from = self.drawn.clone();
        self.anim_start = Some(Instant::now());
    }

    fn root(&self) -> Option<&VNode> {
        let root = self.frame.root.as_ref()?;
        self.zoom.last().and_then(|id| root.find(id)).or(Some(root))
    }

    fn node(&self, id: &str) -> Option<&VNode> {
        self.frame.root.as_ref()?.find(id)
    }

    fn fmt(&self, m: usize, v: f64) -> String {
        (self.cfg.metrics[m].format)(v)
    }

    /// The tile keys act on: the pointer's if it moved last, else the selection.
    fn target(&self) -> Option<String> {
        if self.pointer_active { self.hovered.clone().or(self.selected.clone()) } else { self.selected.clone() }
    }

    // ── layout ────────────────────────────────────────────────────────────

    fn options(&self) -> LayoutOptions {
        LayoutOptions {
            max_depth: self.depth,
            header: 1.375 * self.rem,
            header_inner: 1.0 * self.rem,
            ..LayoutOptions::default()
        }
    }

    fn relayout(&mut self) {
        let size = self.measured.get();
        let area = Rect::new(0.0, 0.0, size.width.as_f32(), size.height.as_f32());
        let opts = self.options();
        self.tiles = match self.root() {
            Some(root) => layout::layout(root, area, self.metric, &opts),
            None => vec![],
        };
    }

    fn prepare(&mut self) -> (Vec<Deco>, Vec<Label>, bool) {
        self.relayout();
        let t = self.anim_start.map_or(1.0, |s| s.elapsed().as_secs_f32() / ANIM.as_secs_f32());
        let running = t < 1.0;
        let k = ease(t);
        let mut decos = Vec::with_capacity(self.tiles.len());
        let mut labels = vec![];
        let mut drawn = HashMap::with_capacity(self.tiles.len());
        let rem = self.rem;
        for tile in &self.tiles {
            let key = tile_key(tile);
            let rect = match self.anim_from.get(&key) {
                Some(from) if running => from.lerp(&tile.rect, k),
                _ => tile.rect,
            };
            drawn.insert(key, rect);
            let header = tile.header.map(|h| {
                let dy = rect.y - tile.rect.y;
                let dx = rect.x - tile.rect.x;
                Rect::new(h.x + dx, h.y + dy, rect.w, h.h)
            });
            let node = tile.id.as_deref().and_then(|id| self.node(id));
            let tag = node.map_or("other", |n| n.tag);
            decos.push(Deco {
                rect,
                depth: tile.depth,
                tag,
                tail: tile.id.is_none(),
                hovered: tile.id.is_some() && tile.id == self.hovered,
                selected: tile.id.is_some() && tile.id == self.selected,
            });
            let owned = header.unwrap_or(rect);
            if owned.w < LABEL_MIN_W_REMS * rem || owned.h < LABEL_MIN_H_REMS * rem || labels.len() >= MAX_LABELS {
                continue;
            }
            match node {
                Some(n) => labels.push(Label {
                    text: if n.sub.is_empty() { n.name.clone() } else { format!("{} {}", n.name, n.sub) },
                    size_text: self.fmt(self.metric, n.values[self.metric]),
                    rect,
                    header,
                    depth: tile.depth,
                }),
                None => labels.push(Label { text: format!("+{} more", tile.others), size_text: String::new(), rect, header: None, depth: tile.depth }),
            }
        }
        self.drawn = drawn;
        if !running {
            self.anim_start = None;
        }
        (decos, labels, running)
    }

    fn tile_at(&self, x: f32, y: f32) -> Option<String> {
        layout::hit(&self.tiles, x, y).and_then(|t| t.id.clone())
    }

    // ── navigation ────────────────────────────────────────────────────────

    fn zoom_to(&mut self, id: &str) {
        let Some(path) = self.frame.root.as_ref().and_then(|r| r.path_to(id)) else { return };
        if self.node(id).is_some_and(|n| n.children.iter().any(|c| c.values[self.metric] > 0.0)) {
            self.zoom = path;
            self.selected = None;
            self.hovered = None;
            self.animate();
        }
    }

    fn descend(&mut self) {
        let target = self.target().or_else(|| {
            // nothing selected: the largest child
            self.root()?.children.iter().max_by(|a, b| a.values[self.metric].total_cmp(&b.values[self.metric])).map(|c| c.id.clone())
        });
        if let Some(id) = target {
            // a leaf opens the node holding it
            let id = if self.node(&id).is_some_and(|n| n.children.is_empty()) {
                self.frame.root.as_ref().and_then(|r| r.path_to(&id)).and_then(|p| p.iter().rev().nth(1).cloned()).unwrap_or(id)
            } else {
                id
            };
            if self.zoom.last() != Some(&id) {
                self.zoom_to(&id);
            }
        }
    }

    fn ascend(&mut self) {
        if self.zoom.len() > 1 {
            let left = self.zoom.pop();
            self.selected = left;
            self.animate();
        }
    }

    fn move_selection(&mut self, d: Direction) {
        self.pointer_active = false;
        let Some(cur) = self.selected.clone() else {
            if let Some(first) = self.tiles.iter().find(|t| t.id.is_some()) {
                self.selected = first.id.clone();
            }
            return;
        };
        let Some(from) = self.tiles.iter().find(|t| t.id.as_deref() == Some(&cur)) else { return };
        let (from_rect, depth) = (from.rect, from.depth);
        let mut best: Option<(f32, String)> = None;
        for t in self.tiles.iter().filter(|t| t.depth == depth && t.id.is_some() && t.id.as_deref() != Some(&cur)) {
            let Some(gap) = d.gap(&from_rect, &t.rect) else { continue };
            let score = d.offset(&from_rect, &t.rect).mul_add(2.5, gap);
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, t.id.clone().unwrap()));
            }
        }
        if let Some((_, id)) = best {
            self.selected = Some(id);
        } else if matches!(d, Direction::Left | Direction::Up) {
            // step out to the parent tile when there is nothing further that way
            let parent = self.frame.root.as_ref().and_then(|r| r.path_to(&cur)).and_then(|p| p.iter().rev().nth(1).cloned());
            if parent.as_ref().is_some_and(|p| self.tiles.iter().any(|t| t.id.as_ref() == Some(p))) {
                self.selected = parent;
            }
        }
    }

    fn cycle_sibling(&mut self, step: isize) {
        self.pointer_active = false;
        let parent_id = self
            .selected
            .as_ref()
            .and_then(|s| self.frame.root.as_ref()?.path_to(s))
            .and_then(|p| p.iter().rev().nth(1).cloned())
            .or_else(|| self.zoom.last().cloned());
        let Some(parent) = parent_id.and_then(|p| self.node(&p)) else { return };
        let mut sibs: Vec<&VNode> = parent.children.iter().filter(|c| c.values[self.metric] > 0.0).collect();
        sibs.sort_by(|a, b| b.values[self.metric].total_cmp(&a.values[self.metric]));
        if sibs.is_empty() {
            return;
        }
        let n = sibs.len() as isize;
        let cur = self.selected.as_ref().and_then(|s| sibs.iter().position(|c| &c.id == s));
        let next = match cur {
            Some(i) => ((i as isize + step) % n + n) % n,
            None => 0,
        } as usize;
        self.selected = Some(sibs[next].id.clone());
    }

    fn set_metric(&mut self, m: usize) {
        if m != self.metric {
            self.metric = m;
            self.animate();
        }
    }

    fn adjust_depth(&mut self, step: i32) {
        let d = (self.depth as i32 + step).clamp(1, 6) as u32;
        if d != self.depth {
            self.depth = d;
            self.animate();
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let key = event.keystroke.key.as_str();
        let control = event.keystroke.modifiers.control;
        let shift = event.keystroke.modifiers.shift;
        if self.show_help {
            if matches!(key, "escape" | "?" | "q" | "/") {
                self.show_help = false;
            }
            cx.notify();
            return;
        }
        match key {
            "enter" => self.descend(),
            "backspace" | "u" => self.ascend(),
            "escape" => {
                if self.selected.is_some() {
                    self.selected = None;
                } else {
                    self.ascend();
                }
            }
            "right" | "l" if !control => self.move_selection(Direction::Right),
            "left" | "h" if !control => self.move_selection(Direction::Left),
            "up" | "k" if !control => self.move_selection(Direction::Up),
            "down" | "j" if !control => self.move_selection(Direction::Down),
            "tab" => self.cycle_sibling(if shift { -1 } else { 1 }),
            "[" => self.adjust_depth(-1),
            "]" => self.adjust_depth(1),
            "t" | "m" if !control => self.set_metric(1 - self.metric),
            "0" => {
                if let Some(r) = self.frame.root.as_ref().map(|r| r.id.clone()) {
                    self.zoom = vec![r];
                    self.animate();
                }
            }
            "?" => self.show_help = true,
            "q" if !control => cx.quit(),
            _ => {}
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let o = self.origin.get();
        let (x, y) = ((event.position.x - o.x).as_f32(), (event.position.y - o.y).as_f32());
        let area = self.measured.get();
        if x < 0.0 || y < 0.0 || x >= area.width.as_f32() || y >= area.height.as_f32() {
            if self.pointer.take().is_some() | self.hovered.take().is_some() {
                self.pointer_active = false;
                cx.notify();
            }
            return;
        }
        self.pointer = Some(Point::new(px(x), px(y)));
        self.pointer_active = true;
        self.hovered = self.tile_at(x, y);
        cx.notify();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let o = self.origin.get();
        let (x, y) = ((event.position.x - o.x).as_f32(), (event.position.y - o.y).as_f32());
        let id = self.tile_at(x, y);
        match event.button {
            MouseButton::Left if event.click_count >= 2 => {
                if let Some(id) = id {
                    self.selected = Some(id);
                    self.pointer_active = false;
                    self.descend();
                }
            }
            MouseButton::Left => {
                if id.is_some() && id == self.selected {
                    self.pointer_active = false;
                    self.descend();
                } else {
                    self.selected = id;
                }
            }
            MouseButton::Right | MouseButton::Navigate(NavigationDirection::Back) => self.ascend(),
            _ => {}
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<'_, Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(d) => d.y,
            ScrollDelta::Pixels(d) => d.y.as_f32() / 24.0,
        };
        if lines.abs() < 0.5 {
            return;
        }
        if lines > 0.0 {
            // toward the pointer: the first-level tile under it
            let o = self.origin.get();
            let (x, y) = ((event.position.x - o.x).as_f32(), (event.position.y - o.y).as_f32());
            if let Some(t) = self.tiles.iter().find(|t| t.depth == 0 && t.id.is_some() && t.rect.contains(x, y)) {
                let id = t.id.clone().unwrap();
                self.zoom_to(&id);
            }
        } else {
            self.ascend();
        }
        cx.notify();
    }
}

#[derive(Clone, Copy, Debug)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    // From disktree's state.rs: the nearest tile that way, not any tile in that half-plane.
    fn gap(self, from: &Rect, rect: &Rect) -> Option<f32> {
        let e = 0.5;
        let g = match self {
            Self::Right => rect.x - from.right(),
            Self::Left => from.x - rect.right(),
            Self::Down => rect.y - from.bottom(),
            Self::Up => from.y - rect.bottom(),
        };
        (g >= -e).then_some(g.max(0.0))
    }
    fn offset(self, from: &Rect, rect: &Rect) -> f32 {
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
        match self {
            Self::Left | Self::Right => (from.h.min(rect.h) - overlap(from.y, from.bottom(), rect.y, rect.bottom())).max(0.0),
            Self::Up | Self::Down => (from.w.min(rect.w) - overlap(from.x, from.right(), rect.x, rect.right())).max(0.0),
        }
    }
}

// ── painting ────────────────────────────────────────────────────────────────

struct Colors {
    label: [Hsla; 2],
    label_dim: Hsla,
    hover: Hsla,
    selected: Hsla,
    inset: Hsla,
}

fn snap(b: Bounds<Pixels>, scale: f32) -> Bounds<Pixels> {
    let r = |v: Pixels| px((v.as_f32() * scale).round() / scale);
    let (l, t) = (r(b.origin.x), r(b.origin.y));
    let (rt, bt) = (r(b.origin.x + b.size.width), r(b.origin.y + b.size.height));
    Bounds::new(Point::new(l, t), Size::new((rt - l).max(px(0.)), (bt - t).max(px(0.))))
}

fn to_window(rect: &Rect, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(Point::new(bounds.origin.x + px(rect.x), bounds.origin.y + px(rect.y)), Size::new(px(rect.w), px(rect.h)))
}

fn paint_tiles(decos: &[Deco], bounds: Bounds<Pixels>, theme: &Theme, colors: &Colors, window: &mut Window) {
    let none = Edges::all(px(0.));
    let solid = gpui_kit::BorderStyle::Solid;
    let scale = window.scale_factor();
    let mut rings: Vec<(u8, Bounds<Pixels>, f32, Hsla)> = vec![];
    for d in decos {
        if d.rect.w <= 0.5 || d.rect.h <= 0.5 {
            continue;
        }
        let b = snap(to_window(&d.rect, bounds), scale);
        let fill = if d.tail { palette::mix(palette::tag_fill(theme, "other", d.depth), colors.inset, 0.35) } else { palette::tag_fill(theme, d.tag, d.depth) };
        window.paint_quad(quad(b, Corners::default(), fill, none, colors.hover, solid));
        if d.depth == 0 && !d.tail {
            let strip = Bounds::new(b.origin, Size::new(b.size.width, px(2.0_f32.min(b.size.height.as_f32()))));
            window.paint_quad(quad(strip, Corners::default(), palette::tag_accent(theme, d.tag), none, colors.hover, solid));
        }
        if d.selected {
            rings.push((2, b, 2.0, colors.selected));
        } else if d.hovered {
            rings.push((1, b, 1.0, colors.hover));
        }
    }
    rings.sort_by_key(|r| r.0);
    for (_, b, w, c) in rings {
        window.paint_quad(quad(b, Corners::default(), gpui_kit::transparent_black(), Edges::all(px(w)), c, solid));
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_labels(labels: &[Label], bounds: Bounds<Pixels>, colors: &Colors, font: &SharedString, name_size: Pixels, size_size: Pixels, window: &mut Window, cx: &mut App) {
    let font = Font { family: font.clone(), ..Font::default() };
    let lh = name_size * 1.35;
    let ts = window.text_system().clone();
    let pad = name_size * 0.42;
    let inset = name_size * 0.25;
    let gap = name_size * 0.67;
    for label in labels {
        let owned = label.header.unwrap_or(label.rect);
        let mask = to_window(&owned, bounds);
        let origin = Point::new(mask.origin.x + pad, mask.origin.y + inset);
        let color = colors.label[usize::from(label.depth > 0)];
        let weight = if label.depth == 0 && label.header.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL };
        let run = TextRun { len: label.text.len(), font: Font { weight, ..font.clone() }, color, ..TextRun::default() };
        let line = ts.shape_line(SharedString::from(label.text.clone()), name_size, &[run], None);
        window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
            let _ = line.paint(origin, lh, TextAlign::Left, None, window, cx);
            if label.size_text.is_empty() {
                return;
            }
            let run = TextRun { len: label.size_text.len(), font: font.clone(), color: colors.label_dim, ..TextRun::default() };
            let sl = ts.shape_line(SharedString::from(label.size_text.clone()), size_size, &[run], None);
            let baseline = origin.y + ((line.ascent - line.descent) - (sl.ascent - sl.descent)) * 0.5;
            let stacked = label.header.is_none() && mask.size.height >= lh * 2.0 + inset;
            if stacked {
                let _ = sl.paint(Point::new(origin.x, origin.y + lh * 0.92), lh, TextAlign::Left, None, window, cx);
                return;
            }
            let room = mask.size.width - (origin.x - mask.origin.x) * 2.;
            let at = if label.header.is_some() && label.depth == 0 {
                Point::new(mask.origin.x + mask.size.width - sl.width() - pad, baseline)
            } else {
                if room - line.width() < size_size * 3.0 {
                    return;
                }
                Point::new(origin.x + line.width() + gap, baseline)
            };
            if at.x > origin.x + line.width() + pad {
                let _ = sl.paint(at, lh, TextAlign::Left, None, window, cx);
            }
        });
    }
}

// ── pieces ────────────────────────────────────────────────────────────────

fn chip(label: impl Into<SharedString>, color: Hsla, theme: &Theme) -> Div {
    div()
        .px(space::XS)
        .py(space::XXS)
        .border_1()
        .border_color(color.opacity(0.5))
        .bg(color.opacity(0.1))
        .text_color(color)
        .text_size(text::CAPTION)
        .font_family(theme.font.clone())
        .child(label.into())
}

fn eyebrow(label: &str, theme: &Theme) -> Div {
    div().text_size(text::CAPTION).text_color(theme.secondary.opacity(0.7)).whitespace_nowrap().child(SharedString::from(label.to_uppercase()))
}

fn figure(label: &str, value: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::XXS)
        .min_w_0()
        .child(eyebrow(label, theme))
        .child(div().text_size(text::TITLE).text_color(theme.bright).whitespace_nowrap().overflow_hidden().text_ellipsis().child(value.into()))
}

fn meter(frac: f32, color: Hsla, theme: &Theme) -> Div {
    div()
        .relative()
        .w_full()
        .h(size::METER)
        .bg(theme.foreground.opacity(0.08))
        .child(div().absolute().left_0().top_0().h_full().w(relative(frac.clamp(0.0, 1.0))).bg(color))
}

fn hint(keys: &'static str, label: &'static str, cx: &App) -> Div {
    let theme = cx.omarchy();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::XS)
        .flex_shrink_0()
        .child(gpui_omarchy::keycap(keys, cx))
        .child(div().text_size(text::CAPTION).text_color(theme.secondary).child(label))
}

fn logo(theme: &Theme, name: &'static str) -> Div {
    let tile = |tag: &str| div().size(space::SM).bg(palette::tag_accent(theme, tag));
    let col = |a: &'static str, b: &'static str| div().flex().flex_col().gap(space::XXS).child(tile(a)).child(tile(b));
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::SM)
        .flex_shrink_0()
        .child(div().flex().flex_row().gap(space::XXS).child(col("training", "browser")).child(col("game", "video render")))
        .child(div().text_size(text::HEADING).font_weight(FontWeight::BOLD).text_color(theme.bright).child(name))
}

impl TreeApp {
    fn top_bar(&self, window: &mut Window, cx: &mut Context<'_, Self>) -> Div {
        let theme = cx.omarchy().clone();
        let entity = cx.entity().downgrade();
        let focus = self.focus.clone();
        let mode = button_group(
            "metric",
            vec![ChoiceItem::new("a", self.cfg.metrics[0].name), ChoiceItem::new("b", self.cfg.metrics[1].name)],
            Some(self.metric),
            move |i, window, cx| {
                let _ = entity.update(cx, |this, cx| {
                    this.set_metric(i);
                    cx.notify();
                });
                window.focus(&focus, cx);
            },
            window,
            cx,
        )
        .w(size::CHOICE)
        .p(space::XXS);
        let depth = self.depth;
        let stepper = |id: &'static str, label: &'static str, step: i32, cx: &mut Context<'_, Self>| {
            button(id, label, ButtonVariant::Secondary, cx)
                .tab_stop(false)
                .disabled(if step < 0 { depth <= 1 } else { depth >= 6 })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.adjust_depth(step);
                    window.focus(&this.focus, cx);
                    cx.notify();
                }))
        };
        let depth_control = with_tooltip(
            div()
                .id("depth")
                .flex()
                .flex_row()
                .items_center()
                .border_1()
                .border_color(theme.control_border())
                .child(div().px(space::SM).text_size(text::BODY).text_color(theme.foreground).child(format!("Depth {depth}")))
                .child(stepper("depth-less", "\u{2212}", -1, cx))
                .child(stepper("depth-more", "+", 1, cx)),
            "Levels drawn at once \u{00b7} [ and ]",
        );
        let headline = if self.frame.headline.is_empty() { "Reading counters\u{2026}".to_string() } else { self.frame.headline.clone() };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(space::LG)
            .px(space::LG)
            .py(space::MD)
            .border_b_1()
            .border_color(theme.divider())
            .child(logo(&theme, self.cfg.app_name))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::SM)
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(text::TITLE)
                            .text_color(theme.bright)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(headline),
                    )
                    .when(self.frame.headline_source == "jev", |this| {
                        this.child(
                            with_tooltip(
                                div().id("jev").flex_shrink_0().text_size(text::CAPTION).text_color(theme.secondary.opacity(0.7)).child("\u{00b7} jev"),
                                "Wording picked by TypeSafe's Jev from local candidates",
                            ),
                        )
                    }),
            )
            .child(div().flex().flex_row().items_center().gap(space::SM).flex_shrink_0().child(mode).child(depth_control))
    }

    fn trail_row(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = cx.omarchy().clone();
        let mut trail = div().flex().flex_row().items_center().gap(space::XXS).min_w_0().overflow_hidden();
        let n = self.zoom.len();
        for (i, id) in self.zoom.iter().enumerate() {
            let label = if i == 0 { self.cfg.root_label.to_string() } else { self.node(id).map_or_else(|| id.clone(), |n| n.name.clone()) };
            if i > 0 {
                trail = trail.child(div().text_color(theme.secondary.opacity(0.5)).child("\u{203a}"));
            }
            let active = i + 1 == n;
            let target = id.clone();
            let crumb = div()
                .id(ElementId::Name(SharedString::from(format!("crumb-{i}"))))
                .px(space::XS)
                .py(space::XXS)
                .text_size(text::BODY)
                .text_color(if active { theme.bright } else { theme.secondary })
                .when(active, |this| this.font_weight(FontWeight::SEMIBOLD))
                .when(!active, |this| this.hover(|s| s.bg(theme.hover_fill())))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(pos) = this.zoom.iter().position(|z| *z == target) {
                        if pos + 1 < this.zoom.len() {
                            this.zoom.truncate(pos + 1);
                            this.animate();
                        }
                    }
                    window.focus(&this.focus, cx);
                    cx.notify();
                }));
            trail = trail.child(crumb);
        }
        let mut figures = div().flex().flex_row().items_center().gap(space::MD).flex_shrink_0().text_size(text::CAPTION);
        for (k, v) in &self.frame.summary {
            figures = figures.child(
                div().flex().flex_row().gap(space::XS).child(div().text_color(theme.secondary).child(k.clone())).child(div().text_color(theme.foreground).child(v.clone())),
            );
        }
        // legend: the tags on screen, largest first
        let mut tags: Vec<(&'static str, f64)> = vec![];
        if let Some(root) = self.root() {
            fn walk(n: &VNode, m: usize, depth: u32, max: u32, out: &mut Vec<(&'static str, f64)>) {
                for c in &n.children {
                    if depth + 1 >= max || c.children.is_empty() {
                        match out.iter_mut().find(|t| t.0 == c.tag) {
                            Some(t) => t.1 += c.values[m],
                            None => out.push((c.tag, c.values[m])),
                        }
                    } else {
                        walk(c, m, depth + 1, max, out);
                    }
                }
            }
            walk(root, self.metric, 0, self.depth, &mut tags);
        }
        tags.retain(|t| t.1 > 0.0);
        tags.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut legend = div().flex().flex_row().items_center().gap(space::MD).min_w_0().overflow_hidden();
        for (tag, _) in tags.iter().take(8) {
            legend = legend.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::XS)
                    .flex_shrink_0()
                    .child(div().flex_shrink_0().size(size::SWATCH).bg(palette::tag_accent(&theme, tag)))
                    .child(div().text_size(text::CAPTION).text_color(theme.secondary).child(*tag)),
            );
        }
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(space::LG)
            .px(space::LG)
            .py(space::SM)
            .child(trail)
            .child(figures)
            .child(div().flex_1())
            .child(legend)
    }

    fn mosaic(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> gpui_kit::Stateful<Div> {
        let theme = cx.omarchy().clone();
        let (decos, labels, running) = self.prepare();
        if running {
            window.request_animation_frame();
        }
        let colors = Colors {
            label: [palette::label_color(&theme, 0), palette::label_color(&theme, 1)],
            label_dim: palette::label_color(&theme, 1).opacity(0.55),
            hover: theme.bright.opacity(0.55),
            selected: palette::highlight(&theme),
            inset: theme.inset,
        };
        let rem = window.rem_size();
        let name_size = px(0.75 * rem.as_f32());
        let size_size = px(0.6875 * rem.as_f32());
        let font = theme.font.clone();
        let origin = Rc::clone(&self.origin);
        let measured = Rc::clone(&self.measured);
        let empty = decos.is_empty();
        let message = if self.frame.root.is_none() {
            "Reading counters\u{2026}".to_string()
        } else if empty && self.frame.phase <= 1 {
            format!("Sampling {}\u{2026} \u{00b7} t shows {} now", self.cfg.metrics[self.metric].name, self.cfg.metrics[1 - self.metric].name)
        } else if empty {
            format!("Nothing to size by {} here yet \u{00b7} t switches to {}", self.cfg.metrics[self.metric].name, self.cfg.metrics[1 - self.metric].name)
        } else {
            String::new()
        };
        let theme2 = theme.clone();
        div()
            .id("treemap")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(theme.inset)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.hovered = None;
                    this.pointer = None;
                    this.pointer_active = false;
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_mouse_move(e, cx)))
            .on_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, window, cx| {
                this.on_mouse_down(e, cx);
                window.focus(&this.focus, cx);
            }))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| this.on_scroll(e, cx)))
            .child(
                canvas(
                    move |bounds, window, _| {
                        origin.set(bounds.origin);
                        if measured.get() != bounds.size {
                            measured.set(bounds.size);
                            window.request_animation_frame();
                        }
                        bounds.size
                    },
                    move |bounds, _, window, cx| {
                        paint_tiles(&decos, bounds, &theme2, &colors, window);
                        paint_labels(&labels, bounds, &colors, &font, name_size, size_size, window, cx);
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(!message.is_empty(), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(text::BODY)
                        .text_color(theme.secondary)
                        .child(message),
                )
            })
    }

    fn side_panel(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = cx.omarchy().clone();
        let target = self.target().or_else(|| self.zoom.last().cloned());
        let panel = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .w(gpui_kit::Rems(PANEL_REMS))
            .flex_shrink_0()
            .min_h_0()
            .px(space::LG)
            .py(space::LG)
            .border_l_1()
            .border_color(theme.divider())
            .bg(theme.surface)
            .child(eyebrow("Selection", &theme));
        let Some(node) = target.as_deref().and_then(|id| self.node(id)) else {
            return panel.child(div().text_color(theme.secondary).child("Point at a tile or select one with the arrows"));
        };
        let m = self.metric;
        let parent_value = target
            .as_deref()
            .and_then(|id| self.frame.root.as_ref()?.path_to(id))
            .and_then(|p| p.iter().rev().nth(1).cloned())
            .and_then(|p| self.node(&p))
            .map_or(node.values[m], |p| p.values[m]);
        let value = self.fmt(m, node.values[m]);
        let (number, unit) = value.split_once(' ').map_or((value.clone(), String::new()), |(a, b)| (a.to_string(), b.to_string()));
        let accent = palette::tag_accent(&theme, node.tag);
        let identity = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::SM)
                    .min_w_0()
                    .child(div().flex_shrink_0().w(space::XS).h(text::HEADING).bg(accent))
                    .child(
                        div()
                            .text_size(text::HEADING)
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.bright)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(node.name.clone()),
                    ),
            )
            .when(!node.sub.is_empty(), |this| this.child(div().text_size(text::CAPTION).text_color(theme.secondary).child(node.sub.clone())));
        let measure = div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(space::SM)
                    .child(div().text_size(text::DISPLAY).line_height(text::DISPLAY).font_weight(FontWeight::BOLD).text_color(theme.bright).child(number))
                    .child(
                        div()
                            .text_size(text::TITLE)
                            .line_height(text::TITLE)
                            .pb(gpui_kit::Rems((text::DISPLAY.0 - text::TITLE.0) * 0.2))
                            .text_color(theme.secondary)
                            .child(format!("{unit} {}", self.cfg.metrics[m].name.to_lowercase())),
                    ),
            )
            .child(meter(if parent_value > 0.0 { (node.values[m] / parent_value) as f32 } else { 0.0 }, palette::highlight(&theme), &theme));
        let other = 1 - m;
        let is_root = self.frame.root.as_ref().is_some_and(|r| r.id == node.id);
        let mut grid = div().flex().flex_col().gap(space::MD).child(
            div()
                .flex()
                .flex_row()
                .child(div().flex_1().min_w_0().child(figure(self.cfg.metrics[other].name, self.fmt(other, node.values[other]), &theme)))
                .when(!is_root, |this| this.child(
                    div().flex_1().min_w_0().child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::XXS)
                            .child(eyebrow("Tag", &theme))
                            .child(div().flex().flex_row().child(chip(if node.jev_tag { format!("{} \u{00b7} jev", node.tag) } else { node.tag.to_string() }, accent, &theme))),
                    ),
                )),
        );
        let mut rows = div().flex().flex_col().gap(space::SM);
        for (k, v) in &node.details {
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XXS)
                    .min_w_0()
                    .child(eyebrow(k, &theme))
                    .child(div().text_size(text::BODY).text_color(theme.foreground).overflow_hidden().child(v.clone())),
            );
        }
        grid = grid.child(rows);
        let kids = node.children.iter().filter(|c| c.values[m] > 0.0).count();
        panel
            .child(identity)
            .child(measure)
            .child(grid)
            .when(kids > 0, |this| {
                this.child(div().text_size(text::CAPTION).text_color(theme.secondary.opacity(0.8)).child(format!("{kids} inside \u{00b7} enter zooms in")))
            })
            .child(div().flex_1())
            .child(div().text_size(text::CAPTION).text_color(theme.secondary.opacity(0.7)).child("Read-only: nothing here can end or change a process."))
    }

    fn key_bar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = cx.omarchy().clone();
        let hints: [(&'static str, &'static str); 7] =
            [("enter", "zoom in"), ("\u{232b}", "up"), ("hjkl", "move"), ("tab", "next"), ("t", "metric"), ("[ ]", "depth"), ("q", "quit")];
        let mut lane = div().flex().flex_row().items_center().gap(space::LG).flex_1().min_w_0().overflow_hidden();
        for (k, l) in hints {
            lane = lane.child(hint(k, l, cx));
        }
        let status = match self.frame.phase {
            0 => "starting\u{2026}".to_string(),
            1 => format!("{} \u{00b7} sampling\u{2026}", self.frame.time),
            _ => format!("updated {} \u{00b7} every {:.1} s", self.frame.time, self.cfg.refresh.as_secs_f32()),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(space::LG)
            .px(space::LG)
            .py(space::XS)
            .border_t_1()
            .border_color(theme.divider())
            .child(lane)
            .child(hint("?", "all keys", cx))
            .when(!self.frame.note.is_empty(), |this| {
                this.child(div().flex_shrink_0().text_size(text::CAPTION).text_color(theme.secondary.opacity(0.7)).child(self.frame.note.clone()))
            })
            .child(div().flex_shrink_0().text_size(text::CAPTION).text_color(theme.secondary.opacity(0.7)).child(status))
    }

    fn tooltip(&self, window: &Window, cx: &mut Context<'_, Self>) -> Option<Div> {
        let pointer = self.pointer?;
        let node = self.hovered.as_deref().and_then(|id| self.node(id))?;
        let theme = cx.omarchy().clone();
        let rem = window.rem_size().as_f32();
        let (w, h) = (size::TOOLTIP.0 * rem, (7.0 + node.details.len().min(6) as f32 * 1.1) * rem);
        let gap = space::MD.0 * rem;
        let o = self.origin.get();
        let win = window.bounds().size;
        let (ax, ay) = (o.x.as_f32() + pointer.x.as_f32(), o.y.as_f32() + pointer.y.as_f32());
        let x = if ax + gap + w > win.width.as_f32() { (ax - gap - w).max(4.0) } else { ax + gap };
        let y = if ay + gap + h > win.height.as_f32() { (ay - gap - h).max(4.0) } else { ay + gap };
        let m = self.metric;
        let mut card = div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(size::TOOLTIP)
            .flex()
            .flex_col()
            .gap(space::XS)
            .px(space::SM)
            .py(space::SM)
            .border_1()
            .border_color(theme.control_border())
            .bg(theme.background.opacity(0.93))
            .text_color(theme.foreground)
            .text_size(text::CAPTION)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::XS)
                    .child(div().min_w_0().font_weight(FontWeight::SEMIBOLD).text_color(theme.bright).overflow_hidden().child(format!("{} {}", node.name, node.sub)))
                    .child(chip(node.tag, palette::tag_accent(&theme, node.tag), &theme)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(space::SM)
                    .child(div().text_size(text::TITLE).font_weight(FontWeight::BOLD).text_color(theme.bright).child(self.fmt(m, node.values[m])))
                    .child(div().text_color(theme.secondary).child(format!("{} \u{00b7} {} {}", self.cfg.metrics[m].name, self.cfg.metrics[1 - m].name, self.fmt(1 - m, node.values[1 - m])))),
            );
        for (k, v) in node.details.iter().take(6) {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(space::SM)
                    .child(div().flex_shrink_0().text_color(theme.secondary).child(k.clone()))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(v.clone())),
            );
        }
        card = card.child(div().text_color(theme.secondary.opacity(0.7)).child(if node.children.is_empty() { "click selects" } else { "enter or click again zooms in" }));
        Some(card)
    }

    fn help(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = cx.omarchy().clone();
        let mut rows: Vec<(&str, &str)> = vec![
            ("click", "Select a tile; click it again to zoom in"),
            ("enter / double-click", "Zoom into the selection (or the tile under the pointer)"),
            ("\u{232b} / esc / right-click", "Go up one level (esc clears the selection first)"),
            ("\u{2190} \u{2191} \u{2193} \u{2192} / hjkl", "Move between tiles at this level"),
            ("tab / shift-tab", "Next or previous by size"),
            ("scroll", "Up zooms into the tile under the pointer, down goes back out"),
            ("[ / ]", "Draw fewer or more levels at once"),
            ("t / m", "Switch what tile sizes mean"),
            ("0", "Back to the top"),
            ("?", "This list"),
            ("q", "Quit"),
        ];
        rows.extend(self.cfg.help.iter().copied());
        let mut keys = div().flex().flex_col().gap(space::SM);
        for (k, l) in rows {
            keys = keys.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(space::MD)
                    .items_center()
                    .child(div().w(size::KEY_LANE).flex_shrink_0().text_size(text::CAPTION).text_color(theme.accent).child(k.to_string()))
                    .child(div().text_size(text::BODY).text_color(theme.foreground).child(l.to_string())),
            );
        }
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme.background.opacity(0.86)).child(
            div()
                .flex()
                .flex_col()
                .gap(space::MD)
                .w(size::HELP)
                .p(space::XL)
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface)
                .child(div().text_size(text::TITLE).font_weight(FontWeight::BOLD).text_color(theme.bright).child("Keyboard and mouse"))
                .child(keys)
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child("? or esc closes \u{00b7} read-only: there is no action that ends or changes a process"),
                ),
        )
    }
}

impl Render for TreeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.rem = window.rem_size().as_f32();
        if self.timing.first_paint.is_none() {
            self.timing.first_paint = self.timing.start.map(|s| s.elapsed());
        }
        let where_ = if self.zoom.len() > 1 { self.zoom.last().and_then(|id| self.node(id)).map(|n| n.name.clone()) } else { None };
        let title = match where_ {
            Some(w) => format!("{} \u{00b7} {w}", self.cfg.app_name),
            None => self.cfg.app_name.to_string(),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let theme = cx.omarchy().clone();
        let width_rems = window.viewport_size().width.as_f32() / self.rem;
        let panel = width_rems >= PANEL_SHOWN_REMS;
        let top = self.top_bar(window, cx);
        let trail = self.trail_row(cx);
        let mosaic = self.mosaic(window, cx);
        let side = panel.then(|| self.side_panel(cx));
        let keys = self.key_bar(cx);
        let tip = self.tooltip(window, cx);
        let help = self.show_help.then(|| self.help(cx));
        div()
            .id("treeview-root")
            .track_focus(&self.focus)
            .key_context("TreeView")
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font.clone())
            .text_size(text::BODY)
            .child(top)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(trail)
                            .child(div().flex().flex_1().min_h_0().px(space::LG).pb(space::SM).child(mosaic)),
                    )
                    .children(side),
            )
            .child(keys)
            .children(tip)
            .children(help)
    }
}
