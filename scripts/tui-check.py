#!/usr/bin/env python3
"""Drives the Ainz interface in a pty this script owns and checks what it draws.

The unit tests cover the model behind the prompt; this covers the wiring between a keystroke
and the screen, which is the part that only a terminal can answer. It builds its own workspace
and config with a fake provider — `/bin/echo` — so nothing here reaches a model or the network,
and it never touches the terminal it is run from.

    pip install pyte
    python3 scripts/tui-check.py [path/to/ainz]
"""

import fcntl
import http.server
import json
import os
import pty
import select
import shutil
import struct
import sys
import tempfile
import termios
import threading
import time

import pyte

ROWS, COLS = 40, 110
ESC, ENTER, TAB = "\x1b", "\r", "\t"
UP, DOWN, LEFT = f"{ESC}[A", f"{ESC}[B", f"{ESC}[D"
SHIFT_UP, SHIFT_DOWN = f"{ESC}[1;2A", f"{ESC}[1;2B"
ALT_LEFT = f"{ESC}[1;3D"
CTRL_A, CTRL_E, CTRL_K, CTRL_U, CTRL_W = "\x01", "\x05", "\x0b", "\x15", "\x17"

CONFIG = """\
provider = "fake"
model = "test"
permissions = "read_only"

[providers.fake]
kind = "process"
command = "/bin/echo"
args = ["a reply from the fake provider"]

[ui]
roster_visible = true
header = "builtin"
inline = {inline}

[memory]
backend = "off"

[synapse]
enabled = false
"""

failures, checks = [], 0


def check(name, condition, detail=""):
    global checks
    checks += 1
    print(f"  {'pass' if condition else 'FAIL'}  {name}" + ("" if condition else f"   {detail}"))
    if not condition:
        failures.append(name)


class Terminal:
    """A pty with a VT model of what was drawn in it."""

    def __init__(self, binary, root, config, answers=True):
        self.answers = answers
        self.screen = pyte.Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.raw = bytearray()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root)
            os.execve(
                binary,
                ["ainz"],
                dict(os.environ, TERM="xterm-256color", AINZ_CONFIG=os.path.join(root, config)),
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
            self.reply(chunk)

    def reply(self, chunk):
        """What a terminal answers: device attributes, and where the cursor is."""
        if not self.answers:
            return
        if b"\x1b[c" in chunk:
            os.write(self.fd, b"\x1b[?1;2c")
        if b"\x1b[6n" in chunk:
            os.write(
                self.fd,
                f"\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R".encode(),
            )

    def send(self, text, settle=0.45):
        os.write(self.fd, text.encode())
        self.pump(settle)

    def body(self):
        return "\n".join(row.rstrip() for row in self.screen.display)

    def prompt(self):
        for row in reversed(self.screen.display):
            if row.strip():
                return row.strip()
        return ""

    def status_row(self):
        for index, row in enumerate(self.screen.display):
            if "fake/test" in row:
                return index
        return None

    def viewport(self):
        start = self.status_row()
        return "" if start is None else "\n".join(r.rstrip() for r in self.screen.display[start:])

    def close(self):
        try:
            os.kill(self.pid, 9)
        except ProcessLookupError:
            pass


def press(column, row):
    """SGR 1006 button-1 press and release at a one-based cell."""
    return f"{ESC}[<0;{column};{row}M{ESC}[<0;{column};{row}m"


def wheel(up, column=20, row=10):
    return f"{ESC}[<{64 if up else 65};{column};{row}M"


def workspace():
    root = tempfile.mkdtemp(prefix="ainz-tui-")
    open(os.path.join(root, "alpha.txt"), "w").write("alpha\n")
    open(os.path.join(root, "beta.txt"), "w").write("beta\n")
    os.mkdir(os.path.join(root, "src"))
    open(os.path.join(root, "src", "main.rs"), "w").write("fn main() {}\n")
    open(os.path.join(root, "config.toml"), "w").write(CONFIG.format(inline="false"))
    open(os.path.join(root, "inline.toml"), "w").write(CONFIG.format(inline="true"))
    return root


def check_prompt(binary, root):
    print("the prompt")
    term = Terminal(binary, root, "config.toml")
    term.pump(6.0)

    term.send("hello world")
    check("typing reaches it", term.prompt().endswith("hello world"), term.prompt())
    term.send(CTRL_A + "X")
    check("ctrl+a goes to the start", "> Xhello world" in term.prompt(), term.prompt())
    term.send(CTRL_E + "!")
    check("ctrl+e goes to the end", term.prompt().endswith("Xhello world!"), term.prompt())
    # the row is padded with spaces, so the trailing space of "Xhello " is not visible
    term.send(CTRL_W)
    check("ctrl+w deletes a word", term.prompt() == "> Xhello", term.prompt())
    term.send(LEFT + LEFT + "Q")
    check("left moves the cursor", term.prompt() == "> XhellQo", term.prompt())
    term.send(ALT_LEFT + "Z")
    check("alt+left moves a word", term.prompt() == "> ZXhellQo", term.prompt())
    term.send(CTRL_U + CTRL_K)
    check("ctrl+u and ctrl+k clear it", term.prompt().strip() in (">", "> "), term.prompt())

    term.send("first prompt" + ENTER, settle=1.4)
    term.send("second prompt" + ENTER, settle=1.4)
    term.send("draft in progress")
    term.send(UP)
    check("up recalls the last prompt", term.prompt().endswith("second prompt"), term.prompt())
    term.send(UP)
    check("up again reaches the older", term.prompt().endswith("first prompt"), term.prompt())
    term.send(DOWN + DOWN)
    check("down returns the draft", term.prompt().endswith("draft in progress"), term.prompt())
    term.send(CTRL_U + CTRL_K)

    term.send("look at @alph")
    check("@ opens a file menu", "alpha.txt" in term.body(), term.body()[-400:])
    term.send(TAB)
    check("tab completes the path", "@alpha.txt" in term.prompt(), term.prompt())
    term.send(CTRL_U + CTRL_K)
    term.send("@src/ma")
    check("@ matches a nested path", "src/main.rs" in term.body(), term.body()[-400:])
    term.send(CTRL_U + CTRL_K)

    term.send("one\\" + ENTER)
    term.send("two")
    check("a trailing backslash makes a newline", "  two" in term.body(), term.body()[-200:])
    term.send(CTRL_U + CTRL_K + "\x08" * 8)

    term.send(ESC, settle=0.3)
    term.send(ESC, settle=0.8)
    check("esc esc puts the last prompt back", term.prompt().endswith("second prompt"), term.prompt())
    check("and says so", "rewound" in term.body(), term.body()[-400:])
    term.send(CTRL_U + CTRL_K)

    term.send("keep me")
    term.send(SHIFT_UP)
    check("shift+up scrolls, not recalls", term.prompt().endswith("keep me"), term.prompt())
    term.send(SHIFT_DOWN + CTRL_U + CTRL_K)

    term.send("mouse test line")
    # a click puts the caret before the character in the cell
    term.send(press(6, ROWS))
    term.send("|")
    check("a click places the cursor", term.prompt() == "> mou|se test line", term.prompt())
    term.send(CTRL_U + CTRL_K)
    term.send(wheel(True) * 3 + wheel(False) * 3)
    check("the wheel leaves the prompt alone", term.prompt().strip() in (">", "> "), term.prompt())
    check("and the transcript survives", "first prompt" in term.body(), term.body()[:200])

    term.send("/hea")
    row = next((i for i, r in enumerate(term.screen.display) if "/headers" in r), None)
    check("the command menu opens", row is not None, term.body()[-400:])
    if row is not None:
        # the menu is over the transcript, to the right of the roster
        term.send(press(40, row + 1))
        check("clicking a suggestion takes it", term.prompt().startswith("> /head"), term.prompt())
    term.send(CTRL_U + CTRL_K)

    term.send("/vim" + ENTER, settle=1.0)
    check("vim mode turns on", "vim keys on" in term.body(), term.body()[-300:])
    term.send("abc def")
    term.send(ESC, settle=0.5)
    check("esc shows normal mode", term.prompt().startswith("▪"), term.prompt())
    term.send("0x")
    check("x deletes under the cursor", term.prompt().endswith("bc def"), term.prompt())
    term.send("dd")
    check("dd clears the line", term.prompt().strip() == "▪", term.prompt())
    term.send("i" + "back to insert")
    check("i returns to insert", term.prompt().startswith(">"), term.prompt())
    term.close()


def check_inline(binary, root):
    print("\ndrawn inline")
    term = Terminal(binary, root, "inline.toml")
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
    check("what was said is on screen", "first inline prompt" in term.body(), term.body()[-400:])
    check("replies too", "a reply from the fake provider" in term.body(), term.body()[-400:])
    check("finished talk is not in the viewport", "first inline prompt" not in term.viewport())
    above = "\n".join(term.screen.display[: term.status_row() or 0])
    check("it is in the scroll above it", "first inline prompt" in above, above[-300:])
    term.send("typing still works")
    check("and the prompt still takes input", term.viewport().rstrip().endswith("typing still works"))
    term.close()

    print("\ndrawn inline where the terminal will not answer")
    term = Terminal(binary, root, "inline.toml", answers=False)
    term.pump(8.0)
    check("it falls back rather than failing to start", "fake/test" in term.body(), term.body()[-300:])
    check("and says why", "inline drawing is not available" in term.body(), term.body()[:600])
    term.send("still usable")
    check("and still takes input", "still usable" in term.body(), term.body()[-200:])
    term.close()


def check_setup(binary, root):
    print("\nprovider setup")
    term = Terminal(binary, root, "config.toml")
    term.pump(6.0)
    term.send("/config" + ENTER, settle=1.5)
    check("the provider list opens", "Claude Code" in term.body(), term.body()[:600])
    check("and offers a custom process", "Custom process" in term.body(), term.body()[:600])
    for _ in range(12):
        if "Executable adapter" in term.body():
            break
        term.send(DOWN, settle=0.2)
    check("the process adapter is reachable", "Executable adapter" in term.body(), term.body()[:600])
    term.send(ENTER, settle=1.2)
    check(
        "its command list holds the agents on this machine",
        "claude" in term.body() or "codex" in term.body(),
        term.body()[:800],
    )
    check("and offers to type another", "Type another" in term.body(), term.body()[:800])
    term.send(ESC, settle=0.5)
    term.close()


class Models(http.server.BaseHTTPRequestHandler):
    """The /models an OpenAI-compatible endpoint serves."""

    def do_GET(self):
        body = json.dumps(
            {"data": [{"id": "zeta-omega-1"}, {"id": "alpha-beta-2"}]}
        ).encode()
        self.send_response(200 if self.path.endswith("/models") else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def check_model_list(binary, root):
    print("\nchoosing a model")
    server = http.server.HTTPServer(("127.0.0.1", 0), Models)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    endpoint = f"http://127.0.0.1:{server.server_address[1]}"

    term = Terminal(binary, root, "config.toml")
    term.pump(6.0)
    term.send("/config" + ENTER, settle=1.5)
    for _ in range(12):
        if "Compatible endpoint" in term.body():
            break
        term.send(DOWN, settle=0.2)
    check("a custom endpoint is offered", "Compatible endpoint" in term.body(), term.body()[:600])
    term.send(ENTER, settle=1.0)

    check("the endpoint list opens", "Type another" in term.body(), term.body()[:600])
    # the last row is "Type another…", and the selection stops there
    term.send(DOWN * 8 + ENTER, settle=0.8)
    term.send(endpoint + ENTER, settle=0.8)
    # no credential for this one: "None" is the first row
    term.send(ENTER, settle=0.8)
    term.send(ENTER, settle=2.5)

    body = term.body()
    check("the endpoint's own models are listed", "zeta-omega-1" in body, body[:800])
    check("all of them", "alpha-beta-2" in body, body[:800])
    check("with room to name another", "Enter another model" in body, body[:800])
    term.send(ESC, settle=0.5)
    term.close()
    server.shutdown()


def check_new_surface(binary, root):
    """The things a session gained after the prompt: rules, an attached image, the plan tool."""
    print("\nthe rest of the session")
    term = Terminal(binary, root, "config.toml")
    term.pump(6.0)

    term.send("/rules" + ENTER, settle=1.0)
    check(
        "rules says there are none yet",
        "no standing rules" in term.body(),
        term.body()[-300:],
    )

    # a pasted image path attaches rather than typing itself into the line
    picture = os.path.join(root, "shot.png")
    open(picture, "wb").write(b"\x89PNG\r\n\x1a\n")
    term.send(f"{ESC}[200~{picture}{ESC}[201~", settle=0.8)
    check("a pasted image attaches", "attached" in term.body(), term.body()[-300:])
    check("and does not land in the line", picture not in term.prompt(), term.prompt())
    check("the prompt shows it", "shot.png" in term.body(), term.body()[-200:])

    # an ordinary paste is still text
    term.send(f"{ESC}[200~just words{ESC}[201~", settle=0.6)
    # the attachment marker sits at the right of the same row, so this is a contains
    check("a text paste still types", "> just words" in term.prompt(), term.prompt())
    term.send(CTRL_U + CTRL_K)

    term.send("/help" + ENTER, settle=1.0)
    body = term.body()
    for command in ["/rules", "/vim", "/inline"]:
        check(f"{command} is in the help", command in body, body[-500:])
    term.close()


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/ainz"
    binary = os.path.abspath(binary)
    if not os.path.isfile(binary):
        sys.exit(f"no ainz binary at {binary}; cargo build first")
    root = workspace()
    try:
        check_prompt(binary, root)
        check_inline(binary, root)
        check_setup(binary, root)
        check_model_list(binary, root)
        check_new_surface(binary, root)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print(f"\n{checks - len(failures)}/{checks} checks passed")
    if failures:
        print("failed:", ", ".join(failures))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
