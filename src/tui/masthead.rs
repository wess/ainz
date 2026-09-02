// the ten pixel mastheads shown on an empty transcript. each is drawn on a half-block canvas
// from one theme plus a scene painter, so the letterforms are shared and only the dressing varies

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
  style::{Color, Style},
  text::{Line, Span},
};

pub(super) const VARIANTS: usize = 10;

// per-launch pick seeded from the clock and pid; no rng dependency for a splash
pub(super) fn select_index(count: usize) -> usize {
  let seed = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
    ^ u128::from(std::process::id());
  (seed % count.max(1) as u128) as usize
}

#[derive(Clone, Copy)]
struct PixelTheme {
  face_top: Color,
  face_bottom: Color,
  highlight: Color,
  outline: Color,
  outline_dark: Color,
  shadow: Color,
  shadow_deep: Color,
}

pub(super) fn render(width: usize, variant: usize) -> Vec<Line<'static>> {
  const LETTERS: [[&str; 7]; 6] = [
    [
      "01110", "10001", "10001", "11111", "10001", "10001", "10001",
    ],
    [
      "01110", "10001", "10000", "10111", "10001", "10001", "01110",
    ],
    [
      "11111", "10000", "10000", "11110", "10000", "10000", "11111",
    ],
    [
      "10001", "11001", "11001", "10101", "10011", "10011", "10001",
    ],
    [
      "11111", "00100", "00100", "00100", "00100", "00100", "00100",
    ],
    [
      "10001", "10001", "01010", "00100", "01010", "10001", "10001",
    ],
  ];
  const THEMES: [PixelTheme; 10] = [
    PixelTheme {
      face_top: Color::Rgb(226, 190, 48),
      face_bottom: Color::Rgb(211, 126, 29),
      highlight: Color::Rgb(255, 232, 111),
      outline: Color::Rgb(62, 188, 221),
      outline_dark: Color::Rgb(24, 44, 52),
      shadow: Color::Rgb(54, 57, 61),
      shadow_deep: Color::Rgb(27, 29, 32),
    },
    PixelTheme {
      face_top: Color::Rgb(224, 230, 229),
      face_bottom: Color::Rgb(105, 116, 122),
      highlight: Color::Rgb(255, 255, 247),
      outline: Color::Rgb(79, 224, 214),
      outline_dark: Color::Rgb(29, 42, 48),
      shadow: Color::Rgb(67, 72, 78),
      shadow_deep: Color::Rgb(25, 28, 34),
    },
    PixelTheme {
      face_top: Color::Rgb(239, 196, 49),
      face_bottom: Color::Rgb(222, 118, 31),
      highlight: Color::Rgb(255, 238, 132),
      outline: Color::Rgb(73, 209, 224),
      outline_dark: Color::Rgb(34, 31, 48),
      shadow: Color::Rgb(91, 62, 105),
      shadow_deep: Color::Rgb(31, 27, 40),
    },
    PixelTheme {
      face_top: Color::Rgb(249, 92, 38),
      face_bottom: Color::Rgb(160, 24, 28),
      highlight: Color::Rgb(255, 211, 61),
      outline: Color::Rgb(255, 142, 28),
      outline_dark: Color::Rgb(65, 17, 25),
      shadow: Color::Rgb(91, 25, 38),
      shadow_deep: Color::Rgb(31, 13, 22),
    },
    PixelTheme {
      face_top: Color::Rgb(137, 237, 52),
      face_bottom: Color::Rgb(39, 143, 69),
      highlight: Color::Rgb(222, 255, 96),
      outline: Color::Rgb(188, 55, 217),
      outline_dark: Color::Rgb(39, 21, 52),
      shadow: Color::Rgb(67, 34, 83),
      shadow_deep: Color::Rgb(20, 17, 30),
    },
    PixelTheme {
      face_top: Color::Rgb(208, 251, 255),
      face_bottom: Color::Rgb(78, 169, 224),
      highlight: Color::Rgb(255, 255, 255),
      outline: Color::Rgb(45, 116, 222),
      outline_dark: Color::Rgb(22, 38, 72),
      shadow: Color::Rgb(38, 73, 127),
      shadow_deep: Color::Rgb(16, 25, 48),
    },
    PixelTheme {
      face_top: Color::Rgb(255, 86, 209),
      face_bottom: Color::Rgb(137, 55, 198),
      highlight: Color::Rgb(255, 191, 239),
      outline: Color::Rgb(54, 231, 225),
      outline_dark: Color::Rgb(38, 20, 68),
      shadow: Color::Rgb(51, 49, 122),
      shadow_deep: Color::Rgb(20, 18, 44),
    },
    PixelTheme {
      face_top: Color::Rgb(246, 199, 53),
      face_bottom: Color::Rgb(176, 111, 22),
      highlight: Color::Rgb(255, 238, 145),
      outline: Color::Rgb(116, 123, 127),
      outline_dark: Color::Rgb(34, 37, 39),
      shadow: Color::Rgb(62, 66, 68),
      shadow_deep: Color::Rgb(21, 23, 24),
    },
    PixelTheme {
      face_top: Color::Rgb(66, 221, 190),
      face_bottom: Color::Rgb(26, 111, 156),
      highlight: Color::Rgb(166, 255, 228),
      outline: Color::Rgb(62, 94, 222),
      outline_dark: Color::Rgb(17, 31, 65),
      shadow: Color::Rgb(30, 56, 105),
      shadow_deep: Color::Rgb(12, 21, 43),
    },
    PixelTheme {
      face_top: Color::Rgb(211, 207, 197),
      face_bottom: Color::Rgb(102, 99, 96),
      highlight: Color::Rgb(255, 250, 232),
      outline: Color::Rgb(196, 46, 54),
      outline_dark: Color::Rgb(41, 28, 30),
      shadow: Color::Rgb(62, 58, 58),
      shadow_deep: Color::Rgb(19, 18, 19),
    },
  ];

  fn put(canvas: &mut [Vec<Option<Color>>], x: isize, y: isize, color: Color) {
    if x >= 0
      && y >= 0
      && let Some(row) = canvas.get_mut(y as usize)
      && let Some(pixel) = row.get_mut(x as usize)
    {
      *pixel = Some(color);
    }
  }

  fn face_color(
    theme: PixelTheme,
    variant: usize,
    letter: usize,
    x: usize,
    y: usize,
    height: usize,
  ) -> Color {
    if (x * 17 + y * 29 + letter * 11).is_multiple_of(19) {
      return theme.highlight;
    }
    match variant {
      1 if letter == 3 || (x + y + letter).is_multiple_of(17) => {
        return Color::Rgb(67, 213, 230);
      }
      1 if (x * 3 + y + letter).is_multiple_of(23) => {
        return Color::Rgb(119, 239, 83);
      }
      4 if (x + y * 2 + letter).is_multiple_of(13) => {
        return Color::Rgb(208, 66, 221);
      }
      5 if (x * 2 + y + letter).is_multiple_of(11) => {
        return Color::Rgb(116, 237, 255);
      }
      6 if letter.is_multiple_of(2) && (x + y).is_multiple_of(7) => {
        return Color::Rgb(61, 223, 228);
      }
      7 if (x + y + letter).is_multiple_of(9) => {
        return Color::Rgb(75, 78, 78);
      }
      8 if (x * 3 + y + letter).is_multiple_of(11) => {
        return Color::Rgb(58, 102, 226);
      }
      9 if (x + y * 3 + letter).is_multiple_of(17) => {
        return Color::Rgb(205, 47, 57);
      }
      _ => {}
    }
    if y < height / 2 {
      theme.face_top
    } else {
      theme.face_bottom
    }
  }

  let variant = variant % 10;
  let theme = THEMES[variant];
  let scale = if width >= 78 { 2 } else { 1 };
  let letter_width = 5 * scale;
  let gap = 1;
  let face_width = LETTERS.len() * letter_width + (LETTERS.len() - 1) * gap;
  let face_height = 7 * scale;
  let depth = if scale == 2 { 3 } else { 2 };
  let margin = depth + 2;
  let top = if scale == 2 { 6 } else { 4 };
  let canvas_width = face_width + margin * 2;
  let canvas_height = top + face_height + depth + if scale == 2 { 5 } else { 3 };
  let mut canvas = vec![vec![None; canvas_width]; canvas_height];
  let mut face = Vec::new();

  for (letter, pattern) in LETTERS.iter().enumerate() {
    let letter_x = margin + letter * (letter_width + gap);
    let letter_y = match variant {
      2 | 8 => [1, 0, 2, 0, 1, 0][letter] * scale / 2,
      4 => [0, 1, 2, 1, 0, 1][letter] * scale / 2,
      9 => [1, 0, 1, 0, 1, 0][letter] * scale / 2,
      _ => 0,
    };
    for (pattern_y, row) in pattern.iter().enumerate() {
      for (pattern_x, bit) in row.bytes().enumerate() {
        if bit != b'1' {
          continue;
        }
        for dy in 0..scale {
          for dx in 0..scale {
            let slant = match variant {
              1 | 6 => (6 - pattern_y) * scale / 4,
              3 => pattern_y * scale / 6,
              _ => 0,
            };
            face.push((
              (letter_x + pattern_x * scale + dx + slant) as isize,
              (top + letter_y + pattern_y * scale + dy) as isize,
              letter,
            ));
          }
        }
      }
    }
  }

  for &(x, y, _) in &face {
    for step in 0..=depth as isize {
      for oy in -1..=1 {
        for ox in -1..=1 {
          let direction = if matches!(variant, 3 | 6 | 9) { -1 } else { 1 };
          put(
            &mut canvas,
            x + step * direction + ox,
            y + step + oy,
            theme.outline,
          );
        }
      }
    }
  }

  for &(x, y, _) in &face {
    for step in 1..=depth as isize {
      let direction = if matches!(variant, 3 | 6 | 9) { -1 } else { 1 };
      put(
        &mut canvas,
        x + step * direction,
        y + step,
        if step == depth as isize {
          theme.shadow_deep
        } else {
          theme.shadow
        },
      );
    }
  }

  for &(x, y, _) in &face {
    for oy in -1..=1 {
      for ox in -1..=1 {
        put(&mut canvas, x + ox, y + oy, theme.outline_dark);
      }
    }
  }

  for &(x, y, letter) in &face {
    let color = face_color(
      theme,
      variant,
      letter,
      x as usize,
      y.saturating_sub(top as isize) as usize,
      face_height,
    );
    put(&mut canvas, x, y, color);
  }

  if matches!(variant, 0 | 2 | 4 | 5 | 6 | 8) {
    let drip_columns = [1, 8, 15, 24, 31];
    for (index, column) in drip_columns.into_iter().enumerate() {
      let x = margin + column * scale;
      let start = top + face_height + depth;
      let length = 1 + (index * 2 + variant) % if scale == 2 { 5 } else { 3 };
      for dy in 0..length {
        put(
          &mut canvas,
          x as isize,
          (start + dy) as isize,
          if dy + 1 == length {
            theme.highlight
          } else {
            theme.outline
          },
        );
        if scale == 2 && dy < 2 {
          put(
            &mut canvas,
            x as isize + 1,
            (start + dy) as isize,
            theme.shadow,
          );
        }
      }
    }
  }

  match variant {
    0 => paint_flame(&mut canvas, canvas_width / 2, theme.outline_dark),
    1 => paint_shards(&mut canvas, canvas_width, theme),
    2 => paint_sparks(&mut canvas, canvas_width, theme),
    3 => paint_inferno(&mut canvas, canvas_width, theme),
    4 => paint_toxic(&mut canvas, canvas_width, theme),
    5 => paint_ice(&mut canvas, canvas_width, theme),
    6 => paint_orbit(&mut canvas, canvas_width, theme),
    7 => paint_industrial(&mut canvas, canvas_width, theme),
    8 => paint_abyss(&mut canvas, canvas_width, theme),
    _ => paint_skull(&mut canvas, canvas_width, theme),
  }

  let left_padding = width.saturating_sub(canvas_width) / 2;
  let mut lines = vec![Line::raw("")];
  for rows in canvas.chunks(2) {
    let top_row = &rows[0];
    let bottom_row = rows.get(1);
    let mut spans = vec![Span::raw(" ".repeat(left_padding))];
    for x in 0..canvas_width {
      let upper = top_row[x];
      let lower = bottom_row.and_then(|row| row[x]);
      spans.push(match (upper, lower) {
        (Some(fg), Some(bg)) => Span::styled("▀", Style::default().fg(fg).bg(bg)),
        (Some(fg), None) => Span::styled("▀", Style::default().fg(fg)),
        (None, Some(fg)) => Span::styled("▄", Style::default().fg(fg)),
        (None, None) => Span::raw(" "),
      });
    }
    lines.push(Line::from(spans));
  }
  lines
}

fn paint_flame(canvas: &mut [Vec<Option<Color>>], center: usize, edge: Color) {
  const FLAME: [&str; 5] = ["..r....", ".rr..r.", ".rorrr.", "..ryr..", "...r..."];
  let left = center.saturating_sub(FLAME[0].len() / 2);
  for (y, row) in FLAME.iter().enumerate() {
    for (x, pixel) in row.chars().enumerate() {
      if pixel == '.' {
        continue;
      }
      for oy in -1..=1 {
        for ox in -1..=1 {
          let px = (left + x) as isize + ox;
          let py = y as isize + oy;
          if px >= 0
            && py >= 0
            && let Some(line) = canvas.get_mut(py as usize)
            && let Some(cell) = line.get_mut(px as usize)
          {
            *cell = Some(edge);
          }
        }
      }
    }
  }
  for (y, row) in FLAME.iter().enumerate() {
    for (x, pixel) in row.chars().enumerate() {
      let color = match pixel {
        'r' => Some(Color::Rgb(210, 76, 72)),
        'o' => Some(Color::Rgb(244, 120, 31)),
        'y' => Some(Color::Rgb(255, 213, 73)),
        _ => None,
      };
      if let Some(color) = color
        && let Some(line) = canvas.get_mut(y)
        && let Some(cell) = line.get_mut(left + x)
      {
        *cell = Some(color);
      }
    }
  }
}

fn paint_shards(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let shards = [
    (width / 5, 1, 3),
    (width / 3, 0, 4),
    (width * 2 / 3, 1, 3),
    (width * 4 / 5, 0, 4),
  ];
  for (x, y, length) in shards {
    for step in 0..length {
      if let Some(row) = canvas.get_mut(y + step)
        && let Some(cell) = row.get_mut(x + step / 2)
      {
        *cell = Some(if step == 0 {
          theme.highlight
        } else {
          theme.outline
        });
      }
    }
  }
}

fn paint_sparks(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let sparks = [
    (width / 7, 2),
    (width / 4, 0),
    (width / 2, 2),
    (width * 3 / 4, 1),
    (width * 6 / 7, 3),
  ];
  for (index, (x, y)) in sparks.into_iter().enumerate() {
    if let Some(row) = canvas.get_mut(y)
      && let Some(cell) = row.get_mut(x)
    {
      *cell = Some(if index.is_multiple_of(2) {
        theme.highlight
      } else {
        theme.outline
      });
    }
  }
}

fn scene_put(canvas: &mut [Vec<Option<Color>>], x: usize, y: usize, color: Color) {
  if let Some(row) = canvas.get_mut(y)
    && let Some(cell) = row.get_mut(x)
  {
    *cell = Some(color);
  }
}

fn paint_inferno(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  for x in 1..width.saturating_sub(1) {
    let height = 1 + (x * 7 % 5);
    if x % 3 == 0 {
      for y in 0..height {
        scene_put(
          canvas,
          x,
          5_usize.saturating_sub(y),
          if y + 1 == height {
            theme.highlight
          } else {
            theme.face_top
          },
        );
      }
    }
  }
}

fn paint_toxic(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let bubbles = [
    (width / 8, 2, 2),
    (width / 3, 0, 1),
    (width * 2 / 3, 1, 2),
    (width * 7 / 8, 0, 1),
  ];
  for (cx, cy, radius) in bubbles {
    for y in 0..=radius * 2 {
      for x in 0..=radius * 2 {
        let edge = x == 0 || y == 0 || x == radius * 2 || y == radius * 2;
        if edge {
          scene_put(canvas, cx + x - radius, cy + y, theme.outline);
        }
      }
    }
    scene_put(canvas, cx, cy + radius, theme.highlight);
  }
}

fn paint_ice(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  for (index, x) in [
    width / 9,
    width / 4,
    width / 2,
    width * 3 / 4,
    width * 8 / 9,
  ]
  .into_iter()
  .enumerate()
  {
    let length = 2 + index % 4;
    for step in 0..length {
      scene_put(
        canvas,
        x + step / 2,
        step,
        if step == 0 {
          theme.highlight
        } else {
          theme.outline
        },
      );
      if x > step / 2 {
        scene_put(canvas, x - step / 2, step, theme.face_bottom);
      }
    }
  }
}

fn paint_orbit(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let center = width / 2;
  for offset in 0..center.saturating_sub(3) {
    if offset % 3 == 0 {
      let y = (offset * 5 / center.max(1)).min(4);
      scene_put(canvas, center + offset, y, theme.outline);
      scene_put(canvas, center - offset, 4 - y, theme.face_top);
    }
  }
  for y in 0..4 {
    scene_put(canvas, center, y, theme.highlight);
    if center + 1 < width {
      scene_put(canvas, center + 1, y, theme.face_bottom);
    }
  }
}

fn paint_industrial(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  for x in 1..width.saturating_sub(1) {
    if x % 4 < 2 {
      scene_put(canvas, x, 1, theme.face_top);
      scene_put(canvas, x, 2, theme.outline_dark);
    }
  }
  for x in (4..width.saturating_sub(4)).step_by(9) {
    scene_put(canvas, x, 4, theme.highlight);
    scene_put(canvas, x + 1, 4, theme.shadow);
  }
}

fn paint_abyss(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let center = width / 2;
  for x in center.saturating_sub(7)..=(center + 7).min(width.saturating_sub(1)) {
    let distance = x.abs_diff(center);
    let y = distance / 3;
    scene_put(canvas, x, y, theme.outline);
    scene_put(canvas, x, 5_usize.saturating_sub(y), theme.face_bottom);
  }
  scene_put(canvas, center, 2, theme.highlight);
  scene_put(canvas, center, 3, theme.shadow_deep);
  for x in [2, width / 6, width * 5 / 6, width.saturating_sub(3)] {
    for y in 1..5 {
      scene_put(canvas, x + y % 2, y, theme.outline);
    }
  }
}

fn paint_skull(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  const SKULL: [&str; 6] = [
    ".xxxxx.", "xx...xx", "x.x.x.x", "xx...xx", ".xxxxx.", "..x.x..",
  ];
  let left = width.saturating_sub(SKULL[0].len()) / 2;
  for (y, row) in SKULL.iter().enumerate() {
    for (x, pixel) in row.chars().enumerate() {
      if pixel == 'x' {
        scene_put(
          canvas,
          left + x,
          y,
          if y == 2 && (x == 2 || x == 4) {
            theme.outline
          } else {
            theme.face_top
          },
        );
      }
    }
  }
}
