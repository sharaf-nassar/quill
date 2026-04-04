# Architecture

Quill is a cross-platform Claude Code and Codex companion built with Tauri (Rust) and React. It tracks usage, analytics, behavioral patterns, plugins, session history, and provider integrations.

## Tech Stack

The application pairs a Rust backend with a React frontend communicating over Tauri IPC.

- **Frontend**: React 19, TypeScript, Recharts, pure CSS dark theme
- **Backend**: Rust (edition 2024), Tauri 2, Axum HTTP server, SQLite (rusqlite), Tantivy full-text search
- **AI**: Anthropic API via rig-core SDK for pattern extraction and memory optimization
- **Build**: Vite (ES2020), Cargo, GitHub Actions CI/CD across Linux/macOS/Windows

## Multi-Window Design

Each major feature runs in its own Tauri window, routed via URL query parameter in [[src/main.tsx]].

The main window hosts a split-pane layout with the [[features#Live Usage View]] and [[features#Analytics Dashboard]]. Secondary windows open for [[features#Session Search]], [[features#Learning System]], [[features#Plugin Manager]], and [[features#Restart Orchestrator]], but [[src/main.tsx]] blocks those windows with an empty state when no provider is enabled.

The QUILL titlebar trigger opens an inline popover for provider enable/disable actions, rendered inside the main window with a backdrop overlay for click-outside dismissal.

### Window Configuration

The main widget lives in `src-tauri/tauri.conf.json`, while dynamically created windows are allowed by `src-tauri/capabilities/default.json` for `runs`, `sessions`, `learning`, `plugins`, `restart`, and `integrations`.

The main window defaults to 280x340px, stays borderless and transparent, and uses the custom titlebar in [[src/components/TitleBar.tsx]] for left-aligned feature controls plus the centered QUILL trigger that opens an inline integrations popover.

## Module Map

The Rust backend in [[src-tauri/src/lib.rs]] registers 68 Tauri commands and starts background tasks on launch.

### Backend Modules

Rust modules under `src-tauri/src/` organized by domain responsibility.

| Module | File | Purpose |
|--------|------|---------|
| Entry point | [[src-tauri/src/lib.rs]] | IPC commands, tray, auto-updater, background tasks |
| HTTP server | [[src-tauri/src/server.rs]] | Axum API on port 19876 for hook data ingestion |
| Storage | [[src-tauri/src/storage.rs]] | SQLite schema, migrations, queries, aggregation |
| Sessions | [[src-tauri/src/sessions.rs]] | Tantivy full-text indexing of session transcripts |
| Learning | [[src-tauri/src/learning.rs]] | Two-stream LLM analysis for behavioral pattern discovery |
| Memory optimizer | [[src-tauri/src/memory_optimizer.rs]] | LLM-driven memory file optimization |
| Plugins | [[src-tauri/src/plugins.rs]] | Plugin and marketplace management |
| Restart | [[src-tauri/src/restart.rs]] | Claude Code instance discovery and restart orchestration |
| Integrations | [[src-tauri/src/integrations/mod.rs]] | Provider detection plus persisted enable and disable lifecycle for Claude and Codex |
| Models | [[src-tauri/src/models.rs]] | All shared data structures and serde types |
| AI client | [[src-tauri/src/ai_client.rs]] | Anthropic API integration via rig-core |
| Git analysis | [[src-tauri/src/git_analysis.rs]] | Commit pattern extraction and hotspot analysis |
| Fetcher | [[src-tauri/src/fetcher.rs]] | Claude API usage bucket fetching |
| Auth | [[src-tauri/src/auth.rs]] | Bearer token generation and storage |
| Config | [[src-tauri/src/config.rs]] | Credential reading and HTTP client setup |
| Claude setup | [[src-tauri/src/claude_setup.rs]] | Legacy/local Claude deployment helpers retained outside startup |
| Prompt utils | [[src-tauri/src/prompt_utils.rs]] | LLM input sanitization and compression |

### Frontend Structure

React and TypeScript sources organized by feature domain under `src/`.

| Directory | Purpose |
|-----------|---------|
| [[src/App.tsx]] | Main window: split-pane live + analytics layout |
| `src/components/` | UI components organized by feature domain |
| `src/hooks/` | 15+ custom hooks for Tauri IPC data fetching |
| `src/windows/` | Secondary window entry points |
| `src/utils/` | Formatting helpers (time, tokens, charts) |
| `src/styles/` | Pure CSS stylesheets (dark theme) |
| [[src/types.ts]] | All TypeScript type definitions (434 lines) |

## Communication Layers

Data flows through three communication channels between the system's components.

### Tauri IPC

The primary frontend-backend channel. React hooks call `invoke()` for request-response and `listen()` for push events, including provider-status refresh via `integrations-updated`. See [[data-flow]] for specific flows.

### HTTP API

An Axum server on port 19876 (configurable via `QUILL_PORT`) receives data from external hook scripts. Bearer token authentication with constant-time comparison. Rate-limited per endpoint type. See [[backend#HTTP API Server]].

### Tauri Events

Backend pushes real-time updates to the frontend via `emit()`.

Current events include `tokens-updated`, `learning-updated`, `learning-log`, `plugin-changed`, `restart-status-changed`, `integrations-updated`, `memory-optimizer-updated`, and `memory-files-updated`.

## Background Tasks

Several background tasks start on app launch in [[src-tauri/src/lib.rs]].

- **Hourly cleanup**: Aggregates snapshots into hourly tables, prunes old data, compresses observations
- **Learning periodic timer**: Runs behavioral analysis every N minutes if configured
- **Plugin update checker**: Polls marketplaces every 4 hours for available updates
- **Session index scan**: Ingests new JSONL session files on startup
- **Integration refresh**: Detects Claude and Codex CLI/home state and emits `integrations-updated`

## Local vs Remote Architecture

Quill supports both local single-machine and distributed multi-host setups.

### Local Setup

On startup, [[src-tauri/src/integrations/manager.rs]] refreshes Claude and Codex provider state for the UI. Provider installers only run after explicit enable confirmation: Claude via [[src-tauri/src/claude_setup.rs]] and Codex via [[src-tauri/src/integrations/codex.rs]].

### Remote Setup

A plugin (`plugin/`) can be installed on remote hosts via the marketplace. Running `/quill:setup` on the remote configures hooks to report back to the desktop widget's IP. The remote MCP server (`plugin/mcp/server.py`) provides session query tools.
