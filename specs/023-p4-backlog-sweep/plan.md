# Plan: p4-backlog-sweep

Implementation plan for the three dispositioned P4 findings: two live
observed-subagent registry fixes in `src-tauri/src/server.rs` and Codex
spawn-metadata identity ingestion in `src-tauri/src/transcript_identity.rs`
with a backfill migration. Design decisions are fixed per the spec's
Clarifications; this plan sequences them, anchors them to files, and checks
them against the constitution.

## Architecture Approach

Three independent work streams, each small and local to existing boundaries
(Constitution 2 — no new crates, no cross-layer restructuring).

**Stream A — fuse merge + enrichment (quill-qqt).** `merge()`
(server.rs:524) absorbs `enrich_model_groups()` (server.rs:622) into a
single lock acquisition. The two methods have exactly one production
caller, back-to-back in the same closure (lib.rs:3509-3518); the two-call
split is the defect. Under one guard: existing merge logic runs, then the
model-group snapshot is taken over the final truncated rows; the guard
drops; the `resolve` DB callback runs after unlock against that snapshot
— exactly how enrichment already applies evidence to its lock-time
snapshot without re-locking (server.rs:679-700), so Constitution 3 (no
registry mutex across DB work) is preserved. The `agents.len() ==
expected` guard (server.rs:657) and `enrich_model_groups` as a separate
public method are deleted: count/chips mismatch becomes impossible by
construction, compile-enforced.
*Rejected:* membership snapshot and enrich-time recompute — both keep the
two-generation architecture; recompute additionally mixes generations
across rows and can rewrite an observed-only row's count against the
null/zero "reserve no element" semantics.

**Stream B — backdated-Stop root invalidation (quill-hzx).** One guard at
the top of the `ObservedCoverage::Active` arm in `observe_agent`
(server.rs:229-250), before the epoch check: a Stop for a known,
currently-open agent whose `at` is earlier than that agent's recorded
Start invalidates the root (the registry's documented null idiom) instead
of silently returning `false` at server.rs:266. Placing it before the
epoch check covers both drop sites because a retained open agent always
has `current.at > epoch` (server.rs:167). Includes a `log::debug!` line
for diagnosability. The epoch guard at server.rs:236 stays unchanged for
unknown agents.
*Rejected:* unconditional close (a stale cross-life Stop from a
SendMessage-restarted agent id would close a running agent — confident
undercount); bounded negative delta (no principled constant; both
false-accept and false-miss survive); sequence numbers (cross-component
wire-contract change to solve what invalidation already covers).

**Stream C — Codex spawn-metadata identity (quill-kdx).**
`codex_metadata` (transcript_identity.rs:253-267) additionally reads the
nested `source.subagent.thread_spawn` object — the only representation
present in both schema eras — plus top-level `thread_source` and
`agent_nickname`. `resolve_codex_native_identity`
(transcript_identity.rs:271-329) populates `agent_id ==
chain_id == source_session_id` for spawn-marked rollouts (replacing the
hardcoded `agent_id: None` at :326), with `parent_chain_id` precedence:
top-level `parent_thread_id` → nested `thread_spawn.parent_thread_id` →
`forked_from_id`. `agent_nickname` rides as a separate display label,
never identity. Restatement tolerance extends first-child-wins to spawn
metadata; conflicting restated spawn identity →
`IdentityError::ConflictingNativeIdentity` (Constitution 5, typed
failures). Existing data is backfilled via migration 39 + reingest flag
(the established migrations 20/21/26/27 pattern), because forward-only
concretely fails: `session_events`' identity index coalesces NULL
agent_id (storage.rs:6689-6690), so re-extracted files would insert
duplicates beside old NULL rows, and model_usage identity comparison
including `agent_id` (model_usage.rs:874, :1188) would churn
`ObservationIdentityMismatch` as files grow.
*Rejected:* nickname-as-identity (255 distinct nicknames across 4,430
subagent rollouts — would merge unrelated threads); evaluation-only bead
(1,424 subagent rollouts index today as parentless non-sidechain roots —
a live correctness defect, decided during clarification); forward-only
ingestion without backfill (duplicate rows, mismatch churn, permanent
mis-rooting).

## Affected Components

- `src-tauri/src/server.rs` — `ObservedSubagentState::merge`
  (:524-:620) gains the resolve closure and absorbs the model-group
  snapshot; `enrich_model_groups` (:622-:702) deleted;
  `observe_agent` (:221-:287) gains the backdated-Stop invalidation
  guard in the `Active` arm.
- `src-tauri/src/lib.rs` — `get_session_breakdown` (:3495-:3521), the
  sole production caller, collapses the two calls into one
  `merge(..., resolve)` call.
- `src-tauri/src/transcript_identity.rs` — `CodexMetadata` +
  `codex_metadata` (:253-:267) read spawn object / `thread_source` /
  `agent_nickname`; `resolve_codex_native_identity` (:271-:329)
  populates `agent_id` and the nickname label; `NativeChainIdentity`
  (:204-:212) gains fields.
- `src-tauri/src/sessions.rs` — reingest-flag handling in the sweep
  (:1447-:1493) gains a `codex_agent_identity_reingest_pending` check,
  same shape as the migration-20/26/27 handlers; Codex extraction at
  :3770-:3792 picks up the populated identity automatically.
- `src-tauri/src/storage.rs` — migration 39
  (`MAX_SUPPORTED_SCHEMA_VERSION` 38 → 39, :103): codex-scoped deletes,
  additive nickname column, reingest flag (pattern at :6306-:6381,
  :6668-:6710).
- `src-tauri/src/model_usage.rs` — native-source identity comparisons
  (:874, :1188) now see non-None Codex `agent_id` from the shared
  resolver (:1400); nickname threaded through source metadata but kept
  out of the equality checks.
- `src-tauri/src/transcript_analytics.rs` — identity comparison (:683)
  and resolver call (:1322) unchanged in shape; Codex sidechains now
  satisfy the existing sidechain ⇒ `agent_id == chain_id` invariant
  Claude already enforces (:1265-:1273).
- `src-tauri/src/retention_engine.rs` — no code change planned;
  verification only: pre-migration `retention_daily_aggregates` rows
  (agent_id='') are preserved and the watermark (drain_target,
  :1415-:1460) blocks reinsertion of re-extracted pre-watermark detail.
- `lat.md/frontend.md` — `Observed Subagent Counts` (:318) updated for
  the fused snapshot semantics.
- `lat.md/live-subagent-count-tests.md` — new specs for the fused-merge
  and backdated-Stop tests (authorization-gated).
- `lat.md/backend.md` — `Codex Identity Restatement And Cycles` (:1371)
  updated for spawn-metadata ingestion, the agent_id invariant, and the
  backfill migration.

## Data Model

No structural schema migration for identity columns — `agent_id` already
exists on every affected table (session_events, tool_actions,
response_times, model_observation_sources, retention_daily_aggregates).

**Migration 39** (data migration; next number verified —
`MAX_SUPPORTED_SCHEMA_VERSION` is 38 at storage.rs:103; spec's "migration
39" is correct). Single transaction, following the migration 20/26
pattern (storage.rs:6309-6381, :6668-6710):

- DELETE codex-scoped re-derivable rows: `session_events`,
  `tool_actions`, `response_times`, `skill_usages`,
  `model_observation_sources` + model observation rollups
  (`model_usage_observations`, `model_usage_hourly` codex scope),
  `transcript_analytics_sources` — all `WHERE provider = 'codex'`.
- Do NOT delete `hook_invocations` (live-only, non-derivable) and NOT
  `retention_daily_aggregates` — pre-migration rows (agent_id='') are
  the sole surviving record of already-pruned detail; per-agent split
  for pruned days is forward-only by necessity.
- Set `codex_agent_identity_reingest_pending` in `settings`; the next
  sweep (sessions.rs:1447-1493) clears `file_mtimes` and reprocesses
  all ~5,435 Codex files. Flag cleared only after every file succeeds
  and the writer commits (existing pattern), so an interrupted sweep
  retries next boot (Constitution 4).

**Nickname label home:** `agent_nickname: Option<String>` is added to
`NativeChainIdentity` (transcript_identity.rs:204-212) and threaded into
the model_usage native-source metadata; persisted as one additive
nullable `agent_nickname TEXT` column on `model_observation_sources`
(storage.rs:6839 — the per-source native identity record all per-agent
analytics join through), added in migration 39 via `ALTER TABLE`. It is
deliberately EXCLUDED from every identity-equality comparison
(model_usage.rs:874, :1188; transcript_analytics.rs:683) and from all
row keys — it is display metadata, merged first-non-null like `cwd`
(model_usage.rs:1204-1206). No per-event copy on `session_events`; one
source-level home suffices.

## API / Interface Changes

- `ObservedSubagentState::merge` signature changes: gains
  `resolve: impl FnOnce(&[ObservedAgentModelKey]) -> Result<HashMap<ObservedAgentModelKey, String>, String>`
  and returns `Result<Vec<SessionBreakdown>, String>` instead of
  `Vec<SessionBreakdown>`.
- `enrich_model_groups` deleted as a public method.
- `get_session_breakdown` (lib.rs:3509-3518) passes the storage
  evidence closure directly into `merge`.
- `NativeChainIdentity` gains `agent_nickname: Option<String>`; Codex
  resolution populates `agent_id` for spawn-marked rollouts
  (`agent_id == chain_id`), keeps `agent_id: None` for
  `thread_source: "user"` / no-spawn rollouts.
- IPC / frontend contract unchanged: `SessionBreakdown` serialization is
  byte-identical (same fields, same null semantics); no frontend changes
  beyond what already renders `agent_id`-derived data. Live hook path
  (server.rs:483-506) unchanged.
- Test churn: every existing test constructing the `merge` +
  `enrich_model_groups` pair must move to the fused signature; tests
  asserting `agent_id == None` for Codex identities must be updated to
  the new expectations. Struct-literal construction sites of
  `NativeChainIdentity` in tests gain the new field.

## Testing Strategy

Test additions are authorization-gated (Constitution 7; the live-count
suite per lat.md/live-subagent-count-tests.md). User authorization is
pending confirmation at the analyze gate. Planned tests, listed for that
confirmation:

- **Fused merge consistency** (Story 1 AC 1-2): a Stop+Start pair
  injected around a single `merge` call yields either the old or the new
  consistent count/chips pair, never a mix; membership-swap-at-constant-
  size (the old guard's blind spot) cannot produce a mislabeled chip.
- **Deadlock safety** (Story 1 AC 3): the `resolve` closure runs with
  the registry mutex released — a resolve that re-enters the registry
  (calls `snapshot`/`observe`) completes without deadlock.
- **Four hzx ordering unit tests** (Story 2 AC): (1) backdated Stop for
  a known open agent invalidates the root (count → None) and logs;
  (2) equal-timestamp Stop tie-break at server.rs:254 still closes;
  (3) pre-epoch Stop for an unknown agent is still silently dropped
  (epoch guard :236 unchanged); (4) a stale re-delivered Start cannot
  reopen a closed agent.
- **Codex fixture tests** (Story 3 AC 1-3): legacy-era fixture
  (nickname + object `source`, no top-level fields) and modern-era
  fixture (duplicated top-level) both resolve
  `agent_id == chain_id == source_session_id`, `is_sidechain == true`,
  parent precedence honored; `thread_source: "user"` and no-spawn
  fixtures keep `agent_id == None`; nickname lands in the label field
  only.
- **Restatement conflict tests**: restated spawn metadata that agrees is
  tolerated (first-child-wins); conflicting restated spawn identity
  returns `ConflictingNativeIdentity`.
- **Backfill idempotency / duplicate-row checks** (Story 3 AC 4): after
  migration 39 + one sweep, no Codex sidechain `session_events` row has
  NULL `agent_id`; re-running the sweep inserts no duplicates (unique
  index storage.rs:6689); `retention_daily_aggregates` agent_id='' rows
  survive.

Gates regardless of test authorization (Constitution 6): `cargo fmt
--check`, `cargo clippy` zero warnings, `cargo test` (existing suite)
green, plus `lat check` (Constitution 8).

## Risks

- **Deadlock if `resolve` re-enters the registry** — mitigated by guard
  scoping: the mutex guard is dropped before `resolve` runs (snapshot-
  then-resolve); the deadlock-safety test pins it.
- **Merge test-call churn** — the signature change touches every
  test-side caller of `merge`/`enrich_model_groups`; mechanical but
  broad. Contained to server.rs tests + lib.rs.
- **Live Codex hook `agent_id` equality with the thread id is inferred,
  not confirmed** — the census showed zero `id == parent_thread_id`
  collisions so the hook-side `agent_id != session_id` filter
  (server.rs:484-488) holds, but verify one captured live
  `SubagentStart` payload during implementation before relying on
  hook/ingestion identity agreement.
- **Boot-time cost of the full reingest sweep** — ~5,435 Codex files,
  same profile as prior reingest migrations (20/21/26/27). Constitution
  10: no new performance budget is proposed since the mechanism and
  corpus match prior measured migrations; log sweep duration once and
  compare against the migration-26 boot as the reproducible check.
- **Retention watermark double-count** — deleted codex detail for days
  already rolled into `retention_daily_aggregates` must not be
  re-inserted by the sweep and later re-aggregated. The watermark
  ordering in `drain_target` (retention_engine.rs:1415-1424) is designed
  to block pre-watermark reinsertion; verify this holds for the
  reingest path explicitly during implementation.

## Sequencing

Ordered work items; blocking edges explicit by name. The two server.rs
fixes are independent of each other and of the Codex stream.

1. **Fuse merge and enrichment in the observed registry** — no
   dependencies. Includes the lib.rs caller change, test churn, and the
   lat.md/frontend.md + live-subagent-count-tests.md updates for this
   behavior.
2. **Backdated-Stop root invalidation guard** — no dependencies.
   Includes its four ordering tests (if authorized) and the
   live-subagent-count-tests.md spec updates.
3. **Codex spawn-metadata resolver ingestion** — no dependencies.
   transcript_identity.rs changes, nickname field, restatement
   extension, fixture + conflict tests (if authorized), and the
   lat.md/backend.md Codex Identity Restatement And Cycles update.
4. **Backfill migration 39 and reingest sweep wiring** — blocked by
   *Codex spawn-metadata resolver ingestion* (deleting rows before the
   resolver produces the new identity would rebuild stale data; the
   cache-invalidation migration must ship in the same release as the
   resolver change).
5. **Codex analytics verification** — blocked by *Backfill migration 39
   and reingest sweep wiring*. Post-sweep checks: no NULL-agent_id
   Codex sidechain event rows, no duplicates, retention aggregate
   preservation, watermark reinsertion guard, model_usage restatement
   stability, one captured live hook payload cross-checked.
6. **Quality gates and lat check** — blocked by *Fuse merge and
   enrichment in the observed registry*, *Backdated-Stop root
   invalidation guard*, and *Codex analytics verification*. fmt /
   clippy / test zero-warning pass and `lat check` across all lat.md
   edits.

## Backlog Refinement

- **quill-qqt** → refine-in-place, target **P3**. Work item: the
  merge/enrich fuse (*Fuse merge and enrichment in the observed
  registry*). Acceptance criteria:
  1. Count and model groups for every row derive from one lock-time
     snapshot inside a single `merge` acquisition; a Stop+Start pair
     landing during the request yields the old or new consistent pair,
     never a mix.
  2. The `agents.len() == expected` guard (server.rs:657) and
     `enrich_model_groups` as a separate public method are deleted;
     the mismatch is impossible by construction.
  3. `merge` gains the `resolve` closure parameter and returns
     `Result`; the sole caller (lib.rs:3509-3518) is updated.
  4. The `resolve` DB evidence callback runs strictly after the
     registry guard drops, applied to the lock-time snapshot.
  5. `SessionBreakdown` serialization is byte-identical; observed-only
     row semantics (null/zero reserve no element) unchanged.
- **quill-hzx** → refine-in-place, target **P3**. Work item: the
  invalidation guard (*Backdated-Stop root invalidation guard*).
  Acceptance criteria:
  1. A Stop for a known, currently-open agent with timestamp earlier
     than its recorded Start invalidates the root — count reads None,
     never a silently-retained overcount.
  2. The guard sits at the top of the `Active` arm before the epoch
     check, covering both drop sites (retained open agents always have
     `current.at > epoch`, server.rs:167).
  3. A `log::debug!` line records the invalidation cause for
     clock-skew diagnosability.
  4. The epoch guard at server.rs:236 is unchanged: pre-epoch Stops
     for unknown agents stay dropped.
  5. Stale re-delivered Starts still cannot reopen a closed agent
     (tie-break at server.rs:254 preserved); recovery is the existing
     path — next SessionStart re-establishes coverage.
- **quill-kdx** → split-and-supersede, target **P2**. Replacement work
  items: *Codex spawn-metadata resolver ingestion*, *Backfill migration
  39 and reingest sweep wiring*, *Codex analytics verification*.
  Acceptance criteria (Clarifications Q3/Q4, Story 3):
  1. Every rollout with spawn metadata (nested
     `source.subagent.thread_spawn` or top-level
     `thread_source: "subagent"`) resolves
     `agent_id == chain_id == source_session_id`,
     `is_sidechain == true`, parent precedence top-level
     `parent_thread_id` → nested `thread_spawn.parent_thread_id` →
     `forked_from_id` — including the 1,281 legacy-era rollouts.
  2. `thread_source: "user"` / no-spawn rollouts keep
     `agent_id == None` exactly as today.
  3. `agent_nickname` is ingested as a display label only (new
     `model_observation_sources.agent_nickname` column), excluded from
     identity equality and row keys.
  4. Conflicting restated spawn identity returns
     `ConflictingNativeIdentity`; agreeing restatements tolerated
     first-child-wins.
  5. Migration 39 deletes only re-derivable codex-scoped rows (not
     `hook_invocations`, not `retention_daily_aggregates`), sets the
     reingest flag, and after one full sweep no Codex sidechain event
     row has NULL `agent_id` and no duplicate rows exist.
  6. Pre-migration `retention_daily_aggregates` rows (agent_id='')
     are preserved; per-agent split for pruned days is forward-only.

## Target Epic

New epic to be created: **"live-count edge cases + codex agent
identity"**. No existing epic fits — all three source beads are
unparented P4s with no discovered-from provenance. The two refined beads
(quill-qqt, quill-hzx) and the three quill-kdx replacement tasks are
parented under it; quill-kdx itself is closed as superseded with a
pointer to its children. Tracked in Beads per Constitution 12; commits
and sync only with explicit authority.

---

**Constitution check.** 1 (local truth): both registry fixes prefer
null/invalidation over a wrong number; ingestion fills identity only
from source evidence. 2 (boundaries): all changes inside existing
server.rs / transcript_identity.rs / storage.rs layers. 3 (responsive
execution): honored by snapshot-then-resolve — no registry mutex held
across the DB callback; migration runs in the existing storage init
path off the UI thread. 4 (recoverable mutation): migration 39 is one
transaction; the reingest flag clears only after full success. 5 (typed
failures): identity conflicts stay `IdentityError` variants. 6:
fmt/clippy/test zero-warning gates in the final work item. 7: all test
additions listed above await explicit authorization at the analyze
gate. 8: three lat.md sections enumerated, `lat check` gated. 9: no UI
change — not applicable. 10: boot-time reingest sweep cost flagged; no
new budget since it matches the measured prior-migration profile, but
sweep duration will be logged and compared (tension noted — if the
migration-26 comparison is unavailable, a one-off measurement is
warranted before release). 11: no external transmission. 12: Beads
epic/task mapping above; no commit/push without authority.

## Alignment fixes applied

Quick pass (single self-check covering spec↔plan alignment and plan
quality). Every spec goal, non-goal, user story acceptance criterion,
backlog input, clarification answer, and non-blocking observation traces
to a plan section; all three backlog sources have complete dispositions
at P2/P3 with verifiable acceptance criteria; sequencing edges are
explicit with no circular or false dependencies; no Non-Goal scope creep
found. Nothing needed fixing — no must-fix or should-fix edits applied.
