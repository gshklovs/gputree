//! Looking inside WSL2 distros (read-only: a shell script that only reads /proc).

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

/// Distros that are currently running (`wsl -l --running -q`).
pub fn running_distros() -> Vec<String> {
    let Ok(out) = Command::new("wsl.exe")
        .args(["-l", "--running", "-q"])
        .env("WSL_UTF8", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()
    else {
        return vec![];
    };
    let mut text = String::from_utf8_lossy(&out.stdout).replace('\0', "");
    // Older wsl.exe ignores WSL_UTF8 and emits UTF-16LE.
    if out.stdout.len() >= 2 && out.stdout[1] == 0 {
        let w: Vec<u16> = out.stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        text = String::from_utf16_lossy(&w);
    }
    text.lines().map(|l| l.trim().trim_start_matches('\u{feff}').to_string()).filter(|l| !l.is_empty()).collect()
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Runs a POSIX sh script as root inside `distro` (base64-wrapped so no quoting can
/// break it) and returns its stdout.
pub fn run_script(distro: &str, script: &str) -> Option<String> {
    let code = b64(script.replace('\r', "").as_bytes());
    let out = Command::new("wsl.exe")
        .args(["-d", distro, "-u", "root", "--", "sh", "-c", &format!("echo {code} | base64 -d | sh")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs `script` in every running distro in parallel; returns (distro, stdout).
pub fn run_everywhere(script: &str) -> Vec<(String, String)> {
    let handles: Vec<_> = running_distros()
        .into_iter()
        .map(|d| {
            let s = script.to_string();
            std::thread::spawn(move || {
                let out = run_script(&d, &s);
                (d, out)
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).filter_map(|(d, o)| o.map(|o| (d, o))).collect()
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn is_interp(tok: &str) -> bool {
    let b = base(tok);
    let b = b.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    matches!(b, "python" | "node" | "bun" | "deno" | "ruby" | "perl" | "bash" | "sh" | "zsh" | "java" | "uv" | "uvx")
}

/// A short, meaningful label for a Linux command line:
/// `/home/u/p/microduck_rl/.venv/bin/python /home/u/p/microduck_rl/.venv/bin/train bd1-walk-flat --x 1`
/// -> ("train bd1-walk-flat", Some("microduck_rl")).
/// Never includes absolute paths (so no usernames).
pub fn shorten(cmd: &str, cwd: &str) -> (String, Option<String>) {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    if toks.is_empty() {
        return ("?".into(), None);
    }
    let mut i = 0;
    let mut script: Option<String> = None;
    if is_interp(toks[0]) && toks.len() > 1 {
        i = 1;
        if matches!(base(toks[0]), "uv" | "uvx") && toks.get(1) == Some(&"run") {
            i = 2;
        }
        while i < toks.len() && toks[i].starts_with('-') {
            if toks[i] == "-m" && i + 1 < toks.len() {
                script = Some(toks[i + 1].to_string());
                i += 1;
                break;
            }
            if toks[i] == "-c" {
                script = Some(format!("{} -c", base(toks[0])));
                i = toks.len();
                break;
            }
            i += 1;
        }
    }
    let script = script.unwrap_or_else(|| {
        let t = toks.get(i).copied().unwrap_or(toks[0]);
        let b = base(t);
        b.strip_suffix(".py").or(b.strip_suffix(".sh")).or(b.strip_suffix(".js")).unwrap_or(b).to_string()
    });
    // positional args until the first flag, at most two; else the first flag (+value)
    let rest: Vec<&str> = toks.iter().skip(i + 1).copied().collect();
    let mut args: Vec<String> = rest.iter().take_while(|t| !t.starts_with('-')).take(2).map(|t| base(t).to_string()).collect();
    if args.is_empty() && !rest.is_empty() {
        args.push(rest[0].to_string());
        if let Some(v) = rest.get(1).filter(|v| !v.starts_with('-')) {
            args.push(base(v).to_string());
        }
    }
    let mut label = script;
    for a in args {
        label.push(' ');
        label.push_str(&a);
    }

    // project: the dir that holds a venv, else the cwd's name
    let mut project = None;
    for t in &toks {
        for marker in ["/.venv/", "/venv/", "/.env/", "/env/", "/.conda/"] {
            if let Some(pos) = t.find(marker) {
                let dir = base(&t[..pos]);
                if !dir.is_empty() {
                    project = Some(dir.to_string());
                }
            }
        }
        if project.is_some() {
            break;
        }
    }
    if project.is_none() {
        let c = cwd.trim_end_matches('/');
        let parts: Vec<&str> = c.split('/').filter(|s| !s.is_empty()).collect();
        let boring = parts.is_empty()
            || (parts.len() <= 2 && matches!(parts[0], "home" | "root"))
            || matches!(parts[0], "proc" | "sys" | "dev");
        if !boring {
            project = parts.last().map(|s| s.to_string());
        }
    }
    (label, project)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortens_venv_train() {
        let (l, p) = shorten(
            "/home/grego/ll/microduck_rl/.venv/bin/python /home/grego/ll/microduck_rl/.venv/bin/train bd1-walk-flat --num_envs 4096",
            "/home/grego/ll/microduck_rl",
        );
        assert_eq!(l, "train bd1-walk-flat");
        assert_eq!(p.as_deref(), Some("microduck_rl"));
    }
    #[test]
    fn module_and_bare() {
        assert_eq!(shorten("python3 -m vllm.entrypoints serve x", "/").0, "vllm.entrypoints serve x");
        assert_eq!(shorten("/usr/bin/Xwayland :0", "/").0, "Xwayland :0");
        assert_eq!(b64(b"hello"), "aGVsbG8=");
    }
}
