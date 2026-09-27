#!/usr/bin/env python3
"""PTY harness for the TUI (grok-build `xai-grok-pager-pty-harness` idea): run the real binary in a
pseudo-terminal, type keys, and dump the emulated screen. Usage:
  sidecar/.venv/bin/python scripts/tui-snapshot.py [--cols 120 --rows 36] [--keep]
Prints one snapshot per scripted step; exits non-zero if the header/banner never renders."""
import argparse, os, pty, select, signal, sys, time
try:
    import pyte
except ImportError:
    sys.exit("pyte is required: sidecar/.venv/bin/pip install pyte  (or pip install 'autoinference-sidecar[dev]')")

ap = argparse.ArgumentParser(); ap.add_argument("--cols", type=int, default=120); ap.add_argument("--rows", type=int, default=36)
ap.add_argument("--bin", default=os.path.join(os.path.dirname(__file__), "..", "target", "release", "autoinference"))
ap.add_argument("--steps", default="intro,kb,trial,help,quit"); ap.add_argument("--delay", type=float, default=2.5); ap.add_argument("--trial-config", default='{"max-num-seqs":256,"kv-cache-dtype":"fp8"}')
a = ap.parse_args()
a.bin = os.path.abspath(a.bin)
if not os.path.exists(a.bin):
    sys.exit(f"binary not found: {a.bin} (cargo build --release first, or pass --bin)")
if not os.environ.get("AUTOINFERENCE_SIDECAR_DIR") and not os.environ.get("AUTOINFERENCE_NO_SIDECAR"):
    os.environ["AUTOINFERENCE_SIDECAR_DIR"] = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "sidecar"))

screen = pyte.Screen(a.cols, a.rows); stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"; os.environ["COLUMNS"] = str(a.cols); os.environ["LINES"] = str(a.rows)
    argv = [a.bin, "--provider", "mock"] + (["--no-sidecar"] if os.environ.get("AUTOINFERENCE_NO_SIDECAR") else []) + ["tui"]
    os.execvp(a.bin, argv)
import fcntl, struct, termios
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", a.rows, a.cols, 0, 0))

def pump(seconds):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: data = os.read(fd, 65536)
            except OSError: return
            if not data: return
            stream.feed(data)

def snap(title):
    print(f"\n┌─ {title} " + "─" * (a.cols - len(title) - 4) + "┐")
    for line in screen.display: print("│" + line.rstrip().ljust(a.cols) + "│")
    print("└" + "─" * (a.cols + 0) + "┘")

def type_line(s):
    for ch in s: os.write(fd, ch.encode()); pump(0.01)
    os.write(fd, b"\r")

ok = True
for step in a.steps.split(","):
    if step == "intro": pump(1.6); snap("intro banner"); ok &= any("AUTOINFERENCE" in l or "▄▀█" in l for l in screen.display) or any("autoinference" in l for l in screen.display)
    elif step == "kb":
        type_line("kb: kv cache dtype"); pump(a.delay); snap("after `kb: kv cache dtype` (tool card + markdown)")
        ok &= any("kb_search" in l for l in screen.display) or print("ASSERT: no kb_search tool card") is not None
    elif step == "trial":
        type_line(f'trial: b200 {a.trial_config}'); pump(1.2)
        if any("approve?" in l for l in screen.display): snap("approval modal"); os.write(fd, b"y")
        pump(a.delay + 1.5); snap("after trial (bench panel: sparkline + pareto)")
        ok &= any(("tok/s ·" in l) or ("cache hit" in l) or ("trial failed" in l) for l in screen.display) or print("ASSERT: no trial result line") is not None
    elif step == "help":
        type_line("/help"); pump(0.8); snap("/help")
        ok &= any("/timeline" in l for l in screen.display) or print("ASSERT: /help did not render") is not None
    elif step == "palette":
        os.write(fd, b"/"); pump(0.4); snap("command palette")
        ok &= any("commands" in l for l in screen.display) or print("ASSERT: palette missing") is not None
        os.write(fd, b"\x1b"); pump(0.2)
    elif step == "quit": os.write(fd, b"\x03"); pump(0.5)
try: os.kill(pid, signal.SIGTERM)
except ProcessLookupError: pass
sys.exit(0 if ok else 1)
