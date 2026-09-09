export const MAX_WIDTH = 240;
export const MAX_LINES = 80;
export const MAX_BYTES = 128 * 1024;

const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const hex = (...values) => {
  if (values.some((value) => !Number.isInteger(value) || value < 0 || value > 255)) {
    throw new Error("ANSI color must be between 0 and 255");
  }
  return "#" + values.map((value) => value.toString(16).padStart(2, "0")).join("");
};

// xterm's 256 colours, so art written elsewhere lands on the grid with the right colours
const BASIC = [
  '#000000', '#cd0000', '#00cd00', '#cdcd00', '#0000ee', '#cd00cd', '#00cdcd', '#e5e5e5',
  '#7f7f7f', '#ff0000', '#00ff00', '#ffff00', '#5c5cff', '#ff00ff', '#00ffff', '#ffffff',
];

const indexed = (value) => {
  if (!Number.isInteger(value) || value < 0 || value > 255) throw new Error("ANSI color must be between 0 and 255");
  if (value < 16) return BASIC[value];
  if (value < 232) {
    const level = (v) => (v ? 55 + v * 40 : 0);
    const cube = value - 16;
    return hex(level(Math.floor(cube / 36) % 6), level(Math.floor(cube / 6) % 6), level(cube % 6));
  }
  const grey = 8 + (value - 232) * 10;
  return hex(grey, grey, grey);
};

// one cell per two pixels: the top pixel is the foreground, the bottom is the background
export const toAns = (state) => {
  const at = (x, y) => state.pixels[y * state.width + x];
  const lines = [];
  for (let y = 0; y < state.height; y += 2) {
    let line = '';
    let current = '';
    for (let x = 0; x < state.width; x++) {
      const top = at(x, y);
      const bottom = y + 1 < state.height ? at(x, y + 1) : -1;
      let sgr = '';
      let glyph = ' ';
      if (top >= 0 && bottom >= 0) {
        if (top === bottom) { sgr = `\x1b[38;2;${rgb(state.palette[top]).join(';')}m`; glyph = '█'; }
        else { sgr = `\x1b[38;2;${rgb(state.palette[top]).join(';')};48;2;${rgb(state.palette[bottom]).join(';')}m`; glyph = '▀'; }
      } else if (top >= 0) { sgr = `\x1b[38;2;${rgb(state.palette[top]).join(';')}m`; glyph = '▀'; }
      else if (bottom >= 0) { sgr = `\x1b[38;2;${rgb(state.palette[bottom]).join(';')}m`; glyph = '▄'; }
      if (sgr !== current) { line += sgr ? `\x1b[0m${sgr}` : '\x1b[0m'; current = sgr; }
      line += glyph;
    }
    lines.push(line + '\x1b[0m');
  }
  return lines.join('\n') + '\n';
};

// style carries across lines, the way ainz reads it
const applySgr = (parameters, colors) => {
  const values = parameters === '' ? [0] : parameters.split(';').map(Number);
  for (let i = 0; i < values.length; i++) {
    const value = values[i];
    if (!Number.isInteger(value) || value < 0) throw new Error('invalid ANSI parameter');
    if (value === 0) { colors.fg = null; colors.bg = null; }
    else if (value === 39) colors.fg = null;
    else if (value === 49) colors.bg = null;
    else if (value >= 30 && value <= 37) colors.fg = indexed(value - 30);
    else if (value >= 90 && value <= 97) colors.fg = indexed(value - 90 + 8);
    else if (value >= 40 && value <= 47) colors.bg = indexed(value - 40);
    else if (value >= 100 && value <= 107) colors.bg = indexed(value - 100 + 8);
    else if (value === 38 || value === 48) {
      let color;
      if (values[i + 1] === 5) { color = indexed(values[i + 2]); i += 2; }
      else if (values[i + 1] === 2) { color = hex(values[i + 2], values[i + 3], values[i + 4]); i += 4; }
      else throw new Error('extended colour must use 5;n or 2;r;g;b');
      if (value === 38) colors.fg = color; else colors.bg = color;
    }
    // bold, underline and friends are accepted and dropped; a pixel grid has no styles
  }
};

const unpack = (glyph, colors) => {
  switch (glyph) {
    case ' ': return [colors.bg, colors.bg];
    case '█': return [colors.fg, colors.fg];
    case '▀': return [colors.fg, colors.bg];
    case '▄': return [colors.bg, colors.fg];
    default: throw new Error(`the grid holds half blocks only, and this uses ${JSON.stringify(glyph)}`);
  }
};

export const fromAns = (text) => {
  if (new TextEncoder().encode(text).length > MAX_BYTES) throw new Error(`over the ${MAX_BYTES / 1024} KiB limit`);
  const source = text.replace(/\r\n/g, '\n');
  const colors = { fg: null, bg: null };
  const rows = [];
  let row = [];
  let index = 0;
  while (index < source.length) {
    const glyph = source[index];
    if (glyph === '\x1b') {
      const escape = /^\x1b\[([0-9;]*)m/.exec(source.slice(index));
      if (!escape) throw new Error('only ANSI SGR colour sequences are supported');
      applySgr(escape[1], colors);
      index += escape[0].length;
    } else if (glyph === '\n') {
      rows.push(row); row = []; index += 1;
    } else if (glyph === '\t') {
      row.push([null, null], [null, null]); index += 1;
    } else {
      row.push(unpack(glyph, colors)); index += 1;
    }
  }
  if (row.length) rows.push(row);
  if (!rows.some((line) => line.some(([top, bottom]) => top || bottom))) {
    throw new Error('the file has no artwork in it');
  }
  const width = Math.max(...rows.map((line) => line.length));
  if (width > MAX_WIDTH) throw new Error(`${width} columns is wider than the ${MAX_WIDTH} limit`);
  if (rows.length > MAX_LINES) throw new Error(`${rows.length} lines is taller than the ${MAX_LINES} limit`);
  const palette = [];
  const slot = (color) => {
    if (!color) return -1;
    const found = palette.indexOf(color);
    if (found >= 0) return found;
    palette.push(color);
    return palette.length - 1;
  };
  const pixels = new Int16Array(width * rows.length * 2).fill(-1);
  rows.forEach((line, y) => {
    line.forEach(([top, bottom], x) => {
      pixels[y * 2 * width + x] = slot(top);
      pixels[(y * 2 + 1) * width + x] = slot(bottom);
    });
  });
  return { width, height: rows.length * 2, pixels, palette };
};
