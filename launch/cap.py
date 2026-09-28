"""Run a command, stamp every stdout chunk (ms since spawn), save JSON + raw bytes."""
import json, os, subprocess, sys, threading, time
name, *cmd = sys.argv[1:]
out_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), os.environ.get("CAP_DIR", "captures"))
os.makedirs(out_dir, exist_ok=True)
env = dict(os.environ)
env.update({"COLUMNS": os.environ.get("CAP_COLS", "100"), "LINES": os.environ.get("CAP_LINES", "120")})
duration = float(os.environ.get("CAP_MAX_S", "60"))
chunks, err = [], []
t0 = time.perf_counter()
p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, bufsize=0)
def rd_err():
    for line in iter(p.stderr.readline, b""):
        err.append(((time.perf_counter() - t0) * 1000, line.decode("utf-8", "replace")))
threading.Thread(target=rd_err, daemon=True).start()
fd = p.stdout.fileno()
def rd():
    while True:
        b = os.read(fd, 65536)
        if not b:
            break
        chunks.append(((time.perf_counter() - t0) * 1000, b))
th = threading.Thread(target=rd, daemon=True); th.start()
try:
    p.wait(timeout=duration)
except subprocess.TimeoutExpired:
    p.terminate(); p.wait()
th.join(2)
total = (time.perf_counter() - t0) * 1000
raw = b"".join(c for _, c in chunks)
open(os.path.join(out_dir, name + ".ansi"), "wb").write(raw)
json.dump({"name": name, "cmd": cmd, "cols": int(env["COLUMNS"]), "lines": int(env["LINES"]), "total_ms": total,
           "first_byte_ms": chunks[0][0] if chunks else None,
           "chunks": [{"t": round(t, 2), "s": c.decode("utf-8", "replace")} for t, c in chunks],
           "stderr": [{"t": round(t, 2), "s": s} for t, s in err]},
          open(os.path.join(out_dir, name + ".json"), "w", encoding="utf-8"), ensure_ascii=False, indent=0)
print(f"{name}: {len(chunks)} chunks, first byte {chunks[0][0] if chunks else None:.1f} ms, total {total:.0f} ms", file=sys.stderr)
