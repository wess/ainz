"""Checks inline mode and the provider setup lists, in a pty this script owns."""

import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time

import pyte

ROOT = "/private/tmp/claude-501/-Users-wess-Desktop-Dev-ainz/d7504dbd-f0c7-4e8d-aa36-577d257e7761/scratchpad/probe"
BINARY = "/Users/wess/Desktop/Dev/ainz/target/debug/ainz"
ROWS, COLS = 40, 110
ESC, ENTER = "\x1b", "\r"
DOWN, UP = f"{ESC}[B", f"{ESC}[A"

failures, checks = [], 0


def check(name, condition, detail=""):
    global checks
    checks += 1
    print(f"  {'pass' if condition else 'FAIL'}  {name}" + ("" if condition else f"   {detail}"))
    if not condition:
        failures.append(name)


class Terminal:
    answers = True

    def __init__(self, config, answers=True):
        self.answers = answers
        self.screen = pyte.Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.raw = bytearray()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(ROOT)
            os.execve(
                BINARY,
                ["ainz"],
                dict(os.environ, TERM="xterm-256color", AINZ_CONFIG=os.path.join(ROOT, config)),
            )
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.set_blocking(self.fd, False)

    def pump(self, seconds=0.6):
        end = time.time() + seconds
        while time.time() < end:
            if not select.select([self.fd], [], [], 0.05)[0]:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.raw += chunk
            self.stream.feed(chunk)
            self.answer(chunk)

    def answer(self, chunk):
        # device attributes, and the cursor position the inline viewport asks for
        if not self.answers:
            return
        if b"\x1b[c" in chunk:
            os.write(self.fd, b"\x1b[?1;2c")
        if b"\x1b[6n" in chunk:
            row = self.screen.cursor.y + 1
            column = self.screen.cursor.x + 1
            os.write(self.fd, f"\x1b[{row};{column}R".encode())

    def send(self, text, settle=0.5):
        os.write(self.fd, text.encode())
        self.pump(settle)

    def body(self):
        return "\n".join(row.rstrip() for row in self.screen.display)

    def status_row(self):
        """The inline viewport sits wherever the cursor was, not at the bottom of the screen."""
        for index, row in enumerate(self.screen.display):
            if "fake/test" in row:
                return index
        return None

    def viewport(self):
        start = self.status_row()
        if start is None:
            return ""
        return "\n".join(row.rstrip() for row in self.screen.display[start:])

    def close(self):
        try:
            os.kill(self.pid, 9)
        except ProcessLookupError:
            pass


# --- inline mode -----------------------------------------------------------
with open(os.path.join(ROOT, "config.toml")) as handle:
    base = [line for line in handle if not line.startswith("inline")]
with open(os.path.join(ROOT, "inline.toml"), "w") as handle:
    handle.write("".join(base).replace("[ui]\n", "[ui]\ninline = true\n"))

print("inline mode")
term = Terminal("inline.toml")
term.pump(6.0)
check("it never leaves the main screen", b"\x1b[?1049h" not in term.raw)
check(
    "and never takes the mouse",
    not any(mode in term.raw for mode in (b"?1000h", b"?1002h", b"?1003h")),
)
check("bracketed paste is still on", b"?2004h" in term.raw)
check("the prompt is drawn", term.viewport().rstrip().endswith(">"), repr(term.viewport()))

term.send("first inline prompt" + ENTER, settle=1.6)
term.send("second inline prompt" + ENTER, settle=1.6)
screen = term.body()
check("what was said is on screen", "first inline prompt" in screen, screen[-600:])
check("replies are too", "a reply from the fake provider" in screen, screen[-600:])
check(
    "the finished talk is above the prompt, not in it",
    "first inline prompt" not in term.viewport(),
    term.viewport(),
)
above = "\n".join(term.screen.display[: term.status_row() or 0])
check("it is in the scroll above", "first inline prompt" in above, above[-400:])
term.send("typing still works")
check(
    "and the prompt still takes input",
    term.viewport().rstrip().endswith("typing still works"),
    repr(term.viewport()),
)
term.close()

# --- a terminal that will not say where the cursor is -----------------------
print("\ninline where the terminal does not answer")
term = Terminal("inline.toml", answers=False)
term.pump(8.0)
check(
    "it falls back instead of failing to start",
    "fake/test" in term.body(),
    term.body()[-400:],
)
check("and says why", "inline drawing is not available" in term.body(), term.body()[:600])
term.send("still usable")
check("and still takes input", "still usable" in term.body(), term.body()[-200:])
term.close()

# --- the setup lists -------------------------------------------------------
print("\nprovider setup")
term = Terminal("config.toml")
term.pump(6.0)
term.send("/config" + ENTER, settle=1.5)
screen = term.body()
check("the provider list opens", "Claude Code" in screen and "Ollama" in screen, screen[:600])
check("and offers a custom process", "Custom process" in screen, screen[:600])

# walk to "Custom process": ollama, litellm, codex, claude, [saved…], http, process
for _ in range(12):
    if "Custom process" in term.body():
        break
    term.send(DOWN, settle=0.15)
rows = term.body().split("\n")
selected = [row for row in rows if "Executable adapter" in row]
term.send(DOWN * 0, settle=0.1)
# step down until the detail pane describes the process adapter
for _ in range(12):
    if "Executable adapter" in term.body():
        break
    term.send(DOWN, settle=0.2)
check("the process adapter can be reached", "Executable adapter" in term.body(), term.body()[:800])

term.send(ENTER, settle=1.2)
screen = term.body()
check(
    "the command list shows agents found on this machine",
    "claude" in screen and "codex" in screen,
    screen[:800],
)
check("and offers to type another", "Type another" in screen, screen[:800])
term.send(ESC, settle=0.6)
term.close()

print()
print(f"{checks - len(failures)}/{checks} checks passed")
if failures:
    print("failed:", ", ".join(failures))
sys.exit(1 if failures else 0)
