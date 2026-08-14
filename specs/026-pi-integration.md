# pi-integration

## Problem Statement

Quill indexes, analyzes, and augments Claude Code and Codex sessions. Users of
the pi coding agent (pi.dev, `@earendil-works/pi-coding-agent`) get none of
that: no session search, no live tracking, no token analytics, no
working-context tools inside their agent. Integrate pi as a third CLI provider
with the same capabilities as the Claude Code and Codex integrations: session
transcript indexing and live session tracking, token and hook telemetry,
`search_history` and working-context tool access from inside pi, a managed
instruction block, and transactional install/uninstall with startup repair.

Unlike Claude Code (settings.json command hooks) and Codex (config.toml
hooks), pi has no external hook system. Its only event surface is in-process
TypeScript extensions. The integration therefore registers itself by deploying
a Quill-owned pi extension instead of hook commands — the architecture
(managed assets, transactional deploy, local HTTP telemetry, transcript
watcher) stays the same.

## Goals

- Pi appears in the Integrations tab as a detectable, enable/disable-able CLI
  provider with the same lifecycle as Claude Code and Codex: detection,
  explicit enable confirmation, transactional install, startup repair
  (stamp-gated), uninstall restoring prior state.
- Pi sessions are indexed and searchable with provider-safe identity
  (`pi` provider tag; no collision bleed with Claude/Codex session ids).
- The live session tracker folds pi transcripts as they are written:
  liveness, cwd (from the header), last model/provider, last-entry
  timestamp. Live cumulative tokens render as an explicit gap for pi in v1 —
  the active-branch tree walk is deferred behind a written budget
  (full-fidelity usage still reaches analytics via the watcher).
- Token/model usage from pi sessions flows into the existing analytics as
  provider `pi` with per-model breakdown underneath (usage only — Quill has
  no pricing table; cost display is out of scope).
- `search_history` and the full working-context tool set are callable from
  inside pi via `pi.registerTool()` (names `quill_`-prefixed), backed by a
  NEW local context HTTP API on Quill's server — today only `search_history`
  is HTTP-backed; the context tools talk to SQLite directly from the MCP
  process, so this feature funds the HTTP surface (including a threat model
  for exec-over-HTTP). No MCP server deployment for pi.
- Context-preservation routing runs in pi: when the feature is enabled, the
  Quill extension's `tool_call` handler applies the `context-router.cjs`
  policy (deny noisy fetch/read dumps with actionable reasons naming the
  `quill_*` tools, taint tracking for fetch-then-read bypasses).
- Pi session lineage and subagent-style activity are tracked: `parentSession`
  header chains (fork/clone/child sessions) link in Quill, and concurrently
  live child sessions surface as open-agent activity. Pi has no native
  subagent concept — what is tracked is what transcripts prove.
- A managed Quill instruction block lives in `~/.pi/agent/AGENTS.md` (pi's
  global context file), owned and removed exactly like the Codex AGENTS.md
  block.
- Event/hook-fire telemetry from the pi extension feeds the existing hooks
  ingestion so the Now-tab breakdown can attribute pi activity.
- The integration honors `PI_CODING_AGENT_DIR` (config dir, default
  `~/.pi/agent`) and `PI_CODING_AGENT_SESSION_DIR` (session dir override),
  persisting the selected dirs in integration state the way the Codex
  integration persists `CODEX_HOME`.

## Non-Goals

- No npm/pi-package publication of the extension; it is a Quill-managed file
  drop, versioned by the deployment stamp.
- No project-local `.pi/extensions/` deployment — global only, which also
  avoids pi's project-trust prompts.
- No MCP server deployment for pi (pi has no native MCP; tool parity comes
  from `pi.registerTool()`).
- The Quill extension never silently mutates pi behavior. Blocking via
  `tool_call` is allowed for exactly one purpose — context-preservation
  routing when that feature is enabled, always with an actionable reason
  string — and `tool_result` is never modified.
- No support for embedding pi via its SDK/RPC modes; only the interactive CLI
  and its on-disk sessions are in scope.
- No Windows support (consistent with the Codex integration).
- No cost display for pi (no token-price table exists in Quill; a pricing
  table would be its own feature).
- Pi does not appear in the Limits section at all — no row, no N/A copy
  (`LimitsSection.tsx` stays claude|codex).
- Deferred for pi v1: restart-orchestration hooks, Memory Optimizer coverage
  of pi instruction files, brevity-profile injection (excluded like MiniMax
  so the global toggle cannot error on pi), ephemeral sessions (no session
  file → skipped), v1-format legacy session files (surfaced as an explicit
  "unsupported version" gap, never silently missing), and pi participation
  in any remote session sync. Disabling pi keeps already-indexed data.

## Backlog Inputs

None. No source backlog issue and no open P4 sources exist for this feature
(closure computed over an empty set — the run creates a fresh epic).

## Target Epic

No existing epic. This run creates the feature epic.

## User Stories

### 1. Detection and enablement

As a pi user, I want Quill to detect my pi install and offer one-click
enablement, so that integrating pi is as easy as Claude Code or Codex.

Acceptance criteria:
- Detection finds the `pi` CLI on PATH (with the same launcher/symlink PATH
  augmentation used for Codex on macOS app launches) and/or a populated pi
  config dir (`$PI_CODING_AGENT_DIR` or `~/.pi/agent`).
- The Integrations tab lists pi with detected/enabled state; enable requires
  explicit confirmation like other CLI providers.
- Detection and status refresh never mutate pi state; `QUILL_DEMO_MODE=1`
  stays read-only.

### 2. Session indexing and search

As a user, I want my pi sessions searchable in Quill, so that past pi work is
as recoverable as Claude/Codex work.

Acceptance criteria:
- The transcript watcher registers the pi session root
  (`$PI_CODING_AGENT_SESSION_DIR` or `~/.pi/agent/sessions/`), including
  late-root recovery when the directory appears after startup.
- The indexer parses pi's JSONL format: header entry
  (`{type:"session", version, id}`), `message` / `compaction` /
  `branch_summary` / `custom` entries, v3 tree structure via `id`/`parentId`.
- Indexed sessions carry `pi` provider identity end to end: search hits,
  context lookup, facets, reindex cleanup.
- Tree sessions index the content users can actually search for without
  duplicating branched prefixes into misleading hit counts (exact branch
  semantics decided in plan).

### 3. Live session tracking

As a user, I want live pi sessions on the Sessions surface, so that Quill is a
real-time cockpit for pi work too.

Acceptance criteria:
- The live tracker folds pi transcript appends: liveness, cwd (from the
  session header), last model/provider, last-entry timestamp; live
  cumulative tokens display as an explicit gap ("—") for pi.
- Branch navigation (`/tree`, `/fork`, `/clone`, compaction) does not corrupt
  live state; rewrites are detected by fingerprint and trigger a cold
  re-fold (constitution 1: gaps stay explicit, never invented).
- A pi session ends via file quiescence under the existing `IDLE_AFTER`
  semantics (shutdown-telemetry acceleration is a possible follow-up, not
  a v1 criterion).
- Ephemeral sessions (no session file) produce no liveness row.

### 4. Token and usage analytics

As a user, I want pi token usage in Quill's analytics, so that per-provider
usage and cost views cover all my agents.

Acceptance criteria:
- Usage rows carry provider `pi`, plus the upstream provider/model strings pi
  records in `AssistantMessage` (`provider`, `model`, `usage`); no cost
  column is populated for pi.
- Ingestion is watcher-side only (extension never POSTs tokens); sessions run
  while Quill was closed are covered by backfill; dedupe key is session
  header id + entry id.
- Spend aggregates sum every `AssistantMessage.usage` (all branches); any
  per-session total surface is labeled active-branch. Pi usage totals equal
  the sum over the transcript set.
- Pi never appears in the Limits section.

### 5. Quill tools inside pi

As a pi user, I want `search_history` and the working-context tools available
in my pi sessions, so that pi agents can use Quill's context store the way
Claude Code and Codex agents do.

Acceptance criteria:
- A new context HTTP API on Quill's local server exposes the working-context
  operations (index, fetch-and-index, execute, search, get-source, stats,
  purge) currently implemented Python-side against SQLite; the exec-over-HTTP
  endpoint has an explicit threat model (loopback-only bind, shared-secret
  auth, bounded output) before it ships.
- The Quill extension registers `quill_`-prefixed tools via
  `pi.registerTool()`; execution calls the local HTTP API with the named
  `lib.cjs` timeout constant and a hard local-only URL guard.
- Tool availability follows the same global feature flags that gate MCP
  working-context tools for other providers (`context_preservation`).
- Quill being closed degrades gracefully: the tool returns a typed, concise
  error result; pi keeps working.

### 6. Managed instruction block

As a user, I want pi's global instructions to carry the Quill guidance block,
so that pi agents route history lookups and large output through Quill.

Acceptance criteria:
- Install writes/refreshes exactly one managed block in
  `~/.pi/agent/AGENTS.md` (config-dir-relative); uninstall removes only the
  managed block, preserving user content byte-for-byte.
- Block content follows the Codex variant (names key tools; pi does not
  surface MCP instructions because there is no MCP).

### 7. Event telemetry

As a user, I want pi activity attributed in the Now-tab hooks/activity
breakdown, so that observability parity holds across providers.

Acceptance criteria:
- When activity tracking is enabled, the extension reports lifecycle events
  (session start/shutdown, agent/turn boundaries, tool execution) to the
  existing hooks-observed ingestion, mapped onto the closest CC/Codex event
  vocabulary.
- Reporting is fire-and-forget with hard timeouts; a slow or dead Quill never
  delays a pi turn (constitution 3).

### 8. Transactional lifecycle

As a user, I want enable/disable to be safe and reversible, so that my pi
configuration survives crashes mid-install (constitution 4).

Acceptance criteria:
- Install/uninstall run inside the existing `.quill-deploy-backup`
  transactional deployment (same-filesystem staged renames, backup manifest,
  crash recovery, mutation-guard serialization).
- Quill-owned artifacts are structurally identifiable (the extension file and
  the AGENTS.md block), so a lost `integration-state.json` cannot cause Quill
  to adopt or orphan user files.
- Startup repair is stamp-gated like Claude/Codex: verify semantics, skip
  when current, transactional reinstall when stale.

### 9. Context routing inside pi

As a user with context preservation on, I want pi steered away from flooding
its transcript with raw dumps, so that pi sessions get the same
context-savings discipline as Claude Code and Codex.

Acceptance criteria:
- When `context_preservation` is enabled, the extension's `tool_call` handler
  applies the `context-router.cjs` policy to pi's tools: deny noisy
  fetch/read dumps with an actionable reason naming the `quill_*`
  replacement (including the ready-to-paste URL rewrite), track
  fetch-to-file taint, and gate subsequent reads of tainted paths.
- Every block carries a reason string; nothing is blocked silently, and
  disabling the feature removes all routing behavior on next `/reload`.
- Routing telemetry flows to the existing context-savings ingestion when
  `context_telemetry` is enabled, with routing-category token fields
  normalized to zero as today.

### 10. Session lineage and subagent-style tracking

As a user, I want pi child and forked sessions linked and counted, so that
multi-session pi activity is visible the way subagent activity is for other
providers.

Acceptance criteria:
- `parentSession` header chains link fork/clone/child sessions to their
  parent in Quill's session views.
- Concurrently live child pi sessions surface as open-agent activity on the
  parent where lineage proves the relationship; absent lineage, sessions
  stay independent rows (nothing invented — constitution 1).
- Pi rows never claim native subagent counts; the surfaced number is live
  linked sessions, labeled as such.

## Constraints

- **Pi has no external hooks.** Extensibility is in-process TypeScript
  extensions auto-discovered from `~/.pi/agent/extensions/` (global) or
  `.pi/extensions/` (project, trust-gated), hot-reloadable via `/reload`.
  Global and CLI extensions load without project-trust prompts.
- **Extension events (verified against pi.dev docs):** `project_trust`,
  `session_start` (reason: startup/reload/new/resume/fork,
  `previousSessionFile`), `session_before_switch`, `session_before_fork`,
  `session_before_compact`/`session_compact`, `session_info_changed`,
  `session_shutdown` (also on Ctrl+C/SIGHUP/SIGTERM), `resources_discover`,
  `input`, `before_agent_start`, `agent_start`, `agent_end`, `agent_settled`,
  `turn_start`, `turn_end`, `message_start/update/end`,
  `tool_execution_start/update/end`, `tool_call` (blockable),
  `tool_result` (modifiable), `model_select`, `thinking_level_select`,
  provider request/response hooks. Quill uses observe-only handlers.
- **Extension context:** `ctx.sessionManager.getSessionFile()` exposes the
  transcript path (undefined for ephemeral sessions); `pi.registerTool()`
  registers LLM-callable tools; `pi.appendEntry()` persists custom entries
  (Quill should not write into user sessions). Bash tool subprocesses receive
  `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`; pi sets
  `AI_AGENT=pi` and `PI_CODING_AGENT=true` process markers.
- **Sessions:** JSONL at
  `<session-dir>/--<cwd-with-slashes-as-dashes>--/<timestamp>_<uuid>.jsonl`;
  session dir is `~/.pi/agent/sessions/` unless `PI_CODING_AGENT_SESSION_DIR`
  overrides. Header `{type:"session", version, id}`. Version 3 current;
  v1 linear and v2 tree files auto-migrate when pi loads them, so on-disk
  files may still be v1/v2 — the parser must tolerate all three.
  `AssistantMessage` embeds `api`, `provider`, `model`, `usage`,
  `stopReason`, `timestamp`. Entries form a tree (`id`/`parentId`) with
  in-place branching — unlike CC/Codex linear JSONL, appends are not always
  tail-extensions of the active path.
- **No native MCP.** Community `pi-mcp-adapter` exists but is not a
  dependency Quill should take; `pi.registerTool()` is the first-party
  surface. The Python MCP venv is not deployed for pi.
- **Config surfaces:** global settings `~/.pi/agent/settings.json`, project
  `.pi/settings.json`, trust decisions `~/.pi/agent/trust.json`. Quill's
  integration should not need to touch settings.json at all (extension
  discovery is directory-based) — a materially simpler deployment than Codex
  (no TOML merge, no trust-hash reconciliation).
- **Global context file:** pi loads `~/.pi/agent/AGENTS.md` (global), then
  parent-directory and cwd AGENTS.md/CLAUDE.md; `AGENTS.override.md`
  replaces a directory's file. `--no-context-files` disables discovery.
- **Extension runs in pi's process.** Any blocking call stalls the user's
  agent. All Quill I/O must be fire-and-forget or bounded (hard timeout,
  local-only base URL), mirroring `lib.cjs`.
- **Install channel:** pi ships via npm (`@earendil-works/pi-coding-agent`)
  or the pi.dev installer script; extension API types import from that
  package. Single-file `quill.ts` with no third-party imports is the target
  shape so no `node_modules` install step is needed.
- **Constitution:** 2 (extend existing Rust/Tauri integration, storage, IPC
  layers — new `src-tauri/src/integrations/pi.rs`, not a parallel system),
  3 (watcher/indexer off UI threads; bounded background work), 4
  (transactional deploy, serialized via the integration mutation guard), 5
  (typed failure boundaries for parse/IO errors), 8 (lat.md sync + `lat
  check`), 11 (extension transmits only to the local Quill API; nothing
  off-device).
- **Design:** DESIGN.md fixes a provider color-code (Claude blue · Codex
  cyan · MiniMax violet · Agent orchid). Pi needs a reserved color/label
  before any UI shows it; that is a DESIGN.md/`.impeccable/design.json`
  update, not an ad-hoc pick.

## Open Questions

1. **Token ingestion source.** Watcher-parsed usage (from `AssistantMessage`
   entries) vs. extension-POSTed usage vs. both with dedupe. Watcher-only
   covers sessions run while Quill was closed and keeps the extension
   smaller; extension-only matches how CC/Codex token hooks work today.
   Where does dedupe live if both?
2. **Tree semantics for index and live tracker.** Index every branch, only
   the active branch, or branch-aware dedupe? How does the live tracker
   identify "the active branch" from appends alone (`branch_summary` /
   `compaction` entries, last-entry parent chain)?
3. **On-disk v1/v2 sessions.** Confirm pi does not rewrite old files on load
   (docs say migration happens on load). How much v1/v2 parsing is worth
   shipping vs. indexing v3-only and labeling older files unsupported?
4. **Ephemeral sessions** (no session file): the extension can still observe
   events. Report activity without a transcript anchor, or skip entirely?
5. **Provider color and naming** for the Glass Cockpit provider code — needs
   a DESIGN.md decision (which reserved hue represents pi?).
6. **Pi version compatibility.** Extension API stability across pi releases;
   is there a minimum pi version to require at detection, and how should
   verification behave when pi's extension API changes shape?
7. **Feature-flag mapping.** Which of the four global integration feature
   flags apply to pi v1? Activity tracking and context telemetry map
   naturally; context-preservation routing (PreToolUse output rerouting)
   may be deferred — its pi analog would require modifying tool behavior,
   which conflicts with observe-only.
8. **Subagent/open-agent semantics.** Pi has no native subagents (community
   extensions add them). Does the live tracker's open-agent membership just
   stay empty for pi, and does anything downstream assume otherwise?
9. **Hook-telemetry vocabulary mapping.** Map pi events onto the existing
   observed-hook event names (e.g. `turn_end` → Stop-equivalent) or extend
   the vocabulary with pi-native names?
10. **Session identity.** Pi session ids come from the header `id` and the
    filename uuid; confirm which one anchors provider-safe identity and
    dedupe across resume/fork (fork writes a new file with
    `previousSessionFile` provenance — should forks link to their parent
    session in Quill?).

## Spec Review

Merged from parallel dimension reviews (scope and stakeholders subagents
completed; requirements, gaps, ambiguity, and feasibility performed
sequentially after repeated subagent API-overload failures), grounded in the
Quill codebase and pi's actual source
(`pi-mono/packages/coding-agent/src/core/session-manager.ts`).

### Critical Questions (answer before planning)

1. **What does the "provider" dimension mean for pi analytics?** Pi is a
   multi-model harness: one session can run Anthropic, OpenAI, or local
   llama.cpp models and switch mid-session (`model_change` entries; each
   `AssistantMessage` records its own upstream `provider`/`model`).
   Recommendation: provider = `pi` (harness) everywhere, with per-model
   breakdown underneath. And note Quill has NO token-price table today —
   every `cost_usd` in the system arrives pre-computed from the Claude
   Code SDK for learning runs — so pi v1 shows usage only, no cost
   (a pricing table would be its own feature). A related semantics call:
   summing every `AssistantMessage.usage` counts abandoned branches,
   which is correct for spend and wrong for session totals (pi's own
   `/session` reports active-branch totals) — recommendation: analytics
   spend sums all entries; any per-session total surface is labeled
   active-branch. — flagged by: gaps, requirements, feasibility.
2. **MVP cut: does v1 ship the in-pi extension at all?** Transcripts alone
   deliver detection, indexing, live tracking, and token analytics (usage
   is embedded in the JSONL) — a transcript-only v1 writes NOTHING to the
   user's machine: no TypeScript asset, no AGENTS.md block, no
   transactional-deploy surface, and "enable" means "index my pi sessions".
   The extension adds in-pi tools (story 5), the instruction block (story
   6 — which must ship WITH the tools it advertises or not at all), and
   event telemetry (story 7); it also drags in the consent question below,
   the pre-1.0 API risk, and the in-process performance budget.
   And story 5 as written is not buildable against the current API: only
   `search_history` is HTTP-backed today — the working-context MCP tools
   open the context SQLite store directly and use subprocess for
   `quill_execute` (`src-tauri/claude-integration/mcp/tools/context.py`;
   no context-store routes exist in `src-tauri/src/server.rs`) — so
   "tools in pi" means either `search_history` only, or funding a new
   context HTTP API surface including its exec-over-HTTP risk.
   Option A: v1 = stories 1–4 only ("pi, observed"), extension as a
   follow-up feature. Option B: v1 adds the extension with
   `quill_search_history` only (no new API surface). Option C: full
   parity v1 (stories 1–8) including a new context HTTP API. — flagged
   by: scope, feasibility, stakeholders, requirements.
3. **Confirm the deferred-feature list as explicit non-goals for pi v1:**
   restart-orchestration hooks, Memory Optimizer coverage of pi files,
   brevity-profile injection for pi (the global brevity toggle iterates
   enabled providers and must explicitly exclude pi the way it excludes
   MiniMax, or it errors), context-preservation routing, CPA/limits band
   (`LimitsSection.tsx` hard-types claude|codex), subagent/agent rails
   (pi has none natively; rows stay empty), fork lineage links, ephemeral
   sessions, v1-format legacy files, and pi participation in any remote
   session sync. Disabling pi keeps already-indexed data (matching
   existing provider behavior). — flagged by: scope, gaps.
4. **Glass Cockpit provider identity for pi.** DESIGN.md's provider
   color-code and `lat.md/frontend.md` disagree today (DESIGN.md: Claude
   blue · Codex cyan; frontend.md:514: Claude orange `#fb923c` · Codex
   blue `#60a5fa`) — reconcile that, then reserve pi's hue and label in
   DESIGN.md + `.impeccable/design.json` before any pi pixel ships. —
   flagged by: stakeholders, constitution 9.
5. **Consent and auto-update posture for executable code (only if the
   extension ships).** The enable-confirmation copy
   (`src/components/settings/IntegrationsTab.tsx:349`) describes "hooks,
   commands, MCP configuration" — it does not describe writing executable
   code that runs inside the user's agent process with shell privileges
   and is silently rewritten by stamp-gated repair. The consent dialog
   must name the file, its path, and the self-update behavior; the
   stale-until-`/reload` window after an atomic rewrite is accepted,
   stated behavior. — flagged by: stakeholders.
6. **Risk acceptance: pi's extension API is pre-1.0 and moves (only if
   the extension ships).** Mitigation: pin a minimum pi version at
   detection (unknown/unparseable version = not installable, typed
   status), verify extension integrity on repair, design `quill.ts` to
   fail silent so a pi upgrade degrades telemetry/tools instead of
   breaking the user's agent. Accept that tools/telemetry lag pi releases
   until Quill updates. — flagged by: feasibility, stakeholders.

### Technical Decisions (self-resolved — veto at the gate to override)

Transcript pipeline (needed in every MVP variant):

- **Token ingestion is watcher-side only.** Usage parses from
  `AssistantMessage` entries; the extension never POSTs token data.
  Single source kills dedupe and covers sessions run while Quill was
  closed. Dedupe key: session header id + entry id (uuidv7). Zero
  in-turn overhead for the pi user.
- **The live tracker does NOT walk the tree in v1.** The hardest problem
  in this feature: active-branch state is a function of the path from
  leaf to root, which is O(whole file) — pi's leaf pointer is private
  in-memory state (`/tree` jumps write zero bytes; only the next
  append's `parentId` reveals a switch), so the existing stateless
  bounded tail fold (`FileTail`, `read_codex_tail`) cannot compute it,
  and an in-memory id→entry map is unbounded and dies with the
  15-minute idle eviction. v1 folds from the tail only what the tail
  can prove: liveness, last model/provider, last-entry timestamp — and
  renders live cumulative tokens as an explicit gap ("—") for pi
  (constitution 1). A tree-walking fold is a follow-up that requires a
  written memory/CPU budget first (constitution 3, 10). Active-branch
  semantics, where needed offline (analytics labels), use pi's own
  `buildSessionPath` rule: parent chain of the last entry in file order.
- **Index every `message` entry exactly once by entry id.** Branches
  share ancestors by reference — no copied prefixes exist in the file —
  so FTS indexing is naturally duplication-free; the tree matters only
  for the live fold.
- **Session files are append-only after an initial deferred flush**
  (entries buffer until the first assistant message, then one `wx`
  write, then appends). Rewrites exist (v1/v2→v3 migration when pi
  reopens an old file; empty-file init) and can leave the file EQUAL OR
  LONGER — a shrink-only guard misses them and would feed half-records
  into the fold. The pi tail stores an `(mtime_ns, len)` fingerprint
  (reuse `transcript_identity.rs#ModelSourceFastFingerprint`) and
  cold-restarts on any discontinuity. `/fork`/`/clone` create NEW files
  (header `parentSession`), never mutate the source.
- **Parse v2+v3; label v1 unsupported.** v3 only renames a role (v2/v3
  share the tree shape — tolerate `hookMessage` as `custom`); v1 has no
  `id`/`parentId` and pi itself rewrites any file it reopens to v3, so
  remaining v1 files are dormant — surface them as an explicit
  unsupported-version gap rather than silently skipping (constitution 1).
  Unknown entry types (`label`, `custom`, future) skip gracefully.
- **Session identity anchors on the header `id`** (uuidv7, also in the
  filename; filename is fallback when the header is unreadable). Header
  carries `cwd` directly — never decode the lossy `--dashed-path--`
  directory name (the same ambiguity is a documented defect in the
  memory-optimizer slug round-trip). `parentSession` stored as
  provenance and rendered as lineage links per story 10.
- **Ephemeral sessions are skipped entirely** — no transcript anchor, no
  liveness row.
- **`IntegrationProvider` gains a `Pi` variant** (closed enum,
  `src-tauri/src/integrations/types.rs`; ~170 compiler-forced decision
  sites — breadth, not depth). Exhaustive matches make every missed
  site a compile error. Watcher roots and ingestion payloads are
  provider-tagged and generalize; the known fixed-arity spots are
  `sessions.rs:334` (`[(IntegrationProvider, &str); 2]`),
  `transcript_watcher.rs:72-91` (`transcript_roots()`), and
  `live_tracker.rs:585-606` (two-root sweep). Every existing provider
  iteration (brevity sync, memory optimizer, restart/resume, quota
  indicator, learning rule scope) gets an explicit pi arm — excluded
  like MiniMax unless the gate says otherwise — so the new variant
  cannot silently change a global toggle's behavior, and each excluded
  surface gets explicit UI copy rather than a silent N/A.
- **The pi model-usage adapter is budgeted as its own task** — the Codex
  analog is ~640 lines (`model_usage.rs:1236-1876`) plus identity
  resolution and delta normalization; pi needs new `SourceRecordShape`
  variants and a pi `NativeChainIdentity` resolver. Mechanical, not
  small.
- **Initial backfill rides the existing reindex path:** enabling pi
  registers its session root and triggers the same full-index sync used
  for Claude/Codex; late-root recovery covers a directory created after
  startup. Backfill of pre-existing sessions is part of story 2's
  acceptance, not an implied side effect.
- **Config-dir precedence follows the Codex pattern:** capture
  `$PI_CODING_AGENT_DIR`/`$PI_CODING_AGENT_SESSION_DIR` (or defaults) at
  enable, persist in `integration-state.json`, keep targeting the
  recorded dirs even if the environment later changes; reject an empty
  env value or a path occupied by a non-directory, as Codex does.
  Production transcript roots are HOME-derived constants today (env
  overrides are demo-mode-only in `data_paths.rs`), so honoring the
  persisted pi dirs needs a real mechanism: resolve the pi session root
  from integration state once at startup, re-resolve on integration-
  state mutation, and add a demo override so demo builds never index
  real pi sessions.
- **Every provider-filtered query and wire contract admits pi or becomes
  provider-keyed.** Aggregations are hardcoded two-provider today
  (`WHERE provider IN ('claude','codex')` at
  `src-tauri/src/storage.rs:12788,12878`; `claude_count`/`codex_count`
  columns in `src-tauri/src/models.rs:339-371`; the
  `IntegrationProvider` union in `src/types.ts:279`) — without a sweep,
  pi rows silently vanish (constitution 1). One regression test asserts
  a pi hook row and a pi usage row surface in the breakdown.
- **Downgrade-safe status persistence:** once `"pi"` lands in the saved
  `ProviderStatus` list, an older Quill's whole-array deserialization
  fails and drops every provider's enablement
  (`src-tauri/src/integrations/manager.rs:590-602` returns empty on
  error) — parse saved statuses per-entry and skip unknown providers
  instead of discarding the array.
- **Fold cost is budgeted (constitution 10):** reuse the existing
  oversize rejection and bounded tail-read patterns
  (`live_tracker.rs#read_codex_tail`); one `stat` per quiet transcript,
  per-append fold bounded by the tail window, full re-fold only on the
  rewrite guard; demonstrated on a synthetic large branched session.
- **Detection mirrors the Codex state table**
  (`(detected_cli, detected_home)` → Installed/Missing/NotInstalled,
  `codex.rs:258-278`), surfaces install failures via `last_error`, and
  handles a non-writable extensions dir as a typed failure.
- **Numeric feature-level success criteria:** a pi session created after
  enable is searchable within the existing index-sync latency bound;
  liveness follows the existing `IDLE_AFTER` semantics; pi usage totals
  equal the sum of `AssistantMessage.usage` over the transcript set.

Extension (whichever release it lands in):

- **Use a plain JSON Schema object for `registerTool.parameters`.** A real
  pi 0.84.1 extension-loader run passed `pi.registerTool()` with a plain
  object, a bare `typebox` import, and a `createRequire` fallback rooted at
  pi's install. The plain object is the smallest deployable shape and pi's
  validator explicitly supports JSON Schema without TypeBox symbols. It
  needs no import, bundled dependency, or companion file, so the lifecycle
  owned manifest remains `quill.ts` plus the managed `AGENTS.md` block.
  Reproduce with `node scripts/spike_pi_register_tool.mjs`; the script uses
  isolated pi config/session directories and removes them after the run.
- **Local-only is a hard guard, not an inherited discipline.** The shared
  `~/.config/quill/config.json` consumed by `lib.cjs` supports a REMOTE
  `config.url` (and `session-sync.cjs` ships transcripts off-device), so
  the pi extension must refuse any non-local URL outright; pi data never
  participates in remote sync in v1 (constitution 11).
- **Tool names are `quill_`-prefixed** (`quill_search_history`, …): pi's
  tool namespace is flat and duplicate semantics are pi-version-
  dependent, so bare names invite collisions with other extensions. The
  pi managed block names the prefixed tools (block content is
  per-provider already). Each `registerTool` is individually try/caught
  so a collision degrades to "no Quill tools", never a failed load.
- **Hardened, self-disabling single file:** fixed filename with a header
  comment naming Quill, the version/stamp, and removal instructions;
  every handler body try/caught; no top-level side effects; hard no-op
  when Quill's config file is missing or unparseable (also the
  Quill-was-deleted orphan path). Install/repair sweeps
  `~/.pi/agent/extensions/` for other Quill-marked files (structural
  ownership, same philosophy as `mcp_entry_is_quill_owned`). Global
  `fetch` is safe (pi requires node ≥ 22.19), but `registerTool`
  parameters use TypeBox at runtime and a bare `typebox` import from
  the extensions dir may not resolve against pi's bundled copy — a
  30-minute spike against real pi (plain object literal vs.
  `createRequire` fallback) is a prerequisite task, because discovering
  this mid-build collapses the no-`node_modules` deployment story.
- **Deployment is a configuration-only transaction — never stage the
  extensions directory.** `publish_stage` renames the ENTIRE target
  directory aside into the backup, which would swallow the user's other
  pi extensions; pi's shape is exactly the existing
  `FileSnapshots::capture` + configuration-only `commit()` path
  (`deploy.rs:205-254`): snapshot the two file paths (`quill.ts`,
  `AGENTS.md`), stage nothing.
- **Performance budget (constitution 10):** handler synchronous work
  single-digit milliseconds; all HTTP unawaited/fire-and-forget with the
  existing bounded local timeout; one measured run on a real session as
  acceptance evidence.
- **Telemetry vocabulary maps onto existing hook-event names** (no
  schema change): `session_start`→SessionStart, `input`→UserPromptSubmit,
  `tool_call`→PreToolUse, `tool_result`→PostToolUse, `turn_end`→Stop,
  `session_shutdown`→SessionEnd, `session_before_compact`→PreCompact,
  `session_compact`→PostCompact. Pi-only events unreported in v1.
  `/api/v1/hooks/observed` currently hard-rejects providers other than
  claude/codex and events outside the Codex CamelCase vocabulary
  (`server.rs:930-992`), so the endpoint contract gains the pi provider
  but keeps the existing event names — the cheap choice, since the
  vocabulary is baked into stored `hook_invocations` rows forever. The
  same ingestion doubles as a support heartbeat ("installed, no pi
  activity seen") in the Integrations row.
- **Feature-flag mapping:** `activity_tracking` gates event telemetry;
  `context_preservation` gates working-context tool registration
  (`context_telemetry` stays dependent on it, as for Codex); `brevity`
  excluded for pi in v1. Flags that are no-ops for pi say so in Settings
  copy.
- **Packaging mirrors existing assets:** `src-tauri/pi-integration/`
  beside `claude`/`codex-integration/`, included in bundle resources,
  hashed into the deployment stamp; deployment uses the existing atomic
  staged-rename transaction.
- **Repair verification for pi matches the Codex verification contract**
  (`codex.rs#verify_with_paths` rigor, not an existence check): the
  deployed extension carries the current payload/version marker
  constant, no non-expected Quill-marked extension file is present
  (orphan detection; a feature toggled off removes its file), the
  AGENTS.md managed block is current, and integration state parses;
  `deployment_is_current` = stamp match AND verification, mirroring
  `codex.rs:338`.
- **Uninstall is manifest-driven and total:** after uninstall no
  Quill-owned artifact remains (extension file, stamp, state, AGENTS
  block) and no user byte changed — the owned manifest defines
  "Quill-owned", as Codex's uninstall does.
- **Install never requires killing pi:** pi discovers extensions at
  process start, so verification asserts on-disk state only and the UI
  states "restart pi or run `/reload` to activate"; a running pi keeping
  the old extension until reload is stated behavior.
- **Timeout values are named, not adjectival:** the extension reuses the
  existing `lib.cjs` timeout constant verbatim; acceptance criteria cite
  the constant rather than "bounded".

### Non-Blocking Observations

- The indexer generalizes to the tree almost for free: `startup_scan`
  already delete-then-reinserts whole sessions on fingerprint change,
  and each tree entry appears in the file exactly once — abandoned
  branches are simply extra searchable docs. Whether their hits get an
  "abandoned branch" label is plan-time polish.
- Uninstall while a pi process runs leaves the loaded extension in memory
  until `/reload`; its bounded fail-silent POSTs to a stopped Quill are
  harmless.
- New-session liveness appears only at the first assistant reply
  (deferred flush) — inherent to pi's design.
- `--no-context-files` and `AGENTS.override.md` users never see the
  managed block; accept the degradation, do not detect it.
- Release-time documentation sweep: PRODUCT.md, README, marketing site,
  and release notes enumerate three providers today; the release-notes
  entry must disclose the file written into `~/.pi/agent/` (if the
  extension ships).
- The pi community's idiomatic channel is pi packages (npm); the managed
  file drop is legitimate and uninstall-safe, but the ownership header
  (above) is what keeps it a good citizen.

## Clarifications

**Q1: What does "provider" mean for pi analytics, and does cost display
ship?**
A: 1A — provider = `pi` (harness) with per-model breakdown; usage only, no
cost (no pricing table exists). Spend sums all branches; per-session totals
labeled active-branch. Addendum: pi is fully absent from the Limits section
— no row, no N/A copy. (Reflected in Goals, Non-Goals, story 4.)

**Q2: Does v1 ship the in-pi extension, and how much of the tool surface?**
A: 2C — full parity including a NEW local context HTTP API so the complete
working-context tool set is available from pi. If the API surface proves too
large for this run, backlog beads may be filed to flush it out fully rather
than cutting scope. (Reflected in Goals, story 5.)

**Q3: Deferred-feature list?**
A: 3B — context-preservation routing and subagent/session tracking are IN
scope (stories 9 and 10; observe-only non-goal rewritten to permit
reasoned `tool_call` blocks for routing only). The rest of the deferred
list stands: restart hooks, Memory Optimizer coverage, brevity injection,
CPA/limits, ephemeral sessions, v1-format files (explicit unsupported
surface), remote sync. (Reflected in Goals, Non-Goals.)

**Q4: Provider color?**
A: 4B — `lat.md/frontend.md` is correct (Claude orange `#fb923c`, Codex
blue `#60a5fa`); DESIGN.md is reconciled to match, and pi reserves a DARK
GREEN hue, chosen distinct from the severity-meter green so the reserved
status semantics stay unambiguous. DESIGN.md + `.impeccable/design.json`
updated before any pi UI lands.

**Q5: Consent copy for executable code?**
A: 5A — approved. The pi enable dialog names the extension file, its path,
and the stamp-repair self-update behavior.

**Q6: Pi pre-1.0 API risk?**
A: 6A — accepted with mitigations: pinned minimum pi version at detection
(unknown version = not installable, typed status), integrity verification
on repair, fail-silent extension design.

## Architecture Approach

Pi becomes the third CLI provider by extension of every existing seam, not a
parallel system (constitution 2): a `Pi` variant on `IntegrationProvider`, a
new `src-tauri/src/integrations/pi.rs` mirroring the Codex module's shape
(detection state table, install/verify/uninstall, integration-state.json),
bundled assets under `src-tauri/pi-integration/`, and provider arms in the
transcript watcher, indexer, live tracker, and model-usage pipeline. The
registration surface is one Quill-owned TypeScript extension file dropped
into `~/.pi/agent/extensions/` — pi has no external hooks — deployed through
the existing configuration-only `FileSnapshots` transaction (never staging
the extensions directory; constitution 4).

The new context HTTP API is implemented natively in Rust, operating on the
SAME context SQLite store the Python MCP tools use (shared schema contract,
WAL concurrency). Alternatives rejected: proxying to a spawned Python helper
(a second runtime on the hot path, worse failure modes) and porting the MCP
server wholesale (nothing pi can speak MCP to). CRITICAL bind posture: the
existing axum app deliberately binds `0.0.0.0` (`server.rs:169-171`, remote
hosts reach it), so `/api/v1/context/*` — especially `execute` — must NOT
mount there: it gets a separate `127.0.0.1`-only listener (or an enforced
peer-address check on every context route), shared-secret auth, output caps,
and working-dir scoping, and the whole mount is gated behind a setting that
is off unless a consumer is enabled (constitution 11; the threat model
starts from the real 0.0.0.0 posture, not an assumed loopback).

The live tracker deliberately does not walk pi's tree in v1: the fold reads
only tail-provable state (liveness, last model, last-entry timestamp) and
renders live cumulative tokens as an explicit gap for pi (constitution 1, 3,
10 — the O(file) walk ships later behind a written budget). Analytics get
full-fidelity usage from the watcher-side model-usage adapter, which parses
whole files off the UI thread on change (constitution 3).

## Affected Components

- `src-tauri/src/integrations/types.rs` — `Pi` variant; `as_str`/`FromStr`.
- `src-tauri/src/integrations/pi.rs` (new) — detection `(cli, home)` state
  table, min-version gate, install/verify/uninstall, orphan sweep of
  Quill-marked extension files, pi-version + dirs in integration state.
- `src-tauri/src/integrations/manager.rs` — provider registration, startup
  repair, mutation-guard coverage, per-entry tolerant `ProviderStatus`
  parse (downgrade safety).
- `src-tauri/src/integrations/deploy.rs` — reused as-is
  (configuration-only snapshot path); no staging of `~/.pi/agent/`.
- `src-tauri/pi-integration/` (new) — `quill.ts`, pi AGENTS section
  templates, extension test suite; hashed into the deployment stamp.
- `src-tauri/src/sessions.rs` — pi JSONL candidate collection, message
  extraction (v2/v3, tolerant of unknown entry types), provider-safe
  identity, reindex cleanup; fixed-arity root list generalized.
- `src-tauri/src/transcript_watcher.rs` — pi root registration + late-root
  recovery; `transcript_roots()` arity.
- `src-tauri/src/data_paths.rs` — pi session-root resolution from
  integration state (startup + on-mutation re-resolve), demo override env.
- `src-tauri/src/live_tracker.rs` — pi tail fold (tail-provable state
  only), `(mtime_ns, len)` fingerprint cold-restart guard, two-root sweep
  generalized, lineage-linked open-agent surfacing.
- `src-tauri/src/model_usage.rs` + `transcript_identity.rs` — pi adapter
  (`SourceRecordShape` variants, native chain identity), budgeted as its
  own task (~Codex-sized).
- `src-tauri/src/storage.rs` + `models.rs` — provider-IN lists gain `pi`;
  count payloads gain an additive `pi_count`; regression test that pi hook
  and usage rows surface.
- `src-tauri/src/server.rs` — `/api/v1/context/*` endpoints (new), hooks
  ingestion accepts provider `pi` with the existing event vocabulary.
- `src-tauri/src/context_store.rs` (new) — Rust operations on the shared
  context DB (index, fetch-and-index via `fetcher.rs`, bounded execute,
  search, get-source, stats, purge).
- `src-tauri/src/restart.rs`, `indicator.rs`, `memory_optimizer.rs`,
  `learning.rs`, `brevity.rs` (+ `manager.rs` brevity sync) — explicit pi
  exclusion arms so the new variant cannot change global-toggle behavior;
  excluded surfaces get explicit copy except Limits, which omits pi
  entirely per clarification.
- Frontend: `src/types.ts` provider union; `src/utils/providers.ts` label
  + `PROVIDER_ORDER` land WITH the enum sweep (the label helper falls
  through to "MiniMax" and the hue helper to `--provider-agent`, so pi
  must never ship mislabeled); `--provider-pi` CSS token in
  `src/styles/index.css` lands with the design task;
  `IntegrationsTab.tsx` pi card + rewritten consent copy naming the
  executable file and self-update; per-provider breakdown columns
  (`SkillBreakdown`/`HookBreakdown` render paths) gain the pi column;
  `LimitsSection.tsx` untouched.
- `src-tauri/tauri.conf.json` — `pi-integration/**/*` added to bundle
  resources; a packaged-build asset-resolution check guards it (the stamp
  hashes the runtime resource dir, so a miss fails only in packaged
  builds — the known AppImage failure class).
- Design: DESIGN.md provider color-code reconciled to `frontend.md`
  (Claude orange `#fb923c`, Codex blue `#60a5fa`), pi = dark green
  distinct from severity green; `.impeccable/design.json` synced.
- `lat.md/` — architecture, data-flow, infrastructure, features, tests
  sections for everything above (constitution 8).

## Data Model

No new main-analytics tables. `provider` TEXT columns accept `pi`;
`hook_invocations` rows carry provider `pi` with the existing event names.
Count IPC payloads (`models.rs:339-371`) gain additive `pi_count` fields
(no wire breakage). Saved `ProviderStatus` persistence switches to
per-entry parsing that skips unknown providers (a downgrade keeps
Claude/Codex enablement). New pi `integration-state.json` (versioned):
selected config/session dirs, recorded pi version, extension filename,
AGENTS state. New per-provider deployment stamp file, as today. The context
store keeps its existing schema; the Rust module and Python tools share it
under a documented writer contract (WAL, identical semantics per
operation). Usage-row dedupe key: session header id + entry id (uuidv7).
No `schema_version` bump: `pi_count` is computed at query time
(`SUM(CASE WHEN provider=…)`), no stored per-provider columns exist, and no
`CHECK (provider …)` constrains the TEXT columns — a task author should not
add a migration. Rollup coverage assertions keyed `(provider, source_key)`
are verified against a three-provider universe. Search indexing stores each
`message` entry exactly once by entry id (tree branches share ancestors by
reference, so no duplicated prefixes exist to inflate hit counts).

## API / Interface Changes

- New local HTTP endpoints `/api/v1/context/{index,fetch,execute,search,
  source,stats,purge}` — on a SEPARATE `127.0.0.1`-only listener (the main
  app binds `0.0.0.0` by design), shared-secret auth, bounded
  request/response sizes; the whole mount is setting-gated (off unless a
  consumer is enabled) and survives pi disable only while that setting is
  on — stated in the uninstall contract. `execute` additionally enforces
  working-dir scoping, output caps, and returns 403 while
  `context_preservation` is off. Explicit threat-model note ships with the
  endpoint.
- `/api/v1/context-savings/events` accepts provider `pi`; the extension
  posts routing-category telemetry there when `context_telemetry` is on,
  with routing token fields normalized to zero as today.
- `/api/v1/hooks/observed` accepts provider `pi`; event vocabulary
  unchanged (mapping: session_start→SessionStart, input→UserPromptSubmit,
  tool_call→PreToolUse, tool_result→PostToolUse, turn_end→Stop,
  session_shutdown→SessionEnd, before/after compact→Pre/PostCompact).
- Integration IPC commands operate on the widened enum; no signature
  changes.
- `quill.ts` extension: registers `quill_`-prefixed tools
  (`quill_search_history` + working-context set), observes lifecycle
  events when `activity_tracking` is on, applies the context-router
  policy on `tool_call` when `context_preservation` is on; every
  registration and handler individually try/caught; hard local-only URL
  guard; named `lib.cjs` timeout constant; no top-level side effects;
  self-no-op when Quill config is absent.
- Managed AGENTS block variant for pi naming the `quill_`-prefixed tools.
- No breaking changes anywhere; all additive.

## Testing Strategy

Automated tests below pin invariants at their owning layer and link
one-to-one with `lat.md` specs; authorization per constitution 7 is
requested at the analyze gate as part of plan approval.

- Parser unit tests: v3 and v2 fixtures (hookMessage-to-custom tolerance),
  v1 detected and surfaced unsupported, malformed lines skipped, header
  validation, unknown entry types (`label`, future) skipped, header `cwd`
  and `parentSession` extraction, `compaction` and `branch_summary` entry
  fixtures, ephemeral/no-file cases produce nothing.
- Live tracker: pi tail fold (liveness, last model, timestamp), deferred
  initial flush (file appears mid-conversation), `(mtime_ns, len)`
  discontinuity forces cold re-fold (equal-length rewrite fixture),
  quiescence to idle via existing `IDLE_AFTER`.
- Model-usage adapter: fixture transcripts where totals equal the sum of
  `AssistantMessage.usage`; branch fixture proving spend counts all
  branches; dedupe on re-scan.
- Storage regression: a pi hook row and a pi usage row surface in the
  provider breakdowns (guards the IN-list sweep).
- Provider status: per-entry parse skips unknown providers, preserves
  known ones.
- Deploy: pi install/uninstall round-trip leaves user bytes identical,
  crash-recovery restores, orphan Quill-marked file swept, other user
  extensions untouched (the never-stage-the-directory invariant).
- Context HTTP API: auth required, listener refuses non-loopback peers,
  size bounds, execute scoping/caps, execute returns 403 while
  `context_preservation` is off, setting-gated mount off by default;
  semantic parity for three named ops (`search`, `source`, `stats`) with
  exact-match on returned refs against the Python tools on one shared
  store.
- Extension (Node suite beside `quill.ts`, like
  `context-router.test.cjs`): handler exceptions never escape, local-only
  guard rejects remote URLs, router policy block/allow cases reusing the
  `context-router.test.cjs` case set (including taint round-trip and the
  ready-to-paste URL rewrite), every block carries a reason string,
  feature-flag-off means zero routing behavior, Quill-down returns a
  typed concise tool error, a timing assertion bounds handler synchronous
  work.
- Verification: repair detects stale stamp, wrong extension hash, missing
  AGENTS block, unexpected Quill-marked files; packaged-build
  asset-resolution check for `pi-integration/**` bundle resources.
- Lineage fixture: a parent with two live children carrying
  `parentSession` plus one unlinked sibling — asserts two linked rows,
  one independent row, and no invented subagent count.
- Demo mode: `QUILL_DEMO_MODE=1` with the pi demo override unset yields
  the empty placeholder root, never the persisted integration-state dir;
  detection stays read-only.
- Consent copy: the pi enable dialog string contains the extension
  filename, its path, and the self-update sentence.
- Performance evidence (constitution 10): one measured run of the
  extension on a real pi session; the watcher fold demonstrated on a
  synthetic large branched session (≥100 MB) staying within the bounded
  tail budget.
- E2E numeric criteria: synthetic pi corpus, enable, searchable within
  15 seconds of transcript write; usage totals equal transcript sums.

### Item 11a measured evidence

Pi 0.84.1 loaded bundled `quill.ts` with isolated config and session roots, called `quill_context_stats` once, persisted the session, and completed in 1016.4 ms end to end. The Node timing case kept synchronous handler work below 10 ms.

## Risks

- **TypeBox runtime resolution is closed for pi 0.84.1.** The extension uses
  a plain JSON Schema object, which passed the real loader and needs no
  runtime import. Re-run `scripts/spike_pi_register_tool.mjs` when raising
  the minimum pi version to catch extension-loader or validator drift.
- **Context HTTP API scope balloon**: the Python surface is ~1,900 lines
  with SSRF guards and FTS; if Rust parity for a niche operation balloons
  mid-build, file follow-up beads per clarification 2C rather than
  blocking the run; `search`/`source`/`stats`/`index` land first,
  `execute`/`fetch` carry the threat-model work.
- **Two writers, one context DB**: mitigated by WAL, a documented
  operation contract, and a parity test; drift between Rust and Python
  semantics is the failure mode to watch.
- **Pi pre-1.0 API churn** (accepted, 6A): min-version pin at detection,
  verify-on-repair, fail-silent design; extension breakage is a verify
  failure, never a crash of the user's agent.
- **Tree-fold scope creep**: v1 is tail-only by decision; any tree walk
  requires a written memory/CPU budget first.
- **Enum sweep breadth** (~170 sites): compiler-forced, but review each
  arm's semantics — the brevity/memory-optimizer/restart exclusions are
  behavior decisions, not just match arms.
- **Dark green vs. severity green**: the design task must pick a pi hue
  measurably distinct from the reserved severity green in both themes.

## Sequencing

Ordered work items; each becomes a bead, order expressed as dependencies.
Every item syncs its own `lat.md` sections as part of its acceptance
(constitution 8) — the closeout item only runs the final `lat check`.

1. **Provider plumbing foundation** — `Pi` enum variant, `as_str`/parse,
   per-entry status parsing (downgrade safety), storage IN-list sweep
   (all THREE lists: `storage.rs:12788`, `12878`, `12951`) + `pi_count`,
   the non-compiler-forced array literals as explicit decisions
   (`rule_watcher.rs:31`, `cpa/aggregate.rs:29`, `memory_optimizer.rs:118`,
   `learning.rs:61`, `restart.rs:1660`, `manager.rs:408`,
   `storage.rs:1119`, `storage.rs:15074`), rollup-coverage three-provider
   check, frontend type union + `providers.ts` label + `PROVIDER_ORDER`
   (pi must never render mislabeled), fixed-arity generalizations,
   exclusion arms with copy, hooks-observed endpoint provider widening,
   pi integration-state schema + session-dir resolver (so downstream
   items read a defined shape). Blocks 2, 7.
2. **Pi transcript parser** — session-manager format module (header,
   v2/v3 entries, tolerance rules, ephemeral no-op). Depends on 1;
   blocks 3.
3. **Watcher + indexer integration** — root resolution from integration
   state + demo override precedence, late-root recovery, candidate
   collection, extraction arm, identity, facets, reindex cleanup,
   backfill on enable. Depends on 2, 7. Blocks 4, 5, 12.
4. **Live tracker pi fold** — tail-provable state (liveness, header cwd,
   last model, timestamp), fingerprint guard, sweep generalization,
   explicit token gap. Depends on 3.
5. **Model-usage adapter** — pi `SourceRecordShape` + identity resolver +
   normalization + diagnostics + active-branch labeling for per-session
   totals. Depends on 3. Blocks 13.
6. **Design reconciliation** — DESIGN.md and frontend.md provider hues
   reconciled (frontend.md correct), pi dark green with ΔE00 ≥ 20 against
   `--severity-good` in both themes, `--provider-pi` CSS token,
   `.impeccable/design.json`. Depends on nothing; blocks 8.
7. **Provider lifecycle module** — `pi.rs` detection state table with
   macOS launcher/symlink PATH augmentation reuse, minimum pi version
   gate (pin: pi ≥ 0.84.0; unknown/unparseable = not installable, typed
   status), typed failure surfacing (`last_error`, non-writable
   extensions dir), config-only install/uninstall transaction, AGENTS
   block, stamp + Codex-rigor verification, orphan sweep,
   disable-retains-data, `tauri.conf.json` bundle resources +
   packaged-build asset check, manager/repair/mutation-guard wiring.
   Depends on 1, 9 (spike outcome fixes the deployed file set the owned
   manifest must name). Blocks 3, 8, 11a.
8. **Integrations UI** — pi card, consent copy naming the executable
   file/path/self-update (string-asserted), settings copy for excluded
   surfaces, per-provider breakdown pi columns. Depends on 6, 7.
9. **TypeBox/registerTool spike** — deliverable: a decision record naming
   the chosen parameter shape plus one passing `registerTool` call
   against pi ≥ the pinned version. Completed against pi 0.84.1: use a
   plain JSON Schema object and deploy no extra files. Depends on nothing
   (do first).
   Blocks 7, 11a.
10. **Context HTTP API** — `context_store.rs` + separate loopback-only
    listener + `/api/v1/context/*` with threat model, setting-gated
    mount, context-savings provider widening. Provider-agnostic — starts
    immediately, no dependencies. Blocks 11a.
11a. **Quill pi extension: tools + telemetry** — `quill.ts` with
    `quill_`-prefixed tools, lifecycle event telemetry, hardening
    invariants, Node test suite, timing assertion, deployed via item 7's
    transaction. Depends on 7, 9, 10.
11b. **Router policy port** — the `context-router.cjs` policy (deny +
    actionable URL rewrite, taint tracking and read-gating) ported into
    the extension with a policy-parity test reusing the
    `context-router.test.cjs` case set; routing telemetry to
    context-savings under `context_telemetry`. Depends on 11a.
12. **Session lineage + open agents** — `parentSession` links in session
    views, lineage-proven live child sessions as open-agent activity,
    frontend label ("live linked sessions", never a subagent count).
    Depends on 3, 4.
13. **Numeric E2E validation** — success-criteria run (searchable ≤ 15 s,
    totals equal sums, fold budget on the large branched fixture).
    Depends on 3, 4, 5.
14. **Docs closeout** — release notes (deployed-file disclosure and the
    one-way downgrade note: older builds drop saved enablement on seeing
    `pi`), PRODUCT.md/README/marketing provider enumerations, final
    `lat check`. Depends on all above.

## Backlog Refinement

None — the spec has no backlog inputs (fresh epic; no open P4 sources in
closure).

## Alignment fixes applied

- (A, must) Goal 3 and story 3 AC contradicted the tail-only live-fold
  decision — both now state liveness/cwd/last-model/timestamp with live
  tokens as an explicit gap; quiescence-only session end; ephemeral no-op.
- (A, must) Taint tracking + ready-to-paste URL rewrite were unscoped —
  router policy port split out as item 11b with a policy-parity test
  reusing the `context-router.test.cjs` cases.
- (A, must) Routing telemetry had no plan element — context-savings
  endpoint gains provider `pi` (item 10) and the extension posts
  routing-category events under `context_telemetry` (item 11b).
- (B, must) "Loopback-only" was false — the main axum app binds `0.0.0.0`
  by design; context API moved to a separate `127.0.0.1` listener with a
  setting-gated mount (off unless a consumer is enabled) and a threat
  model starting from the real bind posture.
- (B, must) `tauri.conf.json` bundle-resource edit and packaged-build
  asset check added (Affected Components + item 7) — the known AppImage
  failure class.
- (B, must) `pi_count` rendering owner added (breakdown columns → item 8).
- (B, must) Minimum pi version pinned (≥ 0.84.0) in item 7.
- (B, must) Item 3's hidden dependency on item 7 made explicit; state
  schema + dir resolver hoisted into item 1.
- (B, must) Non-compiler-forced provider array literals and the third
  IN-list (`storage.rs:12951`) enumerated in item 1.
- (B/A, must) Item 12 lineage got a concrete test fixture; story 10's UI
  label got an owner (item 12 frontend arm).
- (should) Ephemeral-skip, disable-retains-data, compaction/
  branch_summary fixtures, facets, index-once restated in Data Model,
  demo-precedence test, consent-copy string assertion, typed Quill-down
  tool error, reason-string + flag-off routing tests, measured-run and
  large-fixture fold evidence, parity ops named (search/source/stats,
  exact refs), execute-403 test, spike exit artifact defined, E2E bound
  quoted (15 s), `lib.cjs` timeout duplicated with value-equality test,
  per-item lat.md sync (closeout runs `lat check` only), docs sweep and
  downgrade disclosure folded into item 14, edges corrected
  (10 starts immediately; 9 → 7; 3 → 4, 5; 13/14 split), no
  `schema_version` bump stated, Spec Review fork-display line reconciled
  with story 10.
