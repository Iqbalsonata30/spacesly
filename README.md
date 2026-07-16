# Spacesly

Spacesly is a local-first, AI-augmented desktop workspace for orchestrating daily development work. Built with [Tauri v2](https://v2.tauri.app/) (Rust backend) and [SvelteKit 5](https://kit.svelte.dev/) frontend, it combines a kanban board, code editor, shell terminal, and AI agent into a single window.

## Features

- **Kanban Board** — Track work items from Jira or local cards with column workflow (Backlog → Queued → In Progress → Review → Done)
- **Jira Sync** — Import boards and issues via direct Jira REST API or MCP server; transition statuses, assign, and comment on issues
- **AI Agent** — Execute immutable Execution Contracts through bounded workers (OpenAI, Anthropic, Gemini, DeepSeek, or opencode CLI) with structured results and Jira comment posting
- **Code Editor** — Browse and edit workspace files with CodeMirror (syntax highlighting for JS, TS, Rust, Go, HTML, CSS, JSON, Markdown, YAML, Svelte)
- **Local Terminal** — Full PTY shell session embedded in the app via xterm.js and portable-pty
- **AI Chat** — Command-first assistant with workspace, board, and task context
- **Git Integration** — Branch listing, checkout, and repo status from the workspace
- **MCP Connectivity** — Connect any stdio-based MCP server for tool/agent integration
- **Secrets Storage** — API keys and credentials stored encrypted at `~/.config/spacesly/secrets.json`

## Architecture

```
Frontend (SvelteKit 5 SPA)
  ├── +page.svelte          — Main single-page layout
  ├── src/lib/components/   — Board, Editor, Terminal, Chat, Settings, etc.
  ├── src/lib/ipc/          — Typed Tauri IPC wrappers
  └── src/lib/              — State, workflow, AI models, themes

Tauri IPC Bridge (JSON-RPC)

Backend (Rust / src-tauri/)
  ├── application/          — Use cases: execution, files, git, workspace
  ├── domain/                — Pure board/entity and ExecutionRun models
  └── infrastructure/       — Adapters: pty, shell, mcp, jira_rest, ai_worker,
                               execution_store (SQLite), files, git, formatting,
                               secrets, shell_env
```

AI execution is contract-first. The planner-facing UI creates an immutable
Execution Contract once; the backend persists its `ExecutionRun` and `StepRun`
state in `~/.local/share/spacesly/executions.db`. Workers receive only the
contract and execute the current step. SQLite leases prevent duplicate worker
execution, and orphaned running steps are recovered as interrupted after restart.

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Bun](https://bun.sh/) (for frontend dependencies and scripts)
- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) (system libraries for your OS)

### Development

```bash
# Install frontend dependencies
bun install

# Run in development mode (Tauri window + Vite HMR)
bun run tauri:dev

# Lint and type-check the frontend
bun run check

# Run Rust tests
cargo test
```

### Build

```bash
bun run tauri build
```

## Usage

### Board

The workspace initializes with a seeded board ("Daily work orchestration") and default columns. Drag cards between columns to update their status. Click **Sync Jira board** in the toolbar to pull issues from Jira.

### AI Agent

Configure a provider and API key in **Settings → Agent**. Click **Start** on a card or drag it to **Ready/In Progress** to create and execute an immutable contract. Results stream into the card and, for Jira-backed cards, are posted as comments with automatic issue transition to Done. Execution state survives restart in the durable SQLite store.

### Terminal

Open the terminal pane from the toolbar to get a full shell session in your workspace root. Supports resize, multiple sessions, and auto-cleanup of dead sessions.

### Code Editor

Use the file browser to navigate directories and open files. Files open in a CodeMirror tab with syntax highlighting. Edit and save directly within the app.

### MCP Connections

Add reusable MCP server configurations in **Settings → MCP Connections**. These can be used for Jira, Kubernetes/OCP, Bamboo, or any stdio-based MCP tool server.

## Configuration

Jira credentials, AI provider keys, and MCP server configurations are stored locally. Secrets are saved to `~/.config/spacesly/secrets.json` via the Tauri `app-config-dir`.

## Tech Stack

| Layer         | Technology                                     |
| ------------- | ---------------------------------------------- |
| Desktop Shell | Tauri v2                                       |
| Frontend      | SvelteKit 5, Svelte 5 runes                    |
| Editor        | CodeMirror 6                                   |
| Terminal      | xterm.js + portable-pty                        |
| Icons         | Lucide Svelte                                  |
| Backend       | Rust (reqwest, rusqlite, serde)                |
| AI            | OpenAI/Anthropic-compatible API + opencode CLI |
