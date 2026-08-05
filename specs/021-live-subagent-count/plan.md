# Plan: live-subagent-count

## Architecture Approach

Maintain a bounded, process-local lifecycle fold shared by Quill's HTTP hook
server and Tauri commands. Existing provider hook scripts POST lifecycle
evidence to `/api/v1/hooks/observed`; the endpoint validates and folds accepted
events synchronously, then preserves the existing SQLite `hook_invocations`
write as audit history. `get_session_breakdown` keeps its database rollup, then
overlays an `observed_subagent_count` snapshot by exact provider, hostname, and
root session identity before returning IPC rows.

The state model is deliberately ephemeral. Quill restart, disabled tracking,
unsupported or malformed coverage, and bounded-state saturation return `null`.
No positive count is reconstructed from SQLite. Normal start/stop delivery is
exact; undetectable missed or blocked stops remain the documented best-effort
boundary until parent end or Quill restart.

Provider lifecycle payloads expose root session plus agent identity but not
immediate ancestry. Count every root-linked subagent and infer no direct-child
relationship. Current official Codex and Claude Code hook schemas, verified on
2026-08-04, both define `startup`, `resume`, `clear`, and `compact` as the full
`SessionStart.source` set; `startup`, `resume`, and `clear` reset coverage,
while `compact` preserves it. Unsupported sources establish no coverage.
Evidence: [Codex hooks](https://developers.openai.com/codex/config-advanced#hooks)
and [Claude Code hooks](https://code.claude.com/docs/en/hooks#sessionstart).

Alternatives rejected:

- SQLite-derived current state: creates retention, indexing, restart-staleness,
  and 300 ms query risks for data that is intentionally process-local.
- Verified process/heartbeat reconciliation: exceeds the approved hook-observed
  contract and cross-platform scope.
- Direct-child-only counting: neither provider supplies immediate-parent agent
  identity.
- TTL expiry: converts legitimate long-running agents into invented zero/null
  on an arbitrary clock.
- New table, polling loop, transport, dependency, component, badge, or status
  taxonomy: existing layers already cover the required path.

Constitution alignment:

- Principle 1: positive values come only from local observed evidence; gaps are
  nullable and explicit.
- Principles 2 and 3: Rust/Tauri owns state and bounded work; React only renders
  the typed result.
- Principle 4: managed hook updates remain additive, transactional, and
  last-known-good preserving.
- Principle 5: malformed or unsupported lifecycle evidence has typed,
  root-local invalidation and contextual diagnostics.
- Principles 6, 7, 8, 9, and 10: authorized owning-layer tests, full gates,
  lat.md synchronization, neutral accessible UI, and reproducible latency/query
  measurements are completion requirements.
- Principles 11 and 12: no new transmission exists; Beads and delivery authority
  remain explicit.

## Affected Components

### Runtime state and backend contract

- `src-tauri/src/server.rs`
  - Add `ObservedSubagentState`, root/agent keys, bounded lifecycle fold, snapshot
    lookup, activity enable/disable clearing, and table-driven unit tests.
  - Share one `Arc` through `ServerState` and Tauri managed state.
  - Generalize the hook endpoint from Codex-only naming, validate hostname and
    source, fold lifecycle evidence, and emit immediate invalidation.
- `src-tauri/src/lib.rs`
  - Construct/manage the shared state, pass it to `start_server`, clear it after
    successful activity/provider disable, and overlay snapshots onto Sessions
    rows after blocking storage work completes.
- `src-tauri/src/models.rs`
  - Replace historical `has_subagents`/`subagent_count` with required serialized
    `observed_subagent_count: Option<u32>`.
  - Rename the wire payload provider-neutrally and add optional hostname/source
    fields so older observers remain audit-only compatible.
- `src-tauri/src/storage.rs`
  - Remove historical Sessions agent enrichment and map database rows with a
    null observed count before the in-memory overlay.
  - Keep historical analytics storage intact.
  - Preserve distinct same-millisecond audit rows by using agent ID as agent
    hook chain identity.
  - Update rollup, serialization, audit-identity, and query tests.

### Claude Code observation

- `src-tauri/claude-integration/scripts/observe.cjs`
  - Extend the existing observer with a pure lifecycle payload builder carrying
    provider, configured/fallback hostname, root session, event, source, agent,
    cwd, and producer timestamp.
- `src-tauri/claude-integration/scripts/hook-runtime.test.cjs`
  - Test lifecycle payloads without network or provider processes.
- `src-tauri/src/claude_setup.rs`
  - Register `SessionStart`, `SubagentStart`, `SubagentStop`, and `SessionEnd`
    only under activity tracking while preserving foreign hooks and rollback.
  - Record the official source allowlist evidence and activity-off omission.

### Codex observation

- `src-tauri/codex-integration/scripts/hook-observe.cjs`
  - Export/test a pure payload builder and add hostname plus SessionStart source
    without changing the existing one-shot best-effort transport.
- `src-tauri/codex-integration/scripts/hook-observe.test.cjs`
  - Cover provider/session fallback, hostname, source, agent, and timestamp.
- `src-tauri/src/integrations/codex.rs`
  - Preserve existing lifecycle registrations and activity gate; update managed
    script verification for the enriched payload contract.

### Sessions IPC and frontend

- `src/types.ts`
  - Replace optional historical fields with required
    `observed_subagent_count: number | null`.
- `src/hooks/useBreakdownData.ts`
  - Let `hooks-observed-updated` invalidate Sessions as well as Hooks, reusing
    the existing five-second coalescing cache and visibility refresh.
- `src/components/widget/views/UsageView.tsx`
  - Map positive counts to exact `+N` metadata independently of `row.live`;
    omit zero/null; use provider+hostname+session React identity; render meta
    between project name and provider chip with one accessible label.
- `src/utils/format.ts` and the existing Node-to-TypeScript test pattern
  - Add the smallest pure formatter for null/zero omission, exact positive text,
    and singular/plural accessible text.
- `src/styles/index.css`
  - Reuse neutral `.wg-row-meta`/`.wg-num`; add only non-shrinking metadata if
    isolated 320 px evidence requires it.
- `src/mocks/ipcFixtures.ts`
  - Replace historical fixtures with null, zero, singular, and plural observed
    states.
- `src/components/RetentionBanner.tsx` and
  `src/windows/SessionsWindowView.tsx`
  - Remove the dead Sessions mixed-retention surface while preserving the
    remaining Session Search retention behavior.

### Documentation and verification

- `lat.md/backend.md`, `lat.md/data-flow.md`, `lat.md/features.md`, and
  `lat.md/frontend.md`
  - Replace historical count claims with process-local observed lifecycle,
    audit-only persistence, nullable IPC, invalidation, and positive-only UI.
- `lat.md/live-subagent-count-tests.md`
  - Add one concise, one-to-one referenced spec leaf for each authorized owning
    test group.
- `specs/021-live-subagent-count/verification.md`
  - Record targeted/full gates, isolated 320/451 px evidence, event-to-row
    latency, frozen-corpus query measurement, and rollback result.
- Stale historical comments in `scripts/populate_dummy_data.py` and related
  fixtures/types are removed without changing retained analytics storage.

## Data Model

No database schema or migration is added.

Wire payload additions:

- `provider`: `claude | codex`
- `hostname`: configured host identity first, then the same short-host
  normalization already used by token reporting; raw FQDN fallback is forbidden
- `session_id`: provider root session ID
- `hook_event`: lifecycle event name
- `source`: optional SessionStart source
- `agent_id`: required for subagent lifecycle events
- `ts`: observer-produced ISO-8601 UTC timestamp
- existing `cwd`, matcher, and tool fields remain available for audit rows

In-memory keys and bounded state:

- Root key: `(provider, hostname, session_id)`
- Agent key: `agent_id` within a root
- Maximum 1,024 roots and 256 agent lifecycles per root
- The first 1,024 roots retain compact process-lifetime entries; saturation
  leaves additional roots null until restart rather than evicting a watermark
  that delayed events could recreate incorrectly
- Per-root overflow retains an invalid marker and ordering watermark; only a
  newer qualifying epoch may restore coverage
- The deliberately simple fixed cap is documented with a `ponytail:` comment;
  a more complex registry is justified only if saturation is observed

State transitions:

- Missing root: `None`
- Qualifying `SessionStart`: covered epoch with zero open agents
- `compact`: preserve the current epoch; without coverage it stays unknown
- `SubagentStart`: latest event for that agent becomes open
- `SubagentStop`: latest event becomes closed; duplicate stops never underflow
- For the same agent at the same timestamp, stop wins over start
- Exact timestamp ties involving qualifying `SessionStart` or `SessionEnd` are
  ambiguous and invalidate only that root to null
- Later timestamps win; a later start may reopen a reused agent ID
- `SessionEnd`: covered zero and terminal for that epoch
- Agent events after end without a newer qualifying epoch invalidate the root
- Pre-epoch events may be retained inside the bound so delayed delivery of an
  earlier qualifying root start can establish the correct later fold
- Restart or successful activity/provider disable clears affected state
- Successful enable/re-enable remains unknown until a newer qualifying
  `SessionStart`; enablement never invents known zero
- Malformed identity/source invalidates only an identifiable affected root;
  payloads too incomplete to identify a root are rejected and logged
- Missing or unparseable producer timestamps invalidate an identifiable root;
  otherwise-incomplete identity is rejected audit-only with context

Sessions contract:

- Rust: `observed_subagent_count: Option<u32>`
- TypeScript: `observed_subagent_count: number | null`
- `null`: no trustworthy current-boot coverage
- `0`: covered, no hook-observed open root-linked subagents
- Positive: covered, that many observed-open root-linked subagent lifecycles

SQLite `hook_invocations` remains audit/history only. Retention and old rows can
never reconstruct or change the process-local count.

## API / Interface Changes

- `/api/v1/hooks/observed` accepts provider-neutral Claude/Codex observations
  and optional backward-compatible hostname/source fields. Old payloads remain
  auditable but cannot establish live coverage without required identity.
- `start_server` and Tauri managed state receive the same lifecycle-state `Arc`.
- Tauri `get_session_breakdown` returns the required nullable observed count on
  every row; storage-only callers receive null until command-layer enrichment.
- `hooks-observed-updated` also invalidates Sessions data. No new event name,
  poller, or transport is added.
- Lifecycle folding and its invalidation emit happen synchronously after
  validation even if background audit persistence later fails; Hooks audit UI
  retains its existing post-persistence invalidation.
- Visible row order becomes project → positive `+N` metadata → provider →
  tokens → recency. Zero/null produce no metadata element.
- Accessible copy is exactly `1 subagent observed open` or
  `N subagents observed open`, exposed once on a non-focusable element.
- The count remains neutral, exact, tabular, unanimated, and independent of the
  recency-based live dot.

Rust/TypeScript field replacement ships atomically in the Tauri application and
has no supported external IPC consumer. Older binaries can read the unchanged
database. Rollback is reinstalling the previous release and restarting Quill;
no data rollback is required.

## Testing Strategy

The user authorized minimal automated regression tests.

- One table-driven Rust fold test covers initial/restart null, epoch zero,
  duplicate/out-of-order events, stop tie precedence, same-millisecond siblings,
  root/provider/host isolation, compact preservation, parent end, re-enable
  unknown, missing timestamps, malformed-root invalidation, root saturation,
  and per-root overflow recovery only after a newer epoch.
- Endpoint/storage tests cover old audit-only payloads, hostname/source parsing,
  nullable IPC overlay, sibling audit identity, removal of historical SQL, and
  retention-independent values.
- Claude script/setup tests cover payload fields, all four lifecycle groups,
  activity-off omission, managed-hook idempotence, and preservation of foreign
  configuration.
- Codex script/integration tests cover enriched payloads while confirming the
  existing lifecycle registration set and activity gate remain intact.
- A pure frontend formatter test covers null/zero omission, `+1` singular text,
  exact plural values, and a positive count while `row.live` is false without
  adding a DOM framework.
- Isolated browser fixtures at 320 px and 451 px verify long-name ellipsis and
  intact project→count→provider DOM order, exact single ARIA exposure,
  zero/null absence, and count/provider/token/recency columns without touching
  live Quill.
- Existing invalidation must start refresh within five seconds; mounted rows
  must update within six seconds, including measured IPC/render time.
- Performance verification adds no test code. Temporarily restore the removed
  `widget_query_perf_spike.rs` harness from parent of commit
  `2ebadc7ce484908c3345105403dc144a3351b3bd`, run the current production query
  in release mode for ten samples against the pinned 2026-08-02 schema-37
  corpus, record p95 at or below 300 ms in `verification.md`, then delete the
  transient harness before diff review. Harness incompatibility is a reported
  verification blocker, not permission to invent another benchmark path.
- Full gates: targeted CJS/MJS/Rust tests, `cargo fmt --check`, Clippy with
  warnings denied, Rust tests, npm lint/typecheck/build/knip, `lat check`, and
  `git diff --check`.

Every key test receives exactly one adjacent `@lat:` reference to its owning
leaf in `lat.md/live-subagent-count-tests.md`.

## Risks

- **Missed or blocked stop:** positive may remain observed-open until parent end
  or restart. Mitigation: honest naming/contract, no verified-liveness claim,
  process-local reset, and explicit docs.
- **Hook schema/version drift:** unsupported source or missing identity remains
  null. Mitigation: payload/setup tests, backward-compatible audit fields,
  provider-specific source allowlists, and idempotent managed-hook verification.
- **Hostname mismatch:** cross-host fallback would invent data. Mitigation:
  exact host key only; malformed/missing host cannot establish coverage.
- **Concurrent/out-of-order delivery:** scripts run independently. Mitigation:
  producer UTC timestamp, total tie precedence, agent-key idempotence, and tests.
- **State growth:** a long-lived process may saturate the fixed registry.
  Mitigation: retain existing root watermarks, leave new/overflowed roots null,
  recover only on restart/new epoch, and test that no delayed event restores
  false coverage.
- **Query regression:** command overlay should make SQL cheaper, but audit joins
  or scans could creep back. Mitigation: no SQLite reconstruction, frozen-corpus
  p95 evidence, and historical-query removal assertions.
- **Config mutation:** installer changes could affect user hooks. Mitigation:
  additive managed groups, existing last-known-good rollback, foreign-sibling
  preservation tests, and activity-off cleanup.
- **Frontend clipping/accessibility:** extra metadata consumes name width.
  Mitigation: existing flex meta, exact counts, sole name shrinkage, accessible
  singular/plural text, and isolated 320/451 px validation.
- **Rollback:** process-local state and no migration make rollback restart-only;
  reinstalling the prior integration removes managed additions without touching
  foreign configuration.

## Sequencing

### Implement observed subagent monitor and Sessions backend contract — P1

Independent root work item. Owns shared bounded state, endpoint folding,
activity/provider clearing, Rust model, historical SQL cleanup, audit identity,
command-layer overlay, backend tests, and synchronous invalidation even when
audit persistence fails.

Acceptance:

- Complete null/zero/positive lifecycle truth table passes for both providers,
  roots, and hosts.
- No historical agent enrichment remains in Sessions SQL.
- Restart/disable/saturation returns null; SessionEnd returns zero; no audit row
  reconstructs a positive.
- Same-time siblings persist independently in state and audit.
- Storage-only rows start null and Tauri command rows receive exact overlays.

### Add Claude Code lifecycle observations — P1

Independent root work item. Owns the existing Claude observer, setup groups,
script/setup tests, source allowlist evidence, and transactional configuration
preservation.

Acceptance:

- Activity tracking installs exactly one managed observer for SessionStart,
  SubagentStart, SubagentStop, and SessionEnd; disabled tracking installs none.
- Payload includes provider/root/normalized-host/source/agent/time and accepts
  only the verified official source set.
- Reinstall/uninstall preserves foreign hooks and last-known-good config.

### Complete Codex lifecycle observation payloads — P1

Independent root work item. Owns the existing Codex observer payload, managed
script verification, pure tests, and source allowlist evidence.

Acceptance:

- Existing lifecycle registrations and activity gating remain unchanged.
- Payload reliably includes configured/fallback host, source, root, agent, and
  producer time.
- Old/fallback identity behavior remains audit-compatible and tested.

### Render observed subagent counts in Sessions — P2

Depends on the backend contract. Owns strict TypeScript type, Sessions cache
invalidation, row mapping/order/identity, accessible formatter/test, neutral
layout, fixtures, historical retention UI cleanup, and isolated viewport proof.

Acceptance:

- Positive-only exact `+N` appears between project and provider within five
  seconds of refresh start and within six seconds mounted, independently of row
  recency.
- Null/zero omit the element; singular/plural accessible text is exposed once.
- 320 px and 451 px fixtures preserve all fixed columns and name ellipsis.
- No badge, new color, animation, component framework, or polling is added.

### Synchronize architecture and verify delivery — P2

Depends on all four implementation items. Owns lat.md/test-spec synchronization,
`verification.md`, final performance evidence, full gates, rollback check, and
repo-wide removal audit.

Acceptance:

- Every key test has one owning lat.md leaf/reference and `lat check` passes.
- This final task creates `lat.md/live-subagent-count-tests.md` and adds the
  adjacent `@lat:` references to tests after implementation tasks finish,
  avoiding parallel edits to shared test files.
- Frozen-corpus ten-sample p95 is at most 300 ms and recorded reproducibly.
- Accepted-event refresh starts within five seconds and mounted-row latency is
  at most six seconds.
- Repo-wide search finds no stale Sessions historical agent projection or claim.
- Full Rust/frontend/release-quality gates and diff checks pass with zero
  warnings; rollback requires no data migration.

Dependency graph:

- Backend monitor, Claude observation, and Codex observation are initially
  dispatchable in parallel.
- Sessions rendering depends only on the backend monitor contract.
- Architecture/verification depends on backend, both provider tasks, and UI.

## Backlog Refinement

No P4 backlog inputs exist. Closed investigations `quill-kbb` and `quill-pwu`
remain retired source context; the five implementation work items provide new
P1/P2 coverage without reopening or duplicating them.

## Target Epic

Create a new epic titled `Show observed subagent counts in Sessions` and place
the five dependency-wired work items beneath it.

## Alignment fixes applied

- Verified and cited the exact official provider source allowlists; removed the
  invented Claude `fork` source.
- Made root-event timestamp ties fail closed, added re-enable/malformed-ordering
  truth-table cases, and replaced unsafe eviction with saturation watermarks.
- Pinned hostname normalization to the existing short-host reporting path.
- Replaced an unauthorized ignored performance test with a transient archived
  harness measurement and explicit blocker behavior.
- Tightened refresh latency to five-second refresh start plus six-second mounted
  completion, and required isolated DOM/order/ARIA proof including idle rows.
- Assigned final lat.md specs and adjacent test references to the serialized
  verification task.
