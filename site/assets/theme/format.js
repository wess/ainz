export const roles = [
  ["background", "Background", "Terminal canvas", "default"],
  ["text", "Text", "Messages and tool output", "#dadee2"],
  ["muted", "Muted", "Timestamps and secondary details", "#808a94"],
  ["accent", "Accent", "Prompt and active controls", "#53c4be"],
  ["success", "Success", "Replies and completed runs", "#91d28a"],
  ["bar", "Bars", "Status and selection backgrounds", "#184280"],
  ["bar_text", "Bar text", "Text on status and selection bars", "#ffffff"],
  ["border", "Borders", "Pane dividers and frames", "#184280"],
  ["info", "Information", "Your nick, topic, and tools", "#48cdd6"],
  ["warning", "Warning", "Working and approval states", "#e6c75c"],
  ["error", "Error", "Failed runs and errors", "#e06767"],
  ["special", "Special", "Code accents and highlights", "#c676cd"],
  ["bright", "Bright", "Emphasized text", "#ffffff"],
];
export const defaults = Object.fromEntries(roles.map(([role, , , value]) => [role, value]));
export const presets = {
  classic: { ...defaults },
  nazarick: {
    ...defaults, background: "#14121b", text: "#e3decd", muted: "#9e93af",
    accent: "#d5b879", bar: "#492958", bar_text: "#f5ead4", border: "#755087",
    info: "#ba9ed1", special: "#d48ebd", success: "#afcb91",
  },
  paper: {
    background: "#f2efdf", text: "#252d32", muted: "#626057", accent: "#12605a",
    success: "#38631e", bar: "#314f63", bar_text: "#ffffff", border: "#6a7976",
    info: "#22556d", warning: "#825400", error: "#a13239", special: "#703e85",
    bright: "#151a1d",
  },
};
const valid = (role, value) => /^#[\da-f]{6}$/i.test(value)
  || (role === "background" && value === "default");

export const toToml = (colors) => {
  for (const [role, value] of Object.entries(colors)) {
    if (!Object.hasOwn(defaults, role) || !valid(role, value)) throw new Error(`Invalid color: ${role}`);
  }
  return "[colors]\n" + roles.map(([role]) => `${role} = "${colors[role] ?? defaults[role]}"`).join("\n") + "\n";
};

// the designer imports the flat color table it exports; the runtime reads full TOML
export const fromToml = (source) => {
  if (new TextEncoder().encode(source).length > 16 * 1024) throw new Error("Theme exceeds 16 KiB.");
  const colors = { ...defaults };
  const seen = new Set();
  let table = false;
  for (const raw of source.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    if (/^\[colors\]\s*(?:#.*)?$/.test(line) && !table) { table = true; continue; }
    const match = /^([a-z_]+)\s*=\s*(["'])(#[\da-fA-F]{6}|default)\2\s*(?:#.*)?$/.exec(line);
    if (!table || !match || !Object.hasOwn(defaults, match[1]) || seen.has(match[1]) || !valid(match[1], match[3])) {
      throw new Error("Use one [colors] table with unique role = \"#RRGGBB\" entries. See the theme guide.");
    }
    colors[match[1]] = match[3].toLowerCase();
    seen.add(match[1]);
  }
  if (!table) throw new Error("The theme needs a [colors] table.");
  return colors;
};

const luminance = (hex) => {
  const channels = [1, 3, 5].map((offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255)
    .map((value) => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
  return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722;
};
export const contrast = (a, b) => {
  const [light, dark] = [luminance(a), luminance(b)].sort((a, b) => b - a);
  return (light + .05) / (dark + .05);
};
