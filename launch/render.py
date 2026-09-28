"""Deterministic frame capture of web/index.html (window.__render(t)).
  render.py sheet            one frame per beat-ish checkpoint -> sheet/*.png + contact sheet
  render.py frames [--gif]   all frames at 30 fps -> frames/ or gifframes/
  render.py at T [T...]      single frames -> probe_T.png
"""
import sys, json, subprocess, os
SLOW = float(os.environ.get("SLOW", "1"))
from pathlib import Path
from playwright.sync_api import sync_playwright
here = Path(__file__).parent
mode = sys.argv[1]
gif = "--gif" in sys.argv
url = (here / "web" / "index.html").as_uri() + ("#gif" if gif else "")
with sync_playwright() as p:
    br = p.chromium.launch()
    pg = br.new_page(viewport={"width": 1920, "height": 1080})
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    pg.on("console", lambda m: errs.append("console: " + m.text) if m.type == "error" else None)
    pg.goto(url)
    pg.wait_for_function("window.__render !== undefined", timeout=20000)
    T = pg.evaluate("window.__T"); N = pg.evaluate("window.__N")
    stage = pg.locator("#stage")
    if mode == "at":
        for a in sys.argv[2:]:
            if a.startswith("--"): continue
            pg.evaluate(f"window.__render({a})"); stage.screenshot(path=str(here / f"probe_{a}.png"))
    elif mode == "sheet":
        out = here / "sheet"; out.mkdir(exist_ok=True)
        ts = json.loads(sys.argv[2]) if len(sys.argv) > 2 and not sys.argv[2].startswith("--") else None
        for i, t in enumerate(ts):
            pg.evaluate(f"window.__render({t})"); stage.screenshot(path=str(out / f"s{i:02d}.png"))
    else:
        fps = 30
        t0, t1 = (T["gifStart"], T["gifEnd"]) if gif else (0, T["total"])
        d = here / ("gifframes" if gif else "frames"); d.mkdir(exist_ok=True)
        for f in d.glob("*.png"): f.unlink()
        n = round((t1 - t0) * fps * SLOW)
        for i in range(n):
            pg.evaluate(f"window.__render({t0 + i / (fps * SLOW)})")
            stage.screenshot(path=str(d / f"f{i:05d}.png"))
            if i % 150 == 0: print("frame", i, "/", n, flush=True)
    print(json.dumps({"T": T, "N": N}))
    if errs: print("PAGE ERRORS", errs)
    br.close()
