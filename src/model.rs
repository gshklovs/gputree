//! Turns raw counters + names into the adapter -> process model that gets rendered.

use crate::nvidia::NvStats;
use crate::tags;
use crate::win::{Adapter, Raw};
use std::collections::{BTreeMap, HashMap};

const MIB: f64 = 1024.0 * 1024.0;

#[derive(Clone, Debug)]
pub struct Proc {
    pub luid: u64,
    pub pid: u32,
    pub name: String,
    pub ded: f64,
    pub shr: f64,
    pub eng: BTreeMap<String, f64>,
    pub util: f64,
    pub tag: &'static str,
    /// tag came from Jev rather than the local rules
    pub jev_tag: bool,
}

#[derive(Clone, Debug)]
pub struct WslProc {
    pub distro: String,
    pub pid: u32,
    pub rss: f64,
    #[allow(dead_code)] // kept for detail views
    pub user: String,
    #[allow(dead_code)]
    pub cmd: String,
    /// "train bd1-walk-flat"
    pub label: String,
    /// "microduck_rl"
    pub project: Option<String>,
    pub tag: &'static str,
}

impl WslProc {
    /// "train bd1-walk-flat  (microduck_rl · Ubuntu-22.04)"
    pub fn context(&self) -> String {
        match &self.project {
            Some(p) => format!("({p} · {})", self.distro),
            None => format!("({})", self.distro),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Gpu {
    #[allow(dead_code)]
    pub luid: u64,
    pub name: String,
    pub short: String,
    pub integrated: bool,
    pub npu: bool,
    pub mem: f64,
    pub cap: f64,
    pub util: f64,
    pub eng: BTreeMap<String, f64>,
    pub procs: Vec<Proc>,
    pub nv: Option<NvStats>,
}

impl Gpu {
    pub fn pmem(&self, p: &Proc) -> f64 {
        if self.integrated { p.shr } else { p.ded }
    }
}

#[derive(Clone, Debug)]
pub struct Snap {
    pub gpus: Vec<Gpu>,
    pub util_ready: bool,
    pub wsl: Option<Vec<WslProc>>,
    pub time: String,
}

/// Everything collected so far; the snapshot is rebuilt from this on every redraw.
#[derive(Default)]
pub struct Inputs {
    pub raw: Raw,
    pub adapters: HashMap<u64, Adapter>,
    pub npu: Option<String>,
    pub names: HashMap<u32, String>,
    pub paths: HashMap<u32, Option<String>>,
    pub nv: Option<Vec<NvStats>>,
    pub wsl: Option<Vec<WslProc>>,
    /// process name (lowercase) -> tag chosen by Jev
    pub jev_tags: HashMap<String, &'static str>,
    pub time: String,
}

pub fn short_name(name: &str, integrated: bool, npu: bool) -> String {
    let mut s = name.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    for w in ["GeForce ", " Laptop GPU", " GPU", " Graphics"] {
        s = s.replace(w, if w.starts_with(' ') { "" } else { "" });
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if npu {
        return format!("{s} NPU").replace("NPU NPU", "NPU");
    }
    if integrated && (s.is_empty() || s == "Intel" || s == "AMD Radeon" || s == "AMD") {
        let vendor = if s.is_empty() { "integrated" } else { s.split(' ').next().unwrap_or("") };
        return format!("{vendor} iGPU");
    }
    if s.is_empty() { name.to_string() } else { s }
}

pub fn build(inp: &Inputs) -> Snap {
    let raw = &inp.raw;
    let mut procs: Vec<Proc> = raw
        .procs
        .iter()
        .map(|(&(luid, pid), r)| {
            let name = inp.names.get(&pid).cloned().unwrap_or_else(|| if pid == 4 { "System".into() } else { "<exited>".into() });
            let util = r.eng.values().copied().fold(0.0, f64::max);
            let path = inp.paths.get(&pid).cloned().flatten().unwrap_or_default();
            let eng = raw.util_ready.then_some(&r.eng);
            let mut tag = tags::tag(&name, &path, "", eng);
            let mut jev_tag = false;
            if matches!(tag, "other" | "game?") {
                if let Some(t) = inp.jev_tags.get(&name.to_lowercase()) {
                    tag = t;
                    jev_tag = true;
                }
            }
            Proc { luid, pid, name, ded: r.ded, shr: r.shr, eng: r.eng.clone(), util, tag, jev_tag }
        })
        .collect();

    // the VM is doing whatever its busiest Linux workload is
    if let Some(wsl) = &inp.wsl {
        let inner = wsl.iter().filter(|w| w.tag != "other").max_by(|a, b| a.rss.total_cmp(&b.rss));
        if let Some(inner) = inner {
            for p in procs.iter_mut().filter(|p| p.name.eq_ignore_ascii_case("vmwp")) {
                if p.util > 0.0 || p.ded > 0.0 {
                    p.tag = inner.tag;
                }
            }
        }
    }

    let mut luids: Vec<u64> = inp.adapters.keys().copied().chain(raw.adapter_mem.keys().copied()).chain(procs.iter().map(|p| p.luid)).collect();
    luids.sort_unstable();
    luids.dedup();

    let mut gpus = vec![];
    for l in luids {
        let a = inp.adapters.get(&l);
        if a.is_some_and(|a| a.name.starts_with("Microsoft Basic Render")) {
            continue;
        }
        let mine: Vec<Proc> = procs.iter().filter(|p| p.luid == l).cloned().collect();
        let integrated = a.is_none_or(|a| a.total <= 512.0 * MIB);
        let npu = a.is_none();
        let mem = raw.adapter_mem.get(&l).map(|&(d, s)| if integrated { s } else { d }).unwrap_or(0.0);
        let cap = a.map(|a| if integrated { a.shared } else { a.total }).unwrap_or(0.0);
        let mut eng: BTreeMap<String, f64> = BTreeMap::new();
        for p in &mine {
            for (k, v) in &p.eng {
                *eng.entry(k.clone()).or_insert(0.0) += v;
            }
        }
        let mut util = eng.values().copied().fold(0.0, f64::max).min(100.0);
        let name = match (a, &inp.npu) {
            (Some(a), _) => a.name.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) => format!("compute device luid 0x{l:x}"),
        };
        let nv = a.and_then(|a| {
            inp.nv.as_ref()?.iter().find(|n| a.name.contains(n.name.trim_start_matches("NVIDIA ").trim())).cloned()
        });
        if let Some(n) = &nv {
            util = util.max(n.util);
        }
        let g = Gpu { luid: l, short: short_name(&name, integrated, npu), name, integrated, npu, mem, cap, util, eng, procs: mine, nv };
        if !g.procs.is_empty() || g.mem > 0.0 {
            gpus.push(g);
        }
    }
    Snap { gpus, util_ready: raw.util_ready, wsl: inp.wsl.clone(), time: inp.time.clone() }
}

/// Per-tag rollup for one adapter: (tag, mem, util, procs), largest first by metric.
pub struct TagGroup<'a> {
    pub tag: &'static str,
    pub mem: f64,
    pub util: f64,
    pub procs: Vec<&'a Proc>,
}

pub fn by_tag<'a>(g: &'a Gpu, metric_util: bool) -> Vec<TagGroup<'a>> {
    let mut m: Vec<TagGroup> = vec![];
    for p in &g.procs {
        match m.iter_mut().find(|t| t.tag == p.tag) {
            Some(t) => {
                t.mem += g.pmem(p);
                t.util += p.util;
                t.procs.push(p);
            }
            None => m.push(TagGroup { tag: p.tag, mem: g.pmem(p), util: p.util, procs: vec![p] }),
        }
    }
    m.sort_by(|a, b| {
        let (ka, kb) = if metric_util { ((a.util, a.mem), (b.util, b.mem)) } else { ((a.mem, a.util), (b.mem, b.util)) };
        kb.0.total_cmp(&ka.0).then(kb.1.total_cmp(&ka.1))
    });
    m
}

/// Sort key for processes (primary, secondary), descending.
pub fn sort_procs(g: &Gpu, v: &mut [&Proc], metric_util: bool) {
    v.sort_by(|a, b| {
        let k = |p: &Proc| if metric_util { (p.util, g.pmem(p)) } else { (g.pmem(p), p.util) };
        let (ka, kb) = (k(a), k(b));
        kb.0.total_cmp(&ka.0).then(kb.1.total_cmp(&ka.1)).then(a.pid.cmp(&b.pid))
    });
}

pub fn is_active(p: &Proc, all: bool) -> bool {
    all || p.util >= 0.1 || (p.ded + p.shr) >= MIB
}
