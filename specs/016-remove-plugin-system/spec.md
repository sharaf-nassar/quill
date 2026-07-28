# Spec: remove-plugin-system

## Problem Statement

Quill still exposes a Plugins section in the Tools workspace and carries a
complete plugin-management stack for Claude Code and Codex. The product no
longer needs this feature, but its UI, IPC, background polling, settings,
provider adapters, documentation, and assets continue to expand the supported
surface and maintenance burden.

This work removes the Plugin Manager as a product capability. It is a direct
deletion: no compatibility commands, hidden routes, legacy adapters, deprecated
types, or fallback behavior remain.

## Goals

1. Remove every user-visible Plugin Manager surface:
   - the Plugins item and icon from the Tools workspace;
   - the installed, browse, marketplaces, and updates views;
   - plugin result, progress, update-count, empty, and provider-selection UI;
   - the title-bar update badge and any plugin-specific window/view path;
   - plugin update controls and summaries from Settings.
2. Remove the frontend implementation owned only by Plugin Manager, including
   components, window composition, hooks, utilities, contracts, mock IPC
   handlers, and CSS.
3. Remove the Rust/Tauri plugin-management domain, including:
   - Claude and Codex plugin/catalog adapters;
   - install, uninstall, enable, disable, update, marketplace, and bulk-update
     operations;
   - Tauri commands and command registration;
   - plugin update state, scheduled polling, cache behavior, and events;
   - plugin-specific runtime settings and defaults.
4. Remove dependencies, configuration, generated declarations, scripts,
   screenshots, and active documentation that exist only for this feature.
5. Update `lat.md/` so architecture, feature, frontend, backend, infrastructure,
   and data-flow documentation no longer describe Plugin Manager as supported.
6. Leave no executable or reachable compatibility path for the removed feature.
   A repo-wide audit must classify any remaining use of the words `plugin`,
   `plugins`, or `marketplace` as unrelated framework terminology, external
   source data, or intentional historical documentation.
7. Keep all applicable existing formatting, lint, typecheck, Rust, build, and
   `lat check` gates at zero warnings.

## Non-Goals

- Removing Tauri, Vite, ESLint, Sentry, or other framework packages merely
  because their package names use the word `plugin`.
- Removing Quill's app updater, process control, window-state persistence, build
  plugins, or provider integrations used by non-Plugin-Manager features.
- Uninstalling, disabling, deleting, or rewriting provider-owned plugin
  installations, marketplace repositories, manifests, or blocklists.
- Purging inert `plugin_updates.*` rows from Quill's generic settings table.
- Changing analytics parsing for historical Claude records whose source-backed
  hook or skill identity contains plugin terminology.
- Redesigning the remaining Sessions, Learning, Instances, or Settings sections
  beyond layout cleanup required after removing the Plugins entry.
- Rewriting superseded specifications, plans, release records, closed Beads,
  audit logs, session history, or git history to erase truthful historical
  references.
- Adding automated test code without separate user authorization.
- Providing a migration shim, deprecated IPC alias, hidden feature flag, or
  other backward-compatible access to Plugin Manager.

## User Stories

### US-1: Remove the obsolete Tools destination

As a Quill user, I want the obsolete Plugins destination gone, so the Tools
workspace contains only supported capabilities.

**Acceptance Criteria**

1. Tools navigation contains Sessions, Learning, Instances, and Settings, with
   no Plugins item, icon, description, badge, or reserved gap.
2. No valid route, query value, window label, lazy import, keyboard target, or
   other UI action can render Plugin Manager.
3. A stale `"plugins"` query or persisted section value is handled only by
   generic invalid-section validation and selects Sessions. No plugin-specific
   redirect, alias, migration, or compatibility branch exists.
4. Opening Tools normally lands on the same supported default section as before
   and all remaining sections render at desktop and narrow widths.
5. Plugin-specific loading, empty, error, operation-result, and progress states
   are absent from the shipped frontend.

### US-2: Stop all plugin-management work

As a Quill user, I want Quill to stop inspecting and mutating provider plugins,
so an unused feature performs no background I/O and cannot alter provider
configuration.

**Acceptance Criteria**

1. Startup does not create plugin update state or spawn a plugin update checker.
2. Runtime settings no longer expose, read, write, clamp, summarize, or reset
   plugin update enablement or interval values. Existing rows with those keys
   may remain inert in the generic settings table and receive no migration.
3. Quill no longer reads Claude plugin manifests or marketplaces, invokes
   `claude plugin` operations, or calls Codex app-server plugin/catalog methods
   for Plugin Manager.
4. No `plugin-changed`, `plugin-updates-available`, or
   `plugin-bulk-progress` event is emitted or listened for.
5. Plugin installation, removal, enablement, update, catalog, and marketplace
   commands are absent from the Tauri invoke surface.

### US-3: Delete the feature instead of preserving dead abstractions

As a maintainer, I want Plugin Manager's owned code removed, so future work does
not have to distinguish live behavior from abandoned compatibility layers.

**Acceptance Criteria**

1. `src/components/plugins/`, the Plugin Manager window/view, plugin data hook,
   plugin utilities, plugin stylesheet, and plugin-only shared TypeScript types
   are deleted.
2. `src-tauri/src/plugins.rs`, its module declaration, command wrappers,
   managed state, setup path, models, and command registrations are deleted.
3. Shared files retain only code still used by supported features; generic
   provider clients or framework plugin packages are not removed by name alone.
4. The frontend and Rust compiler report no dead imports, dead fields, missing
   registrations, or unreachable plugin-management references.
5. No stub module, no-op command, legacy response type, deprecated setting, or
   compatibility comment is introduced.

### US-4: Keep product documentation truthful

As a contributor, I want current docs and visual assets to match the shipped
product, so Plugin Manager is not advertised or represented as available.

**Acceptance Criteria**

1. README feature lists, screenshots, source trees, and active usage guidance no
   longer mention or show Plugin Manager.
2. Screenshot automation no longer attempts to navigate to or capture the
   removed surface, and the checked-in plugin screenshot is removed.
3. `lat.md/` no longer lists plugin management as a feature, backend domain,
   frontend component group, hook, route, data flow, or infrastructure
   responsibility.
4. Superseded specs and plans, release records, closed Beads, audit logs, and
   git history remain unchanged as historical truth.
5. Unrelated uses of plugin terminology remain accurate and are not rewritten
   into misleading language solely to make text searches empty.

### US-5: Preserve supported behavior around the deletion

As a Quill user, I want all non-plugin features to continue working, so removing
Plugin Manager does not damage app updates, provider integrations, analytics,
learning, sessions, instance management, or settings.

**Acceptance Criteria**

1. Tauri updater, process, and window-state packages and their behavior remain
   intact unless an implementation audit proves a dependency was exclusive to
   Plugin Manager.
2. Claude and Codex integration detection and non-plugin app-server operations
   still compile and behave as before.
3. Source-backed analytics handling of plugin-qualified hook and skill names
   remains intact.
4. Shared transports, provider detection, updater/process/window-state behavior,
   and framework plugin packages remain intact.
5. Existing quality gates and the approved manual smoke matrix pass without new
   warnings or new automated test code.

## Constraints

- The repository constitution governs this removal, especially established
  Rust/Tauri and React boundaries (Principle 2), zero-warning gates (Principle
  6), test authorization (Principle 7), `lat.md` traceability (Principle 8),
  Glass Cockpit UI discipline (Principle 9), and gated delivery (Principle 12).
- Removal must be source-driven. A broad `plugin` text match is only a discovery
  aid because Tauri/Vite plugins and plugin-qualified transcript evidence are
  separate concepts that remain supported.
- No backward compatibility or legacy support is required. Compile-time and IPC
  contracts may make a clean breaking deletion.
- No new automated test files or test cases may be added under current
  authorization. Existing checks and focused manual validation supply evidence.
- Quill must not delete or rewrite provider-owned plugin installations,
  marketplace repositories, manifests, or blocklists.
- Existing `plugin_updates.*` settings rows remain inert; implementation removes
  every code path that knows about them and adds no purge migration.
- Stale plugin destinations use existing generic invalid-section handling and
  select Sessions without plugin-specific compatibility code.
- Current source, README, design inventory, screenshots, automation, and
  `lat.md` must be accurate. Historical specifications and records remain
  immutable.
- Shared provider transports and framework plugins remain wherever supported
  non-Plugin-Manager behavior still consumes them.

## Open Questions

No open questions remain after the clarification gate. Approved decisions are
recorded in Clarifications and reflected throughout this specification.

## Spec Review

### Critical Questions (answer before planning)

1. Should removal leave all provider-owned plugin installations, marketplace
   repositories, manifests, and blocklists untouched? The recommendation is
   **yes**: remove Quill's management capability only. External cleanup would be
   destructive, provider-specific, and require rollback behavior that could
   double the work; flagged by: requirements, gaps, ambiguity, feasibility,
   scope, stakeholders.
2. Should existing `plugin_updates.enabled` and
   `plugin_updates.interval_hours` rows remain inert in Quill's generic settings
   table? The recommendation is **yes**: delete every contract, default, read,
   write, and UI path, but add no data-purge migration or compatibility code;
   flagged by: requirements, gaps, ambiguity, feasibility, scope, stakeholders.
3. How should stale navigation state behave after `"plugins"` stops being a
   valid Tools section? The recommendation is to let the existing generic
   invalid-section validation select Sessions, with no plugin-specific redirect,
   alias, or migration; flagged by: requirements, gaps, ambiguity, feasibility,
   scope, stakeholders.
4. What is the historical-artifact boundary? The recommendation is to remove
   Plugin Manager from current source, README, design inventory, screenshots,
   automation, and `lat.md`, while preserving superseded specs and plans,
   release records, closed Beads, audit logs, and git history as historical
   truth; flagged by: requirements, gaps, ambiguity, feasibility, scope,
   stakeholders.
5. Should shared Claude/Codex clients and framework packages lose only
   Plugin-Manager-exclusive code? The recommendation is **yes**: preserve shared
   transports, provider detection, updater/process/window-state behavior,
   Tauri/Vite/ESLint/Sentry plugins, and source-backed analytics terms; flagged
   by: requirements, ambiguity, feasibility, scope, stakeholders.
6. Is the validation contract sufficient without new automated test code? The
   recommendation is to require existing zero-warning frontend and Rust gates,
   build and `lat check`, plus documented fresh/upgrade startup, four-item Tools
   navigation, keyboard focus, desktop/narrow layout, settings persistence,
   unknown-command, and no-plugin-I/O smoke evidence; flagged by: requirements,
   gaps, ambiguity, feasibility, scope, stakeholders.

### Non-Blocking Observations

- No standalone Plugin Manager Tauri window or capability currently exists. The
  shipped surface is an inline Manage section, though stale query and local
  storage values may still name it.
- No Cargo or npm dependency appears exclusive to Plugin Manager. Dependency and
  lockfile edits should be evidence-driven rather than based on the word
  `plugin`.
- No dedicated frontend tests or Rust tests cover Plugin Manager. Deleting the
  feature adds no test-code obligation under current authorization.
- Removing the section from `SECTION_IDS` already makes stale selections fall
  back to Sessions; planning should verify rather than replace this generic
  behavior.
- The removal must retain the Tools title-bar launcher while deleting only its
  plugin update listener, count, badge, and plugin-specific CSS naming.
- A security and operability audit should cover commands, capabilities, shell
  scopes, events, timers, provider calls, generated declarations, mocks, logs,
  screenshots, and background state even where the initial inventory found no
  plugin-specific entry.
- Deleted Tauri invokes should fail as unknown commands. Mixed-version
  frontend/backend operation is outside scope.
- Active design documentation and `.impeccable/design.json` must change
  together so the Systems Pages inventory stays synchronized.
- A reproducible residual-term audit should record why retained `plugin` matches
  are framework infrastructure, source-backed telemetry, or historical records.
- No performance budget is needed because the spec makes no quantified
  performance claim; removal of the update checker is verified as absent work,
  not as a startup-speed target.
- Layout work should stop at removing the rail item and preserving existing
  responsive behavior; redesigning the remaining Tools workspace is out of
  scope.
- Release guidance should state that Quill no longer manages plugin updates and
  does not uninstall provider-owned plugins.

## Clarifications

**Q1: Should removal leave all provider-owned plugin installations, marketplace
repositories, manifests, and blocklists untouched?**

A: Yes. Remove only Quill's management capability. Do not mutate external
provider-owned plugin state.

**Q2: Should existing `plugin_updates.enabled` and
`plugin_updates.interval_hours` rows remain inert?**

A: Yes. Remove all contracts, defaults, reads, writes, and UI paths without
adding a purge migration or compatibility code.

**Q3: How should stale navigation state behave after `"plugins"` becomes
invalid?**

A: Existing generic invalid-section validation selects Sessions. Add no
plugin-specific redirect, alias, stored-state cleanup, or migration.

**Q4: What is the historical-artifact boundary?**

A: Remove Plugin Manager from current source, README, design inventory,
screenshots, automation, and `lat.md`. Preserve superseded specs and plans,
release records, closed Beads, audit logs, session history, and git history.

**Q5: Should shared Claude/Codex clients and framework packages lose only
Plugin-Manager-exclusive code?**

A: Yes. Preserve shared transports, provider detection, updater/process/window
state, framework plugins, and source-backed analytics terminology.

**Q6: Is the proposed validation contract sufficient without new automated test
code?**

A: Yes. Require existing zero-warning frontend and Rust gates, build and
`lat check`, plus documented fresh/upgrade startup, four-item Tools navigation,
keyboard focus, desktop/narrow layout, settings persistence, unknown-command,
and no-plugin-I/O smoke evidence. Add no automated test code.
