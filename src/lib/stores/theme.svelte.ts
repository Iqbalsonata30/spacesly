import {
  themes,
  defaultThemeId,
  type EditorColors,
  type TerminalColors,
  type ThemeDefinition,
  type ThemeId,
} from "$lib/themes";
import {
  parseThemeMode,
  resolveThemeMode,
  type ResolvedTheme,
  type ThemeMode,
} from "$lib/themeMode";
import { SvelteSet } from "svelte/reactivity";

const THEME_STORAGE_KEY = "spacesly-theme";
const MODE_STORAGE_KEY = "spacesly-color-mode";

export type ThemeSnapshot = {
  id: ThemeId;
  mode: ThemeMode;
  resolvedTheme: ResolvedTheme;
  theme: ThemeDefinition;
};

type ThemeListener = (snapshot: ThemeSnapshot) => void;

let activeId = $state<ThemeId>(readStoredTheme());
let mode = $state<ThemeMode>(readStoredMode());
let systemTheme = $state<ResolvedTheme>(browserSystemTheme());
let resolvedTheme = $state<ResolvedTheme>("dark");
let initializationCount = 0;
let stopSystemListeners: (() => void) | null = null;
let nativeThemeListenerActive = false;
const listeners = new SvelteSet<ThemeListener>();

function readStoredTheme(): ThemeId {
  if (typeof localStorage === "undefined") return defaultThemeId;
  const stored = localStorage.getItem(THEME_STORAGE_KEY);
  return stored && stored in themes ? (stored as ThemeId) : defaultThemeId;
}

function readStoredMode(): ThemeMode {
  if (typeof localStorage === "undefined") return "system";
  return parseThemeMode(localStorage.getItem(MODE_STORAGE_KEY));
}

function browserSystemTheme(): ResolvedTheme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function snapshot(): ThemeSnapshot {
  return { id: activeId, mode, resolvedTheme, theme: themes[activeId] };
}

function applyTheme(notify = true): void {
  resolvedTheme = resolveThemeMode(mode, systemTheme);
  const current = snapshot();

  if (typeof document !== "undefined") {
    const root = document.documentElement;
    const variables = current.theme.css[resolvedTheme];
    for (const [key, value] of Object.entries(variables)) {
      root.style.setProperty(`--${key}`, value);
    }
    root.dataset.theme = activeId;
    root.dataset.themeMode = mode;
    root.dataset.resolvedTheme = resolvedTheme;
    root.style.colorScheme = resolvedTheme;
  }

  if (notify) {
    for (const listener of listeners) listener(current);
  }
}

async function syncNativeThemePreference(): Promise<void> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  try {
    const { setTheme } = await import("@tauri-apps/api/app");
    await setTheme(mode === "system" ? null : mode);
    if (mode === "system") {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const currentTheme = await getCurrentWindow().theme();
      if (currentTheme) {
        systemTheme = currentTheme;
        applyTheme();
      }
    }
  } catch (reason) {
    console.warn("Unable to synchronize the native application theme", reason);
  }
}

async function attachNativeThemeListener(disposed: () => boolean): Promise<() => void> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return () => {};

  try {
    await syncNativeThemePreference();
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const appWindow = getCurrentWindow();
    nativeThemeListenerActive = true;
    const initialTheme = await appWindow.theme();
    if (initialTheme && mode === "system" && !disposed()) {
      systemTheme = initialTheme;
      applyTheme();
    }
    const unlisten = await appWindow.onThemeChanged(({ payload }) => {
      if (mode !== "system") return;
      systemTheme = payload;
      applyTheme();
    });
    if (disposed()) {
      unlisten();
      return () => {};
    }
    return unlisten;
  } catch (reason) {
    nativeThemeListenerActive = false;
    console.warn("Unable to listen for native theme changes; using browser preference", reason);
    return () => {};
  }
}

export function getThemeId(): ThemeId {
  return activeId;
}

export function getTheme(): ThemeDefinition {
  return themes[activeId];
}

export function getThemeMode(): ThemeMode {
  return mode;
}

export function getResolvedTheme(): ResolvedTheme {
  return resolvedTheme;
}

export function getThemeSnapshot(): ThemeSnapshot {
  return snapshot();
}

export function getTerminalTheme(): TerminalColors {
  return themes[activeId].terminal[resolvedTheme];
}

export function getEditorColors(): EditorColors {
  return themes[activeId].editor[resolvedTheme];
}

export function setTheme(id: ThemeId): void {
  if (!(id in themes) || id === activeId) return;
  activeId = id;
  if (typeof localStorage !== "undefined") localStorage.setItem(THEME_STORAGE_KEY, id);
  applyTheme();
}

export function setThemeMode(nextMode: ThemeMode): void {
  if (nextMode === mode) return;
  mode = nextMode;
  if (typeof localStorage !== "undefined") localStorage.setItem(MODE_STORAGE_KEY, nextMode);
  applyTheme();
  void syncNativeThemePreference();
}

export function onThemeChange(listener: ThemeListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function initTheme(): () => void {
  initializationCount += 1;
  applyTheme(false);
  if (initializationCount > 1) return releaseThemeManager;

  let disposed = false;
  let unlistenNative = () => {};
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const handleBrowserThemeChange = (event: MediaQueryListEvent) => {
    if (mode !== "system" || nativeThemeListenerActive) return;
    systemTheme = event.matches ? "dark" : "light";
    applyTheme();
  };
  media.addEventListener("change", handleBrowserThemeChange);
  void attachNativeThemeListener(() => disposed).then((unlisten) => {
    unlistenNative = unlisten;
  });

  stopSystemListeners = () => {
    disposed = true;
    nativeThemeListenerActive = false;
    media.removeEventListener("change", handleBrowserThemeChange);
    unlistenNative();
  };
  return releaseThemeManager;
}

function releaseThemeManager(): void {
  initializationCount = Math.max(0, initializationCount - 1);
  if (initializationCount !== 0) return;
  stopSystemListeners?.();
  stopSystemListeners = null;
}

// Apply the persisted preference during initial component evaluation to avoid a fixed-theme flash.
applyTheme(false);
