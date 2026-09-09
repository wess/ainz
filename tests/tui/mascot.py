"""Verify mascot selection and preview without clearing a live conversation."""

import importlib.util
import os
from pathlib import Path
import shutil
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("output", Path(__file__).with_name("output.py"))
output = importlib.util.module_from_spec(spec)
spec.loader.exec_module(output)
tui = output.tui


def run(binary, columns, rows):
  tui.COLS, tui.ROWS = columns, rows
  root = tui.workspace()
  term = tui.Terminal(binary, root, "config.toml")
  try:
    term.pump(6)
    term.send("keep this conversation" + tui.ENTER, 1)
    term.send("/header mascot" + tui.ENTER, .7)
    tui.check("mascot previews immediately", "A I N Z" in term.body(), term.body())
    tui.check("preview is explicit", "Header preview" in term.body(), term.body())
    tui.check("selection is remembered", 'header = "mascot"' in Path(root, "config.toml").read_text())
    output.capture(term, f"mascot{columns}x{rows}")
    term.send("x")
    tui.check("typing returns to the conversation", "keep this conversation" in term.body(), term.body())
    tui.check("the dismissing key is kept", term.prompt().endswith("x"), term.prompt())
    term.send(tui.CTRL_U + "/clear" + tui.ENTER, .7)
    tui.check("empty buffer keeps the mascot", "A I N Z" in term.body(), term.body())
    term.send("/header ainz" + tui.ENTER, .7)
    tui.check("old name still selects the mascot", "A I N Z" in term.body(), term.body())
  finally:
    term.close()
    shutil.rmtree(root, ignore_errors=True)


def main():
  binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ainz")
  for columns, rows in [(110, 40), (80, 24), (52, 20)]:
    run(binary, columns, rows)
  print(f"{tui.checks - len(tui.failures)}/{tui.checks} checks passed")
  return int(bool(tui.failures))


if __name__ == "__main__":
  sys.exit(main())
