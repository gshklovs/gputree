//! NVIDIA temperature / power / utilisation. NVML first (nvml.dll in System32 on
//! Windows, libnvidia-ml.so.1 on Linux, loaded at run time so the tools start without
//! it), `nvidia-smi` as a fallback. Both are read-only queries.

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct NvStats {
    pub name: String,
    pub util: f64,
    pub temp: Option<u32>,
    pub power_w: Option<f64>,
    /// framebuffer (used, total) in bytes, as nvidia-smi reports it
    pub mem: Option<(f64, f64)>,
}

const MIB: f64 = 1024.0 * 1024.0;

static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

/// The process-wide NVML handle, initialised on first use. None without the driver.
pub fn nvml() -> Option<&'static Nvml> {
    NVML.get_or_init(|| Nvml::init().ok()).as_ref()
}

fn via_nvml() -> Option<Vec<NvStats>> {
    let nvml = nvml()?;
    let n = nvml.device_count().ok()?;
    let mut out = vec![];
    for i in 0..n {
        let Ok(d) = nvml.device_by_index(i) else { continue };
        let Ok(name) = d.name() else { continue };
        out.push(NvStats {
            name,
            util: d.utilization_rates().map(|u| u.gpu as f64).unwrap_or(0.0),
            temp: d.temperature(TemperatureSensor::Gpu).ok(),
            power_w: d.power_usage().ok().map(|mw| mw as f64 / 1000.0),
            mem: d.memory_info().ok().map(|m| (m.used as f64, m.total as f64)),
        });
    }
    (!out.is_empty()).then_some(out)
}

/// Runs `nvidia-smi` with `args` (no console window on Windows) and returns stdout.
pub fn smi(args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    crate::no_window(&mut cmd);
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn via_smi() -> Option<Vec<NvStats>> {
    let text = smi(&["--query-gpu=name,utilization.gpu,temperature.gpu,power.draw,memory.used,memory.total", "--format=csv,noheader,nounits"])?;
    let v: Vec<NvStats> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            (f.len() >= 4).then(|| NvStats {
                name: f[0].to_string(),
                util: f[1].parse().unwrap_or(0.0),
                temp: f[2].parse().ok(),
                power_w: f[3].parse().ok(),
                mem: match (f.get(4).and_then(|v| v.parse::<f64>().ok()), f.get(5).and_then(|v| v.parse::<f64>().ok())) {
                    (Some(u), Some(t)) => Some((u * MIB, t * MIB)),
                    _ => None,
                },
            })
        })
        .collect();
    (!v.is_empty()).then_some(v)
}

pub fn query() -> Vec<NvStats> {
    via_nvml().or_else(via_smi).unwrap_or_default()
}
