import { MAX_BYTES } from "./format.js";

const reserved = new Set(["random", "builtin", "mascot", "mascotascii", "ainz", "ainzascii"]);

export const nameError = (name, kind = "header") => {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,39}$/.test(name)) {
    return "Use 1–40 letters, numbers, underscores, or hyphens. Start with a letter or number.";
  }
  if (kind === "theme" ? name.toLowerCase() === "default" : reserved.has(name.toLowerCase())) {
    return `That name is built in. Choose a name for your own ${kind}.`;
  }
  return "";
};

export const artifactCommand = (content, name, platform, scope, kind) => {
  if (!["header", "theme"].includes(kind)) throw new Error("Unknown installation type.");
  const error = nameError(name, kind);
  if (error) throw new Error(error);
  const bytes = new TextEncoder().encode(content);
  const limit = kind === "header" ? MAX_BYTES : 16 * 1024;
  if (bytes.length > limit) throw new Error(`File exceeds the ${limit / 1024} KiB limit.`);
  if (!["macos", "linux"].includes(platform) || !["user", "project"].includes(scope)) {
    throw new Error("Choose an installation platform and scope.");
  }
  const payload = btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""));
  const folder = `${kind}s`;
  const extension = kind === "header" ? "ans" : "toml";
  const directory = scope === "project" ? `.ainz/${folder}`
    : platform === "macos" ? `$HOME/Library/Application Support/ainz/${folder}`
    : '${XDG_CONFIG_HOME:-$HOME/.config}/ainz/' + folder;
  const decode = platform === "macos" ? "-D" : "--decode";
  return `(
  set -eu
  ainz_dir="${directory}"
  mkdir -p "$ainz_dir"
  ainz_file="$ainz_dir/${name}.${extension}"
  if [ -e "$ainz_file" ] || [ -L "$ainz_file" ]; then
    printf '%s\\n' 'A ${kind} with this name already exists. Choose another name in the designer.' >&2
    exit 1
  fi
  (set -C; printf '%s' '${payload}' | base64 ${decode} > "$ainz_file")
  printf '%s\\n' 'Installed ${name}. In Ainz, run: /${kind} ${name}'
)`;
};

export const installCommand = (art, name, platform, scope) =>
  artifactCommand(art, name, platform, scope, "header");
