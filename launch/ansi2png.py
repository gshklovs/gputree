# ansi2png.py IN.ansi OUT.png : render captured ANSI text as a terminal screenshot
import sys, re, html
from playwright.sync_api import sync_playwright
BASIC = ["#0c0c0c","#c50f1f","#13a10e","#c19c00","#0037da","#881798","#3a96dd","#cccccc"]
BRIGHT = ["#767676","#e74856","#16c60c","#f9f1a5","#3b78ff","#b4009e","#61d6d6","#f2f2f2"]
def c256(n):
    if n < 8: return BASIC[n]
    if n < 16: return BRIGHT[n-8]
    if n < 232:
        n -= 16; v = [0,95,135,175,215,255]
        return "#%02x%02x%02x" % (v[n//36], v[n//6%6], v[n%6])
    g = 8 + (n-232)*10; return "#%02x%02x%02x" % (g,g,g)
def conv(text):
    out = []
    for line in text.split("\n"):
        st = {}; buf = []
        for part in re.split(r"(\x1b\[[0-9;]*[A-Za-z])", line):
            m = re.match(r"\x1b\[([0-9;]*)([A-Za-z])", part)
            if m:
                if m.group(2) != "m": continue
                a = [int(x) for x in m.group(1).split(";") if x] or [0]
                i = 0
                while i < len(a):
                    x = a[i]
                    if x == 0: st = {}
                    elif x == 1: st["b"] = 1
                    elif x == 2: st["d"] = 1
                    elif x == 7: st["r"] = 1
                    elif 30 <= x <= 37: st["fg"] = BASIC[x-30]
                    elif 90 <= x <= 97: st["fg"] = BRIGHT[x-90]
                    elif 40 <= x <= 47: st["bg"] = BASIC[x-40]
                    elif x in (38, 48) and a[i+1] == 5:
                        st["fg" if x == 38 else "bg"] = c256(a[i+2]); i += 2
                    i += 1
                continue
            if not part: continue
            fg = st.get("fg", "#cccccc")
            if st.get("b") and fg in BASIC: fg = BRIGHT[BASIC.index(fg)]
            css = f"color:{fg};"
            if "bg" in st: css += f"background:{st['bg']};"
            if st.get("b"): css += "font-weight:bold;"
            if st.get("d"): css += "opacity:.55;"
            buf.append(f'<span style="{css}">{html.escape(part)}</span>')
        out.append("".join(buf))
    return "\n".join(out)
src = open(sys.argv[1], encoding="utf-8").read()
page = f"""<html><body style="margin:0;background:#0c0c0c"><pre style="margin:0;padding:16px;font:15px/1.25 'Cascadia Mono',Consolas,monospace;color:#ccc;display:inline-block">{conv(src)}</pre></body></html>"""
with sync_playwright() as p:
    b = p.chromium.launch(); pg = b.new_page(viewport={"width": 1100, "height": 400}, device_scale_factor=1.5)
    pg.set_content(page); pg.locator("pre").screenshot(path=sys.argv[2]); b.close()
