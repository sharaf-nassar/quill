# Plugin Manager Design

## Overview

A standalone Plugin Manager window for Quill that provides a GUI for managing Claude Code plugins, browsing marketplace catalogs, managing marketplace sources, and applying updates. Follows the existing standalone window pattern used by Learning and Sessions.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Window type | Standalone window (`?view=plugins`) | Consistent with Learning/Sessions; too much surface area for tray widget |
| Navigation | 4 horizontal tabs | Matches analytics tab pattern; clear separation of concerns |
| Tabs | Installed, Browse, Marketplaces, Updates | Each tab has a focused responsibility |
| Data flow | Hybrid (filesystem reads, CLI mutations) | Fast reads from JSON files; safe writes via `claude plugin` CLI to stay in sync |
| Update detection | On window open + periodic background | Immediate freshness when window opens; background check for badge updates |
| Update surfacing | Badge on plugin manager launch button in main widget | Non-intrusive, visible when glancing at Quill |
| Operation feedback | Inline status per-item, progress bar for bulk ops | Intuitive per-item feedback; bulk progress for Update All |
| Browse organization | Unified list with marketplace grouping | Search across all marketplaces; clear provenance via collapsible groups |
| Architecture | Thin command layer (Approach A) | Mirrors existing patterns; JSON files are small enough that caching is unnecessary |

## UI Design

### Tab Bar

Horizontal tab bar at the top of the window: **Installed**, **Browse**, **Marketplaces**, **Updates**. The Updates tab shows a red badge with the count of available updates (hidden when 0).

### Installed Tab

- **Search bar** to filter installed plugins by name
- **Plugin rows** displaying:
  - Plugin name, version, enabled/disabled badge
  - One-line description
  - Marketplace source and scope (user/project)
  - Action buttons: Disable/Enable toggle and Remove
- **Inline operation status**: when a mutation is in progress, action buttons are replaced by a spinner and status text (e.g., "Removing...")
- **Summary footer**: total count, enabled/disabled breakdown

### Browse Tab

- **Search bar** + **category dropdown filter** (All, development, security, testing, learning, productivity)
- **Marketplace group headers**: collapsible sections showing marketplace name and plugin count
- **Plugin rows** displaying:
  - Plugin name, version, category badge, description, author
  - Install button (green) for available plugins
  - "Installed ✓" label (dimmed) for already-installed plugins
- **Inline install status**: spinner replaces Install button during installation
- **Summary footer**: total plugins across all marketplaces, installed count

### Marketplaces Tab

- **Add marketplace input**: text field for GitHub repo path (e.g., `org/marketplace-repo`) with Add button
- **Marketplace cards** displaying:
  - Marketplace name, source type (github), repo path
  - Last updated timestamp
  - Plugin count and installed count
  - Refresh and Remove action buttons
- **Inline refresh status**: spinner replaces Refresh button during git pull
- **Refresh All** button in footer to update all marketplace repos at once

### Updates Tab

- **Header bar**: update count, last checked timestamp, "Check Now" button, "Update All" button
- **Plugin Updates section**: rows showing current version → new version with per-plugin Update button and inline spinner
- **Marketplace Updates section**: same pattern, or "All up to date ✓" when none available
- **Bulk Update progress** (when Update All is clicked):
  - Progress bar with count (e.g., "2 / 3")
  - Per-item status list: checkmarks for completed, spinner for in-progress
- **Footer**: background check interval and next check time

### Main Widget Integration

- Small "Plugins" button added to the main widget UI
- Badge overlay showing available update count (from background checker)
- Clicking opens the Plugin Manager window or focuses it if already open

## Backend Architecture

### New Module: `src-tauri/src/plugins.rs`

#### Read Functions (Direct Filesystem)

All read functions parse JSON files directly from `~/.claude/plugins/`:

- `get_installed_plugins()` — parse `installed_plugins.json`, enrich with `plugin.json` metadata from `cache/` directory. Returns `Vec<InstalledPlugin>`.
- `get_marketplaces()` — parse `known_marketplaces.json`, read each `marketplace.json` from `marketplaces/` directory. Returns `Vec<Marketplace>`.
- `get_marketplace_plugins(marketplace: &str)` — list all plugins in a specific marketplace manifest. Returns `Vec<MarketplacePlugin>`.
- `get_available_updates()` — compare installed plugin versions against marketplace manifest versions. Returns `UpdateCheckResult`.
- `get_blocklist()` — parse `blocklist.json`. Returns `Vec<BlockedPlugin>`.

#### Mutation Functions (CLI Subprocess)

All mutations shell out to `claude plugin` commands and capture stdout/stderr:

- `install_plugin(name, marketplace)` → `claude plugin install "name@marketplace"`
- `remove_plugin(name)` → `claude plugin uninstall "name"`
- `enable_plugin(name)` → `claude plugin enable "name"`
- `disable_plugin(name)` → `claude plugin disable "name"`
- `update_plugin(name)` → `claude plugin update "name"`
- `add_marketplace(repo)` → `claude plugin marketplace add "repo"`
- `remove_marketplace(name)` → `claude plugin marketplace remove "name"`
- `refresh_marketplace(name)` → `git pull` on the marketplace directory
- `refresh_all_marketplaces()` → `git pull` all marketplace directories

#### Background Update Checker

- Tokio interval task (configurable, default 4 hours) started on app startup
- Calls `get_available_updates()` and stores result in `Arc<Mutex<UpdateCheckResult>>`
- Emits `plugin-updates-available` Tauri event only when the count changes
- On window open, also runs an immediate check

### Tauri Commands (~17 total)

Exposed from `lib.rs` following the existing naming pattern:

**Read commands:**
- `get_installed_plugins() → Vec<InstalledPlugin>`
- `get_marketplaces() → Vec<Marketplace>`
- `get_marketplace_plugins(marketplace: String) → Vec<MarketplacePlugin>`
- `get_available_updates() → UpdateCheckResult`
- `check_updates_now() → UpdateCheckResult`

**Mutation commands:**
- `install_plugin(name: String, marketplace: String) → Result<String>`
- `remove_plugin(name: String) → Result<String>`
- `enable_plugin(name: String) → Result<String>`
- `disable_plugin(name: String) → Result<String>`
- `update_plugin(name: String) → Result<String>`
- `update_all_plugins() → BulkUpdateProgress` (emits progress events)
- `add_marketplace(repo: String) → Result<String>`
- `remove_marketplace(name: String) → Result<String>`
- `refresh_marketplace(name: String) → Result<String>`
- `refresh_all_marketplaces() → Result<String>`

**Window commands:**
- `open_plugin_manager()` — create or focus the plugin manager window

## Frontend Architecture

### New Files

- `src/windows/PluginsWindowView.tsx` — window shell (title bar, drag region, close button)
- `src/components/plugins/PluginsTabs.tsx` — tab bar with active state and update badge
- `src/components/plugins/InstalledTab.tsx` — installed plugins management
- `src/components/plugins/BrowseTab.tsx` — marketplace browser
- `src/components/plugins/MarketplacesTab.tsx` — marketplace source management
- `src/components/plugins/UpdatesTab.tsx` — update management
- `src/hooks/usePluginData.ts` — data fetching hooks
- `src/styles/plugins.css` — all plugin manager styles

### View Routing

Add `plugins` case to `main.tsx` view router:
```
const view = params.get("view");
// existing: "runs", "sessions", "learning"
// new: "plugins"
```

### Data Hooks (`src/hooks/usePluginData.ts`)

- `useInstalledPlugins()` — fetches installed plugins, re-fetches on `plugin-changed` event
- `useMarketplaces()` — fetches marketplace list with their plugin catalogs
- `useAvailableUpdates()` — fetches update check results
- Each hook returns `{ data, loading, error, refresh }` pattern

### Event Flow

1. Mutations invoke the corresponding Tauri command
2. On success, emit a `plugin-changed` Tauri event
3. All hooks listen for `plugin-changed` and re-fetch their data
4. Background update checker emits `plugin-updates-available` — main widget listens for badge
5. Bulk update operations emit `plugin-bulk-progress` events for real-time progress bar updates

### Styling

Pure CSS in `src/styles/plugins.css` using BEM-style class naming:
- `.plugins-tab-bar`, `.plugins-tab-bar__tab`, `.plugins-tab-bar__tab--active`
- `.plugins-row`, `.plugins-row__name`, `.plugins-row__version`, `.plugins-row__status`
- `.plugins-search`, `.plugins-filter`
- `.plugins-marketplace-group`, `.plugins-marketplace-group__header`
- `.plugins-progress`, `.plugins-progress__bar`

## Data Models

### InstalledPlugin

| Field | Type | Source |
|-------|------|--------|
| name | String | installed_plugins.json key |
| marketplace | String | installed_plugins.json key |
| version | String | installation record |
| scope | String | installation record ("user" / "project") |
| enabled | bool | derived from blocklist.json |
| description | String? | plugin.json from cache |
| author | String? | plugin.json from cache |
| installed_at | String | installation record |
| last_updated | String | installation record |
| git_commit_sha | String? | installation record |

### MarketplacePlugin

| Field | Type | Source |
|-------|------|--------|
| name | String | marketplace.json plugins array |
| description | String? | marketplace.json |
| version | String | marketplace.json |
| author | String? | marketplace.json |
| category | String? | marketplace.json |
| source_path | String | marketplace.json |
| installed | bool | cross-referenced with installed_plugins.json |

### Marketplace

| Field | Type | Source |
|-------|------|--------|
| name | String | known_marketplaces.json key |
| source_type | String | known_marketplaces.json source.source |
| repo | String | known_marketplaces.json source.repo |
| install_location | String | known_marketplaces.json |
| last_updated | String? | known_marketplaces.json |
| plugins | Vec<MarketplacePlugin> | marketplace.json |

### PluginUpdate

| Field | Type | Source |
|-------|------|--------|
| name | String | derived |
| marketplace | String | derived |
| current_version | String | installed_plugins.json |
| available_version | String | marketplace.json |

### UpdateCheckResult

| Field | Type |
|-------|------|
| plugin_updates | Vec<PluginUpdate> |
| last_checked | String? |
| next_check | String? |

### BulkUpdateProgress

| Field | Type |
|-------|------|
| total | u32 |
| completed | u32 |
| current_plugin | String? |
| results | Vec<BulkUpdateItem> |

### BulkUpdateItem

| Field | Type |
|-------|------|
| name | String |
| status | String ("success" / "error") |
| error | String? |

## Error Handling

### CLI Command Failures
- Capture stderr from `claude plugin` subprocess
- Return structured error with command and stderr output
- Frontend shows inline error state on affected row (red text, retry option)
- Errors bubble up — never silently swallowed

### Filesystem Edge Cases
- Missing `installed_plugins.json` → return empty plugin list
- Missing `known_marketplaces.json` → return empty marketplace list
- Malformed JSON → return error with file path
- Missing marketplace directory → show "Not cloned" status with Clone button

### Concurrent Operations
- Each mutation is independent — multiple can run simultaneously
- Frontend tracks in-progress operations by plugin name in local state (`Set<string>`)
- Rows with active operations show spinner and disable action buttons
- "Update All" disables individual Update buttons during bulk operation

### Background Update Checker
- Runs on tokio interval, does not block main thread
- On failure, logs error and retries next interval
- Emits event only when update count changes

### Window Lifecycle
- Plugin data fetched when window opens, not on app startup
- Background update checker starts on app startup (for badge)
- Window is lazy — no resource cost until opened
