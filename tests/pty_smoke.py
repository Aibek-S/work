#!/usr/bin/env python3
"""End-to-end smoke test: drives the real `work` binary in a pseudo-terminal.

Checks: TUI renders, keys work (pause, mute, noise, volume, help, restart confirm),
terminal resize, and that every exit path (q, Ctrl+C, SIGTERM, SIGHUP) restores the
terminal (leaves the alternate screen, shows the cursor) and leaves no process behind.

Usage: python3 tests/pty_smoke.py [path/to/work]   (runs with --silent, no audio needed)
"""

import codecs
import fcntl
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/work"
ALT_ON, ALT_OFF, CUR_SHOW = b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?25h"


def set_size(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


CSI = re.compile(r"\x1b\[([?0-9;]*)([A-Za-z])")


class Screen:
    """Tiny VT emulator: enough for ratatui's diff output (cursor moves, clears, text)."""

    def __init__(self, rows, cols):
        self.resize(rows, cols)

    def resize(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.cells = [[" "] * cols for _ in range(rows)]
        self.y = self.x = 0

    def feed(self, text):
        i = 0
        while i < len(text):
            m = CSI.match(text, i)
            if m:
                args, cmd = m.group(1), m.group(2)
                nums = [int(a) if a.isdigit() else 1 for a in args.split(";")] if args else []
                if cmd == "H":
                    self.y = (nums[0] if nums else 1) - 1
                    self.x = (nums[1] if len(nums) > 1 else 1) - 1
                elif cmd == "J" and args in ("2", "3"):
                    self.cells = [[" "] * self.cols for _ in range(self.rows)]
                i = m.end()
                continue
            ch = text[i]
            if ch == "\x1b":
                i += 2  # skip other two-byte escapes
                continue
            if ch == "\r":
                self.x = 0
            elif ch == "\n":
                self.y += 1
            elif ch >= " " and 0 <= self.y < self.rows and 0 <= self.x < self.cols:
                self.cells[self.y][self.x] = ch
                self.x += 1
            i += 1

    def text(self):
        return "\n".join("".join(r) for r in self.cells)


class Session:
    def __init__(self, *args, rows=30, cols=80):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            env = dict(os.environ, TERM="xterm-256color", LANG="C.UTF-8",
                       XDG_CONFIG_HOME="/nonexistent-work-test")
            env.pop("LC_ALL", None)
            os.execve(BIN, [BIN, "--silent", "--no-notify", *args], env)
        set_size(self.fd, rows, cols)
        self.out = b""
        self.screen = Screen(rows, cols)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def read(self, secs=0.6):
        end = time.monotonic() + secs
        while time.monotonic() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    break
                self.out += chunk
                self.screen.feed(self.decoder.decode(chunk))
        return self.out

    def resize(self, rows, cols):
        set_size(self.fd, rows, cols)
        self.screen.resize(rows, cols)
        os.kill(self.pid, signal.SIGWINCH)

    def send(self, data, secs=0.4):
        """Sends keys and returns the resulting screen contents."""
        os.write(self.fd, data)
        self.read(secs)
        return self.screen.text()

    def wait_exit(self, timeout=3.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.read(0.1)
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.read(0.1)
                return os.waitstatus_to_exitcode(status)
        os.kill(self.pid, signal.SIGKILL)
        raise AssertionError("process did not exit")


failures = 0


def check(name, cond, detail=""):
    global failures
    print(f"  {'ok  ' if cond else 'FAIL'} {name}" + (f"  ({detail})" if detail and not cond else ""))
    if not cond:
        failures += 1


def restored(out):
    # The last alternate-screen toggle must be "off", followed by a cursor show.
    return out.rfind(ALT_OFF) > out.rfind(ALT_ON) >= 0 and out.rfind(CUR_SHOW) > out.rfind(ALT_ON)


print("interactive keys")
s = Session("25")
s.read(1.0)
screen = s.screen.text()
check("renders title and timer", "W O R K" in screen and "FOCUS SESSION" in screen)
check("silent start shows Muted", "Muted" in screen)
check("pause", "PAUSED" in s.send(b" "))
check("resume", "FOCUS SESSION" in s.send(b" "))
check("unmute via M", "Volume 35%" in s.send(b"m"))
check("volume up", "Volume 40%" in s.send(b"+"))
check("volume down", "Volume 35%" in s.send(b"-"))
check("next noise pink", "Pink Noise" in s.send(b"n"))
check("next noise brown", "Brown Noise" in s.send(b"n"))
check("help overlay", "Press any key to close" in s.send(b"?"))
check("any key closes help", "Press any key" not in s.send(b"x"))
check("restart asks confirmation", "Restart the session" in s.send(b"r"))
check("restart cancel closes dialog", "Restart the session" not in s.send(b"n"))
s.resize(12, 40)
small = s.send(b"", 0.8)
check("adapts to 40x12 (compact timer)", "24:5" in small or "25:00" in small, small)
s.resize(30, 80)
check("back to big layout", "[Space] Pause" in s.send(b"", 0.8))
s.send(b"m")  # mute again so no device is opened lazily in later steps
os.write(s.fd, b"q")
code = s.wait_exit()
check("q exits with 0", code == 0, f"exit {code}")
check("terminal restored after q", restored(s.out))

for name, action, expected in [
    ("Ctrl+C key", lambda s: os.write(s.fd, b"\x03"), 0),
    ("SIGINT", lambda s: os.kill(s.pid, signal.SIGINT), 130),
    ("SIGTERM", lambda s: os.kill(s.pid, signal.SIGTERM), 143),
    ("SIGHUP", lambda s: os.kill(s.pid, signal.SIGHUP), 129),
]:
    print(name)
    s = Session()
    s.read(0.8)
    action(s)
    code = s.wait_exit()
    check(f"exits with {expected}", code == expected, f"exit {code}")
    check("terminal restored", restored(s.out))

print("plain mode (--no-tui)")
s = Session("1", "--no-tui")
out = s.read(1.2).decode()
check("prints header", "1 min focus session (silent)" in out, out)
check("shows countdown", "00:5" in out or "01:00" in out, out)
os.kill(s.pid, signal.SIGTERM)
code = s.wait_exit()
check("SIGTERM exits with 143", code == 143, f"exit {code}")
check("prints Stopped", "Stopped." in s.out.decode())

print(f"\n{'PASSED' if failures == 0 else f'{failures} FAILED'}")
sys.exit(1 if failures else 0)
