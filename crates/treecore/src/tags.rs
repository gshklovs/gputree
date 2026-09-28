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
        "desktop" if cfg!(windows) => "the Windows shell, compositor or a desktop utility",
        "desktop" => "the display server, compositor, desktop shell or a desktop utility",
        "system" if cfg!(windows) => "a Windows system component, driver or background service",
        "system" => "an operating-system component, kernel thread, daemon or background service",
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
    gamename: Regex,
    gamepath: Regex,
    gamepath_unix: Regex,
    game_unix: Regex,
    launcher: Regex,
    playback: Regex,
    browser: Regex,
    terminal: Regex,
    vm: Regex,
    desktop: Regex,
    app: Regex,
    audio: Regex,
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
    gamedev: re(r"^(godot|megadot|robloxstudiobeta)"),
    // games that do not install under a store's folder
    gamename: re(r"^(robloxplayerbeta|robloxplayerlauncher|minecraft\.windows|league of legends|valorant-win64-shipping|fortniteclient-win64-shipping)$"),
    gamepath: re(r"steamapps\\common|\\epic games\\|\\xboxgames\\|\\riot games\\|battle\.net|\\ea games\\|\\ubisoft\\|\\gog games\\|\\games\\"),
    // Linux: Steam / Heroic / Lutris libraries (checked against the path and command line)
    gamepath_unix: re(r"/steamapps/common/|/heroic/|/lutris/|/games/"),
    // Linux: Proton / Wine and the Steam runtime around a running game
    game_unix: re(r"^(proton|wine|wine64|wine-preloader|wine64-preloader|wineserver|gamescope|pressure-vessel-.*|steam-runtime-launcher-service)$"),
    launcher: re(r"^(steam|steamwebhelper|epicgameslauncher|battle\.net|riotclientservices|eadesktop|galaxyclient|xboxpcapp|lutris|heroic|bottles)$"),
    playback: re(r"^(vlc|mpv|mpc-hc\d*|video\.ui|microsoft\.media\.player|netflix|potplayer|totem|celluloid|smplayer|haruna)"),
    browser: re(r"^(chrome|msedge|firefox|brave|opera|arc|vivaldi|zen|chromium|chromium-browser|firefox-bin|firefox-esr|librewolf|brave-browser)$"),
    terminal: re(r"^(windowsterminal|conhost|wezterm.*|alacritty|openconsole|gnome-terminal-server|konsole|kitty|foot|xterm|tilix|terminator|ghostty|xfce4-terminal|ptyxis)$"),
    vm: re(r"^(vmwp|vmmem.*|qemu-system-.*|qemu-kvm|virtualboxvm|vboxheadless|firecracker|crosvm|vmware-vmx)$"),
    desktop: re(r"^(dwm|explorer|csrss|system|shellhost|startmenuexperiencehost|searchhost|textinputhost|applicationframehost|widgetboard|widgets|lockapp|shellexperiencehost|crossdeviceresume|powertoys.*|microsoft\.cmdpal\.ui|phoneexperiencehost|systemsettings|xorg|xwayland|gnome-shell|kwin_wayland|kwin_x11|plasmashell|mutter|sway|hyprland|weston|xfwm4|cinnamon|gnome-session-binary|xdg-desktop-portal.*|weston-rdp|msrdc)$"),
    app: re(r"^(discord|slack|teams|ms-teams|zoom|claude|code|cursor|spotify|m365copilot|mscopilot|whatsapp|notion|obsidian|msedgewebview2|onedrive|outlook|olk|thunderbird|signal-desktop|telegram-desktop)$"),
    audio: re(r"^(audiodg|pipewire|pipewire-pulse|pulseaudio|wireplumber|jackd)$"),
    winpath: re(r"\\windows\\|\\windowsapps\\"),
});

/// `name` is the process name without `.exe`, `path` the image path (may be empty),
/// `cmd` a command line (WSL rows and Linux processes), `eng` per-engine-type
/// utilisation if known.
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
    // Linux games: a Steam / Heroic / Lutris library path, Proton (a Python script) and
    // Wine, and Steam's `reaper`, which wraps every game it launches (`reaper
    // SteamLaunch AppId=...`; on Windows REAPER is also a DAW, so it needs Steam context).
    // Windows image paths use backslashes, so none of this matches them.
    let steam = |s: &str| s.contains("steam");
    if r.gamepath_unix.is_match(&p)
        || r.gamepath_unix.is_match(&c)
        || r.gamepath.is_match(&c) // a Wine game's command line is a Windows path
        || r.game_unix.is_match(&n)
        || (n == "reaper" && (steam(&p) || steam(&c)))
    {
        return "game";
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
    if r.gamename.is_match(&n) || (n == "javaw" && p.contains("minecraft")) {
        return "game";
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
    if r.audio.is_match(&n) {
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

#[cfg(test)]
mod tests {
    use super::tag;
    use std::collections::BTreeMap;

    fn t(name: &str, path: &str, cmd: &str) -> &'static str {
        tag(name, path, cmd, None)
    }

    #[test]
    fn linux_games() {
        let steam = "/home/u/.local/share/Steam";
        assert_eq!(t("reaper", &format!("{steam}/ubuntu12_32/reaper"), "reaper SteamLaunch AppId=1245620 -- /home/u/x"), "game");
        assert_eq!(
            t("eldenring.exe", "/home/u/.local/share/Steam/steamapps/common/Proton 9.0 (Beta)/files/bin/wine64-preloader", ""),
            "game"
        );
        assert_eq!(t("Game.x86_64", "", &format!("{steam}/steamapps/common/Hades II/Game.x86_64")), "game");
        assert_eq!(t("Hades.exe", "", r"Z:\home\u\.local\share\Steam\steamapps\common\Hades\x64\Hades.exe"), "game");
        assert_eq!(t("proton", "", "python3 /home/u/.steam/steam/steamapps/common/Proton/proton waitforexitandrun"), "game");
        assert_eq!(t("wine64-preloader", "/usr/bin/wine64-preloader", "C:\\Games\\x.exe"), "game");
        assert_eq!(t("wineserver", "", ""), "game");
        assert_eq!(t("steam", &format!("{steam}/ubuntu12_32/steam"), "steam -silent"), "launcher");
        assert_eq!(t("steamwebhelper", "", ""), "launcher");
        // REAPER the DAW (Windows) is not a Steam game
        assert_eq!(t("reaper", r"c:\program files\reaper (x64)\reaper.exe", ""), "other");
    }

    #[test]
    fn linux_media_and_ai() {
        assert_eq!(t("ffmpeg", "/usr/bin/ffmpeg", "ffmpeg -i in.mkv -c:v hevc_vaapi out.mkv"), "video render");
        assert_eq!(t("obs", "/usr/bin/obs", "obs --startreplaybuffer"), "recording");
        assert_eq!(t("ollama", "/usr/local/bin/ollama", "/usr/local/bin/ollama serve"), "ai inference");
        assert_eq!(t("llama-server", "/opt/llama.cpp/build/bin/llama-server", "llama-server -m q.gguf"), "ai inference");
        assert_eq!(t("mpv", "/usr/bin/mpv", "mpv movie.mkv"), "video playback");
    }

    #[test]
    fn linux_python() {
        assert_eq!(t("python3", "/usr/bin/python3.12", "/usr/bin/python3 train.py --epochs 3"), "training");
        assert_eq!(
            t("train bd1-walk-flat", "", "/home/u/p/.venv/bin/python /home/u/p/.venv/bin/train bd1-walk-flat --x 1"),
            "training"
        );
        assert_eq!(t("python3", "", "/usr/bin/python3 -m http.server"), "compute");
        assert_eq!(t("python3", "", "python3 finetune_lora.py"), "training");
    }

    #[test]
    fn linux_desktop_and_friends() {
        assert_eq!(t("Xorg", "/usr/lib/xorg/Xorg", "/usr/lib/xorg/Xorg :0"), "desktop");
        assert_eq!(t("Xwayland", "", "/usr/bin/Xwayland :0"), "desktop");
        assert_eq!(t("gnome-shell", "", ""), "desktop");
        assert_eq!(t("kwin_wayland", "", ""), "desktop");
        assert_eq!(t("gnome-terminal-server", "", ""), "terminal");
        assert_eq!(t("kitty", "", ""), "terminal");
        assert_eq!(t("chromium", "", ""), "browser");
        assert_eq!(t("firefox-bin", "", ""), "browser");
        assert_eq!(t("pipewire", "", ""), "audio");
        assert_eq!(t("qemu-system-x86_64", "", ""), "vm");
        assert_eq!(t("code", "/usr/share/code/code", ""), "app");
    }

    #[test]
    fn linux_engine_hints() {
        let mut e = BTreeMap::new();
        e.insert("videoencode".to_string(), 30.0);
        assert_eq!(tag("gst-launch-1.0", "/usr/bin/gst-launch-1.0", "", Some(&e)), "video render");
        let mut e = BTreeMap::new();
        e.insert("3d".to_string(), 80.0);
        assert_eq!(tag("mygame", "/home/u/mygame/mygame", "", Some(&e)), "game?");
    }

    #[test]
    fn windows_rules_unchanged() {
        assert_eq!(t("vmwp", "", ""), "vm");
        assert_eq!(t("chrome", r"c:\program files\google\chrome\application\chrome.exe", ""), "browser");
        assert_eq!(t("eldenring", r"d:\steamlibrary\steamapps\common\elden ring\game\eldenring.exe", ""), "game");
        assert_eq!(t("audiodg", "", ""), "audio");
        assert_eq!(t("dwm", r"c:\windows\system32\dwm.exe", ""), "desktop");
        let mut e = BTreeMap::new();
        e.insert("3d".to_string(), 80.0);
        assert_eq!(tag("notepad", r"c:\windows\system32\notepad.exe", "", Some(&e)), "other");
    }

    #[test]
    fn games_outside_store_folders() {
        assert_eq!(tag("RobloxPlayerBeta", r"C:\Users\u\AppData\Local\Roblox\Versions\v1\RobloxPlayerBeta.exe", "", None), "game");
        assert_eq!(tag("RobloxPlayerLauncher", "", "", None), "game");
        assert_eq!(tag("RobloxStudioBeta", "", "", None), "game dev");
        assert_eq!(tag("Minecraft.Windows", "", "", None), "game");
        assert_eq!(tag("javaw", r"C:\Users\u\AppData\Roaming\.minecraft\runtime\bin\javaw.exe", "", None), "game");
        assert_eq!(tag("javaw", r"C:\Program Files\Java\bin\javaw.exe", "", None), "other");
        assert_eq!(tag("League of Legends", "", "", None), "game");
        assert_eq!(tag("VALORANT-Win64-Shipping", "", "", None), "game");
        assert_eq!(tag("FortniteClient-Win64-Shipping", "", "", None), "game");
    }

    #[test]
    fn existing_rules_still_hold() {
        assert_eq!(tag("chrome", "", "", None), "browser");
        assert_eq!(tag("python", "", "python train.py", None), "training");
        assert_eq!(tag("dwm", "", "", None), "desktop");
    }
}
