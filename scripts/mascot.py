"""Regenerate the ANSI mascots from grids drawn for their display size."""

from pathlib import Path

palette = {
  "k": (15, 18, 25), "d": (36, 27, 48), "p": (73, 43, 97),
  "v": (130, 78, 158), "g": (177, 127, 54), "G": (234, 192, 100),
  "w": (241, 229, 197), "s": (193, 179, 151),
  "r": (139, 29, 46), "R": (222, 48, 64), "H": (255, 124, 120),
}


def canvas(size):
  return [[" "] * size for _ in range(size)]


def rect(grid, x, y, width, height, color):
  for row in range(y, y + height):
    for column in range(x, x + width):
      grid[row][column] = color


def compact():
  grid = canvas(28)
  def box(x, y, width, height, color):
    rect(grid, x, y, width, height, color)

  # stepped hood; one outline separates every material
  for x, y, w, h, color in [
    (8, 1, 12, 1, "k"), (6, 2, 16, 2, "k"), (5, 4, 18, 12, "k"),
    (6, 16, 16, 2, "k"), (8, 3, 12, 1, "v"), (7, 4, 14, 12, "p"),
    (6, 5, 2, 9, "v"), (20, 5, 2, 9, "d"),
    (8, 5, 1, 9, "G"), (19, 5, 1, 9, "g"),
    (9, 4, 10, 1, "k"), (9, 5, 10, 2, "w"), (8, 7, 12, 5, "w"),
    (9, 12, 10, 2, "w"), (10, 14, 8, 3, "k"),
    (10, 15, 8, 1, "w"), (11, 16, 6, 1, "s"),
  ]:
    box(x, y, w, h, color)

  # two equal sockets, a small nose, and three separated teeth
  box(9, 8, 4, 3, "k")
  box(15, 8, 4, 3, "k")
  box(10, 9, 1, 1, "R")
  box(17, 9, 1, 1, "R")
  box(13, 11, 2, 2, "k")
  box(11, 14, 1, 1, "w")
  box(13, 14, 2, 1, "w")
  box(16, 14, 1, 1, "w")

  # short robes and a black neck gap keep the jaw clear of the collar
  box(7, 18, 14, 8, "k")
  box(5, 21, 18, 6, "k")
  box(6, 22, 16, 4, "p")
  box(7, 21, 2, 4, "v")
  box(19, 21, 2, 4, "d")
  box(9, 20, 10, 6, "d")
  box(8, 20, 1, 6, "G")
  box(19, 20, 1, 6, "g")
  box(8, 26, 12, 1, "g")
  box(9, 19, 10, 1, "w")
  box(11, 20, 6, 1, "G")
  box(11, 22, 6, 1, "s")
  box(13, 21, 2, 3, "w")
  box(12, 24, 4, 2, "r")
  box(13, 24, 2, 1, "R")

  for x in (2, 20):
    box(x+1, 17, 4, 1, "k")
    box(x, 18, 6, 4, "k")
    box(x+1, 18, 4, 3, "r")
    box(x+1, 18, 3, 2, "R")
    box(x+1, 18, 1, 1, "H")
    box(x+1, 21, 4, 1, "g")
  return grid


def tiny():
  grid = canvas(20)
  def box(x, y, width, height, color):
    rect(grid, x, y, width, height, color)

  for x, y, w, h, color in [
    (6, 0, 8, 1, "k"), (4, 1, 12, 2, "k"), (3, 3, 14, 8, "k"),
    (4, 2, 12, 8, "p"), (4, 3, 1, 6, "v"), (5, 3, 1, 6, "G"),
    (14, 3, 1, 6, "g"), (6, 2, 8, 1, "k"), (6, 3, 8, 6, "w"),
    (7, 9, 6, 2, "k"), (7, 10, 6, 1, "w"),
    (6, 5, 3, 3, "k"), (11, 5, 3, 3, "k"),
    (7, 6, 1, 1, "R"), (12, 6, 1, 1, "R"),
    (9, 8, 2, 1, "k"), (8, 9, 1, 1, "w"), (11, 9, 1, 1, "w"),
    (5, 12, 10, 7, "k"), (3, 15, 14, 5, "k"),
    (4, 15, 12, 4, "p"), (7, 14, 6, 5, "d"),
    (6, 14, 1, 5, "G"), (13, 14, 1, 5, "g"),
    (7, 12, 6, 1, "w"), (8, 13, 4, 1, "G"),
    (9, 15, 2, 1, "w"), (8, 17, 4, 2, "r"), (9, 17, 2, 1, "R"),
  ]:
    box(x, y, w, h, color)
  for x in (1, 15):
    box(x, 12, 4, 4, "k")
    box(x+1, 12, 2, 3, "r")
    box(x+1, 12, 2, 2, "R")
    box(x+1, 12, 1, 1, "H")
  return grid


# combine two square pixels into each terminal cell
def encode(canvas):
  height, width = len(canvas), len(canvas[0])
  lines = []
  for y in range(0, height, 2):
    line, previous = "", None
    for x in range(width):
      top, bottom = canvas[y][x], canvas[y+1][x]
      if top == bottom == " ":
        color, glyph = "\x1b[0m", " "
      elif top == " ":
        color, glyph = "\x1b[0;38;2;%d;%d;%dm" % palette[bottom], "▄"
      elif bottom == " ":
        color, glyph = "\x1b[0;38;2;%d;%d;%dm" % palette[top], "▀"
      else:
        color = "\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm" % (palette[top] + palette[bottom])
        glyph = "▀"
      if color != previous:
        line += color
        previous = color
      line += glyph
    lines.append(line + "\x1b[0m")
  lines.extend(["", "\x1b[38;2;239;228;199;1m" + "A I N Z".center(width) + "\x1b[0m"])
  return "\n".join(lines) + "\n"

# the large sprite uses exact integer enlargement; small sprites are never resampled
small = compact()
large = [[pixel for pixel in row for _ in range(2)] for row in small for _ in range(2)]
for name, grid in [("ainz", large), ("compact", small), ("tiny", tiny())]:
  Path(f"assets/mascot/{name}.ans").write_text(encode(grid))

# ship the same compact drawing as a web preview and an editable studio starter
web = Path("site/assets")
web.mkdir(parents=True, exist_ok=True)
web.joinpath("mascot.ans").write_text("\n".join(encode(small).splitlines()[:-2]) + "\n")
paths = []
for color, rgb in palette.items():
  cells = " ".join(f"M{x} {y}h1v1h-1z" for y, row in enumerate(small)
                   for x, pixel in enumerate(row) if pixel == color)
  if cells:
    fill = "#" + "".join(f"{value:02x}" for value in rgb)
    paths.append(f'<path fill="{fill}" d="{cells}"/>')
web.joinpath("mascot.svg").write_text(
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 28 28" '
  'shape-rendering="crispEdges"><title>Ainz mascot</title>' + "".join(paths) + '</svg>\n'
)
