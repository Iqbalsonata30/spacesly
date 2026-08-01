export const themeModes = ["system", "light", "dark"] as const;

export type ThemeMode = (typeof themeModes)[number];
export type ResolvedTheme = Exclude<ThemeMode, "system">;

export function parseThemeMode(value: string | null): ThemeMode {
  return themeModes.includes(value as ThemeMode) ? (value as ThemeMode) : "system";
}

export function resolveThemeMode(mode: ThemeMode, systemTheme: ResolvedTheme): ResolvedTheme {
  return mode === "system" ? systemTheme : mode;
}
