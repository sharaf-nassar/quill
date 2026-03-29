# Features

Quill provides six major features: live usage monitoring, analytics, behavioral learning, session search, plugin management, and instance restart orchestration.

## Live Usage View

Real-time Claude API rate limit visualization in the main window's left pane.

Displays one row per rate limit bucket (`UsageRow`) with three visualization modes controlled by a time mode selector: **pace marker** (vertical line showing current position), **dual bars** (time elapsed vs utilization), or **background fill** (color gradient). Colors indicate severity: green (<50%), yellow (50-80%), red (>=80%). Reset countdown shows time until each bucket resets. Data refreshes every 3 minutes via `fetch_usage_data()`.

## Analytics Dashboard

Three-tab analytics view in the main window's right pane, powered by [[frontend#Custom Hooks]] and Recharts.

### Now Tab

Real-time metrics dashboard with configurable time range (1h, 24h, 7d, 30d).

Six insight cards: **Session Health** (avg duration, tokens, sessions/day with trend), **Response Time** (avg/peak response, idle time), **Project Focus** (top project breakdown), **Learning Progress** (rule counts, confidence distribution), **Efficiency** (tokens-per-LOC ratio), **Velocity** (LOC-per-hour). Below the cards: a 24-hour activity heatmap and a sortable breakdown panel switchable between hosts, projects, and sessions.

### Trends Tab

Week-over-week comparison charts for token usage trends, code velocity, and cache efficiency.

### Charts Tab

Composite Recharts visualization with three synchronized axes: utilization, tokens, and LOC.

Crosshair context synchronizes tooltip position across chart components. Lazy-loaded with React Suspense to reduce initial bundle size.

## Learning System

AI-powered behavioral pattern discovery that analyzes tool-use observations and git history to extract reusable rules.

### How It Works

Two-stream LLM analysis in [[src-tauri/src/learning.rs]] combining tool-use observations with git commit patterns.

**Stream A** extracts patterns from tool-use observations (collected via hook scripts). **Stream B** analyzes git commit patterns via [[src-tauri/src/git_analysis.rs]]. A synthesis step combines findings and applies LLM verdicts on existing rules. Uses Haiku for extraction and Sonnet for synthesis.

### Confidence Scoring

Wilson lower-bound confidence scoring with a 90-day half-life freshness decay.

States: **emerging** (new, low confidence), **confirmed** (high confidence, validated), **stale** (no recent observations), **invalidated** (contradicted by evidence). Anti-patterns are flagged separately.

### Trigger Modes

Analysis can run: **on-demand** (manual), **session-end** (on close), **periodic** (every N minutes), or **both** (session-end + periodic). Configurable via `LearningSettings`.

### UI

The Learning window has two tabs: **Rules** and **Memories** (memory optimization).

The Rules tab splits rules into two sections: **Active Rules** (have `.md` files on disk) and **Discovered** (DB-only candidates). Active rules show only name, confidence, domain, and delete action — no state badge. Discovered rules show state badge and a promote button with inline two-step confirmation. A `StatusStrip` shows observation counts and a "Run Analysis" button. A floating `RunHistory` panel shows past runs with per-phase timing and real-time logs during active runs.

### Rule Storage

Rules are tracked in the `learned_rules` database table and optionally written as `.md` files to `~/.claude/rules/learned/`.

Rules above the confidence threshold are automatically written to disk. Users can manually promote any discovered rule via the UI, writing stored content to disk regardless of confidence.

### Rule Promotion

Users can promote discovered rules to active rules via [[src-tauri/src/storage.rs#Storage#promote_learned_rule]].

The promote flow reads stored content from the DB, sanitizes it, writes the `.md` file, and updates `file_path` in the database. The rule then moves from the Discovered section to Active Rules on the next UI refresh.

## Session Search

Full-text search across all Claude Code session transcripts, powered by Tantivy in [[src-tauri/src/sessions.rs]].

### Indexing

Session JSONL files are incrementally indexed on app startup (by file mtime) and via HTTP endpoint. Each message is enriched with code_changes, commands_run, tool_details, files_modified metadata. Stored in a Tantivy index at the app data directory.

### Search Interface

Search bar with filters for project, host, role, date range, and git branch.

Results show ranked hits with snippets, tools used, files modified, and code changes. A detail panel shows surrounding context (plus/minus 5 messages). Faceted search provides pre-aggregated project and host counts. Pagination with 20 results per page and load-more.

### Batch Code Stats

`useSessionCodeStats` hook lazily fetches LOC stats for visible search results using a ref-based cache to avoid redundant IPC calls.

## Plugin Manager

Plugin installation, marketplace management, and update tracking via [[src-tauri/src/plugins.rs]].

### Plugin Lifecycle

Plugins enumerated from `~/.claude/plugins/installed_plugins.json` with metadata and blocklist-based enable/disable.

Each plugin has: name, version, scope (global/project), marketplace source, enabled state, description, author, install date. Install/remove/update operations delegate to the `claude plugin` CLI.

### Marketplace System

Marketplaces are git repositories registered in `~/.claude/plugins/known_marketplaces.json`. Each marketplace exposes a manifest of available plugins. Refresh pulls latest via git. Users can add custom marketplace repos.

### Update Checking

A background task checks for updates every 4 hours. Lenient semver comparison detects available updates. Bulk update with per-plugin progress events (`plugin-bulk-progress`). Frontend shows update count badge on the TitleBar.

### Plugin UI

Four tabs: **Installed** (list with enable/disable), **Browse** (available from marketplaces), **Marketplaces** (add/remove/refresh repos), **Updates** (available updates with bulk action). Auto-dismisses operation results after 5 seconds.

## Memory Optimizer

LLM-driven optimization of Claude Code memory files via [[src-tauri/src/memory_optimizer.rs]] (1,670 lines).

### Scanning

Recursively scans project directories for memory files in `.claude/`, `.config/quill/`, and `.instincts.md` paths.

Filters out denylisted patterns, minified code, and compiled files. Dynamic budget allocation based on content type and available sections.

### Analysis

Assembles an LLM prompt with memory content, CLAUDE.md, learned rules, and instinct sections.

Calls Haiku to generate optimization suggestions. Suggestion types: **Delete** (remove redundant), **Update** (improve content), **Merge** (combine related files), **Create** (add missing), **Flag** (needs human review).

### Suggestion Lifecycle

Suggestions follow a status flow: pending -> approved/denied, with backup for undo. Group operations allow batch approve/deny.

Approved suggestions are executed (file written/deleted/merged), with original content backed up. Denied suggestions can be un-denied. Executed suggestions can be undone (restores from backup).

### UI

The Memories tab in the Learning window shows a project selector, memory file browser with content preview, and suggestion cards with actions.

Supports custom project management, bulk operations, and approve/deny/undo per suggestion.

## Restart Orchestrator

Graceful restart of running Claude Code instances via [[src-tauri/src/restart.rs]] (1,134 lines).

### Instance Discovery

Parses state files in `~/.cache/quill/claude-state/` to find running Claude Code processes. Extracts PID, session_id, cwd, tty, and terminal type (Tmux vs plain shell). Cleans up stale state files on startup.

### Restart Flow

Four-phase orchestration with real-time status events at each phase transition.

(1) Discover instances, (2) wait for idle (poll stdin/stdout, max 5 min), (3) send restart signal (SIGUSR1 + restart flag file), (4) wait for exit (20s timeout). Each phase emits `restart-status-changed` events.

### Instance Status

Tracked as: Idle, Processing, Unknown, Restarting, Exited, or RestartFailed. The UI shows status indicators per instance with cancel support.

Force restart skips the idle-wait phase.

### Hook Installation

Deploying restart hooks via `install_restart_hooks()` writes state file writer scripts that Claude Code invokes, enabling better process discovery.
