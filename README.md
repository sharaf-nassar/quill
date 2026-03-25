# Quill

<p align="center">
  <img src="src-tauri/icons/quill-original.png" width="128" alt="Quill icon" />
</p>

A cross-platform desktop widget that displays your Claude AI plan usage in a compact, always-on-top floating window. Built with Tauri + React.

## Features

### Live usage
- Per-5-hour and per-7-day usage with progress bars
- Per-model breakdown (Sonnet, Opus, Code, OAuth)
- Color-coded percentages that transition green → yellow → red as usage increases
- Countdown timers showing time until usage resets
- Three time display modes (pace marker, dual bars, background fill)
- Token sparkline showing per-turn token counts over time

### Analytics
- Historical usage charts with dual-axis visualization (utilization + tokens)
- Per-bucket statistics with min/max/average, trend indicators, and sparklines
- **Breakdown panels** — token usage grouped by host, project, or session with per-item data deletion
- Time range selection (1h, 24h, 7d, 30d)

### Session search
- Full-text search across all Claude Code sessions (powered by Tantivy)
- Filter by project, host, role, date range, and git branch
- Snippet highlighting with expandable message context
- Opens in a dedicated search window from the titlebar

### Token tracking
- Per-turn input/output/cache token counts via Claude Code hook
- **Multi-host support** — remote Claude Code instances can report usage over the network
- Token sparkline in the live view and dual-axis chart overlay in analytics

### Learning
- Integrated side panel that shows learned usage rules, observation stats, and analysis history
- Configurable triggers: on-demand, session-end, periodic, or combined
- Rule state tracking (emerging → confirmed → stale → invalidated)
- Domain-grouped rules with confidence scores
- Run history with real-time analysis logs

### Memory optimizer
- Scans your Claude Code memory files and suggests improvements (merge duplicates, update stale content, remove obsolete entries)
- Approval-based workflow — review each suggestion with a diff preview before applying
- Undo any applied change to restore the original file
- Batched "optimize all" to review and apply suggestions across an entire project

### MCP server
- Gives Claude Code direct access to your indexed session history via MCP tools
- **`search_history`** — full-text search across all sessions by content, edits, commands, or tool use
- **`list_projects`** / **`list_sessions`** — browse projects and sessions
- **`get_session_context`** — retrieve surrounding messages for a search hit
- **`get_branch_activity`** — see all work done on a git branch
- **`get_token_usage`** — query token usage and cost analytics
- **`get_learned_rules`** — retrieve learned coding patterns
- **`get_tool_details`** — inspect full tool input/output for a specific action
- Automatically available when the Quill plugin is installed — no extra configuration needed

### Desktop integration
- **System tray** with Show / Always on Top / Check for Update / Quit
- **In-app updater** — checks on startup and every 4 hours; yellow "Update" button appears in the titlebar
- Always-on-top mode (toggleable from tray menu)
- Semi-transparent dark theme with custom drag-to-move titlebar
- Remembers window position and size across restarts
- Auto-refreshes usage every 60 seconds
- Read-only OAuth — reads Claude Code's token, never refreshes it

## Screenshots

<table>
  <tr>
    <td align="center"><strong>Live usage</strong></td>
    <td align="center"><strong>Analytics</strong></td>
    <td align="center"><strong>Learning</strong></td>
  </tr>
  <tr>
    <td><img src="screenshots/live-view.png" width="280" alt="Live usage view with progress bars and token sparkline" /></td>
    <td><img src="screenshots/analytics-view.png" width="320" alt="Analytics view with usage chart and host breakdown" /></td>
    <td><img src="screenshots/learning-panel.png" width="280" alt="Learning panel with rules, confidence scores, and domains" /></td>
  </tr>
</table>

<p align="center">
  <strong>Session search</strong><br/>
  <img src="screenshots/session-search.png" width="300" alt="Session search with filters and highlighted results" />
</p>

## Architecture

```mermaid
%%{init: {'theme': 'base', 'themeVariables': {
  'primaryColor': '#1e293b',
  'primaryTextColor': '#e2e8f0',
  'lineColor': '#94a3b8',
  'secondaryColor': '#334155',
  'tertiaryColor': '#0f172a',
  'fontFamily': 'ui-sans-serif, system-ui, sans-serif',
  'fontSize': '18px',
  'edgeLabelBackground': '#334155'
}}}%%

graph LR
    subgraph Sources [" "]
        Claude(["Local Claude Code"])
        Remote(["Remote Hosts"])
    end

    subgraph PluginBox [" Claude Code Plugin "]
        Hooks(["Hook Scripts"])
        MCP(["MCP Server"])
    end

    Claude -- "&ensp;hooks&ensp;" --> Hooks
    Remote -. "&ensp;hooks · over network&ensp;" .-> Hooks
    Claude <-->|"&ensp;MCP protocol&ensp;"| MCP

    Hooks -- "&ensp;tokens · observations · sessions&ensp;" --> Backend
    MCP -- "&ensp;queries&ensp;" --> Backend

    API(["Anthropic API"])
    GH(["GitHub Releases"])

    API -- "&ensp;usage · every 60s&ensp;" --> Backend
    Backend -. "&ensp;LLM analysis&ensp;" .-> API
    GH -- "&ensp;update check&ensp;" --> Frontend

    subgraph Widget [" Tauri Desktop App "]
        Frontend(["React Frontend"]) <-->|"&ensp;Tauri IPC&ensp;"| Backend(["Rust Backend"])
        Backend <--> SQLite[(SQLite)]
        Backend <--> Tantivy[(Tantivy)]
    end

    style Claude fill:#6366f1,stroke:#818cf8,color:#fff,stroke-width:2px
    style Remote fill:#6366f1,stroke:#818cf8,color:#fff,stroke-width:2px,stroke-dasharray: 5 5
    style Hooks fill:#6366f1,stroke:#818cf8,color:#fff,stroke-width:2px
    style MCP fill:#14b8a6,stroke:#2dd4bf,color:#fff,stroke-width:2px
    style API fill:#f59e0b,stroke:#fbbf24,color:#000,stroke-width:2px
    style GH fill:#10b981,stroke:#34d399,color:#000,stroke-width:2px
    style Frontend fill:#3b82f6,stroke:#60a5fa,color:#fff,stroke-width:2px
    style Backend fill:#ef4444,stroke:#f87171,color:#fff,stroke-width:2px
    style SQLite fill:#a855f7,stroke:#c084fc,color:#fff,stroke-width:2px
    style Tantivy fill:#ec4899,stroke:#f472b6,color:#fff,stroke-width:2px
    style Widget fill:#1e293b,stroke:#475569,color:#e2e8f0
    style PluginBox fill:#1e293b,stroke:#475569,color:#e2e8f0
    style Sources fill:transparent,stroke:transparent

    linkStyle 0 stroke:#818cf8,stroke-width:2px
    linkStyle 1 stroke:#818cf8,stroke-width:2px,stroke-dasharray: 5 5
    linkStyle 2 stroke:#2dd4bf,stroke-width:2px
    linkStyle 3 stroke:#818cf8,stroke-width:2px
    linkStyle 4 stroke:#2dd4bf,stroke-width:2px
    linkStyle 5 stroke:#f59e0b,stroke-width:2px
    linkStyle 6 stroke:#f59e0b,stroke-width:2px,stroke-dasharray: 5 5
    linkStyle 7 stroke:#10b981,stroke-width:2px
    linkStyle 8 stroke:#60a5fa,stroke-width:2px
    linkStyle 9 stroke:#c084fc,stroke-width:2px
    linkStyle 10 stroke:#f472b6,stroke-width:2px
```

## Prerequisites

- [Claude Code](https://docs.anthropic.com/en/docs/claude-code) installed and logged in (`claude /login`)

### For development

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 18+
- System dependencies for Tauri (Linux):
  ```bash
  sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
  ```

## Installation

### From releases

Download the latest release for your platform from the [Releases](../../releases) page:
- **Linux**: `.deb` (recommended) or `.AppImage`
- **Windows**: `.exe`
- **macOS**: `.dmg`

#### Linux setup

**Debian/Ubuntu (recommended)** — installs the binary, desktop entry, and icons system-wide:

```bash
sudo dpkg -i Quill_*_linux_amd64.deb
```

**AppImage** — portable executable, no installation required:

```bash
chmod +x Quill_*_linux_amd64.AppImage
./Quill_*_linux_amd64.AppImage
```

#### Linux uninstall

To fully remove Quill and its data:

```bash
# If installed via .deb:
sudo dpkg -r quill

# If using AppImage:
rm -f ~/Applications/Quill_*_linux_amd64.AppImage

# Remove app data (usage database, auth secret, logs, etc.)
# macOS:
rm -rf ~/Library/Application\ Support/com.quilltoolkit.app
# Linux:
rm -rf ~/.local/share/com.quilltoolkit.app

# Remove hook config
rm -rf ~/.config/quill
```

### From source

```bash
git clone https://github.com/sharaf-nassar/quill.git
cd quill
npm install
cargo tauri build
```

The built binary will be in `src-tauri/target/release/`.

## Setup

The widget reads OAuth tokens from Claude Code's credentials file (`~/.claude/.credentials.json`). Make sure you are logged in:

```bash
claude /login
```

No additional configuration is needed — the widget starts tracking utilization immediately.

## Token Tracking, Learning & Session Search (Optional)

The widget includes an HTTP server (port `19876`, configurable via `QUILL_PORT`) that receives data from Claude Code via hooks. The plugin enables three features:

- **Token tracking** — per-turn input/output/cache token counts, powering the sparkline in the live view and the token overlay on the analytics chart
- **Learning** — observes tool usage patterns across sessions and can analyze them to extract reusable rules (stored in `~/.claude/rules/learned/`)
- **Session search** — indexes Claude Code session transcripts for full-text search with filters

The HTTP server uses bearer-token authentication and rate limiting to secure incoming data.

### Install the hook (Claude Code plugin)

1. Add the marketplace:

```
/plugin marketplace add sharaf-nassar/quill
```

2. Install the plugin:

```
/plugin install quill-hook@sharaf-nassar/quill
```

3. **Restart** Claude Code, then run the setup skill:

```
/quill-hook:setup
```

The setup skill will ask where the widget is running (this machine or a remote IP) and save the config. After setup, every Claude Code turn will report token counts and tool observations to the widget.

### Using the learning panel

Once the plugin is installed and observations are being collected:

1. Click the **✦ button** in the titlebar to open the learning panel
2. Toggle learning **ON** with the switch in the panel header
3. Choose a trigger mode:
   - **On-demand** — click "Analyze" in the panel to run analysis manually
   - **Session-end** — automatically analyzes after each Claude Code session ends
   - **Periodic** — runs analysis on a configurable interval
   - **Combined** — both session-end and periodic enabled together
4. Analysis extracts patterns from observations and creates rule files in `~/.claude/rules/learned/`
5. Learned rules appear as cards in the panel with confidence scores and domain tags

You can also trigger analysis from Claude Code by running the learn skill:

```
/quill-hook:learn
```

### Manual install (alternative)

```bash
curl -fsSL https://raw.githubusercontent.com/sharaf-nassar/quill/main/hooks/install.sh | bash
```

With a remote widget host:

```bash
curl -fsSL https://raw.githubusercontent.com/sharaf-nassar/quill/main/hooks/install.sh | bash -s -- --url http://<widget-ip>:19876 --hostname my-server
```

### Multi-host setup

Multiple machines can report to a single widget. Install the plugin on each machine and point them to the same widget IP during setup. Each machine's hostname appears in the widget for filtering.

### Verify

```bash
# Check the server is running
curl http://localhost:19876/api/v1/health

# Send a test payload
curl -X POST http://localhost:19876/api/v1/tokens \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"test","hostname":"dev","input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}'
```

## Development

```bash
npm install
cargo tauri dev
```

## Controls

- **Drag the title bar** to move the window
- **Drag any edge or corner** to resize
- **Gear icon** to switch between time display modes
- **Chart icon** to toggle the analytics view
- **Star button (✦)** to toggle the learning panel — the window expands rightward to show it and shrinks back when closed
- **Search icon** to open the session search window
- **System tray icon** — left-click to show the widget; menu has Always on Top, Check for Update, and Quit

## Project structure

```
src/                          # React frontend
  main.tsx                    # Entry point
  App.tsx                     # Main app component with learning sidebar
  types.ts                    # Shared TypeScript interfaces
  components/
    TitleBar.tsx              # Custom titlebar (drag, view toggles, search, update)
    SectionHeader.tsx         # Reusable collapsible section header
    UsageRow.tsx              # Usage row with progress bar + token sparkline
    UsageDisplay.tsx          # Container for all usage rows
    analytics/
      AnalyticsView.tsx       # Analytics tab with charts, stats, and breakdowns
      BreakdownPanel.tsx      # Host/project/session breakdown with deletion
      UsageChart.tsx          # Dual-axis chart (utilization + tokens)
      shared.tsx              # Shared analytics utilities
    learning/
      StatusStrip.tsx         # Observation stats and sparkline
      RuleCard.tsx            # Individual learned rule display
      SuggestionCard.tsx      # Memory optimization suggestion with diff view
      MemoriesPanel.tsx       # Memory optimizer panel with approve/undo
      DomainBreakdown.tsx     # Rules grouped by domain
      RunHistory.tsx          # Past analysis run log
      FloatingRunsWindow.tsx  # Floating window for run history with live logs
    sessions/
      SearchBar.tsx           # Full-text search input with debounce
      FilterBar.tsx           # Collapsible filters (project, host, role, date)
      ResultCard.tsx          # Search result with expandable context
  windows/
    LearningWindow.tsx        # Learning panel (integrated sidebar)
    RunsWindowView.tsx        # Standalone run history window
    SessionsWindowView.tsx    # Session search window
  hooks/
    useAnalyticsData.ts       # Fetches utilization history and stats
    useBreakdownData.ts       # Fetches host/project/session breakdowns
    useTokenData.ts           # Fetches token history, stats, hostnames
    useLearningData.ts        # Fetches learning rules, runs, observations
    useMemoryData.ts          # Fetches memory files, optimization runs, suggestions
    useToast.tsx              # Toast notification system
  utils/
    time.ts                   # Relative time formatting
    tokens.ts                 # Token count formatting (1.2k, 1.5M)
  styles/
    index.css                 # Global styles + dark theme
    learning.css              # Learning panel styles
    sessions.css              # Session search styles
src-tauri/                    # Rust backend
  src/
    main.rs                   # Tauri entry point
    lib.rs                    # IPC commands, tray icon, updater, server startup
    ai_client.rs              # Rig Anthropic integration for learning analysis
    auth.rs                   # OAuth token management
    config.rs                 # Credential loading (read-only)
    fetcher.rs                # Usage API calls
    learning.rs               # Learning analysis spawner
    memory_optimizer.rs       # Memory file scanning, LLM analysis, and suggestion execution
    models.rs                 # Data models (usage buckets + token + learning types)
    prompt_utils.rs           # Prompt sanitization utilities
    sessions.rs               # Tantivy full-text session search and indexing
    storage.rs                # SQLite storage with aggregation
    server.rs                 # axum HTTP server for token reporting
  tauri.conf.json             # Tauri window and build configuration
plugin/                       # Claude Code plugin (hook + setup/learn skills)
  .claude-plugin/
    plugin.json               # Plugin manifest
  hooks/
    hooks.json                # PreToolUse, PostToolUse, and Stop hook config
  scripts/
    observe.js                # Captures tool observations (pre/post tool use)
    report-tokens.sh          # Extracts tokens from transcript, POSTs to widget
    session-sync.js           # Syncs session metadata and messages to widget
    session-end-learn.js      # Triggers learning analysis on session end
  skills/
    setup/
      SKILL.md                # Interactive setup wizard
    learn/
      SKILL.md                # Manual learning analysis trigger
    build/
      SKILL.md                # Multi-agent feature coordinator
  commands/
    setup.md                  # Setup command documentation
    learn.md                  # Learn command documentation
    build.md                  # Multi-agent feature build command
  mcp/
    server.py                 # FastMCP server for session history tools
    dependencies.py           # Lifespan and shared state
    tools/
      search.py               # search_history, get_session_context, get_branch_activity
      discovery.py            # list_projects, list_sessions, get_session_overview
      analytics.py            # get_token_usage, get_learned_rules
      details.py              # get_tool_details
hooks/                        # Standalone hook scripts (non-plugin)
  quill-hook.sh               # Standalone Stop hook
  install.sh                  # curl-pipe installer
```

## Releasing

Releases are driven by git tags via `release.sh`. The CI workflow (`.github/workflows/release.yml`) builds and publishes automatically.

```bash
./release.sh bump patch    # v0.3.1 -> v0.3.2
./release.sh bump minor    # v0.3.1 -> v0.4.0
./release.sh retag          # Re-point latest tag to current HEAD
./release.sh latest         # Show current version
```

`bump` and `retag` generate user-facing release notes via Claude, commit them as `release_notes.md`, then tag and push. The CI picks up the notes and applies them to the GitHub release.

The `tauri-action` patches the version in `tauri.conf.json` at build time using the tag — you do not need to update version numbers manually. The workflow builds for all platforms (Linux AppImage + .deb, macOS dmg for Intel + ARM, Windows nsis), then publishes the release.

The in-app updater checks `latest.json` on GitHub Releases on startup and every 4 hours. When an update is found, a yellow "Update" button appears in the titlebar.

## License

MIT
