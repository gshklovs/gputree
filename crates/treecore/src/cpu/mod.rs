//! cputree's data: the Windows process tree with CPU and memory rolled up to parents,
//! per-core load, and the busiest Linux processes inside WSL.

pub mod collect;
pub mod headline;
pub mod model;
pub mod render;
pub mod sys;

/// Layout options for the cputree screen.
#[derive(Clone, Debug)]
pub struct Opts {
    pub metric_mem: bool,
    /// tree levels shown (1 = top level only)
    pub depth: u8,
    pub top: usize,
    pub group: bool,
    pub all: bool,
}
