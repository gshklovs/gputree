//! NVIDIA temperature / power / utilisation. NVML (nvml.dll in System32) first,
//! `nvidia-smi` as a fallback. Both are read-only queries.

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct NvStats {
    pub name: String,
    pub util: f64,
    pub temp: Option<u32>,
    pub power_w: Option<f64>,
}

static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

fn via_nvml() -> Option<Vec<NvStats>> {
    let nvml = NVML.get_or_init(|| Nvml::init().ok()).as_ref()?;
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
        });
    }
    (!out.is_empty()).then_some(out)
}

fn via_smi() -> Option<Vec<NvStats>> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name,utilization.gpu,temperature.gpu,power.draw", "--format=csv,noheader,nounits"])
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v: Vec<NvStats> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            (f.len() >= 4).then(|| NvStats {
                name: f[0].to_string(),
                util: f[1].parse().unwrap_or(0.0),
                temp: f[2].parse().ok(),
                power_w: f[3].parse().ok(),
            })
        })
        .collect();
    (!v.is_empty()).then_some(v)
}

pub fn query() -> Vec<NvStats> {
    via_nvml().or_else(via_smi).unwrap_or_default()
}
