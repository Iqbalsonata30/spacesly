import { glob, readFile } from "node:fs/promises";

const rawActionGlyph = />\s*[×⌄←→↻▾▸]\s*</u;
const decoratedTextAction = />\s*＋\s*[\p{L}]/u;

for await (const path of glob("src/**/*.svelte")) {
  const source = await readFile(path, "utf8");
  if (rawActionGlyph.test(source)) {
    throw new Error(`${path} uses a raw action glyph instead of lucide-svelte.`);
  }
  if (decoratedTextAction.test(source)) {
    throw new Error(`${path} decorates an already labeled action with a full-width plus.`);
  }
  if (source.includes("<svg")) {
    throw new Error(`${path} contains authored SVG instead of the shared icon library.`);
  }
}

console.log("icon usage tests passed");
