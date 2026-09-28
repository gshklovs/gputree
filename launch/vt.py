"""Minimal VT emulator: replay a timestamped capture into screen snapshots (one per chunk).

Output rows are run-length lists [text, fg, bg, flags] with colours resolved to the
Windows Terminal Campbell scheme, so the HTML clone draws exactly the captured text.
"""
import json, re, sys
from pathlib import Path

CAMPBELL = ["#0C0C0C", "#C50F1F", "#13A10E", "#C19C00", "#0037DA", "#881798", "#3A96DD", "#CCCCCC",
            "#767676", "#E74856", "#16C60C", "#F9F1A5", "#3B78FF", "#B4009E", "#61D6D6", "#F2F2F2"]


def xterm256(n):
    if n < 16:
        return CAMPBELL[n]
    if n < 232:
        n -= 16
        lv = [0, 95, 135, 175, 215, 255]
        return "#%02X%02X%02X" % (lv[n // 36], lv[(n // 6) % 6], lv[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02X%02X%02X" % (v, v, v)


class Screen:
    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.grid = [[(" ", None, None, 0) for _ in range(cols)] for _ in range(rows)]
        self.x = self.y = 0
        self.fg = self.bg = None
        self.flags = 0  # 1 bold, 2 dim
        self.pending_wrap = False

    def blank(self):
        return (" ", None, self.bg, 0)

    def lf(self):
        self.y += 1
        if self.y >= self.rows:
            self.grid.pop(0)
            self.grid.append([self.blank() for _ in range(self.cols)])
            self.y = self.rows - 1

    def put(self, ch):
        if self.pending_wrap:
            self.x = 0
            self.lf()
            self.pending_wrap = False
        self.grid[self.y][self.x] = (ch, self.fg, self.bg, self.flags)
        if self.x == self.cols - 1:
            self.pending_wrap = True
        else:
            self.x += 1

    def sgr(self, params):
        ps = [int(p) if p else 0 for p in params.split(";")] if params else [0]
        i = 0
        while i < len(ps):
            p = ps[i]
            if p == 0:
                self.fg = self.bg = None
                self.flags = 0
            elif p == 1:
                self.flags |= 1
            elif p == 2:
                self.flags |= 2
            elif p == 22:
                self.flags &= ~3
            elif 30 <= p <= 37:
                self.fg = p - 30
            elif 90 <= p <= 97:
                self.fg = p - 90 + 8
            elif p == 39:
                self.fg = None
            elif 40 <= p <= 47:
                self.bg = p - 40
            elif 100 <= p <= 107:
                self.bg = p - 100 + 8
            elif p == 49:
                self.bg = None
            elif p in (38, 48) and i + 1 < len(ps):
                if ps[i + 1] == 5 and i + 2 < len(ps):
                    c = ("256", ps[i + 2])
                    i += 2
                elif ps[i + 1] == 2 and i + 4 < len(ps):
                    c = ("rgb", "#%02X%02X%02X" % tuple(ps[i + 2:i + 5]))
                    i += 4
                else:
                    c = None
                if p == 38:
                    self.fg = c
                else:
                    self.bg = c
            i += 1

    def csi(self, params, cmd):
        n = int(params) if params.isdigit() else None
        if cmd == "m":
            self.sgr(params)
        elif cmd == "K":
            for x in range(self.x, self.cols):
                self.grid[self.y][x] = self.blank()
        elif cmd == "J":
            if params == "2":
                self.grid = [[self.blank() for _ in range(self.cols)] for _ in range(self.rows)]
            else:
                for x in range(self.x, self.cols):
                    self.grid[self.y][x] = self.blank()
                for y in range(self.y + 1, self.rows):
                    self.grid[y] = [self.blank() for _ in range(self.cols)]
        elif cmd == "H":
            if params:
                r, _, c = params.partition(";")
                self.y = max(0, int(r or 1) - 1)
                self.x = max(0, int(c or 1) - 1)
            else:
                self.x = self.y = 0
        elif cmd == "F":
            self.y = max(0, self.y - (n or 1))
            self.x = 0
        elif cmd == "A":
            self.y = max(0, self.y - (n or 1))
        elif cmd == "G":
            self.x = max(0, (n or 1) - 1)
        self.pending_wrap = False

    def feed(self, s):
        for m in re.finditer(r"\x1b\[([?0-9;]*)([A-Za-z])|\x1b.|(\r\n|\n)|(\r)|([^\x1b\r\n])", s):
            if m.group(2):
                if m.group(1).startswith("?"):
                    continue
                self.csi(m.group(1), m.group(2))
            elif m.group(3):
                self.x = 0
                self.pending_wrap = False
                self.lf()
            elif m.group(4):
                self.x = 0
                self.pending_wrap = False
            elif m.group(5):
                ch = m.group(5)
                if ch == "\t":
                    for _ in range(8 - self.x % 8):
                        self.put(" ")
                elif ch >= " ":
                    self.put(ch)

    @staticmethod
    def color(c, default):
        if c is None:
            return default
        if isinstance(c, int):
            return CAMPBELL[c]
        if c[0] == "256":
            return xterm256(c[1])
        return c[1]

    def snapshot(self, upto=None):
        out = []
        last = max((y for y in range(self.rows) if any(ch != " " or bg is not None for ch, _, bg, _ in self.grid[y])), default=-1)
        for y in range(last + 1):
            runs = []
            for ch, fg, bg, fl in self.grid[y]:
                fgc = fg
                # Windows Terminal "intense as bright": bold basic colours render bright
                if fl & 1 and isinstance(fg, int) and fg < 8:
                    fgc = fg + 8
                key = (self.color(fgc, "#CCCCCC"), self.color(bg, None), fl)
                if runs and runs[-1][1:] == list(key):
                    runs[-1][0] += ch
                else:
                    runs.append([ch, *key])
            # trim trailing plain spaces
            while runs and runs[-1][2] is None and runs[-1][0].strip() == "":
                runs.pop()
            if runs and runs[-1][2] is None:
                runs[-1][0] = runs[-1][0].rstrip(" ")
            out.append(runs)
        return out


def replay(cap, cols, rows, prompt=None, start_row=0):
    scr = Screen(cols, rows)
    if prompt:
        scr.feed(prompt + "\r\n")
    snaps = [{"t": 0.0, "rows": scr.snapshot()}]
    for c in cap["chunks"]:
        scr.feed(c["s"])
        snaps.append({"t": c["t"], "rows": scr.snapshot()})
    return snaps


if __name__ == "__main__":
    cap_dir = Path(sys.argv[1])
    out = {}
    for f in sorted(cap_dir.glob("*.json")):
        if f.name.endswith(".screens.json"):
            continue
        cap = json.loads(f.read_text(encoding="utf-8"))
        out[f.stem] = {"cols": cap["cols"], "lines": cap["lines"], "total_ms": cap["total_ms"],
                       "stderr": cap["stderr"], "snaps": replay(cap, cap["cols"], cap["lines"])}
    Path(sys.argv[2]).write_text(json.dumps(out, ensure_ascii=False), encoding="utf-8")
    for k, v in out.items():
        print(k, len(v["snaps"]), "snaps, final rows", len(v["snaps"][-1]["rows"]))
