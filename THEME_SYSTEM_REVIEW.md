# Spacesly Theme System Review

## Architecture Review

Before this change, Spacesly had a theme catalog and a small Svelte store, but the visible application was effectively fixed to a dark palette:

- `page.css` shadowed the root theme variables inside `.stage`.
- Settings displayed a static theme card with no controls.
- xterm and CodeMirror each owned an independent hardcoded dark palette.
- System mode used only `matchMedia`, did not use Tauri's native theme event, and leaked its listener on remount.
- Explicit modes did not update the browser/native `color-scheme`.
- Theme change events were dispatched globally but had no consumers.
- `app.css` duplicated the default Amber palette from `themes.ts`.

The implemented data flow is:

```text
Persisted ThemeMode
       +
Tauri native appearance (browser media fallback)
       |
       v
Central Theme Manager
  - mode: system | light | dark
  - resolvedTheme: light | dark
  - active palette definition
       |
       +--> root data attributes + color-scheme + CSS variables
       +--> xterm theme update
       +--> CodeMirror Compartment reconfiguration
       +--> reactive Settings UI
```

`src/lib/stores/theme.svelte.ts` is the only runtime owner of theme state, persistence, OS listeners, resolution, and UI notifications. Components consume resolved colors and never inspect the operating system preference themselves.

## Changes Made

- Added pure `ThemeMode` parsing and resolution in `src/lib/themeMode.ts`.
- Restored persisted mode during module initialization and applies it before mount.
- Added idempotent initialization and listener cleanup.
- Added Tauri `setTheme()` integration and the `core:app:allow-set-app-theme` capability.
- Added native startup detection through `getCurrentWindow().theme()`.
- Added live native updates through `onThemeChanged()`.
- Retained `matchMedia` as a browser and unsupported-platform fallback.
- Added root `data-theme`, `data-theme-mode`, and `data-resolved-theme` attributes.
- Explicitly updates root `color-scheme` for native controls and scrollbars.
- Replaced global custom events with a manager-owned subscription API.
- Reconfigured live xterm and CodeMirror instances without recreating them.
- Replaced the Settings placeholder with accessible System, Light, and Dark radio cards.
- Theme changes persist and apply immediately; the unrelated Settings save action is hidden on the Theme page.
- Removed fixed-dark route and component colors in favor of semantic CSS variables.
- Added contrast and palette-shape tests in `tests/themes.ts`.

## Components Updated

- Application shell, title bar, status bar, sidebar, board, task list, task cards
- Agent Console, Activity Log, progress, approval, error, and result states
- Workspace chat, session menu, messages, input, empty and loading states
- File browser, source control, branch picker, context menus, workspace search
- Settings pages, dialogs, confirmation dialogs, forms, dropdowns, radio controls
- Notifications, popovers, code blocks, detail views, empty and loading states
- xterm terminal palette and CodeMirror chrome/syntax palette
- Buttons, inputs, focus states, disabled states, hover and selected states
- CodeMirror tooltip and native-title tooltips through inherited/native color scheme

Spacesly currently has no authored table component. Existing prose and preformatted output are tokenized; chat content is plain pre-wrapped text rather than a separate Markdown renderer.

## Semantic Tokens

The existing palette primitives remain in `themes.ts`. `app.css` derives component semantics from them:

- Surfaces: `--surface`, `--surface-raised`, `--surface-overlay`, `--surface-inset`, `--surface-hover`, `--surface-selected`
- Borders: `--border-subtle`, `--border-strong`, `--border-interactive`
- Interaction: `--focus-ring`, `--focus-border`, `--selection-bg`, `--selection-border`, `--disabled-opacity`
- Text: existing text hierarchy plus `--text-link`
- Status: `--success`, `--warning`, `--danger`, `--info` with background and border variants
- Progress: `--progress-track`, `--progress-fill`
- Specialized surfaces: `--dialog-bg`, `--menu-bg`, `--tooltip-bg`, `--code-text`
- Elevation: `--shadow-popover`, `--shadow-dialog`

The active palette still owns the actual light and dark values. Adding a future palette requires one catalog entry and no component changes.

## Removed Legacy Theme Code

- Fixed `.stage` palette overrides
- Static theme Settings placeholder
- Hardcoded xterm dark theme
- Hardcoded Soft Dracula CodeMirror theme
- Unused `spacesly-theme-change` global event
- Unused ANSI CSS-variable publication
- Special-case light editor accessor
- Forced `color-scheme: dark` declarations
- Duplicated Amber dark/light palette in `app.css`
- Fixed dark colors in route and component styles

## Accessibility Verification

- Theme tests enforce at least 4.5:1 contrast for primary text, bright text, accent, success, and failure foregrounds against the base background in every palette and resolved mode.
- Light-mode accent/status values that failed this threshold were darkened.
- Keyboard focus remains visible through focus borders, rings, or inset focus indicators.
- Selected, hover, disabled, warning, success, danger, and info states use separate semantic treatments in both modes.
- Text selection uses resolved palette tokens.
- Native controls use the resolved `color-scheme` rather than the OS theme when an explicit mode is selected.

## Performance

Theme switching does not reload or recreate the application. One switch performs a constant-size root CSS-variable update, then updates the existing xterm theme object and reconfigures one CodeMirror theme compartment. Board, session, editor document, terminal buffer, and application business state retain identity.

## Validation

- Theme mode, palette-shape, and WCAG contrast tests pass.
- Existing frontend test suites pass.
- Svelte diagnostics pass with 0 errors and 0 warnings.
- ESLint passes.
- Production frontend build passes.
- Tauri debug application build passes, including capability validation.
- Rust tests pass: 235 tests.
- The built application starts successfully with the repository's software-rendering environment.
- Changed-file Prettier and `git diff --check` pass.

The existing Rust dead-code warnings for `inject_global_environment_pty` and `replace_mcp_connector` remain unrelated to the theme system.

## Screenshots

Current environment limitations prevented automated screenshot capture: no Chromium/Firefox/Playwright browser and no Wayland screenshot utility are installed. The existing untracked `artifacts/Screenshot 2026-08-01 110314.png` predates this implementation and was not modified or represented as evidence.

Required captures remain:

- Light mode: Settings Appearance page with Light selected
- Dark mode: Settings Appearance page with Dark selected
- System mode: Settings Appearance page with System selected and its resolved appearance badge

## Remaining Inconsistencies

No fixed-dark color declarations remain in authored route or component styles. Remaining raw colors are intentionally confined to palette definitions, terminal/editor palette data, and semantic token construction.

Visual screenshot review on Windows and macOS remains outstanding because this implementation environment is Linux-only and cannot capture windows. Native integration uses Tauri's cross-platform app/window APIs rather than platform-specific code.
