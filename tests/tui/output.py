"""Check output lifecycle in a real pty with a gated local provider.

Run after cargo build: python3 tests/tui/output.py [path/to/ainz]
Set AINZ_CAPTURE_DIR to save terminal frames as PNGs (requires Pillow).
"""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys

sys.dont_write_bytecode = True

repo = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("tuicheck", repo / "scripts/tui-check.py")
tui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tui)

PROVIDER = '''import json, pathlib, sys, time
sys.stdin.read()
def wait(name):
    while not pathlib.Path(name).exists():
        time.sleep(.02)
def emit(value):
    print(json.dumps(value), flush=True)
def text(value):
    emit({"type":"stream_event", "event":{"type":"content_block_delta", "delta":{"type":"text_delta", "text":value}}})
wait("start")
text("Checking the output buffer.")
emit({"type":"assistant", "message":{"content":[{"type":"tool_use", "id":"t1", "name":"shell", "input":{"command":"check output"}}]}})
wait("toolend")
emit({"type":"user", "message":{"content":[{"type":"tool_result", "tool_use_id":"t1", "content":"first line\\nsecond line\\nthird line\\nfourth line\\nlast line"}]}})
text("## Output is clear\\n\\n**Result:** each run has a visible ending.\\n\\n- Tool activity stays separate.\\n- `Ctrl+O` expands the output.\\n\\n```rust\\nlet complete = true;\\n```\\nReady for the next prompt.")
wait("finish")
emit({"type":"result", "is_error":pathlib.Path("fail").exists(), "result":"provider failed" if pathlib.Path("fail").exists() else "done"})
'''


def capture(term, name):
    folder = os.environ.get("AINZ_CAPTURE_DIR")
    if not folder:
        return
    from PIL import Image, ImageDraw, ImageFont
    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 16)
    bold = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 16, index=1)
    width, height = 10, 22
    image = Image.new("RGB", (tui.COLS * width + 24, tui.ROWS * height + 24), "#101820")
    draw = ImageDraw.Draw(image)
    colors = {"default": "#dadee2", "white": "#ffffff", "black": "#101820"}
    def color(value):
        return colors.get(value, "#" + value if len(value) == 6 else "#dadee2")
    for y in range(tui.ROWS):
        for x in range(tui.COLS):
            cell = term.screen.buffer[y][x]
            pos = (12 + x * width, 12 + y * height)
            if cell.bg != "default":
                draw.rectangle((*pos, pos[0] + width, pos[1] + height), fill=color(cell.bg))
            draw.text(pos, cell.data, font=bold if cell.bold else font, fill=color(cell.fg))
    Path(folder).mkdir(parents=True, exist_ok=True)
    image.save(Path(folder) / f"{name}.png")


def activity(term):
    row = term.status_row()
    return term.screen.display[row - 1 if row is not None else -3]


def run(binary, inline=False, failure=False, cancel=False, narrow=False):
    tui.COLS = 52 if narrow else 110
    root = tui.workspace()
    term = None
    try:
        Path(root, "provider.py").write_text(PROVIDER)
        config = tui.CONFIG.format(inline=str(inline).lower()).replace('command = "/bin/echo"', f"command = {json.dumps(sys.executable)}")
        config = config.replace('args = ["a reply from the fake provider"]', 'args = ["provider.py"]\noutput = "stream_json"')
        Path(root, "output.toml").write_text(config)
        if failure:
            Path(root, "fail").touch()
        term = tui.Terminal(binary, root, "output.toml")
        term.pump(6)
        term.send("improve the output" + tui.ENTER, .7)
        tui.check("quiet provider has visible working state", "Working" in term.body() and "Completed" not in term.body(), term.body())
        if not inline:
            tui.check("channel topic shows the task", "#main · improve the output" in term.screen.display[0], term.screen.display[0])
        Path(root, "start").touch()
        term.pump(.8)
        tui.check("user message stays on the nick line", "<you> improve the output" in term.body(), term.body())
        tui.check("reply starts on the nick line", "<Ainz> Checking" in term.body(), term.body())
        tui.check("tool is visibly running", "Running tools" in term.body() and "check output" in term.body(), term.body())
        if not inline:
            tui.check("channel topic follows the tool", "shell check output" in term.screen.display[0], term.screen.display[0])
        name = "inline" if inline else "narrow" if narrow else "full"
        capture(term, name + "running")
        if cancel:
            term.send("\x03", 1.2)
            tui.check("cancellation is explicit", "Cancelled" in term.body() and "Completed" not in term.body(), term.body())
            tui.check("interrupted tool stops running", "stopped before" in term.body() and "Running tools" not in term.body(), term.body())
            capture(term, name + "cancelled")
            return
        Path(root, "toolend").touch()
        term.pump(.9)
        tui.check("answer remains active until process exits", "Responding" in term.body() and "Completed" not in term.body(), term.body())
        if inline:
            tui.check("inline tool reaches scrollback with its result", "first line" in term.body() and "Ctrl+O expand" in term.body(), term.body())
            tui.check("inline never freezes the running tool in scrollback", "▸" not in term.body(), term.body())
        Path(root, "finish").touch()
        term.pump(1.2)
        label = "Failed" if failure else "Completed"
        tui.check("final state remains above prompt", label in activity(term), term.body())
        if not inline:
            tui.check("finished channel retains its task", "#main · improve the output" in term.screen.display[0] and "shell" not in term.screen.display[0], term.screen.display[0])
        tui.check("transcript keeps an end marker", term.body().count(label) >= 2, term.body())
        if failure:
            tui.check("failure never claims completion", "Completed" not in term.body(), term.body())
        else:
            tui.check("answer is formatted", "## Output" not in term.body() and "**Result:**" not in term.body(), term.body())
        capture(term, name + label.lower())
        term.send("next question")
        tui.check("typing preserves completion state", label in activity(term), term.body())
        if not inline:
            term.send("\x0f")
            tui.check("expand reveals the remaining tool output", "fourth line" in term.body(), term.body())
    finally:
        if term:
            term.close()
        shutil.rmtree(root, ignore_errors=True)


def control(binary):
    tui.COLS = 110
    root = tui.workspace()
    term = None
    try:
        Path(root, "provider.py").write_text(PROVIDER)
        config = tui.CONFIG.format(inline="false").replace('command = "/bin/echo"', f"command = {json.dumps(sys.executable)}")
        config = config.replace('args = ["a reply from the fake provider"]', 'args = ["provider.py"]\noutput = "stream_json"')
        Path(root, "output.toml").write_text(config)
        term = tui.Terminal(binary, root, "output.toml")
        term.pump(2)
        term.send("begin" + tui.ENTER, .5)
        term.send("".join(f"note {i}\r" for i in range(32)) + "keep this draft\r", 2)
        tui.check("full steering queue keeps the rejected draft", "keep this draft" in term.prompt(), term.body())
        tui.check("full steering queue reports rejection", "steering not queued" in term.body(), term.body())
        capture(term, "steeringfull")
        term.send("\x03", .7)
        tui.check("cancellation bypasses the full steering queue", "Cancelled" in activity(term), term.body())
        tui.check("cancellation preserves the rejected draft", "keep this draft" in term.prompt(), term.body())
    finally:
        if term:
            term.close()
        shutil.rmtree(root, ignore_errors=True)


def hooks(binary):
    tui.COLS = 110
    root = tui.workspace()
    term = None
    try:
        Path(root, "hook.py").write_text(
            'import sys, pathlib, time\nsys.stdin.read()\npathlib.Path("hookready").touch()\n'
            'while not pathlib.Path("release").exists(): time.sleep(.02)\n'
            'pathlib.Path("survived").touch()\n'
        )
        config = tui.CONFIG.format(inline="false")
        config += f'\n[hooks]\nsession_end = [{{command = [{json.dumps(sys.executable)}, "hook.py"]}}]\n'
        Path(root, "output.toml").write_text(config)
        term = tui.Terminal(binary, root, "output.toml")
        term.pump(2)
        term.send("begin" + tui.ENTER, .7)
        tui.check("end hook starts in the workspace", Path(root, "hookready").exists(), term.body())
        tui.check("completion waits for the end hook", "Completed" not in term.body() and "Responding" in activity(term), term.body())
        capture(term, "endhookwaiting")
        term.send("next request" + tui.ENTER, .3)
        tui.check("finishing run rejects steering without losing the draft", "next request" in term.prompt() and "steering not queued" in term.body(), term.body())
        term.send("\x03", .7)
        tui.check("end hook cancellation is visible", "Cancelled" in activity(term), term.body())
        capture(term, "endhookcancelled")
        Path(root, "release").touch()
        term.pump(.3)
        tui.check("cancelled end hook does not keep running", not Path(root, "survived").exists())
    finally:
        if term:
            term.close()
        shutil.rmtree(root, ignore_errors=True)


def main():
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else repo / "target/debug/ainz")
    run(binary)
    run(binary, inline=True)
    run(binary, cancel=True)
    run(binary, failure=True)
    run(binary, narrow=True)
    control(binary)
    hooks(binary)
    print(f"{tui.checks - len(tui.failures)}/{tui.checks} checks passed")
    return int(bool(tui.failures))


if __name__ == "__main__":
    sys.exit(main())
