//! What a data source hands the window: a tree of nodes with two metrics, plus
//! the headline and a few summary figures. Built on the collector thread,
//! drawn on the UI thread.

/// One node of the tree. `values[m]` is the size under metric `m` (e.g. VRAM /
/// util, or CPU / memory). A parent's value is its whole subtree, as in
/// disktree; the layout only uses it for the parent's own tile.
#[derive(Clone, Debug)]
pub struct VNode {
    /// Stable across refreshes (a path of names/pids), so tiles can animate and
    /// the selection can survive new data.
    pub id: String,
    pub name: String,
    /// A dim suffix: "(WSL2 VM)", "×42".
    pub sub: String,
    pub tag: &'static str,
    /// Tag chosen by Jev rather than the local rules.
    pub jev_tag: bool,
    pub values: [f64; 2],
    /// Label / value rows for the tooltip and the side panel (pid, cmd, ...).
    pub details: Vec<(String, String)>,
    pub children: Vec<VNode>,
}

impl VNode {
    pub fn new(id: impl Into<String>, name: impl Into<String>, tag: &'static str, values: [f64; 2]) -> Self {
        VNode {
            id: id.into(),
            name: name.into(),
            sub: String::new(),
            tag,
            jev_tag: false,
            values,
            details: vec![],
            children: vec![],
        }
    }

    pub fn detail(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.details.push((k.into(), v.into()));
        self
    }

    /// The node with this id, and the chain of ids leading to it.
    pub fn find(&self, id: &str) -> Option<&VNode> {
        if self.id == id {
            return Some(self);
        }
        // ids are paths: only descend into a prefix
        self.children.iter().filter(|c| id.starts_with(c.id.as_str())).find_map(|c| c.find(id))
    }

    /// Ids from the root down to `id` (inclusive), if present.
    pub fn path_to(&self, id: &str) -> Option<Vec<String>> {
        if self.id == id {
            return Some(vec![self.id.clone()]);
        }
        for c in self.children.iter().filter(|c| id.starts_with(c.id.as_str())) {
            if let Some(mut p) = c.path_to(id) {
                p.insert(0, self.id.clone());
                return Some(p);
            }
        }
        None
    }
}

/// Everything one refresh produces.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub root: Option<VNode>,
    pub headline: String,
    /// "jev" when Jev chose the wording.
    pub headline_source: &'static str,
    /// Short figures for the trail row: ("CPU", "16%"), ("RAM", "21 of 31 GiB").
    pub summary: Vec<(String, String)>,
    /// 1 = sizes only, 2 = rates filled in, 3 = slow inputs (NVML/WSL/Jev) too.
    pub phase: u8,
    pub time: String,
    /// A quiet note under the mosaic's corner, e.g. the WSL attribution caveat.
    pub note: String,
}

/// How a tool describes its two metrics.
#[derive(Clone, Copy, Debug)]
pub struct Metric {
    pub name: &'static str,
    pub format: fn(f64) -> String,
}
