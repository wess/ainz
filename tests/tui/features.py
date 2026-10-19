#!/usr/bin/env python3
"""Exercise settings, custom keys, and live sessions with a local fixture provider."""
import http.server
import threading
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import tomllib

spec = importlib.util.spec_from_file_location("tui", Path(__file__).parents[2] / "scripts/tui-check.py")
tui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tui)


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if any(message["role"] == "tool" for message in request["messages"]):
            time.sleep(1)
            delta = {"content": "approval finished"}
            finish = "stop"
        else:
            delta = {"tool_calls": [{"index": 0, "id": "write1", "type": "function", "function": {"name": "write", "arguments": json.dumps({"path": "approved.txt", "content": "approved"})}}]}
            finish = "tool_calls"
        body = "data: " + json.dumps({"choices": [{"delta": delta, "finish_reason": finish}]}) + "\n\ndata: [DONE]\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body.encode())


def main():
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ainz")
    root = tui.workspace()
    env = {"HOME": root, "XDG_DATA_HOME": str(Path(root, "data")), "AINZ_CONFIG": str(Path(root, "config.toml"))}
    terminals = []
    server = None

    def terminal(args=None):
        term = tui.Terminal(binary, root, "config.toml", args=args, env=env)
        terminals.append(term)
        term.pump(3)
        return term

    def command(*args):
        return subprocess.run([binary, "--workspace", root, *args], env={**os.environ, **env}, capture_output=True, text=True, timeout=10, check=True).stdout

    def infos():
        return json.loads(command("sessions", "--live", "--json"))

    try:
        term = terminal()
        term.send("/config\r", settle=1)
        tui.check("/config opens unified settings", "Settings" in term.body() and "Provider and model" in term.body(), term.body())
        term.send("/background\r")
        tui.check("settings search finds background sessions", "Background sessions" in term.body(), term.body())
        term.send("\r\x1b", settle=.3)
        term.send("\x1b", settle=1)
        tui.check("background preference persists", "background = true" in Path(root, "config.toml").read_text())
        term.send("/statusline model branch tokens\r")
        tui.check("status fields persist in order", tomllib.loads(Path(root, "config.toml").read_text())["ui"].get("status_line") == ["model", "branch", "tokens"], Path(root, "config.toml").read_text())
        term.send("/statusline\r")
        tui.check("status picker opens", "Space toggles" in term.body(), term.body())
        term.send("\x1b")
        term.send("\x1bOQ", settle=1)
        tui.check("F2 opens settings", "Settings" in term.body(), term.body())
        term.send("/detach\r\r")
        tui.check("shortcut recorder opens", "Record shortcut" in term.body(), term.body())
        term.send("\x1bd\r")
        term.send("\x1b", settle=.3)
        term.send("\x1b", settle=1)
        tui.check("custom detach shortcut persists", tomllib.loads(Path(root, "config.toml").read_text())["ui"].get("keybindings", {}).get("detach") == "alt+d", term.body() + Path(root, "config.toml").read_text())
        term.send("/exit\r", settle=1)
        term.close()

        live = terminal(["--detach"])
        current = None
        for _ in range(20):
            current = next((info for info in infos() if info["state"] == "ready"), None)
            if current:
                break
            live.pump(.25)
        assert current, (infos(), live.body())
        live.send("/rename review\r")
        tui.check("live name updates", any(info["label"] == "review" for info in infos()))
        live.send("\x02d")
        tui.check("old detach shortcut no longer detaches", any(info["attached"] for info in infos()))
        live.send("\x1bd", settle=1)
        tui.check("custom shortcut detaches to shell", "Detached" in live.raw.decode(errors="replace"))
        tui.check("agent remains alive after detach", any(info["id"] == current["id"] and not info["attached"] for info in infos()))
        live.close()

        attached = terminal(["attach", "review"])
        attached.send("hello\r", settle=2)
        tui.check("reattached agent answers", "a reply from the fake provider" in attached.body(), attached.body())
        attached.send("\x02n", settle=3)
        tui.check("new-session chord starts another live session", len([info for info in infos() if info["state"] not in ["finished", "stopped", "interrupted"]]) == 2)
        attached.send("\x02s")
        tui.check("session picker opens", "Ainz sessions" in attached.body(), attached.body())
        attached.send("/review\r\r", settle=1)
        tui.check("picker search returns to named session", "a reply from the fake provider" in attached.body(), attached.body())
        attached.send("/detach\r", settle=1)
        tui.check("slash detach returns to shell", "Detached" in attached.raw.decode(errors="replace"))
        attached.close()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        Path(root, "config.toml").write_text(f'''provider = "approval"
model = "test"
permissions = "ask"
[providers.approval]
kind = "http"
endpoint = "http://127.0.0.1:{server.server_address[1]}/v1"
[ui]
background = true
[memory]
backend = "off"
[synapse]
enabled = false
''')
        approval = terminal(["--initial-prompt", "write the approval fixture"])
        pending = next(info for info in infos() if info["state"] == "needs approval")
        approval.send("\x02d", settle=1)
        tui.check("detach leaves approval pending", any(info["id"] == pending["id"] and info["state"] == "needs approval" and not info["attached"] for info in infos()))
        tui.check("detach does not grant approval", not Path(root, "approved.txt").exists())
        approval.close()
        approval = terminal(["attach", pending["id"]])
        approval.send("y", settle=.1)
        approval.send("\x02d", settle=2)
        tui.check("approval survives reconnect and executes once allowed", Path(root, "approved.txt").read_text() == "approved")
        for _ in range(50):
            if any(info["id"] == pending["id"] and info["state"] == "finished run" for info in infos()):
                break
            time.sleep(.1)
        tui.check("detached completion gets unseen badge", any(info["id"] == pending["id"] and info["unseen"] and info["state"] == "finished run" for info in infos()))
        approval.close()
        attention = terminal(["attach", "review"])
        attention.send("\x02a", settle=1)
        tui.check("attention visits unseen completion", "approval finished" in attention.body(), attention.body())
        tui.check("viewing transcript clears unseen badge", any(info["id"] == pending["id"] and not info["unseen"] for info in infos()))
        attention.send("/detach\r", settle=1)
        attention.close()
        Path(root, "config.toml").write_text('''provider = "slow"
model = "test"
permissions = "auto"
[providers.slow]
kind = "process"
command = "/bin/sh"
args = ["-c", "echo $$ > runningpid; sleep 30; echo done"]
[ui]
background = true
[memory]
backend = "off"
[synapse]
enabled = false
''')
        slow = terminal(["--initial-prompt", "wait"])
        running = next(info for info in infos() if info["state"] == "running")
        child = int(Path(root, "runningpid").read_text())
        command("stop", running["id"])
        for _ in range(30):
            if next(info for info in infos() if info["id"] == running["id"])["state"] == "stopped":
                break
            slow.pump(.1)
        tui.check("stop cancels an active run and ends the host", next(info for info in infos() if info["id"] == running["id"])["state"] == "stopped")
        try:
            os.kill(child, 0)
            alive = True
        except ProcessLookupError:
            alive = False
        tui.check("stop reaps the provider process", not alive)
        slow.close()
        tui.check("live CLI rejects export combination", subprocess.run([binary, "sessions", "--live", "export"], env={**os.environ, **env}, capture_output=True).returncode != 0)
    finally:
        if server:
            server.shutdown()
            server.server_close()
        for term in terminals:
            term.close()
        try:
            Path(root, "config.toml").write_text(tui.CONFIG.format(inline="false"))
            for info in infos():
                if info["state"] not in ["finished", "stopped", "interrupted", "failed"]:
                    command("stop", info["id"])
            for _ in range(30):
                if all(info["state"] in ["finished", "stopped", "interrupted", "failed"] for info in infos()):
                    break
                time.sleep(.2)
        finally:
            shutil.rmtree(root, ignore_errors=True)
    print(f"{tui.checks - len(tui.failures)}/{tui.checks} checks passed")
    return bool(tui.failures)


if __name__ == "__main__":
    sys.exit(main())
