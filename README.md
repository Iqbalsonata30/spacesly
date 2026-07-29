# Spacesly

**A local-first, AI-augmented desktop workspace for orchestrating daily development work.**

Spacesly combines a kanban board, code editor, shell terminal, and AI agent into a single
native desktop window. It is built with [Tauri v2](https://v2.tauri.app/) (Rust backend) and
[SvelteKit 5](https://kit.svelte.dev/) frontend, storing all data locally by default with
optional Jira synchronization.

---

## Features

- **Kanban Board** — Track work items with column workflow (Backlog → Queued → In Progress
  → Review → Done). Supports local cards and Jira issues with drag-and-drop.
- **Jira Sync** — Import boards and issues via direct Jira REST API or an MCP server.
  Transition statuses, assign users, and post comments from within the app.
- **AI Agent** — Execute immutable execution contracts through configurable workers
  (OpenAI, Anthropic, Gemini, DeepSeek, or opencode CLI). Structured results with
  automatic Jira comment posting. SQLite-lease-based concurrency prevents duplicate runs.
- **AI Chat** — Command-first assistant with workspace, board, and task context.
- **Code Editor** — Full-featured workspace file editor using CodeMirror 6 with syntax
  highlighting for JavaScript, TypeScript, Rust, Go, HTML, CSS, JSON, Markdown, YAML,
  and Svelte. Vim keybinding support.
- **LSP Integration** — Language Server Protocol support for code intelligence:
  completions, diagnostics, hover info, go-to-definition, references, document symbols,
  and code actions.
- **Local Terminal** — Embedded PTY shell session via xterm.js and portable-pty with
  resize and multi-session support.
- **Git Integration** — Branch listing/checkout, status view, staging/unstaging, commit,
  push, pull, merge, and rebase.
- **MCP Connectivity** — Connect any stdio-based MCP (Model Context Protocol) server
  for tool and agent integration (Jira, Kubernetes, Bamboo, etc.).
- **File Search & Replace** — Full-text search across workspace files with preview and
  batch replace.
- **Global Environment Variables** — Manage per-variable environment variables (normal
  or secret) that are automatically injected into all external processes (shell, MCP
  servers, AI workers, formatters, Git). Secret values are redacted from logs.
- **Secrets Storage** — API keys and credentials encrypted at rest in
  `~/.config/spacesly/secrets.json` via the system keyring.
- **AI Edit Review** — AI-proposed file edits are reviewed and confirmed before
  application. Workspace trust gating prevents unauthorized write access.
- **Recovery Snapshots** — Unsaved editor content is persisted to SQLite and restored
  after crashes.
- **Themes** — Four color themes (Amber, Indigo, Peach, Slate) with light/dark mode
  and system-following. Terminal ANSI colors and editor themes are synchronized.

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Tauri v2 Shell                     │
│  (Native window, menus, system tray, dialogs)        │
└────────────────────┬────────────────────────────────┘
                     │ Tauri IPC (JSON-RPC + streaming)
                     │
┌────────────────────┴────────────────────────────────┐
│              Frontend (SvelteKit 5 SPA)              │
│                                                     │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────┐  │
│  │  BoardPane   │  │ EditorPane   │  │ Terminal  │  │
│  │  (Kanban)    │  │ (CodeMirror) │  │ (xterm.js)│  │
│  └──────────────┘  └──────────────┘  └───────────┘  │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────┐  │
│  │  ChatPane    │  │ AgentConsole │  │ GitPane   │  │
│  │  (AI Chat)   │  │ (AI Runs)    │  │ (Git Ops) │  │
│  └──────────────┘  └──────────────┘  └───────────┘  │
│                                                     │
│  src/lib/ipc/  (typed Tauri invoke wrappers)         │
└────────────────────┬────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────┐
│              Backend (Rust / src-tauri/)              │
│                                                     │
│  ┌──────────────────────────────────────────────┐   │
│  │  lib.rs  (~90 #[tauri::command] functions)    │   │
│  └──────┬───────────────────────────────────────┘   │
│         │                                           │
│  ┌──────┴──────┐  ┌───────────┐  ┌──────────────┐  │
│  │ application │  │  domain   │  │infrastructure │  │
│  │  (use cases)│  │(pure data)│  │  (adapters)   │  │
│  └─────────────┘  └───────────┘  └──────────────┘  │
│                                                     │
│  Data stores:                                        │
│  ~/.config/spacesly/secrets.json                     │
│  ~/.local/share/spacesly/executions.db  (SQLite)    │
│  ~/.local/share/spacesly/recovery.db   (SQLite)     │
│  ~/.config/spacesly/global_environment.json          │
└─────────────────────────────────────────────────────┘
```

### Key Design Decisions

- **Local-first**: All data lives on disk. There is no cloud dependency. Jira sync is
  optional and pulls data into the local board.
- **Immutable Execution Contracts**: Once an AI execution is created, its contract is
  immutable. Workers execute steps from the persisted contract, preventing drift between
  what was planned and what was executed.
- **SQLite-based Leases**: Execution steps are leased to prevent duplicate worker
  execution. Orphaned runs are recovered as "interrupted" after application restart.
- **Capability-based AI Security**: AI tool calls pass through a tool broker that
  enforces capability-based authorization. High-risk operations require explicit
  workspace trust and operator confirmation.

### Scheduler Agent tool isolation

Scheduler-owned OpenCode Agent sessions keep OpenCode's direct file, shell, and Git built-ins
denied. When a Task Session has a durable `workspace_read`, `workspace_write`, `shell`, or `git`
grant, Spacesly adds an assignment-local stdio MCP server exposing only the corresponding tools.
Every call checks the session ID, attempt, owner, fencing token, lease, workspace ownership, and
durable capability in `scheduler.db` immediately before execution. The workspace root comes only
from the trusted backend envelope resolver, and file paths and shell working directories cannot
escape that canonical root. Cancelling or replacing an assignment invalidates only that session's
authority; running shell commands are terminated when their own fence becomes stale.

External MCP connectors continue to use their existing connector-bound proxy and are unaffected by
the internal workspace tool server.

Chat and Edit Task Session outputs are retained as authoritative typed results. Chat completion is
also projected idempotently into its durable conversation, including after restart. An Edit result
remains queryable after restart, but Spacesly does not automatically reopen the singular editor
review surface because its transient selection and review layout cannot be restored safely; the
operator must request or reopen review explicitly.

---

## Prerequisites

| Dependency                 | Version               | Notes                                                                            |
| -------------------------- | --------------------- | -------------------------------------------------------------------------------- |
| [Rust](https://rustup.rs/) | stable (edition 2021) | Install via `rustup`                                                             |
| [Bun](https://bun.sh/)     | latest                | JavaScript runtime and package manager                                           |
| WebKit system libraries    | —                     | See [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS |

### Linux System Dependencies

```bash
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev

# Fedora
sudo dnf groupinstall "Development Tools"
sudo dnf install webkit2gtk4.1-devel openssl-devel libappindicator-gtk3-devel \
  librsvg2-devel

# Arch Linux
sudo pacman -S webkit2gtk-4.1 base-devel curl wget file openssl appindicator-gtk3 \
  librsvg
```

### macOS

Xcode Command Line Tools are required:

```bash
xcode-select --install
```

---

## Quick Start

```bash
# Clone the repository
git clone https://github.com/iqbalsonata/spacesly.git
cd spacesly

# Install frontend dependencies
bun install

# Run in development mode (Tauri window + Vite HMR)
bun run tauri:dev
```

The application window opens at 1100×700 with a minimum size of 760×400.

---

## Build for Production

```bash
bun run tauri build
```

The bundled application is written to `src-tauri/target/release/bundle/`. Available
formats depend on your platform:

- **Linux**: `.deb`, `.AppImage`, `.rpm`
- **macOS**: `.dmg`, `.app`
- **Windows**: `.msi`, `.exe`

### Optimizations

The release profile applies:

- Single codegen unit for cross-module inlining
- Full LTO (link-time optimization)
- Optimization level 3
- Panic = abort (smaller binary)
- Symbol stripping

---

## Configuration

### Secrets (API Keys, Credentials)

All secrets are stored in `~/.config/spacesly/secrets.json` via the system keyring
and managed through the **Settings** UI:

- **AI Provider Keys**: OpenAI, Anthropic, Gemini, DeepSeek
- **Jira Credentials**: API token or basic auth
- **MCP Server Environment**: Per-server environment variables

> **Note**: Secrets are persisted in the filesystem using the OS keyring where
> available, with a JSON fallback. They are never exposed to the frontend except
> on explicit reveal.

### Global Environment Variables

Managed in **Settings → Global Environment**, these variables are injected into
every external process launched by Spacesly (shell commands, MCP servers, AI workers,
formatters, Git commands). Each variable can be:

- **Normal**: Value is visible in the UI and injected as-is.
- **Secret**: Value is masked in the UI (`••••••••`) and redacted from all process
  output (stdout, stderr, logs).

### AI Providers

Configured in **Settings → Agent**. Each provider requires:

- A display name
- Base URL (for OpenAI-compatible APIs)
- API style (`openai` or `anthropic`)
- Model name
- API key (stored in secrets)

Built-in provider profiles:

| Provider     | Default Base URL                                          |
| ------------ | --------------------------------------------------------- |
| OpenAI       | `https://api.openai.com/v1`                               |
| Anthropic    | `https://api.anthropic.com/v1`                            |
| Gemini       | `https://generativelanguage.googleapis.com/v1beta/openai` |
| DeepSeek     | `https://api.deepseek.com/v1`                             |
| opencode CLI | (local binary)                                            |

### MCP Servers

Configured in **Settings → MCP**. Each server requires:

- A name
- Command and arguments (stdio-based)
- Optional environment variables

---

## Usage Guide

### Board

The workspace initializes with a seeded board ("Daily work orchestration") and
default columns (Backlog, Queued, In Progress, Review, Done).

- **Add a card**: Click the "New Task" button or use the card on the far right.
- **Move a card**: Drag between columns to update its status.
- **Sync Jira**: Click **Sync Jira board** in the toolbar to pull issues from
  a configured Jira project.
- **Execute AI**: Drag a card to **Ready/In Progress** or click **Start** to
  create an immutable execution contract.

### AI Agent

1. **Configure** a provider and API key in **Settings → Agent**.
2. **Test** the connection with the **Test Agent** button.
3. **Start execution**: Click **Start** on a card or drag it to **In Progress**.
4. **Monitor**: The Agent Console panel shows execution progress with streaming output.
5. **Results**: For Jira-synced cards, results are automatically posted as comments
   and the issue transitions to Done.

Execution state survives application restart via the durable SQLite store.

### Code Editor

1. Switch to **Files** mode from the toolbar.
2. Use the file browser to navigate directories. Click folders to expand, click
   files to open them in editor tabs.
3. Edit files with full syntax highlighting. Unsaved changes are indicated by a dot
   on the tab.
4. Save with `Ctrl+S` (or `Cmd+S` on macOS). LSP diagnostics run automatically.

### Terminal

- Click the terminal icon in the toolbar to open a shell session.
- The terminal starts in your workspace root directory.
- Supports resize, multiple concurrent sessions, and auto-cleanup of dead sessions.
- Each session is a fully-isolated PTY via `portable-pty`.

### Git Operations

The Git panel shows:

- Current branch and status
- Changed files with diff indicators (M, A, D, U)
- Stage/unstage individual files or all changes
- Commit with message
- Push, pull, merge, rebase
- Branch creation and checkout

### File Search

The search panel supports:

- Full-text search across all workspace files
- File path filtering via the `.gitignore`-aware `ignore` crate
- Search result preview with context lines
- Batch replace with preview

### LSP (Language Server Protocol)

LSP servers are configured per workspace. Supported features:

- Diagnostics (errors and warnings in the editor gutter)
- Completions (autocomplete as you type)
- Hover information
- Go-to-definition
- Find references
- Document symbols
- Code actions

---

## Project Structure

```
spacesly/
├── src/                          # Frontend (SvelteKit 5 SPA)
│   ├── app.html                  # HTML shell
│   ├── app.css                   # Global styles / theme variables
│   ├── routes/
│   │   ├── +layout.svelte        # Root layout (theme init)
│   │   ├── +layout.ts            # SSR = false (SPA mode)
│   │   ├── +page.svelte          # Main single-page application
│   │   └── page.css              # Page-specific styles
│   └── lib/
│       ├── components/           # UI components (17 modules)
│       │   ├── BoardWorkspace.svelte
│       │   ├── CodeEditor.svelte
│       │   ├── EditorWorkspace.svelte
│       │   ├── FileBrowserPane.svelte
│       │   ├── TerminalWorkspace.svelte
│       │   ├── WorkspaceChatPane.svelte
│       │   ├── AgentConsolePanel.svelte
│       │   ├── TaskCard.svelte
│       │   ├── GitActionsPane.svelte
│       │   ├── GitBranchPicker.svelte
│       │   ├── McpConnectionSettings.svelte
│       │   ├── WorkspaceSearchPane.svelte
│       │   ├── AiEditReview.svelte
│       │   ├── NotificationStack.svelte
│       │   └── WorkspaceRow.svelte
│       ├── ipc/                  # Typed Tauri IPC wrappers
│       │   ├── agent.ts
│       │   ├── execution.ts
│       │   ├── files.ts
│       │   ├── git.ts
│       │   ├── jira.ts
│       │   ├── policy.ts         # Per-command timeout policies
│       │   ├── recovery.ts
│       │   ├── settings.ts
│       │   ├── terminal.ts
│       │   └── workspaceSearch.ts
│       ├── ipc.ts                # Barrel re-export (~1330 lines)
│       ├── stores/
│       │   └── theme.svelte.ts   # Reactive theme store
│       └── *.ts                  # Domain logic modules
│
├── src-tauri/                    # Backend (Rust)
│   ├── Cargo.toml                # Rust dependencies
│   ├── tauri.conf.json           # Tauri app configuration
│   ├── build.rs                  # Tauri build script
│   ├── capabilities/
│   │   └── default.json          # Tauri capability permissions
│   └── src/
│       ├── main.rs               # Entry point
│       ├── lib.rs                # ~90 Tauri commands + Tauri builder
│       ├── application/          # Use cases (services)
│       │   ├── app.rs
│       │   ├── files_service.rs
│       │   ├── git_service.rs
│       │   └── jira_service.rs
│       ├── domain/               # Pure data models
│       │   ├── entity.rs         # Workspace, Board, Card, etc.
│       │   └── execution.rs      # ExecutionRun, StepRun
│       └── infrastructure/       # Adapters (23 modules)
│           ├── execution_store.rs   # SQLite (executions.db)
│           ├── recovery_store.rs    # SQLite (recovery.db)
│           ├── secrets.rs           # Encrypted secrets
│           ├── global_environment.rs# Managed env vars
│           ├── mcp.rs               # MCP client
│           ├── jira_rest.rs         # Jira REST API
│           ├── ai_worker.rs         # AI workers (5 providers)
│           ├── ai_run.rs            # Run lifecycle
│           ├── pty.rs               # PTY terminal
│           ├── shell.rs             # Shell command execution
│           ├── shell_env.rs         # Environment resolution
│           ├── files.rs             # File system operations
│           ├── file_watcher.rs      # File system watcher
│           ├── git.rs               # Git CLI operations
│           ├── formatting.rs        # Code formatters
│           ├── lsp.rs               # LSP server management
│           ├── tool_broker.rs       # AI tool authorization
│           ├── provider_registry.rs # AI provider profiles
│           ├── workspace_cache.rs   # Board state cache
│           ├── workspace_search.rs  # Full-text search
│           └── workspace_trust.rs   # AI workspace trust
│
├── static/                       # Static assets
├── tests/                        # Frontend tests
├── vite.config.js                # Vite configuration
├── svelte.config.js              # SvelteKit configuration
├── tsconfig.json                 # TypeScript configuration
├── eslint.config.js              # ESLint flat config
└── cliff.toml                    # Changelog generator config
```

---

## Tech Stack

| Layer           | Technology                                                                                                                                                                 |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Desktop Shell   | [Tauri v2](https://v2.tauri.app/) (WebKit webview)                                                                                                                         |
| Frontend        | [SvelteKit 5](https://kit.svelte.dev/), Svelte 5 (runes), TypeScript 6                                                                                                     |
| Bundler         | [Vite 8](https://vite.dev/)                                                                                                                                                |
| Editor          | [CodeMirror 6](https://codemirror.net/) (~600 KB, lazy-loaded)                                                                                                             |
| Terminal        | [xterm.js 5](https://xtermjs.org/) + addon-fit (~400 KB, lazy-loaded)                                                                                                      |
| Icons           | [Lucide Svelte](https://lucide.dev/)                                                                                                                                       |
| Backend         | Rust (edition 2021)                                                                                                                                                        |
| Backend Libs    | `tauri` 2, `serde`/`serde_json`, `rusqlite` 0.32 (bundled), `reqwest` 0.12, `tokio`, `portable-pty` 0.9, `notify` 8, `ignore` 0.4, `sha2`, `keyring` 3, `regex` 1, `url` 2 |
| AI Providers    | OpenAI / Anthropic / Gemini / DeepSeek API, opencode CLI                                                                                                                   |
| Linting         | ESLint 10, Prettier 3, `eslint-plugin-svelte`, `typescript-eslint`                                                                                                         |
| Package Manager | [Bun](https://bun.sh/)                                                                                                                                                     |

---

## Development

```bash
# Install dependencies
bun install

# Run Tauri dev mode (window + hot reload)
bun run tauri:dev

# Or run frontend only (browser, no Tauri backend)
bun run dev

# Type-check the frontend
bun run check

# Run Rust tests (in src-tauri/)
cargo test

# Lint
bun run lint

# Format
bun run format
```

### Development Scripts

| Script                 | Description                       |
| ---------------------- | --------------------------------- |
| `bun run dev`          | Vite dev server only (port 1420)  |
| `bun run build`        | Vite production build             |
| `bun run preview`      | Vite preview server               |
| `bun run check`        | Svelte type-checking              |
| `bun run tauri:dev`    | Full Tauri dev mode with Vite HMR |
| `bun run tauri build`  | Production Tauri bundle           |
| `bun run lint`         | ESLint check                      |
| `bun run lint:fix`     | ESLint auto-fix                   |
| `bun run format`       | Prettier format                   |
| `bun run format:check` | Prettier check                    |

---

## Troubleshooting

### Blank window on startup

Ensure your system meets the [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/).
On Linux, the `WEBKIT_DISABLE_DMABUF_RENDERER=1` and `LIBGL_ALWAYS_SOFTWARE=1` environment
variables are set automatically in the `tauri:dev` script to work around GPU compatibility
issues in virtualized environments.

### Terminal not working

The terminal requires a valid login shell. Ensure your default shell is set correctly
(`$SHELL` environment variable). On Windows, PowerShell or CMD should be available.

### Jira sync fails

1. Verify your Jira credentials in **Settings → Jira**.
2. Check that the Jira project key is correct.
3. For MCP-based sync, ensure the MCP server is connected and responding.

### AI execution stalls

1. Check the provider API key in **Settings → Agent**.
2. Test the connection with the **Test Agent** button.
3. Verify the provider base URL and model name are correct.
4. Check the Agent Console panel for error messages.

### File watcher not detecting changes

The file watcher uses the `notify` crate, which relies on OS-specific mechanisms
(`inotify` on Linux, `FSEvents` on macOS, `ReadDirectoryChanges` on Windows). If
changes are not detected, try switching focus away and back to the window, or
manually refresh with the file browser's refresh button.

---

## Limitations

- **Single-window**: Spacesly runs as a single window application. There is no
  multi-window or multi-monitor support.
- **Local-only collaboration**: There is no real-time collaboration or shared
  workspace feature.
- **Jira-only external sync**: The kanban board currently only syncs with Jira.
  Other issue trackers (GitHub Issues, Linear, etc.) are not supported.
- **stdio-based MCP only**: MCP servers must communicate over stdin/stdout.
  SSE-based MCP transport is not supported.

---

## Contributing

1. Fork the repository.
2. Create a feature branch: `git checkout -b feature/my-feature`.
3. Make your changes.
4. Run the checks: `bun run check && cargo test`.
5. Commit using [conventional commits](https://www.conventionalcommits.org/):

   ```
   feat: add new feature
   fix: correct bug in component
   refactor: restructure module
   docs: update README
   ```

6. Push and open a Pull Request.

---

## License

[MIT](LICENSE)
