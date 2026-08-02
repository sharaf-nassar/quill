# Architecture

Quill is a cross-platform Claude Code and Codex companion built with Tauri (Rust) and React. It tracks usage, analytics, behavioral patterns, session history, and provider integrations.

## Tech Stack

The application pairs a Rust backend with a React frontend communicating over Tauri IPC.

- **Frontend**: React 19, TypeScript, an internal SVG viz kit (no charting dependency), pure CSS dark theme
- **Backend**: Rust (edition 2024), Tauri 2, Axum HTTP server, SQLite (rusqlite), Tantivy full-text search
- **AI**: Anthropic API via rig-core SDK for pattern extraction and memory optimization
- **Build**: Vite (ES2020), Cargo, GitHub Actions CI/CD across Linux/macOS/Windows

## Multi-Window Design

The app runs as three Tauri windows routed by a URL query parameter in [[src/main.tsx]]: the main widget, the consolidated Manage workspace, and a release-notes viewer. All three are decorationless, transparent, and freely resizable.

The main window hosts the widget shell described in [[frontend#Main Window Layout]]. The [[features#Session Search]], [[features#Learning System]], [[features#Restart Orchestrator]], and [[features#Settings Window]] surfaces are no longer separate windows — they run as sections inside the Manage workspace, which gates each one inline when no provider is enabled.

The Sessions, Learning, Restart, and Settings management surfaces are consolidated into a single rail-navigated `?view=manage` workspace ([[src/windows/ManageWindowView.tsx]]), opened from the widget titlebar's settings key or the app-scoped ⌘M / Ctrl+M accelerator. It embeds each tool's existing window-view component as a rail section — per-window chrome suppressed via `manage.css` — with inline no-provider states, and folds learning run history into the Learning section. The standalone tool windows, their `?view=` routes, and capabilities entries were retired, leaving only `main`, `manage`, and `release-notes`. The previous inline `ProviderMenu` popover was removed earlier in favor of the dedicated settings surface.

### Window Configuration

The main widget lives in `src-tauri/tauri.conf.json`, while the dynamically created `manage` and `release-notes` windows are allowed by `src-tauri/capabilities/default.json`.

The main window is `resizable: true` and drags freely on both axes. It opens at 360x800 with a 320px minimum width and a 200px minimum height, declares no maximum on either axis, and stays borderless and transparent so the widget can paint its own rounded surface. Its chrome is [[src/components/widget/WidgetTitleBar.tsx]]; the app version moved to the Settings window, which is now the only place it appears.

The 800px height is measured, not chosen. The default Usage view renders 788px tall at the 360px design width with its breakdown saturated at `BREAKDOWN_LIMIT` rows — 43px of non-scrolling chrome (40px titlebar, 1px rule, 2px shell borders) above a 745px content column — and 800 rounds that up so sub-pixel rounding at fractional display scaling cannot reintroduce a scrollbar. Usage is also the tallest view, so Trends, Charts, Models, and Context all fit inside it. The earlier 560 was a placeholder for the deleted content-driven sizer described in [[frontend#Main Window Layout]], and once that sizer was gone it opened the widget with its lower bands cut off.

`resizable: true` is inert by itself on a `decorations: false` window: there is no native frame for the window manager to hit-test, so the flag only means the compositor will honour a resize the app asks for. The affordance that asks is [[src/components/WindowResizeHandles.tsx]], and all three windows are decorationless, so [[src/main.tsx]] mounts it on all three routes — see [[architecture#Multi-Window Design#Window Configuration#Resize Border Geometry]].

Geometry belongs to the user, so `tauri-plugin-window-state` persists the widget's size and position like every other window. The plugin is still built with `skip_initial_state("main")` because its automatic restore replays every flag, and a saved `decorated` or `visible` value would undo the decorationless surface and the close-to-tray contract. Skipping that restore drops all flags for the window, so the two the widget actually wants are restored explicitly in setup with `StateFlags::POSITION | StateFlags::SIZE`; `skip_initial_state` gates only the restore, so saving is unaffected. Other windows keep the plugin's full default behaviour.

Size restore is guarded by a one-time reset, [[src-tauri/src/lib.rs#widget_restore_flags]]. A profile upgrading from the pre-widget split-pane main window still has that window's much wider geometry saved under `main` — several hundred pixels past the widget's 360px — and restoring it would override the config default on the first widget launch. While the [[src-tauri/src/lib.rs#WIDGET_SIZE_RESET_MARKER_KEY]] setting is absent the restore therefore asks for `StateFlags::POSITION` alone, so the config size wins, and the marker is written at that moment rather than after the window is up so a later startup failure cannot replay the reset over a size the user has since chosen. The plugin still saves on exit, which replaces the stale entry with the widget's own geometry; every launch after that restores `POSITION | SIZE` as before. Position is never withheld, and the key is deliberately distinct from the `widget_ui_v1` always-on-top marker, which earlier builds may already have written.

#### Seeded Height Clamp

A default sized for the whole Usage view is taller than a short display can show, so the launch that seeds it caps the height to the monitor work area via [[src-tauri/src/lib.rs#clamp_seeded_widget_height]].

The clamp is gated on `StateFlags::SIZE` being absent from the restore flags rather than on the marker directly, so it runs on exactly the launches where the config height is what opens. A size the user dragged is restored instead and is never touched — reclaiming it is the mistake the deleted `ResizeObserver` sizer made. It runs after `restore_state` so `current_monitor` reports the display the widget was actually parked on, falling back to `primary_monitor` when the compositor cannot place the window yet.

[[src-tauri/src/lib.rs#fit_height_to_work_area]] does the arithmetic: monitor geometry is physical and the config height is logical, so the work area is divided by the monitor's scale factor before the comparison, then [[src-tauri/src/lib.rs#WIDGET_WORK_AREA_MARGIN]] is subtracted to keep the widget off the screen edge — which also absorbs a panel or dock a compositor failed to exclude from the work area — and the result is floored at [[src-tauri/src/lib.rs#WIDGET_MIN_HEIGHT]] to match `minHeight`. It returns `None` when the height already fits or when the monitor reports a zero or non-finite scale factor, and `None` means leave the configured size alone: opening at the size the config asked for beats guessing from numbers that cannot be trusted. Only the height moves, so the 360px design width survives the clamp.

#### Resize Border Geometry

The eight zones have to clear each host window's own chrome, so the geometry is two custom properties on the overlay — `--wrh-edge` and `--wrh-corner` — and the `variant` prop of [[src/components/WindowResizeHandles.tsx]] selects which pair applies.

The widget takes the default 5px edges and 12px corners: 12px is `.wg-shell`'s own radius, so the corner square covers exactly the transparent notch, and the titlebar keycaps and view dropdown still clear it by at least 2px. Manage and release-notes take the `roomy` variant — the same 5px edges, but 8px corners.

The corner is what has to shrink, and the close key in each window is what shrinks it. Manage's is a 26px square 9px in from the right edge and 5.5px down; release-notes' is 28px, 11px in and 7px down. A 12px corner would cut into both; an 8px one clears the Manage key by 1px and the release-notes key by 3px, and happens to equal release-notes' own 8px shell radius. The 5px edge is the most the Manage key's 5.5px top gap allows, and the north edge claiming the top 5px of the titlebar drag region is the usual borderless-window trade. Nothing else is close: the Manage rail's search trigger and section buttons start 11px in, and the ⌘K palette is centred and never within 80px of an edge even at the 720px minimum width.

Manage's two half-pixel numbers are a consequence of its 1px outer border, which only started painting once `--hairline` was defined — see [[lat.md/frontend#Frontend#Styling#Design Tokens]]. The border insets the whole window chrome by 1px while the resize overlay stays fixed to the viewport, so every Manage clearance gained a pixel (north 0 → 0.5px, north-east 0 → 1px, east 3 → 4px, west 5 → 6px, south 7 → 8px, south-west 4 → 5px); the titlebar's own 1px bottom rule shrinks its content box to 35px, which is what re-centres the 26px key at 5.5px instead of 5px. Re-probed in headless Chrome at 960x680 and at the 720x480 minimum, with the ⌘K palette open and closed: no resize zone overlaps any interactive control, and all sixteen `elementFromPoint` samples still resolve to the zone they belong to. Release-notes is untouched — its stylesheet never used the token.

Neither Manage nor release-notes had any affordance at all before this, because the component was originally mounted on the main route alone on the grounds that its geometry was widget-specific; parameterising the two numbers is what let the other two windows share it without inheriting the widget's clearances.

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
| Restart | [[src-tauri/src/restart.rs]] | Claude Code instance discovery and restart orchestration |
| Integrations | [[src-tauri/src/integrations/mod.rs]] | Provider detection plus persisted enable and disable lifecycle for Claude and Codex |
| Indicator | [[src-tauri/src/indicator.rs]] | Primary-provider resolution, compact title text, and warnings for the tray summary |
| Tray keep-alive | [[src-tauri/src/tray_keepalive.rs]] | macOS-only workaround that rebuilds the tray on sleep/wake and screen-parameter changes |
| Models | [[src-tauri/src/models.rs]] | All shared data structures and serde types |
| CC inference client | [[src-tauri/src/cc_client.rs]] | Subprocess-based Claude Code invocation for all LLM inference (replaces the prior direct rig-core/Anthropic path) |
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
| [[src/App.tsx]] | Main window: the resizable widget shell (titlebar, LIMITS, view region) |
| `src/components/` | UI components organized by feature domain |
| `src/hooks/` | Custom hooks for Tauri IPC data fetching, over the shared `useCachedInvoke` primitive |
| `src/windows/` | Secondary window entry points |
| `src/utils/` | Formatting helpers (time, tokens, providers, retention) |
| `src/styles/` | Pure CSS stylesheets; `index.css` carries the design tokens and every widget section |
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

Current events include `tokens-updated`, `learning-updated`, `learning-log`, `restart-status-changed`, `integrations-updated`, `indicator-updated`, `memory-optimizer-updated`, and `memory-files-updated`.

## Background Tasks

Several background tasks start on app launch in [[src-tauri/src/lib.rs]].

All tasks that touch the database or network MUST be spawned async — never block the main thread inside `.setup()`, as this prevents GTK from starting and stalls webview loading.

- **Hourly cleanup**: Aggregates snapshots into hourly tables, prunes old data, compresses observations
- **Learning periodic timer**: Runs behavioral analysis every N minutes if configured
- **Integration refresh + tray summary**: One merged task runs `startup_refresh` (detect providers, save, emit `integrations-updated`) then populates tray summary items. Merged to avoid redundant `detect_all` subprocess calls.
- **Live usage refresh**: Background loop that updates the main widget and tray summary rows. The enable flag (`live_usage.enabled`) and refresh interval (`live_usage.interval_seconds`, 60–600, default 180) are read from the settings table on every iteration so the [[features#Settings Window]] can adjust both at runtime.
- **Transcript rescan loop**: Always-on incremental rescan of both transcript roots (`~/.claude/projects/` and `~/.codex/sessions/**`) via [[src-tauri/src/lib.rs#spawn_transcript_rescan_loop]]. Each tick enumerates candidates and enqueues only sources whose mtime advanced past the previous tick's in-memory watermark (seeded to startup time so the full startup walk is not redone), feeding both the model-observation and transcript-analytics source-keyed queues used by notify. A separate [[src-tauri/src/lib.rs#spawn_startup_model_source_reconciliation]] pass re-admits retained model inventory after every launch, including after the durable backfill is complete. The enable flag (`transcript_rescan.enabled`, default true) and interval (`transcript_rescan.interval_seconds`, 60–600, default 120) are read from the settings table on every tick; these are backend-only keys, not part of [[features#Settings Window]]. Unchanged sources are cheap stat-only no-ops, so duplicate discovery does not reparse them.
- **Rule filesystem watcher**: Optional. The `rule_watcher.enabled` setting (default true) is checked at startup; disabling skips the `notify` watcher entirely. Live re-toggling takes effect after the next app launch since the watcher holds an OS handle.
- **Tray "Check for Update"**: Manual trigger via system tray menu. Uses `tauri-plugin-dialog` to show a native OS confirmation dialog when an update is found (Install / Not Now), or an info dialog when already up to date. The frontend still performs its own 4-hour availability check via `@tauri-apps/plugin-updater`, but the titlebar install action now delegates to [[src-tauri/src/lib.rs#install_app_update]] so Rust owns the install-and-restart boundary.

## Single Instance

Re-launching Quill while it's already running focuses the existing main window instead of starting a duplicate process. The handler is wired in [[src-tauri/src/lib.rs#run]] via `tauri-plugin-single-instance`.

The plugin is registered before every other Tauri plugin so its DBus dispatch handler is in place when the secondary process starts. On a duplicate launch, the secondary process exits and the primary's callback runs [[src-tauri/src/lib.rs#show_main_window]] (`show()` + restore last position + `set_focus()`).

Primary-only startup work that mutates local state runs inside Tauri `.setup()` after plugin setup has completed. This keeps duplicate processes from reaching [[src-tauri/src/lib.rs#initialize_storage_or_report_fatal]] or [[src-tauri/src/lib.rs#cleanup_interrupted_learning_runs]], so an active learning run in the primary cannot be marked `interrupted` by a re-launch.

Without this guard, GTK's `Application` forwards an `activate` signal to the primary, which surfaces as a second `RuntimeRunEvent::Ready` and makes Tauri re-run its internal `setup()`. The second `setup()` rebuilds windows from `tauri.conf.json` and panics with `a webview with label \`main\` already exists`. The primary dies, and the secondary is left orphaned with no webview, no tray icon, and no `tauri::async_runtime::spawn` tasks running.

App-update-driven relaunch must release the single-instance lock before the new process tries to claim it. `AppHandle::restart()` spawns the new binary before the current process exits, so the new instance reaches single-instance init while the primary still owns the D-Bus name (Linux) / distributed-notification port (macOS) / named mutex (Windows), is treated as a duplicate launch, runs `show_main_window` inside the dying primary, and exits silently before the logger plugin initializes, leaving no Quill instance running. [[src-tauri/src/lib.rs#spawn_delayed_relaunch]] instead spawns a fully-detached child and hands off via the `QUILL_RELAUNCH_PARENT_PID` env var. A blocking wait inside the Unix post-fork hook would deadlock `Command::spawn`, which synchronously waits for the post-fork hook to finish before returning to the caller that still needs to invoke `app.exit(0)`. The hook therefore only `setsid`s; the new binary then blocks in [[src-tauri/src/lib.rs#wait_for_predecessor_exit]] before any Tauri plugin is constructed, polling `kill(pid, 0)` until `ESRCH` (capped at 30s) and sleeping 100ms so the dbus-daemon / launchd has time to release the registered name. On Windows the named mutex is released synchronously on parent exit, so detached spawn alone is sufficient and the env var has no effect. Used by both the titlebar install path ([[src-tauri/src/lib.rs#install_app_update]]) and the tray-menu install path ([[src-tauri/src/lib.rs#check_for_update]]).

For the one-time transition where the *currently-installed* binary does not yet know to set `QUILL_RELAUNCH_PARENT_PID` (it spawns the new fix-enabled binary with the older 750 ms heuristic only), [[src-tauri/src/lib.rs#detect_parent_same_binary_pid]] falls back to parent-executable equivalence: on Linux it compares `/proc/<ppid>/exe` against `/proc/self/exe`, on macOS it uses `libc::proc_pidpath` and `std::fs::canonicalize` so symlinks like `/usr/local/bin/quill → /Applications/Quill.app/Contents/MacOS/quill` compare correctly. A match means the parent is almost certainly a previous Quill instance doing an update-driven relaunch, so the new process waits for it the same way.

## macOS Tray Keep-Alive (Workaround)

Workaround for [tauri-apps/tauri#12060](https://github.com/tauri-apps/tauri/issues/12060): on macOS, the tray's `NSStatusItem` subview becomes detached from the menu bar after sleep/wake or screen-parameter changes, leaving the icon invisible.

[[src-tauri/src/tray_keepalive.rs#install]] subscribes the same `block2::RcBlock` to `NSWorkspaceDidWakeNotification` (via `NSWorkspace.sharedWorkspace().notificationCenter()`) and `NSApplicationDidChangeScreenParametersNotification` (via the default `NSNotificationCenter`). On either notification it calls `tray.set_visible(false)` then `tray.set_visible(true)`, which makes `tray-icon` drop the existing `NSStatusItem` (`NSStatusBar::removeStatusItem` + `removeFromSuperview`) and rebuild a fresh one with the cached icon, menu, and title. `set_icon` alone is insufficient because it only updates the existing button's image and would not re-attach a detached subview.

A 500 ms time-based debounce coalesces wake-with-display-change events that fire both notifications nearly simultaneously. The block runs on `NSOperationQueue.mainQueue()` because tray-icon mutations require the main thread. The non-macOS [[src-tauri/src/tray_keepalive.rs#install]] is an empty stub. **Remove this module once the upstream issue ships a fix.**

## Provider Setup

On startup, [[src-tauri/src/integrations/manager.rs]] refreshes all provider state for the UI.

CLI providers (Claude, Codex) run installers after explicit enable confirmation: Claude via [[src-tauri/src/claude_setup.rs]] and Codex via [[src-tauri/src/integrations/codex.rs]]. Service-only providers like MiniMax ([[src-tauri/src/integrations/minimax.rs]]) require only an API key, stored in the SQLite settings table. CPA follows the service-only persistence pattern but remains a cross-provider usage source rather than a `ProviderStatus`; [[src-tauri/src/integrations/cpa.rs#load_connection]] supplies its URL and Rust-owned key to polling.

### Transactional Managed Deployment

Claude and Codex installers treat managed assets and provider configuration as one recoverable transaction.

[[src-tauri/src/integrations/deploy.rs]] builds each scripts, MCP, and templates tree in a temporary directory beside its target so publication uses same-filesystem renames. Before any live mutation, it recovers an unfinished transaction, persists exact snapshots of owned configuration, hook, command, and instruction paths, then writes a transaction marker. Snapshots preserve missing files, regular-file contents and permissions, and symbolic-link state plus referent contents; restoration never grants permissions broader than the captured mode before applying that exact mode. When a snapshotted link was dangling at capture, restoration clears only a regular file or symlink that the install wrote through it and never deletes a directory that later materialized at the referent — the installers only ever write regular files through managed links, so a directory there is user data the transaction never wrote.

After staged assets pass completeness checks, the installer backs up the live trees and publishes their replacements. Provider configuration, hook and instruction updates, and installation verification run while the marker, snapshots, and asset backups remain. Removing the marker is the commit point; an earlier failure or a marker found by the next guarded mutation restores both the old asset trees and every snapshotted path. Recovery is non-destructive and converges: when a rollback cannot complete, the whole transaction state (marker, snapshots, backups, staging) is quarantined into a uniquely named `.quill-recovery-quarantine-*` directory, logged, and recovery returns Ok so the app stays usable and the data stays recoverable by hand. Marker-free recovery deletes orphaned staging trees but quarantines rather than deletes leftover backups and snapshots, because a manually deleted marker is indistinguishable from a crash after commit. Quarantine directories whose modification time is older than ~30 days are pruned best-effort; backup and snapshot cleanup after commit is best effort.

### Mutation Serialization

One process-local guard serializes workflows that can rewrite shared provider integration state.

[[src-tauri/src/integrations/mod.rs#integration_mutation_guard]] spans provider enable and disable, startup repair and rescan, feature synchronization, restart-hook setup, and Memory Optimizer writes to provider instruction files. After acquiring the lock it attempts and aggregates interrupted-install recovery for both CLI providers before allowing the requested mutation. Because recovery now quarantines an unrollbackable transaction and reports success, the guard no longer fails closed on a persistently failing restore — it only refuses the mutation in the rare case that quarantine itself could not complete, so a single wedged transaction can no longer brick every guarded operation. Holding it across staleness checks, filesystem changes, and status persistence prevents one workflow from recovering or overwriting another workflow's in-progress changes, including when a first enable was interrupted before provider status could be saved.

### Startup Repair

For every already-enabled and detected Claude/Codex provider, [[src-tauri/src/integrations/manager.rs#repair_provider]] takes a stamp-gated fast path on every app launch, reinstalling only when the deployment is stale.

"Reinstall" means redeploying managed scripts/mcp/templates, re-registering the MCP server and hooks, and refreshing CLAUDE.md. `verify()` only checks that managed files exist and hooks are registered — never file *contents* — so a bugfix to a managed file that didn't change presence or registration would silently never reach already-installed users. A per-provider deployment stamp (a content hash of the bundled source trees plus the feature flags and app version, written to the batch parent as `.quill-deploy-stamp` only after a clean commit) closes that gap: when the stamp matches the current inputs AND `verify()` still passes, repair skips the install entirely, restoring the cheap startup the unconditional-reinstall approach had regressed (a full install swaps the MCP tree and forces a `uv` resync). On any stamp mismatch or failed verify, the full transactional `install()` runs its idempotent merge/overwrite pass, verifies before commit, and rewrites the stamp. Feature toggles and explicit enable call `install()` directly since their input change already alters the stamp.

`QUILL_DEMO_MODE=1` keeps startup refresh and manual rescan read-only: provider
detection and isolated status persistence still run, but interrupted-deployment
recovery and enabled-provider repair are skipped so a demo launch cannot mutate
real provider configuration.
