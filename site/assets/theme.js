import { roles, defaults, presets, toToml, fromToml, contrast } from "./theme/format.js";
import { artifactCommand, nameError } from "./studio/install.js";
import { copyText } from "./studio/clipboard.js";

const $ = (id) => document.getElementById(id);
const STORAGE = "ainz-theme";
let colors = { ...defaults };
const history = [];
const checkpoint = () => {
  history.push({ ...colors });
  if (history.length > 40) history.shift();
  $("undo").disabled = false;
};
const persist = () => {
  try {
    localStorage.setItem(STORAGE, JSON.stringify({ name: $("name").value, art: toToml(colors) }));
    $("draft").textContent = "Draft saved locally";
  } catch { $("draft").textContent = "Storage unavailable; download to keep your theme"; }
};
const delivery = () => {
  const name = $("name").value;
  const error = nameError(name, "theme");
  $("name-help").textContent = error || "The filename and your /theme command.";
  $("name").setAttribute("aria-invalid", String(Boolean(error)));
  for (const id of ["installcopy", "selectcopy", "save", "copy"]) $(id).disabled = Boolean(error);
  $("selectcommand").textContent = `/theme ${name || "NAME"}`;
  $("install").textContent = error ? "Name your theme to create the command."
    : artifactCommand(toToml(colors), name, $("platform").value, $("scope").value, "theme");
  $("destination").textContent = $("scope").value === "project"
    ? "Run from your project folder. Installs in .ainz/themes/."
    : $("platform").value === "macos" ? "Installs in ~/Library/Application Support/ainz/themes/."
      : "Installs in $XDG_CONFIG_HOME/ainz/themes, or ~/.config/ainz/themes by default.";
};
const preview = () => {
  const background = colors.background === "default" ? "#10171a" : colors.background;
  for (const [role, value] of Object.entries(colors)) {
    $("terminal").style.setProperty(`--theme-${role}`, role === "background" ? background : value);
  }
  const pairs = [["Text", colors.text, background], ["Muted", colors.muted, background],
    ["Bar text", colors.bar_text, colors.bar]];
  $("contrast").textContent = pairs.map(([label, fg, bg]) => {
    const ratio = contrast(fg, bg);
    return `${label} ${ratio.toFixed(1)}:1${ratio < 4.5 ? " (low contrast)" : ""}`;
  }).join(" · ") + (colors.background === "default" ? " · Default background previewed on #10171a." : "");
  delivery(); persist();
};
const fields = () => {
  $("defaultbg").checked = colors.background === "default";
  $("colors").replaceChildren(...roles.map(([role, label, description]) => {
    const field = document.createElement("label");
    field.className = "color-field";
    const title = document.createElement("span");
    const strong = document.createElement("strong");
    strong.textContent = label;
    const hint = document.createElement("small");
    hint.textContent = description;
    title.append(strong, hint);
    const input = document.createElement("input");
    input.type = "color";
    input.value = colors[role] === "default" ? "#10171a" : colors[role];
    input.disabled = role === "background" && $("defaultbg").checked;
    input.setAttribute("aria-label", `${label} color`);
    const value = document.createElement("code");
    value.textContent = colors[role];
    input.addEventListener("change", () => {
      checkpoint(); colors[role] = input.value; value.textContent = input.value; preview();
    });
    field.append(input, title, value);
    return field;
  }));
  preview();
};
for (const button of document.querySelectorAll("[data-preset]")) {
  button.addEventListener("click", () => {
    checkpoint(); colors = { ...presets[button.dataset.preset] }; fields();
    $("notice").textContent = `${button.textContent} palette loaded. Undo restores your previous colors.`;
  });
}
$("undo").addEventListener("click", () => {
  if (history.length) colors = history.pop();
  $("undo").disabled = history.length === 0; fields();
});
$("defaultbg").addEventListener("change", () => {
  checkpoint(); colors.background = $("defaultbg").checked ? "default" : "#10171a"; fields();
});
$("name").addEventListener("input", () => { delivery(); persist(); $("notice").textContent = ""; });
for (const id of ["platform", "scope"]) $(id).addEventListener("change", delivery);
$("installcopy").addEventListener("click", () => copyText($("install").textContent, "Install command copied. Paste it into your shell."));
$("selectcopy").addEventListener("click", () => copyText($("selectcommand").textContent, "Theme command copied. Run it inside Ainz."));
$("copy").addEventListener("click", () => copyText(toToml(colors), "Theme TOML copied."));
$("save").addEventListener("click", () => {
  const url = URL.createObjectURL(new Blob([toToml(colors)], { type: "text/plain;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url; link.download = `${$("name").value}.toml`; link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  $("notice").textContent = "Download started. The theme guide includes manual installation steps.";
});
$("openbutton").addEventListener("click", () => $("open").click());
$("open").addEventListener("change", async (event) => {
  const file = event.target.files[0];
  if (!file) return;
  try {
    if (file.size > 16 * 1024) throw new Error("Theme exceeds 16 KiB.");
    const imported = fromToml(await file.text());
    checkpoint(); colors = imported;
    $("name").value = file.name.replace(/\.toml$/i, "").replace(/[^a-zA-Z0-9_-]/g, "") || "mytheme";
    $("error").textContent = ""; fields();
  } catch (error) { $("error").textContent = `Could not open ${file.name}: ${error.message}`; }
  event.target.value = "";
});
$("platform").value = /Mac|iPhone|iPad/.test(navigator.platform) ? "macos" : "linux";
try {
  const saved = JSON.parse(localStorage.getItem(STORAGE));
  if (saved && typeof saved.name === "string") {
    colors = fromToml(saved.art); $("name").value = saved.name;
  }
} catch { /* a damaged local draft must not stop the designer */ }
fields();
