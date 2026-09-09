import { test, expect } from "bun:test";
import { mkdtemp, readFile, writeFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fromAns, toAns } from "../../site/assets/studio/format.js";
import { artifactCommand } from "../../site/assets/studio/install.js";
import { defaults, presets, toToml, fromToml, contrast } from "../../site/assets/theme/format.js";

const pixelColors = (art) => [...art.pixels].map((index) => art.palette[index] ?? null);

test("ANSI preserves foreground, background, transparency and extended colors", () => {
  const source = "\x1b[38;2;12;34;56;48;2;65;43;21m▀\x1b[0m \x1b[38;5;196m▄\x1b[0m\n";
  const art = fromAns(source);
  expect(pixelColors(art)).toEqual(["#0c2238", null, null, "#412b15", null, "#ff0000"]);
  expect(pixelColors(fromAns(toAns(art)))).toEqual(pixelColors(art));
});

test("mascot starter round trips through the pixel editor", async () => {
  const art = fromAns(await Bun.file("site/assets/mascot.ans").text());
  expect(art.width).toBe(28);
  expect(art.height).toBe(28);
  expect(pixelColors(fromAns(toAns(art)))).toEqual(pixelColors(art));
});

test("ANSI import rejects controls, invalid colors and oversized artwork", () => {
  for (const source of ["", "text", "\x1b[2J", "\x1b]0;title\x07", "\x1b[38;2;999;0;0m█",
    "\x1b[38;5;256m█", "\x1b[31m" + "█".repeat(241), "\x1b[31m" + "█\n".repeat(81),
    " ".repeat(128 * 1024 + 1)]) expect(() => fromAns(source)).toThrow();
});

test("theme exports parse as real TOML and keep every runtime palette role", async () => {
  const source = await Bun.file("src/theme.rs").text();
  const runtime = [...source.matchAll(/\("([a-z_]+)", Color::/g)].map((match) => match[1]);
  expect(Object.keys(defaults).sort()).toEqual(runtime.sort());
  for (const colors of Object.values(presets)) {
    const toml = toToml(colors);
    expect(Bun.TOML.parse(toml)).toEqual({ colors });
    expect(fromToml(toml)).toEqual(colors);
  }
  expect(contrast("#ffffff", "#000000")).toBe(21);
});

test("theme import rejects unknown, duplicate and malformed entries", () => {
  for (const source of ["", "[colors]\ntext = 'red'", "[colors]\nunknown = '#ffffff'",
    "[colors]\ntext = '#ffffff'\ntext = '#000000'", "[colors]\n[commands]",
    " ".repeat(16385)]) expect(() => fromToml(source)).toThrow();
});

test("installation writes exact files and refuses to overwrite files or symlinks", async () => {
  const root = await mkdtemp(join(tmpdir(), "ainzstudio"));
  const platform = process.platform === "darwin" ? "macos" : "linux";
  try {
    for (const [kind, extension, content] of [
      ["theme", "toml", toToml(presets.nazarick)],
      ["header", "ans", await Bun.file("site/assets/mascot.ans").text()],
    ]) {
      const command = artifactCommand(content, "mine", platform, "project", kind);
      const run = () => Bun.spawn(["sh", "-c", command], { cwd: root, stdout: "pipe", stderr: "pipe" });
      expect(await run().exited).toBe(0);
      const path = join(root, `.ainz/${kind}s/mine.${extension}`);
      expect(await readFile(path, "utf8")).toBe(content);
      await writeFile(path, "keep me");
      expect(await run().exited).toBe(1);
      expect(await readFile(path, "utf8")).toBe("keep me");
      await rm(path);
      await symlink(join(root, "absent"), path);
      expect(await run().exited).toBe(1);
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("installation rejects names that could escape the file or shell", () => {
  for (const name of ["../outside", "$(date)", "`date`", "a b", "a\nb", "-test", ""]) {
    expect(() => artifactCommand("[colors]\n", name, "macos", "project", "theme")).toThrow();
  }
  expect(() => artifactCommand("", "default", "macos", "project", "theme")).toThrow();
  expect(() => artifactCommand("", "mascot", "macos", "project", "header")).toThrow();
});
