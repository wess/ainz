import { test, expect } from "bun:test";
import { resolve, dirname } from "node:path";
import { stat } from "node:fs/promises";

test("site links, assets and fragment targets resolve", async () => {
  const root = resolve("site");
  for await (const relative of new Bun.Glob("**/*.html").scan(root)) {
    const file = resolve(root, relative);
    const html = await Bun.file(file).text();
    const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]);
    expect(new Set(ids).size, `${relative}: duplicate id`).toBe(ids.length);
    for (const [, value] of html.matchAll(/(?:href|src)="([^"]+)"/g)) {
      if (/^(https?:|mailto:|data:)/.test(value)) continue;
      const [path, fragment] = value.split("#");
      let target = path ? path.startsWith("/ainz/") ? resolve(root, path.slice(6)) : resolve(dirname(file), path) : file;
      expect(target.startsWith(root), `${relative}: ${value}`).toBe(true);
      if ((await stat(target)).isDirectory()) target = resolve(target, "index.html");
      expect(await Bun.file(target).exists(), `${relative}: ${value}`).toBe(true);
      if (fragment) {
        const content = await Bun.file(target).text();
        expect(content.includes(`id="${fragment}"`), `${relative}: ${value}`).toBe(true);
      }
    }
  }
});

test("designer modules reference existing controls", async () => {
  for (const [page, entry] of [["masthead", "masthead.js"], ["theme", "theme.js"]]) {
    const html = await Bun.file(`site/${page}/index.html`).text();
    const visited = new Set();
    const pending = [resolve("site/assets", entry)];
    while (pending.length) {
      const path = pending.pop();
      if (visited.has(path)) continue;
      visited.add(path);
      const source = await Bun.file(path).text();
      for (const [, id] of source.matchAll(/(?:\$|getElementById)\("([^"]+)"\)/g)) {
        expect(html.includes(`id="${id}"`), `${page}: missing ${id}`).toBe(true);
      }
      for (const [, target] of source.matchAll(/from "(\.[^"]+)"/g)) {
        pending.push(resolve(dirname(path), target));
      }
    }
  }
});

test("tutorial paths are published and stamped with the package version", async () => {
  const cargo = await Bun.file("Cargo.toml").text();
  const version = cargo.match(/^version = "([^"]+)"/m)?.[1];
  const paths = ["gettingstarted", "customizing", "plugins", "advanced"];
  const hub = await Bun.file("site/tutorial/index.html").text();

  expect(version).toBeTruthy();
  for (const path of paths) {
    expect(hub.includes(`href="${path}/"`), `tutorial hub: ${path}`).toBe(true);
    const page = await Bun.file(`site/tutorial/${path}/index.html`).text();
    expect(page.includes(`Ainz ${version}`), `${path}: release version`).toBe(true);
    expect(page.includes(">Tutorials</a>"), `${path}: tutorial navigation`).toBe(true);
  }
});
