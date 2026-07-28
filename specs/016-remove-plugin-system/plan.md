# Plan: remove-plugin-system

## Architecture Approach

Remove Plugin Manager as a complete vertical slice instead of hiding it behind a
flag. Delete its React composition, Rust domain, Tauri IPC, background work,
runtime settings, events, active documentation, and assets. Shared systems remain
when they have a supported non-Plugin-Manager consumer.

The removal boundary is semantic rather than text-based:

- **Delete:** code and active material whose responsibility is installing,
  removing, enabling, disabling, updating, listing, or advertising provider
  plugins and marketplaces through Quill.
- **Preserve:** Tauri/Vite/ESLint/Sentry framework plugins, app updating,
  process/window-state behavior, shared provider transports, source-backed
  plugin-qualified analytics evidence, and immutable historical records.
- **Do not migrate:** provider-owned plugin state and existing
  `plugin_updates.*` rows remain untouched. Once their contracts and consumers
  disappear, old settings rows are inert unknown keys in the generic store.
- **Use generic invalid-state handling:** deleting `"plugins"` from the Manage
  section registry makes stale query, event, and local-storage values invalid;
  the existing resolver selects Sessions without a feature-specific branch.

Alternatives rejected:

- Hiding only the navigation leaves callable IPC, background I/O, settings, and
  maintenance burden.
- A feature flag or deprecated command layer violates the approved
  no-compatibility boundary.
- Uninstalling provider plugins or purging settings adds destructive mutation
  and rollback work without helping remove executable capability.
- Erasing every textual `plugin` match would damage framework configuration,
  source-backed analytics, and historical truth.
- Refactoring shared provider clients is unnecessary because Codex plugin RPC
  DTOs and helpers are contained in the feature-owned Rust module.

### Constitution Check

- **Principle 1 — Local source-backed truth:** preserve provider state,
  transcript terminology, and historical records; classify residual matches
  instead of rewriting them.
- **Principle 2 — Established stack and boundaries:** remove Rust/Tauri domain
  and IPC ownership together with React feature layers; keep shared transports
  in their current owners.
- **Principle 3 — Responsive execution:** delete the plugin checker, timers,
  filesystem scans, provider subprocesses, and catalog requests; add no
  replacement background work.
- **Principle 4 — Recoverable mutation:** perform no database purge, provider
  uninstall, marketplace deletion, or manifest rewrite. Rollback is a source
  revert because state is untouched.
- **Principle 5 — Typed failure boundaries:** removed invokes become ordinary
  unknown-command failures; stale destinations use existing generic validation
  and cannot render a blank section.
- **Principle 6 — Zero-warning quality gates:** require frontend lint,
  typecheck/build, Rust format/clippy/test/build, hook-runner, residual-audit,
  and `lat check` success.
- **Principle 7 — Authorized behavior testing:** add no tests. Use existing
  automated gates and the approved manual matrix.
- **Principle 8 — Architecture traceability:** update all active `lat.md`
  descriptions and remove links to deleted symbols before `lat check`.
- **Principle 9 — Glass Cockpit discipline:** preserve Tools geometry, focus,
  keyboard behavior, density, and responsive states after removing one rail
  item; make no redesign.
- **Principle 10 — Measured performance:** claim no performance improvement.
  Verify absence of plugin work rather than inventing a startup budget.
- **Principle 11 — Explicit external transmission:** delete marketplace/catalog
  network and provider mutation paths; retain no replacement transmission.
- **Principle 12 — Gated delivery:** implementation stays in Beads, passes both
  human gates and repository checks, and receives no push without authority.

## Affected Components

### Frontend files deleted

- `src/windows/PluginsWindowView.tsx`
- `src/components/plugins/PluginsTabs.tsx`
- `src/components/plugins/InstalledTab.tsx`
- `src/components/plugins/BrowseTab.tsx`
- `src/components/plugins/MarketplacesTab.tsx`
- `src/components/plugins/UpdatesTab.tsx`
- `src/hooks/usePluginData.ts`
- `src/utils/plugins.ts`
- `src/styles/plugins.css`

### Frontend files edited

- `src/windows/ManageWindowView.tsx` — remove the lazy section, icon,
  `ManageSection` member, section definition, palette entry, and render branch.
  Keep generic section resolution so stale `"plugins"` values select Sessions.
- `src/components/TitleBar.tsx` — retain the Tools launcher; remove update count,
  event listener, badge, and plugin-specific class.
- `src/types.ts` — remove Plugin Manager DTOs and the two plugin update fields
  from `RuntimeSettings`.
- `src/hooks/useRuntimeSettings.ts` — remove plugin update defaults.
- `src/components/settings/GeneralTab.tsx` — remove plugin update summary text.
- `src/components/settings/PerformanceTab.tsx` — remove the checker toggle and
  interval input.
- `src/mocks/ipcFixtures.ts` — remove feature commands and runtime fields while
  preserving generic Tauri `plugin:*` browser-mock protocol handlers.
- `src/styles/manage.css` — remove selectors for the deleted embedded view.
- `src/styles/index.css` — remove the plugin-specific Tools button selector.
- `src/styles/settings.css` — remove or correct stale Plugin Manager wording
  without changing unrelated styles.

### Backend files deleted or edited

- `src-tauri/src/plugins.rs` — delete the complete plugin/marketplace domain,
  including Claude filesystem/CLI logic, Codex JSON-RPC DTOs and calls, update
  comparison/cache, bulk progress, validation, and scheduled checker.
- `src-tauri/src/lib.rs` — remove the module, setting constants/bounds,
  `RuntimeSettings` load/save handling, fourteen command wrappers,
  `refresh_update_cache`, managed checker state/setup, event emission, and
  command registration.
- `src-tauri/src/models.rs` — remove
  `plugin_updates_enabled` and `plugin_updates_interval_hours` plus defaults.
- `src-tauri/src/claude_setup.rs` — correct the active stale comment that calls
  Quill's configuration directory a plugin configuration directory; behavior is
  unrelated and remains unchanged.

No product-plugin capability is present in
`src-tauri/capabilities/default.json`, and no manifest dependency is currently
exclusive to `plugins.rs`. `Cargo.toml`, `Cargo.lock`, `package.json`,
`package-lock.json`, `tauri.conf.json`, and generated Tauri schemas change only
if a post-deletion ownership audit proves a feature-exclusive entry.

### Active documentation and assets

- `README.md` — remove feature, controls, screenshot, and source-tree claims.
- `screenshots/plugins.png` — delete the obsolete product image.
- `scripts/take_screenshots.sh` — remove plugin coordinates/capture and fix
  progress counts while leaving supported captures intact.
- `DESIGN.md` and `.impeccable/design.json` — remove Plugins from the Systems
  Pages inventory in lockstep.
- `lat.md/lat.md`, `lat.md/architecture.md`, `lat.md/backend.md`,
  `lat.md/data-flow.md`, `lat.md/features.md`, and `lat.md/frontend.md` — remove
  current feature, layer, IPC, event, background-task, settings, route, hook,
  type, style, and five-section descriptions; preserve unrelated framework and
  analytics terminology.

Superseded specs/plans, release records, closed Beads, audit logs, session
history, `docs/superpowers/`, and git history are not edited solely to erase
historical Plugin Manager references.

## Data Model

No schema migration or new entity is introduced.

- Remove the two plugin update fields from the Rust and TypeScript
  `RuntimeSettings` contracts and their defaults, parsing, clamping,
  persistence, mock payloads, and settings UI.
- Leave existing `plugin_updates.enabled` and
  `plugin_updates.interval_hours` rows untouched in the generic settings table.
  No live type or code path retains knowledge of those keys.
- Delete frontend and Rust transient DTOs for installed plugins, marketplaces,
  updates, results, and bulk progress.
- Delete in-process update-checker cache/state. It is not durable data and needs
  no migration.
- Do not read, write, delete, uninstall, or normalize provider-owned plugin
  manifests, blocklists, marketplace repositories, or installation records.

An upgrade therefore changes only the application contract. A pre-removal
database remains readable because the settings table accepts unrelated keys,
and rollback requires no state reconstruction.

## API / Interface Changes

The following Tauri commands are removed from both wrappers and
`generate_handler!` registration:

- `get_installed_plugins`
- `get_marketplaces`
- `get_available_updates`
- `check_updates_now`
- `install_plugin`
- `remove_plugin`
- `enable_plugin`
- `disable_plugin`
- `update_plugin`
- `update_all_plugins`
- `add_marketplace`
- `remove_marketplace`
- `refresh_marketplace`
- `refresh_all_marketplaces`

Removed events:

- `plugin-changed`
- `plugin-bulk-progress`
- `plugin-updates-available`

Removed UI and settings contracts:

- `"plugins"` is no longer a valid `ManageSection` or command-palette
  destination.
- Installed, Browse, Marketplaces, and Updates tabs no longer exist.
- The Tools title-bar control no longer owns a plugin update badge.
- Runtime settings payloads no longer include plugin update enablement or
  interval fields.
- Browser IPC fixtures no longer emulate Plugin Manager commands.

These are intentional breaking removals. No deprecated invoke aliases, hidden
routes, feature flags, mixed-version frontend/backend support, or provider-native
replacement UI is supplied. Invoking a deleted command fails as unknown.

The generic Manage resolver remains the only stale-state behavior: invalid URL,
stored, or event section values resolve to Sessions or are ignored according to
the existing path. No plugin-specific redirect or cleanup is added.

## Testing Strategy

No automated test code is added. Existing tests may run unchanged.

### Static and automated gates

Run from the repository root unless noted:

```bash
npm run lint
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
cargo build --manifest-path src-tauri/Cargo.toml --all-targets
lat check
```

Run the repository's actual pre-commit hook runner after staging, fix any
formatter changes, re-stage, and rerun it before commit.

Run four residual audits:

```bash
git ls-files | rg '(^|/)(plugins|Plugins)(/|\.|$)|usePluginData|PluginsWindow'
git grep -n -E 'plugin_updates|pluginUpdatesEnabled|pluginUpdatesIntervalHours|plugin-(changed|bulk-progress|updates-available)|get_installed_plugins|get_marketplaces|get_available_updates|check_updates_now|install_plugin|remove_plugin|enable_plugin|disable_plugin|update_plugin|update_all_plugins|add_marketplace|remove_marketplace|refresh_marketplace|refresh_all_marketplaces'
git grep -n -E 'known_marketplaces\.json|installed_plugins\.json|Plugin Manager|plugins\.png' -- README.md DESIGN.md .impeccable lat.md scripts src src-tauri/src
rg -n 'styles/plugins|components/plugins|utils/plugins|PluginsWindow|usePluginData' src
git grep -niE '\bplugins?\b|\bmarketplaces?\b'
```

The first and fourth audits must return no hit. Every second, third, and broad
audit hit must be reviewed across source, package/lock manifests, Tauri
capabilities and shell scopes, generated schemas/declarations, workflows,
scripts, configuration, assets, documentation, specs, and release material.
Active Plugin Manager hits are removed. Every retained hit is recorded in the
release-audit Bead notes as framework infrastructure, source-backed telemetry,
or immutable history; historical paths are classified rather than silently
excluded.

### Reproducible runtime evidence

The final validation task owns one evidence record in its Bead notes. Raw
`strace`, command, and build output should be indexed through Quill working
context rather than committed.

1. Create a temporary validation directory with `mktemp -d`; use existing
   `QUILL_DEMO_MODE=1` plus `QUILL_DATA_DIR` routing and never override `$HOME`.
2. Copy a pre-removal Quill database into the isolated data directory and use
   Python's standard-library `sqlite3` module in read-only mode to record both
   `plugin_updates.*` row values before launch. The standalone `sqlite3` CLI is
   not assumed.
3. Record `stat` metadata and `sha256sum` values for existing provider-owned
   plugin manifests, marketplace records, blocklists, and installation records.
   Missing paths are recorded as absent, not created.
4. Run the Linux debug app under `strace -ff -e trace=process,file,network` for
   at least 45 seconds, exceeding the removed checker's former 30-second initial
   delay. Record the trace prefix and app-log location.
5. Re-query the copied database and recompute provider-file metadata/hashes.
   Values must be unchanged. Summarize trace evidence showing no plugin file
   access, `claude plugin`/marketplace git subprocess, or Codex plugin/catalog
   request; classify unrelated updater/live-usage network activity.
6. Put before/after values, hash comparison, trace summary, residual
   classification, manual matrix outcomes, and quality-gate results in the
   release-audit Bead notes.

### Manual smoke matrix

Use isolated local app/config data; never mutate the user's provider files.

- Fresh start: app opens with no plugin checker state, event subscription,
  provider subprocess, marketplace/catalog request, or plugin filesystem access.
- Upgrade start: launch against a copied pre-removal database containing both
  `plugin_updates.*` rows; app opens normally, unrelated settings persist, and
  the rows are neither read nor rewritten.
- Stale destination: seed `quill-manage-section=plugins`, open
  `?view=manage&section=plugins`, and emit `manage:navigate("plugins")`; URL and
  stored values select Sessions through generic validation, while an invalid
  event cannot produce a blank view.
- Tools workspace: verify exactly Sessions, Learning, Instances, and Settings
  in rail and command palette; ArrowUp/ArrowDown wrapping, Enter selection,
  focus indication, `aria-current`, and Close/Back actions remain correct.
- Normal Tools launch: with no stored/query override, Tools selects Sessions,
  matching the pre-removal default.
- Responsive UI: inspect supported desktop and narrow layouts, including hover,
  focus, loading, empty, and provider-disabled states of remaining sections.
- Title bar and settings: Tools still opens/focuses Manage; no update badge or
  plugin event listener exists; General and Performance settings contain no
  plugin update text or controls; remaining settings save and reload.
- Removed contract: invoking one deleted command rejects as unknown, and no
  removed event can be observed.
- External state: snapshot provider plugin/marketplace file existence and mtimes
  before and after launch; confirm Quill leaves them unchanged.
- Provider integration: verify Claude and Codex detection still render and run
  one existing read-only status refresh without Plugin Manager RPC methods.
- Framework behavior: verify window position/state restoration manually and
  confirm updater/process/window-state imports, registrations, manifests, and
  generated permissions remain unchanged and compile through existing gates.
- Source-backed analytics: run the existing
  `plugin_root_preserved_verbatim` and
  `plugin_root_quoted_with_args_preserves_path` tests, then index an isolated
  transcript fixture containing a plugin-qualified Skill name and confirm its
  normalized skill plus raw `skill://` evidence remain visible.

Coverage mapping:

- US-1: residual audits plus stale-destination, Tools, and responsive checks.
- US-2: Rust/static audit plus fresh/upgrade startup and external-state checks.
- US-3: source-file and symbol audits plus all compile/build gates.
- US-4: README/design/asset review and `lat check`.
- US-5: frontend/Rust gates plus title-bar, settings, analytics terminology, and
  remaining Tools smoke checks.

## Risks

- **False-positive plugin matches:** broad search includes framework packages,
  Tauri IPC protocol, analytics evidence, and history. Mitigate with exact
  feature-string audits and a reviewed residual classification.
- **Cross-layer contract drift:** Rust and TypeScript `RuntimeSettings` must lose
  fields together. Treat frontend and backend excision as parallel work that
  converges before build validation.
- **Accidental shared-infrastructure deletion:** preserve Tools launcher,
  updater/process/window-state plugins, provider transports, Tauri browser mock
  protocol, and analytics normalization. Require a live-consumer check before
  deleting any shared symbol or dependency.
- **Stale navigation regression:** removing a section changes rail wrapping and
  command-palette count. Verify existing `SECTION_IDS` validation and the
  four-item keyboard/focus matrix.
- **Silent remaining I/O:** UI deletion alone would not stop update polling or
  provider mutation. Audit setup, managed state, command registration, events,
  subprocess strings, RPC method strings, and provider file paths.
- **Documentation link breakage:** deleting `plugins.rs` invalidates `lat.md`
  code links. Update active architecture in the same change and require
  `lat check`.
- **Screenshot automation drift:** the script already contains assumptions from
  earlier window layouts. Limit changes to removing the obsolete capture and
  correcting counts; record unrelated script defects separately.
- **Concurrent integration:** the adjacent retention molecule owns
  `specs/015-*`; this run uses `specs/016-*`. Rebase against current `main`
  before squash integration and inspect conflicts from both sides.

Rollback is source-only: reverting the removal restores code that can still read
the inert settings rows and provider-owned state because this plan performs no
data or external-state migration. Verify rollback against copies: revert the
source commit in a disposable worktree, reopen the copied old database/provider
fixture, and confirm the restored feature can read the unchanged state. No
migration rollback step exists.

## Sequencing

### Remove the Rust plugin-management backend

**Priority:** P1. **Dependencies:** none.

Delete `plugins.rs`; remove runtime fields, setting constants, wrappers, events,
checker state/setup, and command registration from shared Rust files. Verify the
crate compiles and confirm no plugin-management process, file, RPC, or event
string remains. Acceptance requires Rust format, clippy, test, and build gates
plus exact removed-symbol and provider-operation greps. This owns Goals 3 and 6
plus US-2 and the backend half of US-3.

### Remove the Plugin Manager frontend

**Priority:** P1. **Dependencies:** none.

Delete feature-owned views/components/hooks/utilities/styles; remove navigation,
badge/listeners, shared types, settings controls, runtime defaults, and command
fixtures. Preserve generic stale-section validation and unrelated Tauri
`plugin:*` mocks. Verify the four-section rail/palette and TypeScript contract.
Acceptance requires frontend lint, typecheck, and build gates; absence of deleted
files/imports/runtime fields; and focused stale/default navigation checks. This
owns Goals 1 and 2 plus US-1 and the frontend half of US-3.

### Synchronize active product and architecture material

**Priority:** P2. **Dependencies:** Rust backend removal and frontend removal.

Update README, screenshot automation, design inventory/sidecar, and six active
`lat.md` documents; delete the plugin screenshot. Preserve historical records
and unrelated terminology. Acceptance requires exact active-doc/asset searches,
DESIGN/sidecar parity, corrected screenshot counts, and `lat check`. This owns
Goals 4 and 5 plus US-4.

### Audit boundaries and validate the release

**Priority:** P1. **Dependencies:** backend removal, frontend removal, and active
documentation synchronization.

Run residual classification, zero-warning quality gates, hook runner, and the
manual fresh/upgrade/navigation/responsive/no-I/O matrix. Verify supported
provider, updater, analytics, Tools, and settings behavior and record any
unrelated pre-existing defect as separate Beads work rather than widening this
feature. Before launching the app, capture the copied database and provider-file
safety baseline described above. Record the required residual classifications,
runtime evidence, command results, manual outcomes, and a release-note draft
stating that Quill no longer manages plugin updates and leaves provider plugins
installed in this task's Bead notes; do not rewrite historical release records.
This owns Goal 7, US-5, and final evidence for every acceptance criterion.

The dependency frontier starts with backend and frontend excision in parallel,
joins at active-documentation synchronization, and finishes with one
release-audit task. No task is created for external cleanup, settings migration,
historical rewriting, shared-client refactoring, UI redesign, or new tests.

## Alignment fixes applied

- **Must-fix, spec↔plan:** added a repo-wide tracked-content audit and explicit
  ownership checks for dependencies, capabilities/scopes, generated material,
  workflows, scripts, configuration, assets, active docs, and history.
- **Must-fix, plan quality:** made no-I/O and no-mutation evidence reproducible
  with isolated demo data, read-only SQLite inspection, provider-file hashes,
  a 45-second process/file/network trace, and one Bead-notes evidence record.
- **Must-fix, spec↔plan:** added concrete preservation checks for normal Tools
  defaulting, provider detection/status, framework updater/process/window-state
  ownership, and plugin-qualified analytics evidence.
- **Should-fix, plan quality:** removed the unnecessary safety-baseline blocker
  so backend and frontend work start in parallel; runtime baseline capture now
  occurs immediately before final smoke execution.
- **Should-fix, both passes:** added camelCase setting-field searches, the stale
  `claude_setup.rs` comment, and command-level acceptance gates for every DAG
  node.
- **Should-fix, both passes:** defined rollback verification against copied old
  state and placed residual, smoke, gate, and release-guidance evidence in the
  final validation Bead notes.
