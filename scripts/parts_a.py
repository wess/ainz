"""Drives Ainz in a pty this script owns, with a real VT screen model, and checks the prompt.

Nothing here touches the user's terminal: the pty is created, driven and killed by this process.
"""

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

ESC = "\x1b"
UP, DOWN, LEFT, RIGHT = f"{ESC}[A", f"{ESC}[B", f"{ESC}[D", f"{ESC}[C"
SHIFT_UP, SHIFT_DOWN = f"{ESC}[1;2A", f"{ESC}[1;2B"
ALT_LEFT, ALT_RIGHT = f"{ESC}[1;3D", f"{ESC}[1;3C"
CTRL_A, CTRL_E, CTRL_K, CTRL_U, CTRL_W, CTRL_O, CTRL_C = (
    "\x01",
    "\x05",
    "\x0b",
    "\x15",
    "\x17",
    "\x0f",
    "\x03",
)
ENTER = "\r"
TAB = "\t"

failures = []
checks = 0


class Terminal:
    def __init__(self):
        self.screen = pyte.Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(ROOT)
            env = dict(
                os.environ,
                TERM="xterm-256color",
                AINZ_CONFIG=os.path.join(ROOT, "config.toml"),
                COLUMNS=str(COLS),
                LINES=str(ROWS),
            )
            env.pop("AINZ_MODEL", None)
            os.execve(BINARY, ["ainz"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.set_blocking(self.fd, False)

    def pump(self, seconds=0.6):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.stream.feed(chunk)

    def send(self, text, settle=0.45):
        os.write(self.fd, text.encode())
        self.pump(settle)

    def line(self, index):
        return self.screen.display[index].rstrip()

    def prompt(self):
        """The prompt line: the last non-empty line, which is the input row."""
        for row in reversed(self.screen.display):
            if row.strip():
                return row.strip()
        return ""

    def status(self):
        for row in reversed(self.screen.display):
            if "│" in row and ("ready" in row or "thinking" in row or "tok" in row):
                return row.strip()
        return ""

    def body(self):
        return "\n".join(row.rstrip() for row in self.screen.display)

    def close(self):
        try:
            os.kill(self.pid, 9)
        except ProcessLookupError:
            pass


def check(name, condition, detail=""):
    global checks
    checks += 1
    if condition:
        print(f"  pass  {name}")
    else:
        failures.append(name)
        print(f"  FAIL  {name}   {detail}")


term = Terminal()
term.pump(6.0)
print("start:", repr(term.status()))

# --- the line editor -------------------------------------------------------
term.send("hello world")
check("typing reaches the prompt", term.prompt().endswith("hello world"), term.prompt())

term.send(CTRL_A)
term.send("X")
check("ctrl+a goes to the start", "> Xhello world" in term.prompt(), term.prompt())

term.send(CTRL_E)
term.send("!")
check("ctrl+e goes to the end", term.prompt().endswith("Xhello world!"), term.prompt())

term.send(CTRL_W)
# pyte pads the row with spaces, so the trailing space of "Xhello " is not visible here
check("ctrl+w deletes a word", term.prompt() == "> Xhello", term.prompt())

term.send(LEFT + LEFT + "Q")
check("left moves the cursor", term.prompt() == "> XhellQo", term.prompt())

term.send(ALT_LEFT + "Z")
check("alt+left moves a word", term.prompt() == "> ZXhellQo", term.prompt())

term.send(CTRL_U + CTRL_K)
check("ctrl+u and ctrl+k clear the line", term.prompt().strip() in (">", "> "), term.prompt())

# --- history ---------------------------------------------------------------
term.send("first prompt" + ENTER, settle=1.4)
term.send("second prompt" + ENTER, settle=1.4)
term.send("draft in progress")
term.send(UP)
check("up recalls the last prompt", term.prompt().endswith("second prompt"), term.prompt())
term.send(UP)
check("up again reaches the older one", term.prompt().endswith("first prompt"), term.prompt())
term.send(DOWN)
term.send(DOWN)
check("down returns the draft", term.prompt().endswith("draft in progress"), term.prompt())
term.send(CTRL_U + CTRL_K)

# --- @ path completion -----------------------------------------------------
term.send("look at @alph")
menu = term.body()
check("@ opens a file menu", "alpha.txt" in menu, menu[-400:])
term.send(TAB)
check("tab completes the path", "@alpha.txt" in term.prompt(), term.prompt())
term.send(CTRL_U + CTRL_K)

term.send("@src/ma")
check("@ matches a nested path", "src/main.rs" in term.body(), term.body()[-400:])
term.send(CTRL_U + CTRL_K)

# --- a newline without shift+enter -----------------------------------------
term.send("one\\" + ENTER)
term.send("two")
check(
    "a trailing backslash makes a newline",
    "one" in term.body() and term.prompt().endswith("two"),
    term.prompt(),
)
check("the prompt grew a row", "  two" in term.body(), term.body()[-200:])
term.send(CTRL_U + CTRL_K)
term.send("\x08" * 8)

# --- rewind ----------------------------------------------------------------
term.send(ESC, settle=0.3)
term.send(ESC, settle=0.8)
check(
    "esc esc puts the last prompt back",
    term.prompt().endswith("second prompt"),
    term.prompt(),
)
check("and says so in the transcript", "rewound" in term.body(), term.body()[-500:])
term.send(CTRL_U + CTRL_K)

# --- scrolling is not history ----------------------------------------------
term.send("keep me")
term.send(SHIFT_UP)
check(
    "shift+up scrolls rather than recalling",
    term.prompt().endswith("keep me"),
    term.prompt(),
)
term.send(SHIFT_DOWN)
term.send(CTRL_U + CTRL_K)

# --- the mouse -------------------------------------------------------------
def press(column, row):
    """SGR 1006 button-1 press then release at a 1-based cell."""
    return f"{ESC}[<0;{column};{row}M{ESC}[<0;{column};{row}m"


def wheel(up, column=20, row=10):
    return f"{ESC}[<{64 if up else 65};{column};{row}M"


term.send("mouse test line")
# the prompt is the last row; clicking its fourth cell lands between "mo" and "use"
term.send(press(6, ROWS))
term.send("|")
# clicking a cell puts the caret before the character in it
check("a click places the cursor", term.prompt() == "> mou|se test line", term.prompt())
term.send(CTRL_U + CTRL_K)

before = term.body()
term.send(wheel(True) * 3)
term.send(wheel(False) * 3)
check("the wheel does not disturb the prompt", term.prompt().strip() in (">", "> "), term.prompt())
check("and the transcript survives it", "first prompt" in term.body(), term.body()[:200])

term.send("/hea")
menu_row = None
for index, row in enumerate(term.screen.display):
    if "/headers" in row:
        menu_row = index + 1
        break
check("the command menu opens", menu_row is not None, term.body()[-400:])
if menu_row:
    # the menu sits over the transcript, to the right of the roster
    term.send(press(40, menu_row))
    check(
        "clicking a suggestion takes it",
        term.prompt().startswith("> /head"),
        term.prompt(),
    )
term.send(CTRL_U + CTRL_K)

# --- vim -------------------------------------------------------------------
term.send("/vim" + ENTER, settle=1.0)
check("vim mode is on", "vim keys on" in term.body(), term.body()[-300:])
term.send("abc def")
term.send(ESC, settle=0.5)
check("esc shows the normal-mode prompt", term.prompt().startswith("▪"), term.prompt())
term.send("0")
term.send("x")
check("x deletes under the cursor", term.prompt().endswith("bc def"), term.prompt())
term.send("dd")
check("dd clears the line", term.prompt().strip() == "▪", term.prompt())
term.send("i")
term.send("back to insert")
check("i returns to insert", term.prompt().startswith(">"), term.prompt())
term.send(CTRL_U + CTRL_K)
term.send("/vim" + ENTER, settle=1.0)

print()
print(f"{checks - len(failures)}/{checks} checks passed")
if failures:
    print("failed:", ", ".join(failures))
    print("\n--- final screen ---")
    print(term.body())
term.close()
sys.exit(1 if failures else 0)
