//! Command-line flags. Hand-rolled to keep the binary small and startup instant.
//! Accepts GNU style (`--metric util`, `--metric=util`, `-d 2`) and the old
//! PowerShell spelling (`-Metric util`, `-NoWsl`) so muscle memory keeps working.

pub const HELP: &str = "\
gputree - a disktree-style tree of what is using your GPUs. Read-only: never touches processes.

usage: gputree [--metric vram|util] [--depth 1-3] [--watch SECONDS] [--top N]
               [--group] [--all] [--no-wsl] [--no-ai] [--width N] [--no-color]

  GPU adapter -> Windows process -> engines (3d / copy / compute / video...)
                                 -> for the WSL2 VM (vmwp): the Linux processes holding /dev/dxg
  Largest first. Process bars are relative to the adapter (memory) or 100% (util).

  -m, --metric   rank by vram (default) or util
  -d, --depth    levels under each adapter (1-3, default 3)
  -w, --watch    redraw every N seconds until Ctrl+C
  -n, --top      processes shown per adapter (default 10; --all lifts it)
  -g, --group    insert a tag layer: adapter -> tag (training, game, ...) -> process
  -a, --all      list every process with a GPU handle, including idle ones
      --no-wsl   skip looking inside WSL distros
      --no-ai    skip the Jev headline / re-tagging call (JEV_API_KEY)
      --width    override the terminal width (also GPUTREE_WIDTH)
      --no-color plain output (also NO_COLOR; automatic when piped)
  -h, --help     this text
";

#[derive(Clone, Debug)]
pub struct Args {
    pub metric_util: bool,
    pub depth: u8,
    pub watch: f64,
    pub top: usize,
    pub group: bool,
    pub all: bool,
    pub no_wsl: bool,
    pub no_ai: bool,
    pub width: Option<usize>,
    pub no_color: bool,
    pub timing: bool,
}

pub fn parse() -> Result<Args, String> {
    let mut a = Args {
        metric_util: false,
        depth: 3,
        watch: 0.0,
        top: 10,
        group: false,
        all: false,
        no_wsl: false,
        no_ai: false,
        width: None,
        no_color: false,
        timing: false,
    };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        let arg = &raw[i];
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with('-') => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let key = flag.trim_start_matches('-').to_ascii_lowercase().replace('-', "");
        let mut value = |name: &str| -> Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            i += 1;
            raw.get(i).cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match key.as_str() {
            "h" | "help" | "?" => return Err(String::new()),
            "m" | "metric" => {
                let v = value("--metric")?.to_ascii_lowercase();
                a.metric_util = match v.as_str() {
                    "util" => true,
                    "vram" | "mem" => false,
                    _ => return Err(format!("--metric must be vram or util, not '{v}'")),
                }
            }
            "d" | "depth" => {
                let v = value("--depth")?;
                a.depth = match v.parse::<u8>() {
                    Ok(n @ 1..=3) => n,
                    _ => return Err(format!("--depth must be 1-3, not '{v}'")),
                }
            }
            "w" | "watch" => {
                let v = value("--watch")?;
                a.watch = v.parse::<f64>().map_err(|_| format!("--watch needs seconds, not '{v}'"))?;
                if !(a.watch >= 0.0) {
                    return Err("--watch must be >= 0".into());
                }
            }
            "n" | "top" => {
                let v = value("--top")?;
                a.top = v.parse().map_err(|_| format!("--top needs a number, not '{v}'"))?;
            }
            "width" => {
                let v = value("--width")?;
                a.width = Some(v.parse().map_err(|_| format!("--width needs a number, not '{v}'"))?);
            }
            "g" | "group" => a.group = true,
            "a" | "all" => a.all = true,
            "nowsl" => a.no_wsl = true,
            "noai" => a.no_ai = true,
            "nocolor" => a.no_color = true,
            "timing" => a.timing = true,
            _ => return Err(format!("unknown flag '{arg}' (try --help)")),
        }
        i += 1;
    }
    if a.width.is_none() {
        if let Some(w) = std::env::var("GPUTREE_WIDTH").ok().and_then(|v| v.parse().ok()) {
            a.width = Some(w);
        }
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        a.no_color = true;
    }
    Ok(a)
}
