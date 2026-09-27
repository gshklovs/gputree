//! A disktree-style treemap window over any two-metric tree: the shared GUI of
//! gputree-gui and cputree-gui.
//!
//! Much of the look is adapted from disktree (https://github.com/tobi/disktree),
//! Copyright (c) 2026 Tobi Lütke, MIT License: the squarified layout with
//! header bands, the tag-hued muted palette, the mosaic painting and label
//! placement, the top bar / trail / side panel / key bar structure, and the
//! keyboard model. Each adapted file says so at its top. disktree's mark,
//! review and removal flow is not here: these windows are read-only.

pub mod app;
pub mod appearance;
pub mod layout;
pub mod model;
pub mod palette;
pub mod ui;

pub use app::Config;
pub use model::{Frame, Metric, VNode};

use std::cell::RefCell;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use gpui_kit::{AppContext as _, WindowOptions, px, size};

/// Open the window at once and hand `start` a sender for frames; `start`
/// should spawn the collector thread and return immediately.
pub fn run(cfg: Config, start: impl FnOnce(Sender<Frame>)) {
    let started = Instant::now();
    let (tx, rx) = channel();
    start(tx);
    let name = cfg.app_name;
    let state = RefCell::new(Some((cfg, rx)));
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_omarchy::init(cx);
        appearance::apply(cx.window_appearance(), cx);
        let Some((cfg, rx)) = state.borrow_mut().take() else { return };
        let slot = RefCell::new(Some((cfg, rx)));
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::new(
                        gpui_kit::point(px(80.), px(60.)),
                        size(px(1400.), px(860.)),
                    ))),
                    titlebar: Some(gpui_kit::TitlebarOptions { title: Some(name.into()), ..Default::default() }),
                    app_id: Some(name.to_owned()),
                    window_min_size: Some(size(px(760.), px(480.))),
                    ..Default::default()
                },
                move |window, cx| {
                    appearance::follow(window);
                    let (cfg, rx) = slot.borrow_mut().take().expect("the window is built once");
                    cx.new(|cx| app::TreeApp::new(cfg, rx, started, cx))
                },
            )
            .expect("open the window");
        let _ = window.update(cx, |this, window, cx| {
            let focus = this.focus.clone();
            window.focus(&focus, cx);
        });
        cx.activate(true);
    });
}

/// Borrow the console of the terminal the window was started from, so
/// `--help` and errors reach it (a windowed program gets none of its own).
pub mod console {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole};
    pub fn attach() {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    pub fn detach() {
        unsafe {
            FreeConsole();
        }
    }
}
