// masthead studio: a half-block pixel editor that reads and writes the .ans files ainz renders

const MAX_WIDTH = 240;
const MAX_LINES = 80;
const MAX_BYTES = 128 * 1024;
const ROSTER_FIT = 72;
const STORAGE = 'ainz-masthead';

const PRESETS = [
  '#e2be30', '#d37e1d', '#ffe86f', '#3ebcdd', '#182c34', '#363b3d',
  '#f95c26', '#a0181c', '#ffd33d', '#ff8e1c', '#89ed34', '#2f8f2a',
  '#c4f2ff', '#48cdd6', '#1a4280', '#c676cd', '#e0e6e5', '#69747a',
  '#dadee2', '#000000',
];

// 5x7 glyphs; the letters ainz paints its own name with come first so stamps match
const FONT = {
  A: ['01110', '10001', '10001', '11111', '10001', '10001', '10001'],
  B: ['11110', '10001', '10001', '11110', '10001', '10001', '11110'],
  C: ['01110', '10001', '10000', '10000', '10000', '10001', '01110'],
  D: ['11110', '10001', '10001', '10001', '10001', '10001', '11110'],
  E: ['11111', '10000', '10000', '11110', '10000', '10000', '11111'],
  F: ['11111', '10000', '10000', '11110', '10000', '10000', '10000'],
  G: ['01110', '10001', '10000', '10111', '10001', '10001', '01110'],
  H: ['10001', '10001', '10001', '11111', '10001', '10001', '10001'],
  I: ['11111', '00100', '00100', '00100', '00100', '00100', '11111'],
  J: ['00111', '00010', '00010', '00010', '00010', '10010', '01100'],
  K: ['10001', '10010', '10100', '11000', '10100', '10010', '10001'],
  L: ['10000', '10000', '10000', '10000', '10000', '10000', '11111'],
  M: ['10001', '11011', '10101', '10101', '10001', '10001', '10001'],
  N: ['10001', '11001', '11001', '10101', '10011', '10011', '10001'],
  O: ['01110', '10001', '10001', '10001', '10001', '10001', '01110'],
  P: ['11110', '10001', '10001', '11110', '10000', '10000', '10000'],
  Q: ['01110', '10001', '10001', '10001', '10101', '10010', '01101'],
  R: ['11110', '10001', '10001', '11110', '10100', '10010', '10001'],
  S: ['01111', '10000', '10000', '01110', '00001', '00001', '11110'],
  T: ['11111', '00100', '00100', '00100', '00100', '00100', '00100'],
  U: ['10001', '10001', '10001', '10001', '10001', '10001', '01110'],
  V: ['10001', '10001', '10001', '10001', '10001', '01010', '00100'],
  W: ['10001', '10001', '10001', '10101', '10101', '10101', '01010'],
  X: ['10001', '10001', '01010', '00100', '01010', '10001', '10001'],
  Y: ['10001', '10001', '01010', '00100', '00100', '00100', '00100'],
  Z: ['11111', '00001', '00010', '00100', '01000', '10000', '11111'],
  0: ['01110', '10001', '10011', '10101', '11001', '10001', '01110'],
  1: ['00100', '01100', '00100', '00100', '00100', '00100', '01110'],
  2: ['01110', '10001', '00001', '00010', '00100', '01000', '11111'],
  3: ['11111', '00010', '00100', '00010', '00001', '10001', '01110'],
  4: ['00010', '00110', '01010', '10010', '11111', '00010', '00010'],
  5: ['11111', '10000', '11110', '00001', '00001', '10001', '01110'],
  6: ['00110', '01000', '10000', '11110', '10001', '10001', '01110'],
  7: ['11111', '00001', '00010', '00100', '01000', '01000', '01000'],
  8: ['01110', '10001', '10001', '01110', '10001', '10001', '01110'],
  9: ['01110', '10001', '10001', '01111', '00001', '00010', '01100'],
  ' ': ['00000', '00000', '00000', '00000', '00000', '00000', '00000'],
  '-': ['00000', '00000', '00000', '11111', '00000', '00000', '00000'],
  _: ['00000', '00000', '00000', '00000', '00000', '00000', '11111'],
  '.': ['00000', '00000', '00000', '00000', '00000', '01100', '01100'],
  '!': ['00100', '00100', '00100', '00100', '00100', '00000', '00100'],
  ':': ['00000', '01100', '01100', '00000', '01100', '01100', '00000'],
  '/': ['00001', '00010', '00010', '00100', '01000', '01000', '10000'],
};

const $ = (id) => document.getElementById(id);

const state = {
  name: 'custom',
  width: 72,
  height: 20,
  palette: PRESETS.slice(),
  pixels: new Int16Array(72 * 20).fill(-1),
  color: 0,
  tool: 'paint',
  zoom: 10,
  text: 'AINZ',
  scale: 2,
};

// ---- model

const at = (x, y) => state.pixels[y * state.width + x];
const put = (x, y, value) => {
  if (x < 0 || y < 0 || x >= state.width || y >= state.height) return;
  state.pixels[y * state.width + x] = value;
};

const resize = (width, height) => {
  width = Math.max(1, Math.min(MAX_WIDTH, width | 0));
  height = Math.max(2, Math.min(MAX_LINES * 2, height | 0));
  if (height % 2) height += 1;
  const next = new Int16Array(width * height).fill(-1);
  for (let y = 0; y < Math.min(height, state.height); y++) {
    for (let x = 0; x < Math.min(width, state.width); x++) next[y * width + x] = at(x, y);
  }
  state.width = width;
  state.height = height;
  state.pixels = next;
};

const fill = (x, y, value) => {
  const target = at(x, y);
  if (target === value) return;
  const stack = [[x, y]];
  while (stack.length) {
    const [px, py] = stack.pop();
    if (px < 0 || py < 0 || px >= state.width || py >= state.height || at(px, py) !== target) continue;
    put(px, py, value);
    stack.push([px + 1, py], [px - 1, py], [px, py + 1], [px, py - 1]);
  }
};

const stamp = (x, y) => {
  let cursor = x;
  for (const raw of state.text.toUpperCase()) {
    const glyph = FONT[raw] || FONT[' '];
    glyph.forEach((row, gy) => {
      [...row].forEach((bit, gx) => {
        if (bit !== '1') return;
        for (let dy = 0; dy < state.scale; dy++) {
          for (let dx = 0; dx < state.scale; dx++) {
            put(cursor + gx * state.scale + dx, y + gy * state.scale + dy, state.color);
          }
        }
      });
    });
    cursor += 6 * state.scale;
  }
};

// ---- files

const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const hex = (r, g, b) => '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');

// xterm's 256 colours, so art written elsewhere lands on the grid with the right colours
const BASIC = [
  '#000000', '#cd0000', '#00cd00', '#cdcd00', '#0000ee', '#cd00cd', '#00cdcd', '#e5e5e5',
  '#7f7f7f', '#ff0000', '#00ff00', '#ffff00', '#5c5cff', '#ff00ff', '#00ffff', '#ffffff',
];

const indexed = (value) => {
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
const toAns = () => {
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
    lines.push(line.replace(/ +$/, '') + '\x1b[0m');
  }
  while (lines.length && lines[lines.length - 1] === '\x1b[0m') lines.pop();
  return lines.join('\n') + '\n';
};

// style carries across lines, the way ainz reads it
const applySgr = (parameters, colors) => {
  const values = parameters === '' ? [0] : parameters.split(';').map(Number);
  for (let i = 0; i < values.length; i++) {
    const value = values[i];
    if (!Number.isInteger(value)) throw new Error('invalid ANSI parameter');
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

const fromAns = (text) => {
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
  while (rows.length && !rows[rows.length - 1].some(([top, bottom]) => top || bottom)) rows.pop();
  if (!rows.length) throw new Error('the file has no artwork in it');
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
  state.width = width;
  state.height = rows.length * 2;
  state.pixels = pixels;
  state.palette = palette.concat(PRESETS.filter((color) => !palette.includes(color)));
  state.color = 0;
};

const download = (text) => {
  const blob = new Blob([text], { type: 'text/plain' });
  const link = document.createElement('a');
  link.href = URL.createObjectURL(blob);
  link.download = `${state.name}.ans`;
  link.click();
  window.setTimeout(() => URL.revokeObjectURL(link.href), 1000);
};

const save = () => {
  try { localStorage.setItem(STORAGE, JSON.stringify({ name: state.name, art: toAns() })); } catch {}
};

const restore = () => {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE));
    if (!saved) return false;
    fromAns(saved.art);
    state.name = saved.name || state.name;
    return true;
  } catch { return false; }
};

// ---- view

const canvas = $('grid');
const context = canvas.getContext('2d');

const draw = () => {
  const z = state.zoom;
  canvas.width = state.width * z;
  canvas.height = state.height * z;
  context.fillStyle = '#05070a';
  context.fillRect(0, 0, canvas.width, canvas.height);
  for (let y = 0; y < state.height; y++) {
    for (let x = 0; x < state.width; x++) {
      const value = at(x, y);
      if (value < 0) continue;
      context.fillStyle = state.palette[value];
      context.fillRect(x * z, y * z, z, z);
    }
  }
  context.strokeStyle = 'rgba(255,255,255,0.07)';
  context.lineWidth = 1;
  for (let x = 0; x <= state.width; x++) { context.beginPath(); context.moveTo(x * z + 0.5, 0); context.lineTo(x * z + 0.5, canvas.height); context.stroke(); }
  // terminal rows are two pixel rows tall; mark them so the half-block seam is visible
  for (let y = 0; y <= state.height; y += 2) { context.beginPath(); context.moveTo(0, y * z + 0.5); context.lineTo(canvas.width, y * z + 0.5); context.stroke(); }
  if (state.width > ROSTER_FIT) {
    context.strokeStyle = 'rgba(198,118,205,0.7)';
    context.beginPath(); context.moveTo(ROSTER_FIT * z + 0.5, 0); context.lineTo(ROSTER_FIT * z + 0.5, canvas.height); context.stroke();
  }
};

// drawn at terminal-cell proportions, so it shows exactly what the transcript will
const preview = () => {
  const target = $('preview');
  const view = target.getContext('2d');
  const cell = 8;
  target.width = state.width * cell;
  target.height = (state.height / 2) * cell * 2;
  view.fillStyle = '#000';
  view.fillRect(0, 0, target.width, target.height);
  for (let y = 0; y < state.height; y++) {
    for (let x = 0; x < state.width; x++) {
      const value = at(x, y);
      if (value < 0) continue;
      view.fillStyle = state.palette[value];
      view.fillRect(x * cell, y * cell, cell, cell);
    }
  }
};

const stats = () => {
  const bytes = new TextEncoder().encode(toAns()).length;
  const lines = state.height / 2;
  const notes = [
    `${state.width} × ${state.height} px`,
    `${state.width} cols × ${lines} lines`,
    `${(bytes / 1024).toFixed(1)} KiB`,
    state.width <= ROSTER_FIT ? 'fits beside the roster' : `wider than ${ROSTER_FIT}: hidden when the roster is open`,
  ];
  if (bytes > MAX_BYTES) notes.push('over the 128 KiB limit');
  $('stats').textContent = notes.join(' · ');
  $('install').textContent = `mkdir -p .ainz/headers\nmv ~/Downloads/${state.name}.ans .ainz/headers/\nainz   # then /header ${state.name}`;
};

const palette = () => {
  const target = $('palette');
  target.replaceChildren();
  state.palette.forEach((color, index) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'swatch' + (index === state.color ? ' selected' : '');
    button.style.background = color;
    button.title = color;
    button.setAttribute('aria-label', `color ${color}`);
    button.addEventListener('click', () => { state.color = index; $('color').value = color; palette(); });
    target.appendChild(button);
  });
};

const refresh = () => { draw(); preview(); stats(); save(); };

// ---- input

let painting = false;
const cell = (event) => {
  const box = canvas.getBoundingClientRect();
  return [
    Math.floor((event.clientX - box.left) / box.width * state.width),
    Math.floor((event.clientY - box.top) / box.height * state.height),
  ];
};

const apply = (event) => {
  const [x, y] = cell(event);
  if (x < 0 || y < 0 || x >= state.width || y >= state.height) return;
  const erase = state.tool === 'erase' || event.buttons === 2;
  if (state.tool === 'pick') {
    const value = at(x, y);
    if (value >= 0) { state.color = value; $('color').value = state.palette[value]; palette(); }
    return;
  }
  if (state.tool === 'fill') fill(x, y, erase ? -1 : state.color);
  else if (state.tool === 'text') stamp(x, y);
  else put(x, y, erase ? -1 : state.color);
  draw();
};

canvas.addEventListener('contextmenu', (event) => event.preventDefault());
canvas.addEventListener('pointerdown', (event) => {
  painting = state.tool === 'paint' || state.tool === 'erase';
  apply(event);
  if (!painting) refresh();
});
canvas.addEventListener('pointermove', (event) => { if (painting) apply(event); });
window.addEventListener('pointerup', () => { if (painting) { painting = false; refresh(); } });

document.querySelectorAll('[data-tool]').forEach((button) => {
  button.addEventListener('click', () => {
    state.tool = button.dataset.tool;
    document.querySelectorAll('[data-tool]').forEach((other) => other.classList.toggle('selected', other === button));
  });
});

$('name').addEventListener('input', (event) => {
  const value = event.target.value.replace(/[^A-Za-z0-9_-]/g, '');
  event.target.value = value;
  state.name = value || 'custom';
  stats(); save();
});
$('width').addEventListener('change', (event) => { resize(+event.target.value, state.height); event.target.value = state.width; refresh(); });
$('height').addEventListener('change', (event) => { resize(state.width, +event.target.value); event.target.value = state.height; refresh(); });
$('zoom').addEventListener('change', (event) => { state.zoom = +event.target.value; draw(); });
$('text').addEventListener('input', (event) => { state.text = event.target.value; });
$('scale').addEventListener('change', (event) => { state.scale = +event.target.value; });
$('color').addEventListener('input', (event) => {
  const color = event.target.value;
  let index = state.palette.indexOf(color);
  if (index < 0) { state.palette.push(color); index = state.palette.length - 1; }
  state.color = index;
  palette();
});
$('clear').addEventListener('click', () => { state.pixels.fill(-1); refresh(); });
$('save').addEventListener('click', () => download(toAns()));
$('copy').addEventListener('click', async () => {
  try { await navigator.clipboard.writeText(toAns()); $('copy').textContent = 'copied'; }
  catch { $('copy').textContent = 'select and copy'; }
  window.setTimeout(() => { $('copy').textContent = 'copy the file'; }, 1200);
});
$('open').addEventListener('change', async (event) => {
  const file = event.target.files[0];
  if (!file) return;
  try {
    fromAns(await file.text());
    state.name = file.name.replace(/\.(ans|ansi|txt)$/i, '').replace(/[^A-Za-z0-9_-]/g, '') || state.name;
    $('name').value = state.name;
    $('width').value = state.width;
    $('height').value = state.height;
    $('error').textContent = '';
    palette();
    refresh();
  } catch (error) {
    $('error').textContent = `could not open ${file.name}: ${error.message}`;
  }
  event.target.value = '';
});

// ---- start

if (!restore()) {
  stamp(2, 5);
}
$('name').value = state.name;
$('width').value = state.width;
$('height').value = state.height;
$('color').value = state.palette[state.color];
palette();
refresh();
