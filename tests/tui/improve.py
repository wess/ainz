#!/usr/bin/env python3
"""Check the improvement switch and idle result delivery in an owned terminal."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tomllib
import uuid

spec = importlib.util.spec_from_file_location("tui", Path(__file__).parents[2] / "scripts/tui-check.py")
tui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tui)


def main():
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ainz")
    root = tui.workspace()
    env = {"HOME": root, "XDG_DATA_HOME": str(Path(root, "data"))}
    term = tui.Terminal(binary, root, "config.toml", env=env)
    try:
        term.pump(3)
        term.send("/settings\r", settle=1)
        term.send("/automatic improvement\r")
        tui.check("settings finds the automatic improvement switch", "Automatic improvement" in term.body(), term.body())
        term.send("\r\x1b", settle=.3)
        term.send("\x1b", settle=1)
        config = tomllib.loads(Path(root, "config.toml").read_text())
        tui.check("one switch enables background improvement", config["improve"]["enabled"])

        data = Path(root, "Library/Application Support") if sys.platform == "darwin" else Path(root, "data")
        automatic = data / "ainz/improvements/automatic"
        state = {"projects": {str(Path(root).resolve()): {"latest": {
            "id": str(uuid.uuid4()), "experiment": None, "error": "fixture drift prevented",
        }}}, "active": None}
        pending = automatic / "fixture.tmp"
        pending.write_text(json.dumps(state))
        os.replace(pending, automatic / "state.json")
        term.pump(1.5)
        tui.check("a background result reaches an idle conversation", "fixture drift prevented" in term.body(), term.body())
        term.pump(.5)
        tui.check("the result notice is not duplicated", term.body().count("fixture drift prevented") == 1, term.body())
        term.send("/improve off\r", settle=1)
        config = tomllib.loads(Path(root, "config.toml").read_text())
        tui.check("the conversation can disable automatic improvement", not config["improve"]["enabled"] and "automatic improvement: off" in term.body(), term.body())
        term.send("/exit\r", settle=.5)
    finally:
        term.close()
        shutil.rmtree(root, ignore_errors=True)
    print(f"{tui.checks - len(tui.failures)}/{tui.checks} checks passed")
    return bool(tui.failures)


if __name__ == "__main__":
    sys.exit(main())
