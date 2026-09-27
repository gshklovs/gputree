//! Colour that means something.
//!
//! Adapted from disktree's `disktree-app/src/palette.rs`
//! (https://github.com/tobi/disktree), Copyright (c) 2026 Tobi Lütke, MIT
//! License. The same scheme: every hue at one muted level, deeper tiles lift
//! slightly, a saturated accent for the first level's strip and the legend,
//! and the theme's amber kept apart for the selection. Here the hue is the
//! workload tag (training, browser, ...) instead of disktree's data category.

use gpui_kit::base::ThemeAppearance;
use gpui_kit::{Hsla, Rgba};
use gpui_omarchy::Theme;

/// (hue, chroma) per tag, echoing the terminal chip colours.
const fn hue(tag: &str) -> (f32, f32) {
    match tag.as_bytes() {
        b"training" => (0.005, 1.0),
        b"ai inference" => (0.86, 1.0),
        b"compute" => (0.76, 0.9),
        b"video render" => (0.105, 1.0),
        b"recording" | b"video playback" => (0.13, 0.85),
        b"game" | b"game?" => (0.36, 1.0),
        b"launcher" => (0.42, 0.7),
        b"3d / cad" | b"game dev" => (0.61, 1.0),
        b"browser" => (0.53, 1.0),
        b"app" => (0.66, 0.35),
        b"terminal" => (0.58, 0.3),
        b"vm" => (0.9, 0.7),
        b"system" => (0.6, 0.1),
        b"desktop" => (0.6, 0.16),
        b"audio" => (0.6, 0.12),
        _ => (0.6, 0.06),
    }
}

const fn dark(theme: &Theme) -> bool {
    matches!(theme.appearance, ThemeAppearance::Dark)
}

pub fn tag_fill(theme: &Theme, tag: &str, depth: u32) -> Hsla {
    let (h, chroma) = hue(tag);
    let step = depth.min(4) as f32;
    let (s, l) = if dark(theme) { (0.26 * chroma, step.mul_add(0.028, 0.215)) } else { (0.30 * chroma, step.mul_add(-0.03, 0.84)) };
    mix(Hsla { h, s, l, a: 1.0 }, theme.inset, 0.12)
}

pub fn tag_accent(theme: &Theme, tag: &str) -> Hsla {
    let (h, chroma) = hue(tag);
    let (s, l) = if dark(theme) { (0.42 * chroma, 0.52) } else { (0.45 * chroma, 0.46) };
    Hsla { h, s, l, a: 1.0 }
}

/// The one strong colour: the selection.
pub const fn highlight(theme: &Theme) -> Hsla {
    theme.warning
}

pub fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| (b - a).mul_add(t, a);
    let (a, b) = (from.to_rgb(), to.to_rgb());
    Hsla::from(Rgba { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b), a: lerp(a.a, b.a) })
}

pub fn label_color(theme: &Theme, depth: u32) -> Hsla {
    let base = if dark(theme) { theme.bright } else { theme.foreground };
    if depth == 0 { base } else { base.opacity(0.88) }
}
