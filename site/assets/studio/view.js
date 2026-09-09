import { state } from "./model.js";
import { toAns, MAX_BYTES } from "./format.js";
import { nameError, installCommand } from "./install.js";

export const $ = (id) => document.getElementById(id);
export const notice = (message) => { $("notice").textContent = message; };

export const draw = () => {
  for (const id of ["grid", "preview"]) {
    const target = $(id);
    const zoom = id === "grid" ? state.zoom : 8;
    target.width = state.width * zoom;
    target.height = state.height * zoom;
    const context = target.getContext("2d");
    context.fillStyle = "#10171a";
    context.fillRect(0, 0, target.width, target.height);
    state.pixels.forEach((pixel, index) => {
      if (pixel < 0) return;
      context.fillStyle = state.palette[pixel];
      context.fillRect((index % state.width) * zoom, Math.floor(index / state.width) * zoom, zoom, zoom);
    });
    if (id === "grid") {
      context.strokeStyle = "#ffffff18";
      context.lineWidth = 1;
      context.beginPath();
      for (let x = 0; x <= target.width; x += zoom) {
        context.moveTo(x + .5, 0); context.lineTo(x + .5, target.height);
      }
      for (let y = 0; y <= target.height; y += zoom) {
        context.moveTo(0, y + .5); context.lineTo(target.width, y + .5);
      }
      context.stroke();
    }
  }
};

export const palette = () => {
  $("palette").replaceChildren(...state.palette.map((color, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "swatch";
    button.style.background = color;
    button.classList.toggle("selected", index === state.color);
    button.setAttribute("aria-label", `Use ${color}`);
    button.setAttribute("aria-pressed", String(index === state.color));
    button.addEventListener("click", () => {
      state.color = index;
      $("color").value = color;
      palette();
    });
    return button;
  }));
};

export const delivery = () => {
  const art = toAns(state);
  const bytes = new TextEncoder().encode(art).length;
  const empty = !state.pixels.some((pixel) => pixel >= 0);
  const error = nameError(state.name) || (empty ? "Draw something before installing or exporting." : "")
    || (bytes > MAX_BYTES ? "This artwork exceeds 128 KiB. Reduce the canvas or use fewer colors." : "");
  $("name-help").textContent = error || "This becomes the filename and your /header command.";
  $("name").setAttribute("aria-invalid", String(Boolean(nameError(state.name))));
  for (const id of ["installcopy", "selectcopy", "save", "copy"]) $(id).disabled = Boolean(error);
  $("stats").textContent = `${state.width} columns × ${state.height / 2} terminal rows · ${(bytes / 1024).toFixed(1)} KiB`;
  const terminalWidth = Math.max(72, state.width + 26);
  const terminalHeight = state.height / 2 + 7;
  $("fit").textContent = `Allow at least ${terminalWidth} columns × ${terminalHeight} rows with the roster open. Actual font proportions vary.`;
  $("selectcommand").textContent = `/header ${state.name || "NAME"}`;
  $("install").textContent = error ? "Name and draw your header to create the command."
    : installCommand(art, state.name, $("platform").value, $("scope").value);
  $("destination").textContent = $("scope").value === "project"
    ? "Run from your project folder. Installs in .ainz/headers/."
    : $("platform").value === "macos"
      ? "Installs in ~/Library/Application Support/ainz/headers/."
      : "Installs in $XDG_CONFIG_HOME/ainz/headers, or ~/.config/ainz/headers by default.";
};

export const sync = () => {
  $("name").value = state.name;
  $("width").value = state.width;
  $("height").value = state.height;
  $("color").value = state.palette[state.color];
  draw(); palette(); delivery();
};
