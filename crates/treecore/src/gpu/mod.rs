//! gputree's data: GPU counters, adapters, per-process GPU memory/engines, NVML, and
//! the Linux processes holding /dev/dxg inside WSL.

pub mod collect;
pub mod drm;
pub mod headline;
pub mod model;
pub mod nvidia;
pub mod render;
pub mod sys;

/// Layout options for the gputree screen.
#[derive(Clone, Debug)]
pub struct Opts {
    pub metric_util: bool,
    pub depth: u8,
    pub top: usize,
    pub group: bool,
    pub all: bool,
}
