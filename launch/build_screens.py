import json
from pathlib import Path
from vt import replay
BS = chr(92)
P = "PS C:" + BS + "Users" + BS + "grego> "
pick = {"smi": ("smi_23", "nvidia-smi"), "cold": ("gputree_cold_9", "gputree"), "watch": ("gputree_watch", "gputree -w .5"),
        "offline": ("gputree_offline_1", "gputree"), "cpu": ("cputree_2", "cputree"),
        "legacy": ("legacy_2", "." + BS + "legacy" + BS + "gputree.ps1")}
out = {}
for k, (f, cmd) in pick.items():
    cap = json.loads(Path(f"captures/{f}.json").read_text(encoding="utf-8"))
    out[k] = {"file": f, "cmd": cmd, "prompt": P, "total_ms": cap["total_ms"], "first_byte_ms": cap["first_byte_ms"],
              "stderr": cap["stderr"], "snaps": replay(cap, 100 if k != "legacy" else 160, cap["lines"], prompt=P + cmd)}
    print(k, f, len(out[k]["snaps"]), "snaps; rows", len(out[k]["snaps"][-1]["rows"]))
Path("web").mkdir(exist_ok=True)
Path("web/screens.js").write_text("window.SCREENS=" + json.dumps(out, ensure_ascii=False) + ";", encoding="utf-8")
