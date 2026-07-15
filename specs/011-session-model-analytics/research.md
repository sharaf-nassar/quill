# Research: Session Model Analytics

Research resolves provider evidence, persistence, reconciliation, aggregation,
and UI decisions needed to implement the approved specification without model-ID
product rules.

## R1. Persist observations, sources, and one backfill state

**Decision**: Add `model_usage_observations`, `model_observation_sources`, and a
singleton `model_backfill_state` in migration 28. Keep observations for the
lifetime of their unsuppressed retained source. Deletion removes observations
while retaining only durable source fingerprint/suppression state. The `30D`
control limits query scope; model analytics adds no age-based TTL, model catalog,
alias registry, or hourly model rollup.

**Rationale**: Observation rows preserve turns, nullable token dimensions, chain
ordering, gaps, and switches. Source rows make append/rewrite/removal handling
idempotent. One state row is sufficient for the user-visible pending/running/
complete/partial/failed lifecycle. Indexed SQLite queries over the 100,000-row
target retain the source fidelity needed for detail views.

**Alternatives considered**: One model column on `token_snapshots` cannot express
switches, tokenless turns, or source reconciliation. A normalized model table
would introduce catalog semantics without value. Pre-aggregated hourly rows lose
chain detail and complicate changed-source replacement.

## R2. Accept only co-located provider evidence

**Decision**: Use provider adapters that emit a shared observation shape:

- Claude `assistant` records may co-locate `message.model` and `message.usage`;
  emit one turn observation with only the token dimensions actually present.
- Codex `turn_context` records provide explicit `payload.model` and optional
  `payload.turn_id`; emit a tokenless turn observation.
- Codex `event_msg` token-count records provide usage but no reliable model
  foreign key; emit unattributed token observations.
- Ignore quota/model labels and direct message payloads that lack co-located
  session evidence. Never carry a preceding or nearby model onto token usage.

**Rationale**: This is the narrowest rule that satisfies complete accepted-ID
capture without manufacturing attribution. OpenAI documents `turn_context` as
the per-turn model context and notes that its protocol is not exhaustive. The
Claude Agent SDK exposes assistant messages with independent `model` and `usage`
fields. Provider adapters isolate future source-shape changes from model identity,
which stays data-driven.

**Primary sources**:

- [OpenAI Codex protocol v1](https://github.com/openai/codex/blob/main/codex-rs/docs/protocol_v1.md)
- [OpenAI Codex protocol source](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs)
- [Anthropic Claude Agent SDK message types](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/types.py)

**Alternatives considered**: Associating Codex token events with the most recent
turn context appears useful but violates the approved reliable-evidence boundary.
The existing token-report scripts aggregate live usage and cannot replay model
identity, so they remain unchanged.

## R3. Treat model identifiers as opaque provider-qualified data

**Decision**: Trim surrounding Unicode whitespace, accept 1–256 Unicode scalar
values, reject control characters, and preserve every other character exactly.
Identity is `(provider, raw_model_id)` with binary, case-sensitive comparison.

**Rationale**: Provider strings can include dates, aliases, punctuation, and
unseen versions. Exact storage makes new Claude, GPT, and other provider models
available automatically without a release while provider qualification prevents
cross-provider collisions.

**Alternatives considered**: Lowercasing, family extraction, friendly names,
allowlists, and alias merging all rewrite evidence or require ongoing product
maintenance. They are explicitly out of scope.

## R4. Normalize cumulative counters without duplication

**Decision**: Normalize each Codex cumulative token dimension independently:

1. Track the previous valid cumulative value separately for each dimension.
2. A dimension's first valid value contributes its full value from zero.
3. A later value at or above its baseline contributes `current - previous`.
4. A later value below its baseline starts a new segment for that dimension and
   contributes the full current value from zero.
5. A missing or invalid dimension contributes null and does not advance its
   previous valid baseline.
6. Emit a token row only when at least one delta is positive; when another
   dimension changes, preserve valid explicit-zero sibling deltas.

Skip records whose computed deltas are all zero. A decrease emits a bounded local
reset warning, but no persisted gap and no backfill failure. When any cumulative
`total_token_usage` object exists, never add `last_token_usage`; if only a last
value exists and uniqueness cannot be proven, leave token dimensions absent and
log the source limitation instead of risking an overcount.

**Rationale**: Whole-source replay plus deterministic deltas produces the same
rows after every pass. Conservative omission keeps coverage incomplete but true.

**Alternatives considered**: Summing every cumulative record grossly overcounts.
Adding both total deltas and last usage double-counts. Guessing reset boundaries
from timestamps is not source-backed.

## R5. Reconcile one source at a time in a background runner

**Decision**: Migration inserts pending backfill state. App setup starts a
nonblocking, process-guarded runner; an interrupted `running` state returns to
`pending`. Discovery assigns a generation and stable owning root key to every
source, then persists configured, completed, and failed root counts plus explicit
inventory completeness. Each changed source is parsed before a short transaction
deletes only that source's prior rows,
inserts its replacement rows, and updates its fingerprint. Unreadable sources
retain their last successful rows and make the run partial. Removed sources are
pruned only within their persisted root key after that exact root completes. A
partial run can still have complete inventory when every root was enumerated but
individual sources failed.

Fast comparison uses nanosecond mtime and size. Changed candidates receive a
SHA-256 content fingerprint after reading. Unchanged fingerprints are no-ops.
Progress and data events follow committed batches, not speculative work.

Live transcript notifications enter a model queue keyed by provider and canonical
source path before the existing session-keyed search queue can coalesce payloads.
Only repeat notifications for the same source coalesce, so parent and subagent
paths that share a session remain independently reconciled.

**Rationale**: Claude parent and subagent files can share a root session ID, so
session-scoped replacement can erase sibling evidence. Source-scoped replacement
avoids that race, limits lock duration, and makes retry deterministic without
rebuilding unrelated search, tool, or skill indexes.

**Alternatives considered**: One transaction for the full history blocks other
work and loses all progress on interruption. Reusing the current search-index
mtime sweep couples unrelated data and has insufficient source identity.

## R6. Preserve root sessions and independent chains

**Decision**: Store native source session ID, resolved analytics/root session ID,
chain ID, optional parent chain ID, sidechain flag, and agent ID. Claude uses the
parent session as root and `agentId` for a subagent chain. Codex uses thread and
`parent_thread_id` metadata to resolve a root graph; a child source remains its
own chain. Reconcile sources again if later metadata improves root resolution.

Ownership is layered: `sessions.rs` discovers provider roots, canonical source
paths, filesystem provenance, and root completion; provider adapters parse
transcript-native session/parent/chain metadata; the reconciliation coordinator
resolves the cross-source root graph. Path-layout hints never override conflicting
transcript-native metadata.

Only `turn` observations participate in switch adjacency. Partition by provider,
analytics session, and chain; order by timestamp, source ordinal, then stable
record key. A null/invalid model turn resets adjacency. Token-only unattributed
records affect coverage but do not break turn adjacency. Repeated equal identities
do not switch.

Primary model is highest attributed tokens, then turns, then case-sensitive,
locale-independent Unicode-scalar lexicographic provider and raw identifier.
SQLite `BINARY` comparison over valid UTF-8 implements this ordering. Every
calculation uses the active range.

**Rationale**: Native chains prevent concurrent parent/subagent activity from
creating false switches. Explicit null turns enforce the specification's gap
semantics, while token-only Codex evidence is not itself a model turn.

**Alternatives considered**: A single session-level model hides changes. Ordering
all parent and child events together manufactures transitions. Letting missing
turns disappear manufactures adjacency across unknown work.

## R7. Use bounded aggregate and detail IPC

**Decision**: Add separate commands for summary/table analytics, chart history,
model-session paging, session chain history, and retry. Use camelCase Tauri
responses and one serialized `{ code, message }` error envelope rather than plain
strings. History buckets are 5 minutes for `1H`, 1 hour for `24H`, 6 hours for
`7D`, and 1 day for `30D`. Page model sessions by an opaque keyset cursor with a
default limit of 20.

Emit `model-analytics-updated` after committed status or observation changes.
From the first event, frontend hooks open a fixed one-second coalescing window;
later events join that window without extending its deadline. The resulting
refresh advances one frontend-only generation shared by aggregate and mounted
detail hooks. A 60-second fallback uses the same refresh path. Detail queries
remain lazy; loaded session pages replay from page one through their prior page
count, expanded histories refetch, and collapsed history caches invalidate.

**Rationale**: Summary data stays bounded, chart density matches current range
intent, and details do not inflate initial tab latency. Keyset paging remains
stable while new observations arrive.

**Alternatives considered**: One nested response forces all session histories to
load up front. Offset paging can skip or repeat sessions under live updates.
Polling alone misses the five-second visibility target or wastes local work.

## R8. Keep Models independent of live usage snapshots

**Decision**: Adjust `App.tsx` so Analytics remains mounted without an enabled
live provider. Restrict `AnalyticsView`'s global snapshot-empty gate to Now,
Trends, and Charts; Models and Context render independently. Populate active-range
session/provider scope from actual normalized timestamps joined to unsuppressed
sources, including null-model observations for entirely unattributed providers;
never infer continuous activity from source first/last bounds. Use that exact
scope to distinguish global no-session, filter-empty, and no-model-evidence states.

Models uses a compact three-value rail, neutral attributed/unattributed history,
one signal-blue selected-model overlay, a semantic sortable table, and an inline
detail panel. Raw IDs use monospace and remain copyable. Provider badges retain
the established provider palette; models receive no generated colors. Narrow
layouts scroll columns horizontally instead of hiding data.

**Rationale**: Transcript-derived model analytics exists independently of quota
snapshots. The design follows the Glass Cockpit's dense systems-page language and
avoids a misleading rainbow model taxonomy.

**Alternatives considered**: Reusing the current global empty state hides valid
history. Per-model colors fail at unbounded cardinality. Card grids reduce scan
density and sorting clarity.

## R9. Follow deletion intent without retry resurrection

**Decision**: Existing session/project/host deletion removes matching model
observations and marks retained source rows suppressed at their last successful
content fingerprint. An unchanged retry skips suppressed evidence. A genuinely
changed source remains excluded from every inventory-derived scope fact until its
replacement commits successfully and atomically clears suppression. A removed
source is pruned after complete discovery.

**Rationale**: Immediate removal satisfies retention behavior, while the durable
fingerprint prevents a full retry from instantly resurrecting a deliberately
deleted session whose transcript still exists. Allowing changed files to return
matches the existing expectation that new retained activity can be indexed.

**Alternatives considered**: Deleting only observation rows allows the next
backfill to resurrect them. Permanently suppressing a path would hide future
activity after the file changes. Deleting source transcripts would expand the
feature's authority beyond analytics cleanup.

## R10. Validate with existing checks and controlled manual fixtures

**Decision**: Use repository lint, typecheck, build, Rust checks, `lat check`, and
demo-mode manual scenarios under the specification's fixed four-logical-core,
8-GiB, local-SSD benchmark protocol. Record exact hardware, operating system,
build mode, fixture hashes/counts, cache policy, timer boundaries, and raw results.
Do not prescribe new automated test code because project instructions require
explicit user authorization.

**Rationale**: The feature still receives reproducible validation through isolated
fixture directories and observable database/UI outcomes without exceeding the
current request.

**Alternatives considered**: Adding unit or integration tests would be desirable
for parser and aggregation edge cases, but requires a separate explicit request.
