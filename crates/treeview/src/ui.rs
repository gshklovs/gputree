//! Spacing and type tokens, in rem.
//!
//! From disktree's `disktree-app/src/ui.rs` (https://github.com/tobi/disktree),
//! Copyright (c) 2026 Tobi Lütke, MIT License; the subset this window uses.

use gpui_kit::Rems;

pub mod space {
    use super::Rems;
    pub const XXS: Rems = Rems(0.125);
    pub const XS: Rems = Rems(0.25);
    pub const SM: Rems = Rems(0.5);
    pub const MD: Rems = Rems(0.75);
    pub const LG: Rems = Rems(1.0);
    pub const XL: Rems = Rems(1.5);
}

pub mod text {
    use super::Rems;
    pub const CAPTION: Rems = Rems(0.6875);
    pub const BODY: Rems = Rems(0.75);
    pub const TITLE: Rems = Rems(0.875);
    pub const HEADING: Rems = Rems(1.125);
    pub const DISPLAY: Rems = Rems(2.5);
}

pub mod size {
    use super::Rems;
    pub const SWATCH: Rems = Rems(0.625);
    pub const METER: Rems = Rems(0.3125);
    pub const TOOLTIP: Rems = Rems(18.0);
    pub const HELP: Rems = Rems(32.5);
    pub const KEY_LANE: Rems = Rems(7.0);
    pub const CHOICE: Rems = Rems(10.0);
}

/// The side panel's width, in rem.
pub const PANEL_REMS: f32 = 23.0;
/// Below this window width (rem) the panel gives the mosaic its room.
pub const PANEL_SHOWN_REMS: f32 = 52.0;
