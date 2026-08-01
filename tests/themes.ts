import { defaultThemeId, themeList, themes } from "../src/lib/themes";
import { parseThemeMode, resolveThemeMode, themeModes } from "../src/lib/themeMode";

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

function relativeLuminance(hex: string): number {
  const channels = hex
    .slice(1)
    .match(/../g)!
    .map((value) => Number.parseInt(value, 16) / 255)
    .map((value) => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4));
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrastRatio(foreground: string, background: string): number {
  const first = relativeLuminance(foreground);
  const second = relativeLuminance(background);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

assert(parseThemeMode(null) === "system", "missing mode should default to system");
assert(parseThemeMode("unknown") === "system", "invalid mode should default to system");
for (const mode of themeModes) {
  assert(parseThemeMode(mode) === mode, `${mode} should be accepted`);
}
assert(resolveThemeMode("system", "light") === "light", "system should resolve to OS light");
assert(resolveThemeMode("system", "dark") === "dark", "system should resolve to OS dark");
assert(resolveThemeMode("light", "dark") === "light", "explicit light should ignore OS dark");
assert(resolveThemeMode("dark", "light") === "dark", "explicit dark should ignore OS light");

assert(defaultThemeId in themes, "default theme must exist");
assert(themeList.length === Object.keys(themes).length, "theme list and theme record must agree");
assert(
  new Set(themeList.map((theme) => theme.id)).size === themeList.length,
  "theme IDs must be unique",
);

for (const theme of themeList) {
  assert(themes[theme.id] === theme, `${theme.id} must use one shared definition`);
  assert(
    JSON.stringify(Object.keys(theme.css.dark).sort()) ===
      JSON.stringify(Object.keys(theme.css.light).sort()),
    `${theme.id} CSS token keys must match across modes`,
  );
  assert(
    JSON.stringify(Object.keys(theme.terminal.dark).sort()) ===
      JSON.stringify(Object.keys(theme.terminal.light).sort()),
    `${theme.id} terminal token keys must match across modes`,
  );
  assert(
    JSON.stringify(Object.keys(theme.editor.dark).sort()) ===
      JSON.stringify(Object.keys(theme.editor.light).sort()),
    `${theme.id} editor token keys must match across modes`,
  );

  for (const resolved of ["light", "dark"] as const) {
    const palette = theme.css[resolved];
    for (const token of ["text-primary", "text-bright", "accent", "status-ok", "status-fail"]) {
      const ratio = contrastRatio(palette[token], palette["bg-base"]);
      assert(ratio >= 4.5, `${theme.id} ${resolved} ${token} contrast is ${ratio.toFixed(2)}:1`);
    }
  }
}

console.log("theme tests passed");
