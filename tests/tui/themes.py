"""Exercise palette loading and installed artwork through a live terminal."""

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


def has_color(term, color, background=False):
  return any(
    (cell.bg if background else cell.fg) == color
    for row in term.screen.buffer.values() for cell in row.values()
  )


def main():
  binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ainz")
  root = tui.workspace()
  term = tui.Terminal(binary, root, "config.toml")
  try:
    term.pump(5)
    folder = Path(root, ".ainz/themes")
    folder.mkdir(parents=True, exist_ok=True)
    theme = folder / "custom.toml"
    theme.write_text('[colors]\nbackground = "#14121b"\ntext = "#e3decd"\nbar = "#492958"\n')
    term.send("/theme custom" + tui.ENTER, .7)
    tui.check("new theme discovered while running", "theme custom;" in term.body(), term.body())
    tui.check("theme changes text", has_color(term, "e3decd"))
    tui.check("theme changes background", has_color(term, "14121b", True))
    tui.check("theme changes bars", has_color(term, "492958", True))
    tui.check("theme remembered", 'theme = "custom"' in Path(root, "config.toml").read_text())
    output.capture(term, "themecustom")
    term.close()
    term = tui.Terminal(binary, root, "config.toml")
    term.pump(5)
    tui.check("saved theme restored on launch", has_color(term, "492958", True))
    theme.write_text('[colors]\nbar = "#123456"\n')
    term.send("/theme custom" + tui.ENTER, .7)
    tui.check("same name reloads updated file", has_color(term, "123456", True))
    theme.write_text('[colors]\nbar = "broken"\n')
    term.send("/theme custom" + tui.ENTER, .7)
    tui.check("invalid reload keeps current palette", has_color(term, "123456", True))
    term.send("/themes" + tui.ENTER, .7)
    body = term.body()
    tui.check("invalid file reported",
      all(part in body for part in ["invalid:", "custom.toml", "#RRGGBB"]), body)
    term.send("/header mascot" + tui.ENTER, .7)
    tui.check("mascot keeps original crimson", has_color(term, "de3040") or has_color(term, "de3040", True))
    term.send("x")
    term.send(tui.CTRL_U + "/theme default" + tui.ENTER, .7)
    tui.check("default resets bars", has_color(term, "184280", True))
    tui.check("default selection saved", 'theme = "default"' in Path(root, "config.toml").read_text())
    headers = Path(root, ".ainz/headers")
    headers.mkdir(parents=True, exist_ok=True)
    (headers / "fresh.txt").write_text("FRESH ARTWORK")
    term.send("/header fresh" + tui.ENTER, .7)
    tui.check("new header discovered without restart", "FRESH ARTWORK" in term.body(), term.body())
  finally:
    term.close()
    shutil.rmtree(root, ignore_errors=True)
  print(f"{tui.checks - len(tui.failures)}/{tui.checks} checks passed")
  return int(bool(tui.failures))


if __name__ == "__main__":
  sys.exit(main())
