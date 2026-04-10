# Features

Quill provides six major features: live usage monitoring, analytics, behavioral learning, session search, plugin management, and instance restart orchestration.

## Live Usage View

Real-time provider-aware rate limit visualization in the main window's left pane.

Displays one row per live metric bucket (`UsageRow`) with three visualization modes controlled by a time mode selector: **pace marker** (vertical line showing current position), **dual bars** (time elapsed vs utilization), or **background fill** (color gradient). Colors indicate severity: green (<50%), yellow (50-80%), red (>=80%). Reset countdown shows time until each bucket resets. Data refreshes every 3 minutes via `fetch_usage_data()`.

Claude rows come from the Anthropic OAuth usage API, Codex rows come from `codex app-server` `account/rateLimits/read` (with transcript `token_count` data as a compatibility fallback), and MiniMax rows come from the MiniMax coding plan API at `api.minimax.io`. MiniMax is a service-only provider that requires an API key (stored in SQLite) rather than a local CLI. The live pane groups rows by provider when multiple are enabled, and it can keep rendering cached rows for a provider if live polling fails so other providers do not blank the entire view. Its in-memory usage cache is keyed only by provider identity and enabled state so transient detection churn does not dislodge a fresh snapshot. Claude reuses recently persisted snapshots across window reopens and app restarts, and a 429 response writes a short-lived cooldown so Quill does not retry the live endpoint on every remount.

The top of the live pane now starts with a shared workload summary rail that renders once whenever at least one provider is enabled. It preserves the older `Sessions`, `Projects`, and range-scoped `Tokens` cards while aggregating those counts across the enabled providers instead of showing Codex-only activity.

That summary rail includes the same 1h/6h/12h/24h window selector and freshness indicator as the old Codex workload module, but its data now comes from provider-filtered token and session history fetched across Claude, Codex, or both.

Codex now uses the same quota-style live rows as Claude instead of a separate workload widget. The old active-session ledger is gone, and Codex reset countdowns are derived from the direct app-server rate-limit response, which also adds model-specific rows such as Codex Spark when the account exposes them. When the Codex account has a finite credit balance, it is displayed in the Codex provider header row (right-aligned next to "Usage limits").

The shared workload rail lives in [[src/components/live/LiveSummaryModule.tsx]], the provider row sections live in [[src/components/live/ProviderUsageModule.tsx]], and [[src/components/UsageDisplay.tsx]] now owns the top summary, provider grouping, provider errors, and the time-mode selector for detailed limit rows. When the container is wide enough (≥500px), provider sections display side by side via a CSS container query on `.usage-display`; below that threshold they stack vertically.

## Analytics Dashboard

Three-tab analytics view in the main window's right pane, powered by [[frontend#Custom Hooks]] and Recharts.

The dashboard uses a shared metric selector that groups buckets by label (e.g. "5 hours", "7 days") across all providers via [[src/types.ts#mergeBucketsByLabel]]. When multiple providers share a label, their utilization is averaged. The dropdown shows unified labels without provider prefixes.

### Now Tab

Real-time metrics dashboard with configurable time range (1h, 24h, 7d, 30d).

Six insight cards: **Session Health** (avg duration, tokens, sessions/day with trend), **Response Time** (avg/peak response, idle time), **Project Focus** (top project breakdown), **Learning Progress** (rule counts, confidence distribution), **Efficiency** (tokens-per-LOC ratio), **Velocity** (LOC-per-hour). Below the cards: a 24-hour activity heatmap and a sortable breakdown panel switchable between hosts, projects, and sessions.

When a session row is selected, the breakdown stores both provider and session id, which keeps token history, compact token stats, and delete-session actions aligned with the right Claude or Codex transcript.

### Trends Tab

Week-over-week comparison charts for token usage trends, code velocity, and cache efficiency.

### Charts Tab

Composite Recharts visualization with three synchronized axes: utilization, tokens, and LOC.

Crosshair context synchronizes tooltip position across chart components. Lazy-loaded with React Suspense to reduce initial bundle size, and the utilization series follows the currently selected merged bucket from the shared analytics metric selector.

## Learning System

AI-powered behavioral pattern discovery that analyzes tool-use observations and git history to extract reusable rules.

### How It Works

Two-stream LLM analysis in [[src-tauri/src/learning.rs]] combining tool-use observations with git commit patterns.

**Stream A** extracts patterns from provider-scoped tool-use observations collected by Claude or Codex hooks. **Stream B** analyzes git commit patterns via [[src-tauri/src/git_analysis.rs]]. A synthesis step combines findings and applies LLM verdicts on existing rules. Uses Haiku for extraction and Sonnet for synthesis.

### Confidence Scoring

Wilson lower-bound confidence scoring with a 90-day half-life freshness decay.

States: **emerging** (new, low confidence), **confirmed** (high confidence, validated), **stale** (no recent observations), **invalidated** (contradicted by evidence). Anti-patterns are flagged separately.

### Trigger Modes

Analysis can run: **on-demand** (manual), **session-end** (on close), **periodic** (every N minutes), or **both** (session-end + periodic). Configurable via `LearningSettings`.

### UI

The Learning window has two tabs: **Rules** and **Memories** (memory optimization), plus a provider filter for combined, Claude-only, or Codex-only views.

The Rules tab splits rules into two sections: **Active Rules** (have `.md` files on disk) and **Discovered** (DB-only candidates). Both rules and runs show provider-scope badges so shared Claude-plus-Codex rules are distinct from provider-specific ones. A `StatusStrip` shows scoped observation counts and a "Run Analysis" button. A floating `RunHistory` panel shows past runs with per-phase timing, provider scope, and real-time logs during active runs.

### Rule Storage

Rules are tracked in the `learned_rules` database table with `provider_scope` provenance and optionally written as `.md` files to provider-specific learned-rule directories.

Rules above the confidence threshold are automatically written to disk. Claude-only rules live under `~/.claude/rules/learned/`, Codex-only rules live under `~/.config/quill/learned-rules/codex/`, and shared rules live under `~/.config/quill/learned-rules/shared/`. Users can manually promote any discovered rule via the UI, writing stored content to the directory implied by its provider scope regardless of confidence.

### Rule Watcher

[[src-tauri/src/rule_watcher.rs]] monitors learned-rule directories for real-time filesystem changes using the `notify` crate.

On Create/Remove/Modify events for `.md` files, a debounced (300ms) reconciliation pass diffs the DB against the filesystem: new files are INSERTed with frontmatter-parsed metadata (`source = 'manual'`), deleted files are soft-suppressed (`beta += 5.0`), and modified files have their `content` and `content_hash` updated via [[src-tauri/src/storage.rs#Storage#reconcile_learned_rules]]. Emits `learning-updated` for instant UI refresh.

### Rule Promotion

Users can promote discovered rules to active rules via [[src-tauri/src/storage.rs#Storage#promote_learned_rule]].

The promote flow reads stored content from the DB, sanitizes it, writes the `.md` file, and updates `file_path` in the database. The rule then moves from the Discovered section to Active Rules on the next UI refresh.

## Session Search

Full-text search across Claude Code and Codex session transcripts, powered by Tantivy in [[src-tauri/src/sessions.rs]].

### Indexing

Opening Session Search triggers an mtime-based transcript sync, and hook endpoints can also ingest updates. Indexed messages include code_changes, commands_run, tool_details, and files_modified metadata.

### Search Interface

Search bar with filters for project, host, role, date range, and git branch.

Results show ranked hits with snippets, tools used, files modified, and code changes. A detail panel shows surrounding context (plus/minus 5 messages). Faceted search provides pre-aggregated project and host counts. Pagination with 20 results per page and load-more.

### Batch Code Stats

`useSessionCodeStats` hook lazily fetches LOC stats for visible search results using a ref-based cache to avoid redundant IPC calls.

## Plugin Manager

Plugin installation, marketplace management, and update tracking via [[src-tauri/src/plugins.rs]].

### Plugin Lifecycle

Plugins are enumerated per enabled provider and normalized into one shared UI model with provider badges.

Claude plugins come from `~/.claude/plugins/installed_plugins.json` with blocklist-based enable/disable and CLI-backed install/remove/update actions. Codex plugins come from `codex app-server` plugin APIs and expose install/remove plus provider-native enabled state, but not separate enable/disable or versioned update actions.

### Marketplace System

Claude marketplaces are git repositories registered in `~/.claude/plugins/known_marketplaces.json`. Each marketplace exposes a manifest of available plugins, refresh pulls latest via git, and users can add or remove custom marketplace repos.

Codex marketplaces are discovered from `codex app-server` catalog responses. Quill can refresh the Codex catalog, but add/remove marketplace actions stay Claude-only because Codex does not expose that mutation surface.

### Update Checking

Claude plugin updates are polled every 4 hours.

Lenient semver comparison detects available updates, and bulk update with per-plugin progress events (`plugin-bulk-progress`) remains Claude-only because Codex does not expose versioned update metadata.

### Plugin UI

The plugin window stays shared but behaves per provider.

It switches between enabled providers and keeps one tab set: **Installed**, **Browse**, **Marketplaces**, and **Updates**. The Installed tab hides enable/disable controls for Codex, the Marketplaces tab disables add/remove for Codex, and the Updates tab shows Codex-specific guidance when only refresh is available. Operation result banners auto-dismiss after 5 seconds.

## Memory Optimizer

LLM-driven optimization of provider-aware memory and instruction files via [[src-tauri/src/memory_optimizer.rs]] (1,670 lines).

### Scanning

Recursively scans project directories for Quill memory files plus provider instruction files such as `CLAUDE.md` and `AGENTS.md`.

Filters out denylisted patterns, minified code, and compiled files. Dynamic budget allocation changes based on whether memory files and instruction files are both present.

### Analysis

Assembles an LLM prompt with memory content, provider-scoped instruction files, learned rules, and instinct sections.

Calls Haiku to generate provider-scoped optimization suggestions. Suggestion types: **Delete** (remove redundant), **Update** (improve content), **Merge** (combine related files), **Create** (add missing), **Flag** (needs human review).

### Suggestion Lifecycle

Suggestions follow a status flow: pending -> approved/denied, with backup for undo. Group operations allow batch approve/deny.

Approved suggestions are executed (file written/deleted/merged), with original content backed up. Denied suggestions can be un-denied. Executed suggestions can be undone (restores from backup).

### UI

The Memories tab in the Learning window shows a project selector, provider filter, instruction and memory file browser with content preview, and suggestion cards with actions.

Supports custom project management, bulk operations, provider badges on files and suggestions, and approve/deny/undo per suggestion.

## Restart Orchestrator

Graceful restart of running Claude and Codex sessions via [[src-tauri/src/restart.rs]].

### Instance Discovery

Uses provider-specific discovery with a shared row model.

Claude instances come from Quill state files in `~/.cache/quill/claude-state/` plus process scanning. Codex instances come from process scanning and `~/.codex/sessions/` metadata queues per cwd so multiple same-directory sessions can still map to distinct restart rows.

### Restart Flow

Four-phase orchestration with real-time status events at each phase transition.

(1) Discover instances, (2) wait for idle where supported, (3) send SIGTERM and wait for exit, (4) resume with provider-specific commands. Claude uses `claude --resume`; Codex uses `codex resume`. Each phase emits `restart-status-changed` events.

Codex does not expose a reliable idle signal, so its rows stay `Unknown` before restart and Quill proceeds directly to termination/resume instead of pretending it observed an idle transition.

### Instance Status

Tracked as: Idle, Processing, Unknown, Restarting, Exited, or RestartFailed. The UI shows status indicators per instance with cancel support.

Force restart skips the idle-wait phase.

### Hook Installation

Restart hook actions are provider-aware.

Claude install writes Quill hook scripts into `~/.claude/settings.json` plus shell integration. Codex restart setup currently installs shell integration only, while Codex integration installs only telemetry/session hooks; the `qbuild-guard.sh` edit guard remains Claude-only because Codex hook coverage does not intercept `apply patch` edits. The shared shell integration is only removed when the last restart-capable provider is disabled, and the restart window groups instances by provider with setup banners per provider when integration is missing.
