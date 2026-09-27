//! Light or dark, following Windows' setting.
//!
//! From disktree's `disktree-app/src/appearance.rs`
//! (https://github.com/tobi/disktree), Copyright (c) 2026 Tobi Lütke, MIT
//! License: gpui-omarchy stays on its dark default without an Omarchy theme,
//! so the app applies the crate's own dark and light palettes itself.

use gpui_kit::{App, Window, WindowAppearance};
use gpui_omarchy::Theme;

pub fn apply(appearance: WindowAppearance, cx: &mut App) {
    let theme = match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::flexoki_light(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::tokyo_night(),
    };
    theme.apply(cx);
}

pub fn follow(window: &Window) {
    window.observe_window_appearance(|window, cx| apply(window.appearance(), cx)).detach();
}
