export type ThemeId = "amber" | "indigo" | "peach" | "slate";

export interface TerminalColors {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export interface EditorColors {
  keyword: string;
  operator: string;
  string: string;
  number: string;
  comment: string;
  docComment: string;
  variable: string;
  property: string;
  link: string;
  text: string;
  textSecondary: string;
  textMuted: string;
  bg: string;
  bgCard: string;
  accentRgba: string;
  borderColor: string;
}

export interface ThemeDefinition {
  id: ThemeId;
  name: string;
  css: { dark: Record<string, string>; light: Record<string, string> };
  terminal: { dark: TerminalColors; light: TerminalColors };
  editor: { dark: EditorColors; light: EditorColors };
}

/** Extract unique solid hex colors from a theme's dark CSS tokens (order-preserving). */
export function getPreviewColors(theme: ThemeDefinition): string[] {
  const seen = new Set<string>();
  const colors: string[] = [];
  for (const v of Object.values(theme.css.dark)) {
    const hex = v.toLowerCase();
    if (/^#[0-9a-f]{6}$/.test(hex) && !seen.has(hex)) {
      seen.add(hex);
      colors.push(hex);
    }
  }
  return colors;
}

const premiumLightCss: Record<string, string> = {
  "bg-sidebar": "rgba(248, 251, 255, 0.84)",
  "bg-base": "#eef3fa",
  "bg-titlebar": "rgba(255, 255, 255, 0.76)",
  "bg-card": "rgba(255, 255, 255, 0.78)",
  "bg-hover": "rgba(94, 127, 232, 0.08)",
  "bg-active": "#e7edff",
  border: "rgba(91, 116, 160, 0.14)",
  "border-light": "rgba(91, 116, 160, 0.26)",
  "text-muted": "#afbacb",
  "text-dim": "#8b99b1",
  "text-secondary": "#627391",
  "text-primary": "#283b61",
  "text-bright": "#203252",
  accent: "#4563bf",
  "status-ok": "#2f775d",
  "status-pr-open": "#416fa8",
  "status-fail": "#b34c57",
  "diff-add": "#2f775d",
  "diff-add-bg": "#e6f5ee",
  "diff-del": "#b34c57",
  "diff-del-bg": "#fcebed",
  error: "#b34c57",
  "error-bg": "#fcebed",
  "toast-bg": "rgba(255, 255, 255, 0.88)",
  "toast-border": "rgba(91, 116, 160, 0.16)",
  "overlay-bg": "rgba(35, 49, 78, 0.24)",
  "input-inset-bg": "rgba(248, 251, 255, 0.88)",
  "input-inset-focus": "rgba(255, 255, 255, 0.96)",
  "btn-subtle-bg": "rgba(255, 255, 255, 0.72)",
  "btn-subtle-hover": "rgba(94, 127, 232, 0.1)",
  "pill-btn-hover": "rgba(94, 127, 232, 0.09)",
  "code-inline-bg": "rgba(94, 127, 232, 0.08)",
  "code-block-bg": "rgba(236, 242, 252, 0.92)",
  "img-remove-bg": "rgba(35, 49, 78, 0.68)",
  "img-remove-hover": "rgba(35, 49, 78, 0.84)",
  "bg-dev": "#f1effc",
  "border-dev": "rgba(117, 101, 175, 0.2)",
};

const premiumLightTerminal: TerminalColors = {
  background: "#f8fbff",
  foreground: "#283b61",
  cursor: "#5e7fe8",
  cursorAccent: "#ffffff",
  selectionBackground: "#5e7fe844",
  black: "#afbacb",
  red: "#cf606a",
  green: "#3e9877",
  yellow: "#a87930",
  blue: "#4f80bc",
  magenta: "#8b68b8",
  cyan: "#3f8d9d",
  white: "#283b61",
  brightBlack: "#8b99b1",
  brightRed: "#dc737c",
  brightGreen: "#4ca989",
  brightYellow: "#bd8735",
  brightBlue: "#5e7fe8",
  brightMagenta: "#9b78c8",
  brightCyan: "#50a0b0",
  brightWhite: "#203252",
};

const premiumLightEditor: EditorColors = {
  keyword: "#5e67b9",
  operator: "#627391",
  string: "#357f64",
  number: "#a95d70",
  comment: "#7d8ba3",
  docComment: "#71829d",
  variable: "#283b61",
  property: "#456ea8",
  link: "#4f70d0",
  text: "#283b61",
  textSecondary: "#627391",
  textMuted: "#8b99b1",
  bg: "#f8fbff",
  bgCard: "#f1f5fc",
  accentRgba: "94, 127, 232",
  borderColor: "rgba(91, 116, 160, 0.22)",
};

const amber: ThemeDefinition = {
  id: "amber",
  name: "Soft Dracula",
  css: {
    dark: {
      "bg-sidebar": "#0f0d12",
      "bg-base": "#111016",
      "bg-titlebar": "#1c1b21",
      "bg-card": "#191820",
      "bg-hover": "#1f1e27",
      "bg-active": "#282631",
      border: "#25232d",
      "border-light": "#34313d",
      "text-muted": "#5f596e",
      "text-dim": "#706980",
      "text-secondary": "#a19ab4",
      "text-primary": "#d7d0e2",
      "text-bright": "#f1edf5",
      accent: "#b8d6e4",
      "status-ok": "#b9d6aa",
      "status-pr-open": "#b89adf",
      "status-fail": "#f0b0aa",
      "diff-add": "#9dbb83",
      "diff-add-bg": "#18241c",
      "diff-del": "#f0b0aa",
      "diff-del-bg": "#2a181d",
      error: "#f0b0aa",
      "error-bg": "#2a181d",
      "toast-bg": "rgba(25, 24, 32, 0.84)",
      "toast-border": "rgba(255, 255, 255, 0.06)",
      "overlay-bg": "rgba(0, 0, 0, 0.5)",
      "input-inset-bg": "rgba(0, 0, 0, 0.25)",
      "input-inset-focus": "rgba(0, 0, 0, 0.3)",
      "btn-subtle-bg": "rgba(255, 255, 255, 0.06)",
      "btn-subtle-hover": "rgba(255, 255, 255, 0.1)",
      "pill-btn-hover": "rgba(255, 255, 255, 0.08)",
      "code-inline-bg": "rgba(255, 255, 255, 0.05)",
      "code-block-bg": "rgba(0, 0, 0, 0.3)",
      "img-remove-bg": "rgba(0, 0, 0, 0.65)",
      "img-remove-hover": "rgba(0, 0, 0, 0.85)",
      "bg-dev": "#191726",
      "border-dev": "#312d43",
    },
    light: premiumLightCss,
  },
  terminal: {
    dark: {
      background: "#09090d",
      foreground: "#d7d0e2",
      cursor: "#b8d6e4",
      cursorAccent: "#111016",
      selectionBackground: "#9983c444",
      black: "#111016",
      red: "#f0b0aa",
      green: "#9dbb83",
      yellow: "#e7d38f",
      blue: "#8db9d6",
      magenta: "#b89adf",
      cyan: "#91c3d0",
      white: "#d7d0e2",
      brightBlack: "#706980",
      brightRed: "#f3c0bb",
      brightGreen: "#b9d6aa",
      brightYellow: "#f0dfa8",
      brightBlue: "#b8d6e4",
      brightMagenta: "#c9a7e8",
      brightCyan: "#a8dce8",
      brightWhite: "#f1edf5",
    },
    light: premiumLightTerminal,
  },
  editor: {
    dark: {
      keyword: "#b89adf",
      operator: "#aaa4bb",
      string: "#9dbb83",
      number: "#d8a789",
      comment: "#767b90",
      docComment: "#858aa0",
      variable: "#d8d2e4",
      property: "#91c3d0",
      link: "#8db9d6",
      text: "#d8d2e4",
      textSecondary: "#a19ab4",
      textMuted: "#706980",
      bg: "#191820",
      bgCard: "#15141b",
      accentRgba: "153, 131, 196",
      borderColor: "#34313d",
    },
    light: premiumLightEditor,
  },
};

// ── Indigo (purple-lavender-ice palette) ──────────────────

const indigo: ThemeDefinition = {
  id: "indigo",
  name: "Indigo",
  css: {
    dark: {
      "bg-sidebar": "#111014",
      "bg-base": "#141318",
      "bg-titlebar": "#1b1a1f",
      "bg-card": "#19181d",
      "bg-hover": "#1f1e24",
      "bg-active": "#27262c",
      border: "#222128",
      "border-light": "#32313a",
      "text-muted": "#48475a",
      "text-dim": "#62607a",
      "text-secondary": "#9088a8",
      "text-primary": "#d8d2e4",
      "text-bright": "#eee8f4",
      accent: "#A3C7D6",
      "status-ok": "#7e9e6b",
      "status-pr-open": "#7e8ec8",
      "status-fail": "#c87878",
      "diff-add": "#7e9e6b",
      "diff-add-bg": "#141e16",
      "diff-del": "#c87878",
      "diff-del-bg": "#201418",
      error: "#d06060",
      "error-bg": "#201418",
      "toast-bg": "rgba(20, 19, 24, 0.85)",
      "toast-border": "rgba(255, 255, 255, 0.06)",
      "overlay-bg": "rgba(0, 0, 0, 0.5)",
      "input-inset-bg": "rgba(0, 0, 0, 0.25)",
      "input-inset-focus": "rgba(0, 0, 0, 0.3)",
      "btn-subtle-bg": "rgba(255, 255, 255, 0.06)",
      "btn-subtle-hover": "rgba(255, 255, 255, 0.1)",
      "pill-btn-hover": "rgba(255, 255, 255, 0.08)",
      "code-inline-bg": "rgba(255, 255, 255, 0.05)",
      "code-block-bg": "rgba(0, 0, 0, 0.3)",
      "img-remove-bg": "rgba(0, 0, 0, 0.65)",
      "img-remove-hover": "rgba(0, 0, 0, 0.85)",
      "bg-dev": "#171620",
      "border-dev": "#2a2932",
    },
    light: premiumLightCss,
  },
  terminal: {
    dark: {
      background: "#141318",
      foreground: "#d8d2e4",
      cursor: "#A3C7D6",
      cursorAccent: "#141318",
      selectionBackground: "#A3C7D644",
      black: "#1f1e24",
      red: "#c87878",
      green: "#7e9e6b",
      yellow: "#d4b878",
      blue: "#A3C7D6",
      magenta: "#9F73AB",
      cyan: "#78b8b8",
      white: "#d8d2e4",
      brightBlack: "#625e78",
      brightRed: "#e09090",
      brightGreen: "#a0c890",
      brightYellow: "#e8d098",
      brightBlue: "#b8d8e8",
      brightMagenta: "#b898c0",
      brightCyan: "#98d0d0",
      brightWhite: "#eee8f4",
    },
    light: premiumLightTerminal,
  },
  editor: {
    dark: {
      keyword: "#A3C7D6",
      operator: "#9088a8",
      string: "#7e9e6b",
      number: "#9F73AB",
      comment: "#625e78",
      docComment: "#706890",
      variable: "#d8d2e4",
      property: "#b0a8c8",
      link: "#A3C7D6",
      text: "#d8d2e4",
      textSecondary: "#9088a8",
      textMuted: "#48475a",
      bg: "#141318",
      bgCard: "#19181d",
      accentRgba: "163, 199, 214",
      borderColor: "#32313a",
    },
    light: premiumLightEditor,
  },
};

// ── Peach (warm coral-rose palette) ───────────────────────

const peach: ThemeDefinition = {
  id: "peach",
  name: "Peach",
  css: {
    dark: {
      "bg-sidebar": "#110d0b",
      "bg-base": "#141010",
      "bg-titlebar": "#1b1614",
      "bg-card": "#1a1614",
      "bg-hover": "#201b18",
      "bg-active": "#2a2320",
      border: "#201b18",
      "border-light": "#3a322e",
      "text-muted": "#4a4240",
      "text-dim": "#6a5854",
      "text-secondary": "#967e78",
      "text-primary": "#d8c0b6",
      "text-bright": "#ecdcd4",
      accent: "#e09880",
      "status-ok": "#7e9e6b",
      "status-pr-open": "#7e8ec8",
      "status-fail": "#c87878",
      "diff-add": "#7e9e6b",
      "diff-add-bg": "#1a2a1a",
      "diff-del": "#c87e7e",
      "diff-del-bg": "#2a1a1a",
      error: "#ee8888",
      "error-bg": "#3a1a1a",
      "toast-bg": "rgba(20, 16, 16, 0.80)",
      "toast-border": "rgba(255, 255, 255, 0.06)",
      "overlay-bg": "rgba(0, 0, 0, 0.5)",
      "input-inset-bg": "rgba(0, 0, 0, 0.25)",
      "input-inset-focus": "rgba(0, 0, 0, 0.3)",
      "btn-subtle-bg": "rgba(255, 255, 255, 0.06)",
      "btn-subtle-hover": "rgba(255, 255, 255, 0.1)",
      "pill-btn-hover": "rgba(255, 255, 255, 0.08)",
      "code-inline-bg": "rgba(255, 255, 255, 0.05)",
      "code-block-bg": "rgba(0, 0, 0, 0.3)",
      "img-remove-bg": "rgba(0, 0, 0, 0.65)",
      "img-remove-hover": "rgba(0, 0, 0, 0.85)",
      "bg-dev": "#191420",
      "border-dev": "#252030",
    },
    light: premiumLightCss,
  },
  terminal: {
    dark: {
      background: "#141010",
      foreground: "#d8c0b6",
      cursor: "#e09880",
      cursorAccent: "#141010",
      selectionBackground: "#e0988044",
      black: "#201b18",
      red: "#c87878",
      green: "#7e9e6b",
      yellow: "#d4a870",
      blue: "#7e90a8",
      magenta: "#b87888",
      cyan: "#7e9e98",
      white: "#d8c0b6",
      brightBlack: "#6a5854",
      brightRed: "#e09898",
      brightGreen: "#a0c890",
      brightYellow: "#e8c098",
      brightBlue: "#a0b0c8",
      brightMagenta: "#d0a0b0",
      brightCyan: "#a0c8c0",
      brightWhite: "#ecdcd4",
    },
    light: premiumLightTerminal,
  },
  editor: {
    dark: {
      keyword: "#e09880",
      operator: "#967e78",
      string: "#7e9e6b",
      number: "#c87878",
      comment: "#6a5854",
      docComment: "#7a6864",
      variable: "#d8c0b6",
      property: "#c0a89e",
      link: "#7e90c8",
      text: "#d8c0b6",
      textSecondary: "#967e78",
      textMuted: "#4a4240",
      bg: "#141010",
      bgCard: "#1a1614",
      accentRgba: "224, 152, 128",
      borderColor: "#3a322e",
    },
    light: premiumLightEditor,
  },
};

// ── Slate (high-contrast neutral palette) ────────────────

const slate: ThemeDefinition = {
  id: "slate",
  name: "Slate",
  css: {
    dark: {
      "bg-sidebar": "#0e0e10",
      "bg-base": "#121214",
      "bg-titlebar": "#1a1a1e",
      "bg-card": "#18181c",
      "bg-hover": "#1e1e24",
      "bg-active": "#28282e",
      border: "#2a2a30",
      "border-light": "#3c3c44",
      "text-muted": "#4a4a54",
      "text-dim": "#6a6a78",
      "text-secondary": "#9090a0",
      "text-primary": "#d4d4dc",
      "text-bright": "#ececf0",
      accent: "#7eaacc",
      "status-ok": "#7e9e6b",
      "status-pr-open": "#7e8ec8",
      "status-fail": "#c87878",
      "diff-add": "#7e9e6b",
      "diff-add-bg": "#141e16",
      "diff-del": "#c87878",
      "diff-del-bg": "#201418",
      error: "#d06060",
      "error-bg": "#201418",
      "toast-bg": "rgba(18, 18, 20, 0.88)",
      "toast-border": "rgba(255, 255, 255, 0.08)",
      "overlay-bg": "rgba(0, 0, 0, 0.55)",
      "input-inset-bg": "rgba(0, 0, 0, 0.28)",
      "input-inset-focus": "rgba(0, 0, 0, 0.35)",
      "btn-subtle-bg": "rgba(255, 255, 255, 0.07)",
      "btn-subtle-hover": "rgba(255, 255, 255, 0.12)",
      "pill-btn-hover": "rgba(255, 255, 255, 0.09)",
      "code-inline-bg": "rgba(255, 255, 255, 0.06)",
      "code-block-bg": "rgba(0, 0, 0, 0.32)",
      "img-remove-bg": "rgba(0, 0, 0, 0.68)",
      "img-remove-hover": "rgba(0, 0, 0, 0.88)",
      "bg-dev": "#161420",
      "border-dev": "#282636",
    },
    light: premiumLightCss,
  },
  terminal: {
    dark: {
      background: "#121214",
      foreground: "#d4d4dc",
      cursor: "#7eaacc",
      cursorAccent: "#121214",
      selectionBackground: "#7eaacc44",
      black: "#1e1e24",
      red: "#c87878",
      green: "#7e9e6b",
      yellow: "#d4b878",
      blue: "#7eaacc",
      magenta: "#a07898",
      cyan: "#78b0b0",
      white: "#d4d4dc",
      brightBlack: "#6a6a78",
      brightRed: "#e09090",
      brightGreen: "#a0c890",
      brightYellow: "#e8d098",
      brightBlue: "#a0c8e0",
      brightMagenta: "#c098b0",
      brightCyan: "#98d0cc",
      brightWhite: "#ececf0",
    },
    light: premiumLightTerminal,
  },
  editor: {
    dark: {
      keyword: "#7eaacc",
      operator: "#9090a0",
      string: "#7e9e6b",
      number: "#a07898",
      comment: "#6a6a78",
      docComment: "#7a7a88",
      variable: "#d4d4dc",
      property: "#b0b0c0",
      link: "#7eaacc",
      text: "#d4d4dc",
      textSecondary: "#9090a0",
      textMuted: "#4a4a54",
      bg: "#121214",
      bgCard: "#18181c",
      accentRgba: "126, 170, 204",
      borderColor: "#3c3c44",
    },
    light: premiumLightEditor,
  },
};

export const themes: Record<ThemeId, ThemeDefinition> = { amber, indigo, peach, slate };
export const themeList: ThemeDefinition[] = [amber, indigo, peach, slate];
export const defaultThemeId: ThemeId = "amber";
