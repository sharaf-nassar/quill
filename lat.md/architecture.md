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

The main window hosts a split-pane layout with the [[features#Live Usage View]] and [[features#Analytics Dashboard]]. Secondary windows open for [[features#Session Search]], [[features#Learning System]], [[features#Plugin Manager]], [[features#Restart Orchestrator]], and [[features#Settings Window]], with [[src/main.tsx]] blocking provider-dependent windows when no provider is enabled.

The titlebar's right-side cogwheel button opens the standalone Settings window via `?view=settings`. The previous inline `ProviderMenu` popover has been removed in favor of the dedicated window so all toggles and runtime preferences live in one comprehensive surface.

### Window Configuration

The main widget lives in `src-tauri/tauri.conf.json`, while dynamically created windows are allowed by `src-tauri/capabilities/default.json` for `runs`, `sessions`, `learning`, `plugins`, `restart`, `settings`, and `release-notes`.

The main window defaults to 280x340px, stays borderless and transparent, and uses the custom titlebar in [[src/components/TitleBar.tsx]] for left-aligned feature controls, a centered static `QUILL` brand label, and a right-aligned cluster with a cogwheel button that opens the Settings window, followed by the version and close controls.

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
| Indicator | [[src-tauri/src/indicator.rs]] | Primary-provider resolution, compact title text, and warnings for the tray summary |
| Tray keep-alive | [[src-tauri/src/tray_keepalive.rs]] | macOS-only workaround that rebuilds the tray on sleep/wake and screen-parameter changes |
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
| [[src/types.ts]] | Shared TypeScript type definitions for Rust IPC models and frontend state |

## Communication Layers

Data flows through three communication channels between the system's components.

### Tauri IPC

The primary frontend-backend channel. React hooks call `invoke()` for request-response and `listen()` for push events.

Provider-status refresh uses `integrations-updated`, while indicator refresh uses `indicator-updated`. See [[data-flow]] for specific flows.

### HTTP API

An Axum server on port 19876 (configurable via `QUILL_PORT`) receives data from external hook scripts. Bearer token authentication with constant-time comparison. Rate-limited per endpoint type. See [[backend#HTTP API Server]].

### Tauri Events

Backend pushes real-time updates to the frontend via `emit()`.

Current events include `tokens-updated`, `learning-updated`, `learning-log`, `plugin-changed`, `restart-status-changed`, `integrations-updated`, `indicator-updated`, `memory-optimizer-updated`, and `memory-files-updated`.

## Background Tasks

Several background tasks start on app launch in [[src-tauri/src/lib.rs]].

All tasks that touch the database or network MUST be spawned async — never block the main thread inside `.setup()`, as this prevents GTK from starting and stalls webview loading.

- **Hourly cleanup**: Aggregates snapshots into hourly tables, prunes old data, compresses observations
- **Learning periodic timer**: Runs behavioral analysis every N minutes if configured
- **Plugin update checker**: Polls marketplaces for available updates. Both the master enable flag (`plugin_updates.enabled`) and the interval (`plugin_updates.interval_hours`, 1–24, default 4) are read from the settings table on every tick so the [[features#Settings Window]] can adjust both at runtime without a restart.
- **Integration refresh + tray summary**: One merged task runs `startup_refresh` (detect providers, save, emit `integrations-updated`) then populates tray summary items. Merged to avoid redundant `detect_all` subprocess calls.
- **Live usage refresh**: Background loop that updates the main widget and tray summary rows. The enable flag (`live_usage.enabled`) and refresh interval (`live_usage.interval_seconds`, 60–600, default 180) are read from the settings table on every iteration so the [[features#Settings Window]] can adjust both at runtime.
- **Rule filesystem watcher**: Optional. The `rule_watcher.enabled` setting (default true) is checked at startup; disabling skips the `notify` watcher entirely. Live re-toggling takes effect after the next app launch since the watcher holds an OS handle.
- **Tray "Check for Update"**: Manual trigger via system tray menu. Uses `tauri-plugin-dialog` to show a native OS confirmation dialog when an update is found (Install / Not Now), or an info dialog when already up to date. The frontend still performs its own 4-hour availability check via `@tauri-apps/plugin-updater`, but the titlebar install action now delegates to [[src-tauri/src/lib.rs#install_app_update]] so Rust owns the install-and-restart boundary.

## Single Instance

Re-launching Quill while it's already running focuses the existing main window instead of starting a duplicate process. The handler is wired in [[src-tauri/src/lib.rs#run]] via `tauri-plugin-single-instance`.

The plugin is registered before every other Tauri plugin so its DBus dispatch handler is in place when the secondary process starts. On a duplicate launch, the secondary process exits and the primary's callback runs [[src-tauri/src/lib.rs#show_main_window]] (`show()` + restore last position + `set_focus()`).

Primary-only startup work that mutates local state runs inside Tauri `.setup()` after plugin setup has completed. This keeps duplicate processes from reaching [[src-tauri/src/lib.rs#initialize_storage_or_exit]] or [[src-tauri/src/lib.rs#cleanup_interrupted_learning_runs]], so an active learning run in the primary cannot be marked `interrupted` by a re-launch.

Without this guard, GTK's `Application` forwards an `activate` signal to the primary, which surfaces as a second `RuntimeRunEvent::Ready` and makes Tauri re-run its internal `setup()`. The second `setup()` rebuilds windows from `tauri.conf.json` and panics with `a webview with label \`main\` already exists`. The primary dies, and the secondary is left orphaned with no webview, no tray icon, and no `tauri::async_runtime::spawn` tasks running.

App-update-driven relaunch must release the single-instance lock before the new process tries to claim it. `AppHandle::restart()` spawns the new binary before the current process exits, so the new instance reaches single-instance init while the primary still owns the D-Bus name (Linux) / distributed-notification port (macOS) / named mutex (Windows), is treated as a duplicate launch, runs `show_main_window` inside the dying primary, and exits, leaving no Quill instance running. [[src-tauri/src/lib.rs#spawn_delayed_relaunch]] instead spawns a fully-detached child. On Unix the child uses the post-fork hook to `setsid` and sleep before loading the new binary image, so it only reaches single-instance init after the primary has finished exiting. On Windows the named mutex is released synchronously on parent exit, so detached spawn alone is sufficient. Used by both the titlebar install path ([[src-tauri/src/lib.rs#install_app_update]]) and the tray-menu install path ([[src-tauri/src/lib.rs#check_for_update]]).

## macOS Tray Keep-Alive (Workaround)

Workaround for [tauri-apps/tauri#12060](https://github.com/tauri-apps/tauri/issues/12060): on macOS, the tray's `NSStatusItem` subview becomes detached from the menu bar after sleep/wake or screen-parameter changes, leaving the icon invisible.

[[src-tauri/src/tray_keepalive.rs#install]] subscribes the same `block2::RcBlock` to `NSWorkspaceDidWakeNotification` (via `NSWorkspace.sharedWorkspace().notificationCenter()`) and `NSApplicationDidChangeScreenParametersNotification` (via the default `NSNotificationCenter`). On either notification it calls `tray.set_visible(false)` then `tray.set_visible(true)`, which makes `tray-icon` drop the existing `NSStatusItem` (`NSStatusBar::removeStatusItem` + `removeFromSuperview`) and rebuild a fresh one with the cached icon, menu, and title. `set_icon` alone is insufficient because it only updates the existing button's image and would not re-attach a detached subview.

A 500 ms time-based debounce coalesces wake-with-display-change events that fire both notifications nearly simultaneously. The block runs on `NSOperationQueue.mainQueue()` because tray-icon mutations require the main thread. The non-macOS [[src-tauri/src/tray_keepalive.rs#install]] is an empty stub. **Remove this module once the upstream issue ships a fix.**

## Local vs Remote Architecture

Quill supports both local single-machine and distributed multi-host setups.

### Local Setup

On startup, [[src-tauri/src/integrations/manager.rs]] refreshes all provider state for the UI.

CLI providers (Claude, Codex) run installers after explicit enable confirmation: Claude via [[src-tauri/src/claude_setup.rs]] and Codex via [[src-tauri/src/integrations/codex.rs]]. Service-only providers like MiniMax ([[src-tauri/src/integrations/minimax.rs]]) require only an API key, stored in the SQLite settings table.

### Remote Setup

A plugin (`plugin/`) can be installed on remote hosts via the marketplace. Running `/quill:setup` on the remote configures hooks to report back to the desktop widget's IP. The remote MCP server (`plugin/mcp/server.py`) provides session query tools.
