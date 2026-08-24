# pi-analytics-completeness

## Problem Statement

Quill ingests Pi session transcripts for search, runtime, tool, lifecycle,
and per-message model usage analytics, but it silently drops session
evidence that later analysis needs. The 2026-08-24 audit measured the gap
against the 80 most recent local Pi sessions (~30,700 entries, 12,685
assistant messages, 16,670 tool results):

- `usage.reasoning` — per-message reasoning token counts — is present on
  99.7% of assistant messages and non-zero on 55% (6,989), but
  `pi_usage_dimension` parses only input/output/cacheRead/cacheWrite and
  `model_usage_observations` has no reasoning column. Reasoning spend and
  reasoning-vs-direct behavior cannot be analyzed at all.
- Compaction and branch-summary entries carry their own LLM `usage` that
  belongs to no assistant message. The two observed compactions cost
  $3.24 and $1.31 (311K + 252K tokens) and are absent from every token and
  cost total. Their `tokensBefore` evidence (922K and 924K) also never
  reaches any savings accounting, so Pi compaction savings are untracked.
- Assistant `stopReason` (12,263 toolUse / 374 stop / 19 aborted / 16
  error) and `errorMessage` (35 occurrences) are dropped. Interruption
  rate, provider-error rate, and truncated turns are invisible.
- `toolResult.isError` is dropped: 667 failed tool calls (4.0% of all
  calls) are indistinguishable from successes in `tool_actions`.
- Assistant `thinking` content blocks appear on 69% of assistant messages,
  but the Pi event classifier tests only `text` and `toolCall` block
  types, so Pi never produces an `asst_thinking` session event. The
  documented rationale — "evidence is unavailable" — is true only for the
  live extension push path, not for retained transcript parsing.
- `thinking_level_change` entries (231 observed: xhigh 151, off 80) are
  dropped, so a zero-reasoning message cannot be distinguished between
  "model chose not to think" and "thinking was off".
- `session_info` display names exist on 68 of 80 sessions and are stored
  nowhere in Quill, leaving no human-friendly grouping key.
- `custom_message` entries (655 observed: subagent-notify 325, supervisor
  requests 143, lat-reminder 95, …) participate in LLM context but are
  neither searchable nor counted.
- `toolResult.details` dictionaries (5,063 observed) are dropped,
  including exact `diff`/`patch`/`firstChangedLine` for 1,265 edits and
  subagent `runId` linkage on 285 delegation results. 197 tool results
  containing image blocks (screenshots) vanish without even a marker.
- The transcript records only message-append timestamps, so per-tool
  wall-clock duration for parallel calls and thinking duration within a
  message are unobservable from the file. Only the live extension can see
  `tool_execution_start`/`tool_execution_end` and streaming
  `thinking_start`/`thinking_end` boundaries.

Everything except the last item is sitting in already-retained JSONL and
is backfillable for all history through existing reingest machinery. This
feature closes the capture gaps so Pi sessions can be analyzed accurately
later — spend, outcomes, reasoning behavior, tool reliability, and
structure — without guessing.

## Goals

- Parse `usage.reasoning` into a first-class reasoning-token dimension on
  Pi per-message model usage observations, without double-counting it
  into totals that already include it.
- Account compaction and branch-summary LLM usage in token and cost
  analytics as summary-kind observations, and persist compaction
  `tokensBefore` as savings evidence.
- Persist assistant turn outcomes: `stopReason` and error presence per
  assistant message, queryable for interruption and failure analysis.
- Persist tool failure state: `toolResult.isError` on the corresponding
  `tool_actions` row for Pi, using a provider-neutral column.
- Emit `asst_thinking` session events for Pi assistant messages that
  contain thinking blocks, from retained transcript parsing, with
  historical backfill via reingest.
- Persist the thinking-level timeline from `thinking_level_change`
  entries so reasoning analysis can be conditioned on the active setting.
- Persist Pi session display names from `session_info` entries and expose
  them through session search and context surfaces.
- Make Pi `custom_message` injected context searchable with its
  `customType`, without polluting runtime turn analytics.
- Retain bounded `toolResult.details` payloads and an image-content
  marker on Pi tool rows, respecting the existing `tool_detail` payload
  carve-out.
- Extend the Pi tracking extension (protocol v2) with wall-clock span
  receipts for tool execution and thinking, and fold them into durable
  per-tool duration and per-message reasoning-duration evidence.
- Update `lat.md` contracts and owning test specs — including flipping
  the pinned "Pi thinking events are always zero" expectation — then pass
  `lat check` and all zero-warning gates.

## Non-Goals

- Populating the new provider-neutral columns (`is_error`, stop reason,
  reasoning tokens, durations) for Claude or Codex transcripts. Columns
  are designed to admit them; parsing them is follow-up work.
- Parsing `model_change` entries (233 observed). Every Pi assistant
  message already carries provider/model; the entry adds nothing.
- Parsing `bashExecution`, `label`, or user-image content (zero observed
  in the audit window), or storing thinking text, image bytes, or
  unbounded detail payloads anywhere.
- Tracking `--no-session` sessions or changing live-push ownership:
  retained transcript reconciliation remains the authoritative source for
  all Pi analytics rows, and `/sessions/messages` continues to reject Pi
  `asst_thinking` kinds.
- Changing retention policy semantics beyond documenting how the new
  evidence is pruned with its host tables.
- New analytics views or dashboards beyond the minimal field exposure
  clarified at the gate (summaries bucket, session names, outcome
  counts); dedicated reasoning/failure views are separate features.
- Windows support or Memory Optimizer coverage.

## Backlog Inputs

No open issues cover Pi analytics capture gaps. Open item `quill-oyie.9`
(qualify exact-pair Pi tracking release, epic "Harden Pi agent tracking")
is adjacent, not overlapping: this feature's protocol v2 additions must
coordinate with the exact-pair contract that epic enforces, and its
span-receipt work must not ship a wire-shape change outside that
discipline. Feature specs 026-028 are required historical context.

## Target Epic

Create a new epic titled **Complete Pi session analytics capture**. Do
not add children to `quill-oyie`.

## Source Authority

None. No visual artifact is referenced by this feature; all evidence is
measured transcript data cited in the Problem Statement.

## User Stories

### 1. Analyze reasoning spend per message

As a user analyzing model behavior, I want reasoning token counts on
every Pi assistant message so I can compare reasoning-heavy and direct
work across models, projects, and time.

Acceptance criteria:

- Pi persisted usage rows and `model_usage_observations` carry a nullable
  `reasoning_tokens` dimension parsed from `usage.reasoning`, with the
  same bounds validation as existing token dimensions.
- Reasoning tokens are an informational subset dimension: no total,
  rollup, or cost figure changes solely because reasoning tokens are now
  parsed (Pi `cost.total` already includes them).
- A message without `usage.reasoning` (0.3% observed) stores NULL, not
  zero, so absence is distinguishable from measured zero.
- Historical Pi sources backfill the dimension through the existing
  reingest marker without manual steps.

### 2. Count summary spend and compaction savings

As a user tracking spend, I want compaction and branch-summary LLM usage
counted so Pi token and cost totals stop undercounting real money.

Acceptance criteria:

- `compaction` and `branch_summary` entries with a `usage` object
  produce one summary-kind usage observation each, keyed by entry id,
  with tokens, costs, and reasoning parsed like assistant usage.
- Summary observations carry no fabricated model identity: model fields
  stay empty with the existing `model_evidence = 'missing'` state unless
  the entry names one.
- Session, hourly, and provider totals include summary spend; per-model
  breakdowns expose it as a distinct "Summaries (unattributed)" bucket
  so totals visibly reconcile, never as a named model.
- Each compaction entry's `tokensBefore` persists as savings evidence on
  its summary observation row, deduplicated by entry identity across
  reparses; `context_savings_events` integration is out of scope (see
  Technical Decisions).
- Reingest backfills observations for history.

### 3. See turn outcomes and provider errors

As a user diagnosing sessions, I want stop reasons and error evidence per
assistant message so aborted, truncated, and failed turns are queryable.

Acceptance criteria:

- Per-message Pi usage observations persist `stop_reason` and a boolean
  error-presence flag derived from `errorMessage`, both nullable and
  provider-neutral. Known values (stop/length/toolUse/error/aborted) are
  normalized; an unknown value is stored verbatim under a bounded length
  with a bounded diagnostic, never a source failure, and the column has
  no SQL CHECK enum.
- Aborted and error turns are countable per session, per model, and per
  time window through a dedicated outcome aggregation query path.
- Rows for providers that do not yet supply the fields remain NULL and
  are excluded from outcome denominators rather than counted as success;
  denominators use NOT NULL rows only.

### 4. Measure tool failure rates

As a user evaluating agent reliability, I want failed tool calls marked
so tool failure rate is a real metric.

Acceptance criteria:

- `tool_actions` gains a provider-neutral nullable `is_error` flag; Pi
  result correlation sets it from `toolResult.isError` for retained and
  notify parsing through the shared tool-row builder.
- Failure state survives the `tool_detail` payload carve-out: the flag is
  set even where payload columns are suppressed.
- Existing category-agnostic row counts and breakdown semantics are
  unchanged; the flag only adds information. Duplicate results for one
  call id stay last-write-wins, including `is_error`.
- Historical backfill lands via reingest.

### 5. See Pi thinking in the event timeline

As a user comparing reasoning behavior across providers, I want Pi
assistant messages with thinking blocks to emit `asst_thinking` events so
Pi stops being the blind spot in cross-provider turn-shape analysis.

Acceptance criteria:

- The Pi retained classifier emits `asst_thinking` (ordered before
  `asst_text`/`asst_tool_use` by event ordinal) when an assistant message
  contains at least one `thinking` block; empty blocks count as presence,
  matching Claude's classification.
- Runtime turn folding results are unchanged except where thinking-only
  messages previously produced no event at all.
- The pinned zero-count expectation for Pi thinking events is replaced by
  a positive spec; the live `/sessions/messages` rejection of Pi
  `asst_thinking` kinds remains and stays covered.
- The lat.md statement that Pi thinking evidence "is unavailable" is
  corrected to name the live push path only.
- Historical backfill lands via reingest.

### 6. Condition analysis on thinking level

As a user analyzing reasoning, I want the thinking-level timeline so
zero-reasoning messages can be attributed to settings rather than model
choice.

Acceptance criteria:

- `thinking_level_change` entries persist as source-owned setting-change
  rows (provider, session, chain, timestamp, source ordinal, setting
  name, value) in a narrow provider-neutral table replaced atomically
  with the source's other analytics rows.
- The active thinking level for any assistant message is derivable as
  the latest (timestamp, ordinal) row at or before it; state before the
  first change is NULL (unobserved); repeated levels are kept as
  distinct observations.
- Reingest backfills history; live-path behavior is unchanged.

### 7. Find sessions by name

As a user reviewing past work, I want Pi session display names captured
so sessions can be grouped and found by the name I gave them.

Acceptance criteria:

- The latest `session_info.name` on a source (last by source ordinal)
  persists to that source's analytics registry row and is exposed as a
  nullable additive field in search results, session context, and MCP/Pi
  compact views in this feature (clarified 3A).
- Renames converge on reparse; a cleared name clears the stored value.

### 8. Search injected context

As a user auditing what shaped a session, I want extension-injected
`custom_message` context searchable so subagent notifications and
reminders are visible evidence.

Acceptance criteria:

- Pi `custom_message` entries index as search documents with a distinct
  retained role and their `customType` as searchable metadata, bounded by
  existing content caps; `display:false` entries index too (display
  governs TUI rendering only).
- They emit no session events and do not perturb runtime, response-time,
  or turn analytics (asserted, not assumed).
- Non-context `custom` entries (e.g. `quill-tracking`) remain excluded,
  and the existing one-time legacy-document cleanup is not re-triggered
  and preserves the new role.

### 9. Retain tool result details and image markers

As a user doing deep tool analysis, I want bounded tool result metadata
retained so edit diffs and delegation linkage stop vanishing.

Acceptance criteria:

- Pi tool rows persist `toolResult.details` as bounded JSON in a nullable
  provider-neutral column: stored only when the serialized value fits
  the 10KB cap, else NULL — never truncated into invalid JSON — and
  suppressed for `tool_detail`-category rows per the payload carve-out.
- Pi tool rows persist a result-image count so screenshot-bearing results
  (197 observed) are identifiable without storing image bytes.
- Existing line-count derivation from tool input is unchanged in this
  feature, even where `details.diff` is present.

### 10. Measure wall-clock tool and thinking durations

As a user analyzing latency, I want true per-tool durations and
per-message thinking durations from the live extension, since the
transcript can never provide them.

Acceptance criteria:

- The Quill Pi extension observes `tool_execution_start`/`end` and
  streaming `thinking_start`/`thinking_end` boundaries, buffers spans in
  memory, and appends compact `quill-tracking` span receipts: one per
  finalized tool call (by call id, last-write-wins) and one per
  finalized thinking block (by message id + content index). Bounded
  hot-path cost: no payload copying, no per-delta work beyond a type
  check, with a measured microbenchmark bound.
- Protocol v2 gains the span receipt event kinds behind a capability
  digest bump; the cross-language contract fixture covers them; the
  exact-pair coordination rules from feature 028 apply. Older reporters
  remain fully accepted without spans. A pinned minimum Pi version with
  verified `message_update` payload shapes is a prerequisite.
- Retained reconciliation folds span receipts into a nullable
  `duration_ms` on the owning `tool_actions` row and, summed across a
  message's thinking blocks, a nullable `reasoning_duration_ms` on the
  owning per-message usage observation. Missing or malformed spans
  degrade to NULL plus a bounded diagnostic, never guessed values.
- Span evidence is forward-only; its absence for historical sessions is
  explicit (NULL), and no backfill is claimed.

## Constraints

- Source ownership: every new durable row or column is source-owned,
  written through the existing atomic snapshot replacement, prune-safe
  under generation rules, and idempotent across reparses. Live pushes
  never own any of the new evidence.
- Wire discipline: protocol v2 changes ship only with the coordinated
  fixture, version, and deployment gates established by feature 028, and
  must not conflict with the open exact-pair qualification
  (`quill-oyie.9`).
- Schema discipline: one migration; nullable provider-neutral columns;
  NULL means unobserved, never zero; `observation_kind` CHECK extended
  via table rebuild; `stop_reason` carries no CHECK enum so unknown
  future values cannot poison a source.
- Retention: uniform pruning with host tables (clarified 5A). New
  `tool_actions` columns are pruned with their rows and respect the
  insert-time watermark; summary observations prune with
  `model_usage_observations` under the existing engine; the new
  setting-change table is not touched by the engine in this feature and
  that survival is documented. Deletion expectations are stated in user
  docs alongside existing retention copy.
- Privacy boundary (clarified 4B): `details` JSON and `custom_message`
  content ride the `/sessions/messages` remote-push wire as additive
  optional fields under the same 10KB bounds. Off-device transmission
  remains opt-in and user-controlled through the existing remote-sync
  configuration, and the new fields are documented per constitution
  P11. Local transcript parsing remains authoritative on the owning
  machine.
- Payload bounds: all new stored payloads observe the existing 10KB caps
  and the `tool_detail` carve-out; no thinking text, image bytes, or
  unbounded JSON is stored.
- Accounting integrity: reasoning tokens must never be added to totals
  that already include them; summary spend must never double-count when a
  source reparses.
- Test authority: new and flipped automated tests are authorized as part
  of this feature per the lat.md test-spec process; each behavior lands
  with its owning spec in `lat.md/` and `lat check` passes.
- Gates: `cargo fmt --check`, `clippy -D warnings`, eslint, typecheck,
  knip, and the uv-wrapped cargo test suite stay zero-warning.
- Backfill safety: the historical reingest this feature relies on must
  not starve live folding — the recorded regression in
  `docs/solutions/runtime-errors/live-rails-degrade-when-retained-ingest-loops.md`
  (retained ingest looping on one watcher thread degraded live agent
  rails) is required context for sequencing the reingest work.

## Open Questions

All resolved at the clarify gate or by recorded technical decisions:

- Summary-spend presentation → resolved 1A: distinct unattributed bucket
  (see Clarifications).
- Context savings pairing → resolved by technical decision:
  `tokens_before` persists on the summary observation row; "after"
  pairing and savings-view integration are deferred follow-ups.
- Diff-derived `code_change` line counts → deferred follow-up feature
  with its own reingest (explicit non-goal here).
- Session-name facet → capture plus exposure now (3A); facet work is a
  deferred follow-up.
- Cross-provider parity (`is_error`, stop reasons, reasoning tokens for
  Claude/Codex) → one parity follow-up feature after this ships
  (explicit non-goal here).

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility,
scope, stakeholders) against constitution principles, followed by one
alignment auto-fix round. Merged verdict was BLOCK until the clarify
answers and the decisions below landed; both are now folded in.

### Critical Questions (answered at the gate)

1. Summary spend presentation — per-model breakdowns cannot attribute
   compaction/branch-summary spend to a named model
   (`src-tauri/src/models.rs:1732` requires model identity). Answered 1A.
2. Release slicing — MVP split vs one combined release. Answered 2B.
3. Session-name exposure — surface now vs persistence-only. Answered 3A.
4. Privacy boundary for new payloads — local-only vs included in remote
   sync. Answered 4B.
5. Retention expectations — uniform pruning vs exemptions. Answered 5A.

### Technical Decisions (self-resolved — veto to override)

- Absent-field semantics: fields the Pi format guarantees on a present
  record observe absence as negative evidence (`isError` absent ⇒ 0,
  `errorMessage` absent ⇒ no-error); optional evidence stores NULL
  (`usage.reasoning`, `stopReason`, uncorrelated results). Outcome
  denominators use NOT NULL rows only.
- Unknown enum values never fail a source: `stop_reason` gets no SQL
  CHECK enum; known values are validated at parse, unknown values are
  stored verbatim (bounded) with a bounded diagnostic — a future Pi stop
  reason must not poison the session (avoids the
  `PiSourceIdentity`-style source drop at
  `transcript_analytics.rs:1899`).
- Summary observation identity: `source_record_key` namespace
  `pi_summary_v1:{header-len}:{entry-id}`, `observation_kind='summary'`,
  `turn_id` = entry id, `token_evidence='direct'`; excluded from turn
  counters, included in token rollups/snapshots so totals reconcile.
- Compaction savings evidence lives on the summary observation row
  (nullable `tokens_before`), not in `context_savings_events` — that
  table is append-only, source-less, and its normalizer zeroes estimates
  for foreign categories (`storage.rs:1677-1689`); savings-view
  integration is deferred.
- `details` JSON is stored only if it fits the 10KB cap after
  serialization, else NULL — never truncated into invalid JSON
  (`truncate()` appends a marker, `sessions.rs:2699`). Image count and
  error flag persist regardless.
- Search role for injected context is the literal `custom_message`;
  the one-time legacy Pi cleanup query is updated to preserve it;
  `display:false` entries index too (display governs TUI only). Tantivy
  schema bump and full index rebuild are budgeted in the plan.
- Span wire grammar (story 10): receipt kinds `tool_span`
  (`tool_call_id`, `started_at_ms`, `ended_at_ms`, last-write-wins per
  call id) and `thinking_span` (`message_id`, `content_index`, same
  times; one receipt per finalized thinking block, summed per message
  into `reasoning_duration_ms`); per-entry span cap; malformed spans
  degrade to NULL plus bounded diagnostic; cross-language fixture covers
  all kinds. Protocol follows 028's exact-pair discipline — older
  reporters simply produce no spans (NULL), never a parse failure.
- Migration: expanding the `observation_kind` CHECK requires the
  SQLite 12-step table rebuild; it runs synchronously in `Storage::init`
  with a measured startup budget and disk preflight; migrations remain
  forward-only per existing scheme.
- Reingest is provider-scoped for this feature (Pi roots only), runs on
  the retained worker after live folding, and carries a measured budget
  — the recorded live-rail starvation learning is the guardrail.
- Setting-change rows carry `source_ordinal`; the active level for a
  message is the latest (timestamp, ordinal) at-or-before it; state
  before the first change is NULL (unobserved), repeated levels are
  kept as distinct observations.
- Cost scope: per-observation costs persist (already parsed for Pi);
  session/provider cost totals derive at query time; no new cost rollup
  columns in this feature.
- Empty `thinking` blocks count as presence, matching Claude's
  presence-only classification (`sessions.rs:4559`).
- Duplicate `toolResult` for one call id stays last-write-wins,
  including `is_error`.
- Live `/sessions/messages` keeps rejecting Pi `asst_thinking`; the
  pinned zero-count storage test is replaced by a positive
  retained-parser spec plus a separate live-rejection spec (P7
  authorization recorded in this spec's Constraints).
- Summary entry types are exactly `compaction` and `branch_summary`;
  no other summary-bearing entry type exists in the format.

### Non-Blocking Observations

- Session-name and `customType` API fields must be additive nullable
  across `SearchHit`, `SessionContext`, compact output, MCP, and
  TypeScript types; exact field list lands in the plan.
- lat.md supersession list (plan must enumerate): `data-flow.md:269,281`
  (Pi role exclusions), `:297` (thinking availability),
  `session-search-tests.md:11-21`, `pi-live-session-tests.md:72-74`,
  backend schema sections, Models-view totals contract.
- Next-day asks recorded as explicit non-goals: name/customType facets,
  outcome/reasoning dashboards, exports, detail viewers, diff-derived
  line counts, cross-provider parity.
- Extension work pins a minimum Pi version with verified
  `message_update` payloads before the span slice starts.

## Clarifications

**Q1: Summary spend presentation — unattributed bucket or totals-only?**
A: 1A — distinct "Summaries (unattributed)" bucket in per-model
breakdowns; totals visibly reconcile. Story 2 acceptance updated.

**Q2: Release slicing — MVP split or one combined release?**
A: 2B — one combined release. All ten stories ship in this epic as one
delivery. Internal sequencing still lands retained/backfillable capture
first and the extension span slice last, coordinated with the open
`quill-oyie.9` exact-pair qualification — that ordering is a build
constraint, not a release split.

**Q3: Session-name exposure now or persistence-only?**
A: 3A — capture and surface now as nullable additive fields in search
results, session context, and compact views. Facets deferred.

**Q4: Privacy boundary for `details` JSON and `custom_message` content?**
A: 4B — included in remote sync. The `/sessions/messages` push protocol
gains additive optional fields carrying both payloads under the same
10KB bounds, so a remote Quill aggregating other machines retains them
too. Off-device transmission stays governed by the user's existing
opt-in remote-sync configuration and the new fields are documented
(constitution P11).

**Q5: Retention of new evidence?**
A: 5A — uniform pruning with host tables; no exemptions. Spec's retention
constraint corrected; deletion expectations documented.

## Architecture Approach

Every transcript-derived signal rides the existing retained-source
pipeline unchanged in shape: one bounded parse per source
(`parse_transcript_analytics_source`), evidence structs extended in
place, one atomic snapshot replacement
(`replace_transcript_analytics_snapshot`), prune-safe under generations.
No new pipelines, schedulers, or ownership models. The one wire change
is additive optional fields on the existing push endpoint (clarified
4B), received by a dedicated wire-receiver work item. The Pi parser
(`sessions.rs` Pi branch) gains block/entry handling; Pi evidence
extraction (`transcript_analytics.rs`) gains usage dimensions, summary
observations, setting rows, and the session name. Live-path ownership is
unchanged — retained reconciliation authoritatively replaces live rows
and the Pi `asst_thinking` rejection stays — while `/sessions/messages`
gains additive optional fields carrying `details` JSON and
`custom_message` content; older clients omit them and store NULL.

Alternatives considered and rejected:

- Feeding compaction savings into `context_savings_events` — rejected:
  the table is append-only, source-less, and its normalizer zeroes
  foreign-category estimates; savings evidence lives on the summary
  observation row instead (`tokens_before`).
- A separate reasoning/outcome side table — rejected: per-assistant-
  message grain already exists as `model_usage_observations`; nullable
  columns there avoid a join and a second identity scheme.
- Live-pushing thinking/span evidence as the durable source — rejected:
  retained reconciliation replaces Pi rows wholesale; durable spans must
  arrive via `quill-tracking` entries in the JSONL itself.
- Tantivy-stored session names — rejected: renames would force document
  rewrites; a registry column plus response-time enrichment keeps
  renames one-row cheap.

The extension span slice is the only forward-only element: `quill.ts`
observes `message_update` stream boundaries and
`tool_execution_start`/`end`, buffers spans in memory, and appends
compact `quill-tracking` span receipts (one per finalized tool call, one
per finalized thinking block). Retained parsing folds receipts into
`duration_ms` and `reasoning_duration_ms`; the receipts are consumed at
parse time and not stored as rows. Protocol v2 gains the two receipt
kinds behind a capability digest bump under 028's exact-pair discipline.
Because Pi tracking-entry parsing rejects unrecognized tracking events
(`pi_session.rs:198-224`, `pi_tracking.rs:320-362`), the extension half
and the Rust receiver/fixture half ship as one atomic exact-pair item —
the extension must never append receipt kinds the paired build cannot
parse.

Constitution check: P1 (NULL-vs-negative-evidence rules, no invented
data), P2 (existing Rust/Tauri + strict-TS layers only), P3 (parse work
stays on retained workers; migration budgeted), P4 (single migration
with generalized backup/preflight, atomic snapshot replacement,
forward-only), P5 (unknown enums become bounded diagnostics, never
source failures), P7 (tests authorized here, pinned one-to-one with
lat.md specs), P8 (supersession list enumerated; owning lat.md updates
attached per work item), P10 (migration, reingest, index-rebuild, and
span hot-path budgets measured), P11 (documented opt-in wire fields),
P12 (Beads-tracked, gated delivery).
Learnings check: the live-rail starvation learning is honored by
sequencing reingest after live folding on the retained worker and
measuring fold latency during backfill.

## Affected Components

- `src-tauri/src/storage.rs` — migration (CHECK rebuild, new columns,
  new table, registry column, generalized pre-migration backup and disk
  preflight), insert/replace loops, rollup queries, outcome aggregation
  query path, summary bucket aggregation.
- `src-tauri/src/sessions.rs` — Pi event classifier (thinking), tool-row
  builder (is_error, details, image count), custom_message indexing,
  cleanup-query guard, Tantivy schema bump, search response enrichment.
- `src-tauri/src/transcript_analytics.rs` — Pi usage parse (reasoning,
  stop_reason, error flag), summary observations, setting rows, session
  name, span-receipt folding.
- `src-tauri/src/pi_session.rs` — entry-type recognition for
  `thinking_level_change`, `session_info`, `compaction`,
  `branch_summary`, `custom_message`.
- `src-tauri/src/pi_tracking.rs` + `src-tauri/src/models.rs` — protocol
  v2 span receipt kinds, capability digest, validation.
- `src-tauri/src/server.rs` + `src-tauri/src/models.rs` — wire receiver
  for the 4B additive optional push fields: custom-message role
  admission, tool-evidence fields, bounds validation, old-envelope
  behavior (today the handler admits only user/assistant roles and
  builds no tool rows — `server.rs:2074-2076,2236`).
- `src-tauri/pi-integration/quill.ts` — span observation, buffering,
  receipt append; fixture generator update.
- `src-tauri/src/retention_engine.rs` — no behavior change; archive
  column list follows schema; docs updated.
- `src/types.ts`, `src/components/widget/views/ModelsView.tsx`, Sessions
  breakdown surfaces — nullable additive fields (session name, summary
  bucket, outcome counts).
- `src-tauri/claude-integration/mcp/` search surface — additive
  session-name field in compact results.
- `lat.md/` — data-flow, backend schema, session-search-tests,
  pi-live-session-tests, pi-notify-index-tests, models/runtime sections;
  supersession list from the Spec Review; owning specs land with each
  work item.

## Data Model

One migration (next schema version), forward-only, with a generalized
pre-migration backup, disk preflight, documented restore runbook, and
failed-rebuild recovery tests (current backup code is schema-45-specific
— `storage.rs:820-882`):

- `model_usage_observations`: SQLite 12-step table rebuild to extend
  `observation_kind` CHECK to `('turn','token','summary')`; new nullable
  columns `reasoning_tokens INTEGER` (bounds-checked like siblings),
  `stop_reason TEXT` (no CHECK enum; bounded length),
  `had_error INTEGER` (0/1), `tokens_before INTEGER` (summary rows),
  `reasoning_duration_ms INTEGER`. Indexes preserved through rebuild
  with row-count and index-presence assertions.
- `tool_actions`: additive nullable columns `is_error INTEGER` (0/1),
  `details_json TEXT` (store-if-fits ≤10KB, valid JSON or NULL),
  `result_image_count INTEGER`, `duration_ms INTEGER`. No index changes.
- New table `session_setting_events(provider TEXT, source_key TEXT,
  session_id TEXT, chain_id TEXT, parent_chain_id TEXT,
  source_ordinal INTEGER, timestamp TEXT, setting TEXT, value TEXT)`
  with uniqueness on `(provider, source_key, setting, source_ordinal)`;
  replaced atomically with its source's snapshot; not retention-pruned
  in this feature (documented).
- `transcript_analytics_sources`: new nullable `session_name TEXT`,
  written from the latest `session_info` entry (last by source ordinal;
  cleared name clears the column).
- Pi summary observations: `source_record_key` =
  `pi_summary_v1:{header-len}:{entry-id}`, `observation_kind='summary'`,
  `turn_id` = entry id, `token_evidence='direct'`, model fields empty
  with `model_evidence='missing'` unless the entry names one; excluded
  from turn counters, included in token rollups/snapshots. Migration 48
  adds `token_snapshots.observation_kind IN ('turn','summary')` with a
  legacy/default value of `turn`, so snapshot and cleanup totals include
  summary spend without counting it as a turn.
- Tantivy: schema bump adding `custom_type` field; Pi `custom_message`
  entries index with role literal `custom_message`; full index rebuild
  on first open after upgrade (budgeted with the migration).

## API / Interface Changes

- IPC (additive, nullable): search results, session context, and batch
  breakdown responses gain `session_name`; session breakdown gains tool
  failure counts; a dedicated outcome aggregation query path exposes
  aborted/error/interruption counts per session, per model, and per time
  window with NOT-NULL denominators; models overview gains a
  `summary_usage` aggregate rendered by ModelsView as a distinct
  "Summaries (unattributed)" band entry (never a pseudo-model row).
- MCP + Pi compact search views: additive `session_name`; content
  budgets unchanged.
- `/api/v1/sessions/messages` (clarified 4B): additive optional
  per-message fields — `custom_type` plus content for injected-context
  messages, and tool-result `details_json`, `is_error`, and
  `result_image_count` — validated to the same 10KB bounds; absent
  fields store NULL so older clients keep working; the receiver also
  admits the `custom_message` role and durable source-less tool-evidence
  writes (both new — today it admits only user/assistant and builds no
  tool rows). Pi `asst_thinking` kinds remain rejected (existing typed
  error).
- Protocol v2 `quill-tracking`: new receipt kinds `tool_span`
  (`tool_call_id`, `started_at_ms`, `ended_at_ms`; last-write-wins per
  call id) and `thinking_span` (`message_id`, `content_index`, same
  time fields; one receipt per finalized thinking block); per-entry span
  cap; capability digest bump; cross-language fixture extended. Older
  reporters remain valid — absent spans mean NULL durations.
- TypeScript types mirror every additive field; no breaking changes
  anywhere.

## Testing Strategy

Authorized per Constraints (constitution P7); every behavior lands with
its owning lat.md spec, one-to-one with tests, attached to its work item
(not deferred to the final sweep):

- Parser (Rust unit): thinking-block event emission incl. empty-block
  presence and thinking-only messages; reasoning/stop_reason/had_error
  parse incl. absent-field NULL-vs-negative rules and unknown stop
  reasons (bounded diagnostic, source survives); summary observations
  for `compaction`/`branch_summary` incl. `tokens_before` and identity
  stability across reparses; setting rows incl. equal-timestamp ordinal
  ordering; session-name last-wins and clear; `is_error` last-write-wins
  on duplicate results; details store-if-fits boundary (fits, oversize,
  malformed); image count; span receipt folding (well-formed, malformed
  → NULL + diagnostic, duplicate call id, multi-block message summing).
- Regression invariants (Rust): reasoning parsing changes no existing
  total or rollup; summary spend reconciles across session, hourly, and
  provider totals and the unattributed bucket; category-agnostic tool
  counts unchanged; mixed-block message folding unchanged; input-derived
  line counts unchanged where `details.diff` exists.
- Storage (Rust): migration rebuild preserves rows/indexes and extends
  CHECK; generalized backup/preflight and failed-rebuild restore path;
  token snapshot migration defaults legacy rows to turns and admits only
  turn/summary; new-column watermark behavior on `tool_actions` inserts;
  summary rows excluded from turn counters and included in token rollups;
  retention
  prunes summary rows with their table (updated retention specs);
  prune/generation safety for `session_setting_events`; outcome
  aggregation denominators (NOT NULL only) per session/model/window.
- Search (Rust): `custom_message` docs with `custom_type` field incl.
  `display:false`; zero-session-event/runtime-neutral assertion for
  indexed custom messages; negative cases for non-context `custom` and
  `quill-tracking` entries; cleanup query preserves the new role;
  schema-bump rebuild path; response enrichment joins names without
  unbounded lookups.
- Live contract (Rust): Pi `asst_thinking` rejection retained; the
  pinned zero-count storage test replaced by a positive retained-parser
  spec plus a live-rejection spec; the new optional wire fields proven
  for bounds enforcement, NULL-when-absent storage, custom-message role
  admission, durable tool-evidence writes, and old-client envelopes
  against the same handler.
- Wire (TS↔Rust fixture): span receipt kinds accepted by the real
  deserializer/validator; older-envelope acceptance unchanged.
- Extension (node test): span buffering emits one receipt per finalized
  thinking block and per tool call across interleaved parallel tools;
  no per-delta allocation beyond a type check.
- Performance evidence (P10): measured against a hash-pinned fixture
  corpus derived from the audit window — migration + index rebuild wall
  time on a production-scale copy; reingest pass wall time with expected
  row counts and marker-clear/retry assertions; live-fold p95 latency
  during backfill within the existing fold-overhead budget; span
  hot-path microbenchmark bound.

## Risks

- Migration + Tantivy rebuild happen on the same first launch —
  mitigation: measure combined budget on a production-scale DB before
  release; generalized preflight disk check and pre-migration backup
  with restore runbook; rebuild resumes on interruption per existing
  index rebuild semantics.
- Historical reingest can starve live folding (recorded learning) —
  mitigation: Pi-scoped marker only, retained-worker sequencing after
  live folds, measured fold p95 during backfill as acceptance evidence.
- Exact-pair span slice can collide with the open `quill-oyie.9`
  qualification — mitigation: the span item is atomic (extension +
  receiver + fixture), sequenced last, blocked on `quill-oyie.9`
  closure, and lands behind the digest bump.
- CHECK-rebuild regression risk on a large `model_usage_observations` —
  mitigation: 12-step rebuild inside one transaction with row-count and
  index-presence assertions; forward-only with generalized backup.
- Summary rows double-counting into turn analytics — mitigation:
  explicit exclusion spec + fixture asserting turn counters unchanged
  when summary rows exist.
- `details_json` growth — mitigation: store-if-fits 10KB, category
  carve-out respected, measured DB growth on the audit corpus.
- Wire compatibility for the new optional push fields — mitigation:
  optional end to end; oversized or malformed values reject per the
  endpoint's existing row-validation semantics; envelope fixtures cover
  old-client and new-client payloads.

## Normative Visual Coverage

None (0 rows). Source Authority names no visual artifact.

## Backlog Refinement

None. No P4 backlog inputs exist for this feature (verified across open
and deferred issues); `quill-oyie.9` remains an external coordination
constraint, not an input.

## Sequencing

Order expressed as dependencies, not title codes. One combined release
(clarified 2B). Items 2-5 form a serialized single-writer chain because
they share `transcript_analytics.rs`, `sessions.rs`, and the storage
insert seam; parallel workers cannot see each other's unlanded work.
Each capture item includes its owning lat.md spec updates.

1. **Schema migration, storage plumbing, and evidence foundation**
   (P0) — the one migration: CHECK rebuild, all new columns,
   `session_setting_events`, `session_name` registry column; generalized
   pre-migration backup, disk preflight, restore runbook, and
   failed-rebuild tests; every new evidence-struct field and storage
   binding extended end-to-end with empty producers, so later items only
   populate fields. Blocks everything below. Acceptance: migration,
   backup/restore, watermark, and prune specs pass; measured migration
   budget on the pinned corpus recorded.
2. **Pi usage evidence** (P1, needs 1) — reasoning tokens, stop_reason,
   had_error on per-message rows; summary observations with
   `tokens_before`; rollup mapping (turn-counter exclusion, token
   inclusion). Acceptance: parser, rollup, and regression-invariant
   specs; totals reconcile on the pinned corpus.
3. **Pi tool evidence** (P1, needs 2) — `is_error`, `details_json`,
   `result_image_count` through the shared tool-row builder for retained
   and notify paths. Acceptance: correlation, carve-out, and
   store-if-fits specs.
4. **Thinking events and setting timeline** (P1, needs 3) — thinking
   block classification into `asst_thinking`; `thinking_level_change`
   rows; pinned-test replacement (positive retained spec + live
   rejection spec). Acceptance: event-order, join-rule, and
   folding-neutrality specs.
5. **Session names and injected-context search** (P1, needs 4) —
   registry write + response enrichment; `custom_message` indexing incl.
   `display:false`, `custom_type` field, cleanup guard, schema bump.
   Acceptance: search, cleanup, runtime-neutrality, and negative-case
   specs; rebuild budget measured with item 1's.
6. **Remote push wire receiver** (P1, needs 1, 3, 5) — the 4B additive
   optional fields on `/sessions/messages`: custom-message role
   admission, tool-evidence fields with durable source-less writes,
   bounds validation, old-envelope behavior; single owner of
   `server.rs`/`models.rs` wire shapes. Acceptance: wire-bound,
   role-admission, durable-write, and old-client envelope specs.
7. **Read surfaces** (P1, needs 2-5) — IPC/MCP/TS additive fields;
   outcome aggregation query path (per session/model/window, NOT-NULL
   denominators); ModelsView summaries band; Sessions breakdown
   outcome/failure counts. Acceptance: aggregation specs, IPC/MCP
   contract tests, typecheck/lint/knip clean; Flat Polish conformance
   for the new band (P9).
8. **Pi-scoped reingest and backfill** (P1, needs 2-5) — provider-scoped
   marker, retained-worker orchestration, marker-clear/retry assertions,
   measured fold p95 during backfill. Acceptance: full history
   backfilled on the pinned corpus with expected row counts; starvation
   guardrail evidence within the existing fold-overhead budget.
9. **Extension span slice — atomic exact-pair** (P2, needs 1; blocked on
   `quill-oyie.9` closure) — quill.ts span observation and receipts,
   protocol kinds + digest bump, cross-language fixture, and Rust
   folding into `duration_ms`/`reasoning_duration_ms`, shipped as one
   item because Pi tracking-entry parsing rejects unknown receipt kinds;
   prerequisite: pinned minimum Pi version with verified
   `message_update` payload shapes. Acceptance: wire fixture, folding,
   multi-block summing, and hot-path microbenchmark specs.
10. **lat.md supersession sweep and gate qualification** (P2, needs
    all) — apply the supersession list, `lat check`, zero-warning gates,
    retention/deletion and P11 wire-field documentation. Acceptance:
    `lat check` passes; CI gates green; docs updated.

## Alignment fixes applied

- Rebuilt the spec artifact from session history after the file was
  found deleted from disk mid-pipeline (uncommitted; cause unknown), and
  applied this round's fixes in the same write.
- Removed nonexistent "tree-summary" entries from Goals/Story 2; summary
  types are exactly `compaction` and `branch_summary` (align, must).
- Story 3 reworded: known-value normalization, bounded unknown
  passthrough, no SQL CHECK — removing the validation contradiction
  (align, must).
- Resolved all stale 4A/local-only remnants: Architecture, Affected
  Components (`server.rs` now owns wire receiver work), API changes
  (align, must).
- Added the dedicated outcome aggregation query path (per
  session/model/window, NOT-NULL denominators) to API + Testing +
  Sequencing item 7 (align, must).
- Added custom_message `display:false`, runtime-neutrality, and
  negative-exclusion tests (align, must).
- Fixed span receipt grammar: one receipt per finalized thinking block
  (message id + content index), summed per message; multi-block test
  added (align, must).
- Sequencing rewritten: items 2-5 serialized as a single-writer chain
  over shared files; item 1 expanded to own all evidence-struct fields
  and storage bindings (quality, must).
- New wire-receiver item (6) owns all `/sessions/messages` changes —
  handler currently admits only user/assistant and writes no tool rows
  (quality, must).
- Span slice made one atomic exact-pair item blocked on `quill-oyie.9`;
  removed "may start anytime" (quality, must).
- Item 1 now owns generalized pre-migration backup, disk preflight,
  restore runbook, and failed-rebuild tests (quality, must).
- Measurable criteria added: hash-pinned fixture corpus, expected row
  counts, fold p95 ceiling, marker-clear/retry assertions, IPC/MCP
  contract tests (quality, should).
- Owning lat.md updates attached to each capture item; final item
  narrowed to supersession sweep + gates (quality, should).
- Regression-invariant test list added (unchanged totals, reconciling
  summary spend, unchanged tool counts/folding/line counts) (align,
  should).
- Minimum verified Pi version made an explicit prerequisite of the span
  item (align, should).
