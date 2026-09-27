//! What kind of workload is this? Name/path/cmdline rules first, then GPU-engine hints.
//! A faithful port of Get-Tag from the PowerShell version (see legacy/gputree.ps1).

use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Every tag the rules (and Jev) can produce, in display-priority order.
pub const ALL: [&str; 18] = [
    "training",
    "ai inference",
    "compute",
    "video render",
    "recording",
    "video playback",
    "game",
    "launcher",
    "3d / cad",
    "game dev",
    "browser",
    "app",
    "terminal",
    "desktop",
    "system",
    "vm",
    "audio",
    "other",
];

/// One-line meaning of each tag, used as Jev choice criteria.
pub fn describe(tag: &str) -> &'static str {
    match tag {
        "training" => "training or fine-tuning a machine-learning model (RL, deep learning)",
        "ai inference" => "running an AI model: LLM server, image generation, speech recognition",
        "compute" => "general GPU compute: Python/numeric/scientific work, CUDA or OpenCL jobs",
        "video render" => "encoding, transcoding or editing video",
        "recording" => "screen or game recording / live streaming",
        "video playback" => "watching or decoding video",
        "game" => "a video game that is being played",
        "launcher" => "a game store or game launcher",
        "3d / cad" => "3D modelling, rendering or CAD software",
        "game dev" => "a game engine editor",
        "browser" => "a web browser",
        "app" => "a regular desktop application (chat, editor, office, music, notes...)",
        "terminal" => "a terminal emulator or console host",
        "desktop" => "the Windows shell, compositor or a desktop utility",
        "system" => "a Windows system component, driver or background service",
        "vm" => "a virtual machine",
        "audio" => "audio processing",
        _ => "none of the above, or it cannot be determined",
    }
}

/// SGR colour for a tag chip.
pub fn color(tag: &str) -> &'static str {
    match tag {
        "training" => "1;31",
        "ai inference" => "1;35",
        "compute" => "35",
        "video render" => "1;33",
        "recording" | "video playback" => "33",
        "game" => "1;32",
        "game?" | "launcher" => "32",
        "3d / cad" | "game dev" => "1;34",
        "browser" => "36",
        "app" | "terminal" => "37",
        "vm" => "35",
        _ => "2", // desktop, audio, system, other
    }
}

/// Map any string (e.g. a Jev answer) back onto a static tag.
pub fn intern(s: &str) -> Option<&'static str> {
    ALL.iter().copied().find(|t| *t == s)
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("tag regex")
}

struct Rules {
    py: Regex,
    training: Regex,
    ai: Regex,
    compute: Regex,
    render: Regex,
    recording: Regex,
    cad: Regex,
    gamedev: Regex,
    gamepath: Regex,
    launcher: Regex,
    playback: Regex,
    browser: Regex,
    terminal: Regex,
    vm: Regex,
    desktop: Regex,
    app: Regex,
    winpath: Regex,
}

static R: LazyLock<Rules> = LazyLock::new(|| Rules {
    py: re(r"(^|/|\s)python[0-9.]*\s"),
    training: re(r"\btrain(ing)?\b|rsl_rl|isaac|torchrun|deepspeed|accelerate launch|lightning|\bppo\b|finetune|fine_tune"),
    ai: re(r"ollama|llama-server|llama\.cpp|vllm|comfyui|stable.?diffusion|lm ?studio|koboldcpp|whisper|text-generation|sglang"),
    compute: re(r"^(jupyter|julia|matlab)"),
    render: re(r"^(ffmpeg|handbrake|adobe premiere pro|premiere|resolve|afterfx|vegas\d*|topaz|shotcut|kdenlive|adobe media encoder|capcut|clipchamp)"),
    recording: re(r"^(obs\d*|streamlabs|nvidia share|gamebar)"),
    cad: re(r"^(blender|maya|3dsmax|cinema 4d|houdini|fusion360|freecad|solidworks|sketchup|rhino|unrealeditor|unity|substance)"),
    gamedev: re(r"^(godot|megadot)"),
    gamepath: re(r"steamapps\\common|\\epic games\\|\\xboxgames\\|\\riot games\\|battle\.net|\\ea games\\|\\ubisoft\\|\\gog games\\|\\games\\"),
    launcher: re(r"^(steam|steamwebhelper|epicgameslauncher|battle\.net|riotclientservices|eadesktop|galaxyclient|xboxpcapp)$"),
    playback: re(r"^(vlc|mpv|mpc-hc\d*|video\.ui|microsoft\.media\.player|netflix|potplayer)"),
    browser: re(r"^(chrome|msedge|firefox|brave|opera|arc|vivaldi|zen)$"),
    terminal: re(r"^(windowsterminal|conhost|wezterm.*|alacritty|openconsole)$"),
    vm: re(r"^(vmwp|vmmem.*)$"),
    desktop: re(r"^(dwm|explorer|csrss|system|shellhost|startmenuexperiencehost|searchhost|textinputhost|applicationframehost|widgetboard|widgets|lockapp|shellexperiencehost|crossdeviceresume|powertoys.*|microsoft\.cmdpal\.ui|phoneexperiencehost|systemsettings)$"),
    app: re(r"^(discord|slack|teams|ms-teams|zoom|claude|code|cursor|spotify|m365copilot|mscopilot|whatsapp|notion|obsidian|msedgewebview2|onedrive|outlook|olk)$"),
    winpath: re(r"\\windows\\|\\windowsapps\\"),
});

/// `name` is the process name without `.exe`, `path` the image path (may be empty),
/// `cmd` a command line (WSL only), `eng` per-engine-type utilisation if known.
pub fn tag(name: &str, path: &str, cmd: &str, eng: Option<&BTreeMap<String, f64>>) -> &'static str {
    let r = &*R;
    let n = name.to_lowercase();
    let p = path.to_lowercase();
    let c = cmd.to_lowercase();
    let py = r.py.is_match(&c) || n.starts_with("python");
    if r.training.is_match(&c) {
        return "training";
    }
    if r.ai.is_match(&format!("{n} {c}")) {
        return "ai inference";
    }
    if py || r.compute.is_match(&n) {
        return "compute";
    }
    if r.render.is_match(&n) {
        return "video render";
    }
    if r.recording.is_match(&n) {
        return "recording";
    }
    if r.cad.is_match(&n) {
        return "3d / cad";
    }
    if r.gamedev.is_match(&n) {
        return "game dev";
    }
    if r.gamepath.is_match(&p) {
        return "game";
    }
    if r.launcher.is_match(&n) {
        return "launcher";
    }
    if r.playback.is_match(&n) {
        return "video playback";
    }
    if r.browser.is_match(&n) {
        return "browser";
    }
    if r.terminal.is_match(&n) {
        return "terminal";
    }
    if r.vm.is_match(&n) {
        return "vm";
    }
    if n == "audiodg" {
        return "audio";
    }
    if matches!(n.as_str(), "system" | "idle" | "registry") {
        return "system";
    }
    if r.desktop.is_match(&n) {
        return "desktop";
    }
    if r.app.is_match(&n) {
        return "app";
    }
    if let Some(e) = eng {
        let g = |k: &str| e.get(k).copied().unwrap_or(0.0);
        if g("videoencode") >= 1.0 {
            return "video render";
        }
        if g("videodecode") >= 1.0 {
            return "video playback";
        }
        if g("compute") >= 1.0 {
            return "compute";
        }
        if g("3d") >= 20.0 && !p.is_empty() && !r.winpath.is_match(&p) {
            return "game?";
        }
    }
    "other"
}
