import { FONT } from "./font.js";
import { MAX_WIDTH, MAX_LINES } from "./format.js";

export const PRESETS = [
  '#e2be30', '#d37e1d', '#ffe86f', '#3ebcdd', '#182c34', '#363b3d',
  '#f95c26', '#a0181c', '#ffd33d', '#ff8e1c', '#89ed34', '#2f8f2a',
  '#c4f2ff', '#48cdd6', '#1a4280', '#c676cd', '#e0e6e5', '#69747a',
  '#dadee2', '#000000',
];


export const state = {
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

// pixel operations

export const at = (x, y) => state.pixels[y * state.width + x];
export const put = (x, y, value) => {
  if (x < 0 || y < 0 || x >= state.width || y >= state.height) return;
  state.pixels[y * state.width + x] = value;
};

export const resize = (width, height) => {
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

export const fill = (x, y, value) => {
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

export const stamp = (x, y) => {
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

