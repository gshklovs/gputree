//! Command-line flags. Hand-rolled to keep the binary small and startup instant.
//! Accepts GNU style (`--metric mem`, `--metric=mem`, `-d 2`) and the old
//! PowerShell spelling (`-Metric mem`, `-NoWsl`) so muscle memory keeps working.

pub const HELP: &str = "cputree - a disktree-style tree of what is using your CPU. Read-only: never touches processes.

usage: cputree [--metric cpu|mem] [--depth N] [--watch SECONDS] [--top N]
               [--group] [--all] [--no-wsl] [--no-ai] [--width N] [--no-color]

  The real parent -> child process tree. Each row shows its subtree's CPU (rolled up
  like a directory's size in disktree), its own CPU, and its subtree's memory.
  Same-name siblings are merged (chrome x40). The WSL2 VM drills into the Linux
  processes inside it, with their own CPU.

  -m, --metric   rank by cpu (default) or mem
  -d, --depth    tree levels shown (1-12, default 4)
  -w, --watch    redraw every N seconds until Ctrl+C
  -n, --top      children shown per node (default 8; --all lifts it)
  -g, --group    group by tag instead: tag (training, browser, ...) -> process
  -a, --all      no merging and no limits: every process
      --no-wsl   skip looking inside WSL distros
      --no-ai    skip the Jev headline / re-tagging call (JEV_API_KEY)
      --width    override the terminal width (also CPUTREE_WIDTH)
      --no-color plain output (also NO_COLOR; automatic when piped)
  -h, --help     this text
";

#[derive(Clone, Debug)]
pub struct Args {
    pub metric_mem: bool,
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
        metric_mem: false,
        depth: 4,
        watch: 0.0,
        top: 8,
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
                a.metric_mem = match v.as_str() {
                    "mem" | "memory" | "ram" => true,
                    "cpu" => false,
                    _ => return Err(format!("--metric must be cpu or mem, not '{v}'")),
                }
            }
            "d" | "depth" => {
                let v = value("--depth")?;
                a.depth = match v.parse::<u8>() {
                    Ok(n @ 1..=12) => n,
                    _ => return Err(format!("--depth must be 1-12, not '{v}'")),
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
        if let Some(w) = std::env::var("CPUTREE_WIDTH").ok().and_then(|v| v.parse().ok()) {
            a.width = Some(w);
        }
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        a.no_color = true;
    }
    Ok(a)
}
