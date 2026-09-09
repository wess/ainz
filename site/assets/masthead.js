import { copyText as copy } from "./studio/clipboard.js";
import { state, PRESETS, resize, fill, stamp, put, at } from "./studio/model.js";
import { fromAns, toAns, MAX_BYTES } from "./studio/format.js";
import { $, sync, draw, palette, delivery, notice } from "./studio/view.js";

const STORAGE = "ainz-masthead";
const undo = [], redo = [];
const snapshot = () => ({
  name: state.name, width: state.width, height: state.height,
  palette: state.palette.slice(), pixels: state.pixels.slice(), color: state.color,
});
const history = () => {
  $("undo").disabled = undo.length === 0;
  $("redo").disabled = redo.length === 0;
};
const checkpoint = () => {
  undo.push(snapshot());
  if (undo.length > 40) undo.shift();
  redo.length = 0;
  history();
};
const persist = () => {
  try {
    localStorage.setItem(STORAGE, JSON.stringify({
      name: state.name, width: state.width, height: state.height,
      art: state.pixels.some((pixel) => pixel >= 0) ? toAns(state) : null,
    }));
    $("draft").textContent = "Draft saved locally";
  } catch { $("draft").textContent = "Browser storage unavailable; download to keep your work"; }
};
const refresh = () => { sync(); persist(); };
const travel = (source, target) => {
  if (!source.length) return;
  target.push(snapshot());
  Object.assign(state, source.pop());
  refresh(); history();
};
const load = (art) => {
  Object.assign(state, fromAns(art), { color: 0 });
  state.palette.push(...PRESETS.filter((color) => !state.palette.includes(color)));
};
const starter = async (name) => {
  try {
    let art;
    if (name === "mascot") {
      const response = await fetch(new URL("./mascot.ans", import.meta.url));
      if (!response.ok) throw new Error("The mascot could not be loaded. Try Wordmark or open a saved file.");
      art = await response.text();
      fromAns(art);
    }
    checkpoint();
    if (art) { load(art); state.name = "myainz"; }
    else {
      resize(48, 20);
      state.pixels.fill(-1);
      state.name = name === "wordmark" ? "wordmark" : "custom";
      if (name === "wordmark") { state.palette = PRESETS.slice(); state.color = 0; stamp(1, 3); }
    }
    $("error").textContent = "";
    refresh();
  } catch (error) { $("error").textContent = error.message; }
};

let painting = false;
let cursor = [0, 0];
const apply = (x, y, erase = false) => {
  if (x < 0 || y < 0 || x >= state.width || y >= state.height) return;
  if (state.tool === "pick") {
    const color = at(x, y);
    if (color >= 0) { state.color = color; $("color").value = state.palette[color]; palette(); }
  } else if (erase || state.tool === "erase") put(x, y, -1);
  else if (state.tool === "fill") fill(x, y, state.color);
  else if (state.tool === "text") stamp(x, y);
  else put(x, y, state.color);
  draw();
};
const pointer = (event) => {
  const bounds = $("grid").getBoundingClientRect();
  cursor = [
    Math.floor((event.clientX - bounds.left) / bounds.width * state.width),
    Math.floor((event.clientY - bounds.top) / bounds.height * state.height),
  ];
  apply(...cursor, event.buttons === 2);
};
$("grid").addEventListener("contextmenu", (event) => event.preventDefault());
$("grid").addEventListener("pointerdown", (event) => {
  checkpoint();
  $("grid").setPointerCapture(event.pointerId);
  painting = ["paint", "erase"].includes(state.tool);
  pointer(event);
  if (!painting) refresh();
});
$("grid").addEventListener("pointermove", (event) => { if (painting) pointer(event); });
for (const type of ["pointerup", "pointercancel"]) {
  $("grid").addEventListener(type, () => { if (painting) { painting = false; refresh(); } });
}
$("grid").addEventListener("keydown", (event) => {
  const movement = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[event.key];
  if (movement) {
    event.preventDefault();
    cursor = [Math.max(0, Math.min(state.width - 1, cursor[0] + movement[0])),
      Math.max(0, Math.min(state.height - 1, cursor[1] + movement[1]))];
    draw();
    const context = $("grid").getContext("2d");
    context.strokeStyle = "#ffe657";
    context.strokeRect(cursor[0] * state.zoom + .5, cursor[1] * state.zoom + .5, state.zoom - 1, state.zoom - 1);
  } else if (event.key === " ") {
    event.preventDefault(); checkpoint(); apply(...cursor); refresh();
  }
});

document.querySelectorAll("[data-tool]").forEach((button) => {
  button.addEventListener("click", () => {
    state.tool = button.dataset.tool;
    document.querySelectorAll("[data-tool]").forEach((other) => {
      other.classList.toggle("selected", other === button);
      other.setAttribute("aria-pressed", String(other === button));
    });
  });
});
document.querySelectorAll("[data-starter]").forEach((button) => {
  button.addEventListener("click", () => starter(button.dataset.starter));
});
$("undo").addEventListener("click", () => travel(undo, redo));
$("redo").addEventListener("click", () => travel(redo, undo));
window.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "z"
      && !["INPUT", "SELECT", "TEXTAREA"].includes(event.target.tagName)) {
    event.preventDefault();
    if (event.shiftKey) travel(redo, undo); else travel(undo, redo);
  }
});
$("name").addEventListener("input", (event) => {
  state.name = event.target.value; delivery(); persist(); notice("");
});
for (const id of ["width", "height"]) $(id).addEventListener("change", () => {
  checkpoint(); resize(Number($("width").value), Number($("height").value)); refresh();
});
$("zoom").addEventListener("change", (event) => { state.zoom = Number(event.target.value); draw(); });
$("text").addEventListener("input", (event) => { state.text = event.target.value; });
$("scale").addEventListener("change", (event) => { state.scale = Number(event.target.value); });
$("color").addEventListener("input", (event) => {
  const color = event.target.value;
  if (!state.palette.includes(color)) state.palette.push(color);
  state.color = state.palette.indexOf(color); palette();
});
for (const id of ["platform", "scope"]) $(id).addEventListener("change", () => { delivery(); notice(""); });
$("clear").addEventListener("click", () => { checkpoint(); state.pixels.fill(-1); refresh(); });

$("installcopy").addEventListener("click", () => copy($("install").textContent, "Install command copied. Paste it into your shell."));
$("selectcopy").addEventListener("click", () => copy($("selectcommand").textContent, "Header command copied. Run it inside Ainz."));
$("copy").addEventListener("click", () => copy(toAns(state), "ANSI artwork copied."));
$("save").addEventListener("click", () => {
  const url = URL.createObjectURL(new Blob([toAns(state)], { type: "text/plain;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url; link.download = `${state.name}.ans`; link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  notice("Download started. The header guide includes manual installation steps.");
});
$("openbutton").addEventListener("click", () => $("open").click());
$("open").addEventListener("change", async (event) => {
  const file = event.target.files[0];
  if (!file) return;
  try {
    if (file.size > MAX_BYTES) throw new Error("The file exceeds the 128 KiB header limit.");
    const art = await file.text();
    fromAns(art);
    checkpoint(); load(art);
    state.name = file.name.replace(/\.(ans|ansi)$/i, "").replace(/[^a-zA-Z0-9_-]/g, "") || "custom";
    $("error").textContent = ""; refresh();
  } catch (error) { $("error").textContent = `Could not open ${file.name}: ${error.message}`; }
  event.target.value = "";
});

$("platform").value = /Mac|iPhone|iPad/.test(navigator.platform) ? "macos" : "linux";
let restored = false;
try {
  const saved = JSON.parse(localStorage.getItem(STORAGE));
  if (saved && typeof saved.name === "string") {
    if (saved.art) load(saved.art);
    else { resize(saved.width || 48, saved.height || 20); state.pixels.fill(-1); }
    state.name = saved.name; restored = true;
  }
} catch { /* a damaged local draft must not stop the editor */ }
refresh();
if (!restored) await starter("mascot");
