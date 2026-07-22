# Design: Source-Owned Claude and Codex Runtime Analytics

**Date:** 2026-07-21
**Status:** Approved — pending implementation plan

## Problem

Quill discovers each retained parent or sub-agent transcript as a separate file,
but replaces transcript-derived SQLite rows by `(provider, session_id)`. Parent,
sibling, and child files share that session identity, so processing any one file
can erase rows emitted by the others. Session-keyed live-notify coalescing can
also discard simultaneous sibling updates.

The defect affects every table populated by the transcript extraction pass:
`session_events`, `response_times`, `tool_actions`, `skill_usages`, and
`hook_invocations`.

Observed impact confirms this is not theoretical:

- Claude: 81 sessions contain 1,268 retained child sources and 1,349 expected
  chains, but only 80 chains survive in storage, including 26 child chains.
- Codex: all 50,843 `session_events` rows are marked as parent rows with
  `agent_id = NULL`. Four recent retained child rollouts have 513.603 seconds
  of combined lifetime and zero runtime events.

Codex has a second defect. Runtime extraction treats repeated `session_meta`
records as replacement identity, so ancestor metadata restated in a forked
rollout can overwrite the child's identity. It also omits the reasoning and
tool-loop records needed to reconstruct active LLM time.

## Decisions

1. A retained transcript source is the lifecycle owner of every row extracted
   from that source.
2. Replacement is atomic across all five transcript-derived tables.
3. Runtime is additive across chains: five parallel agents active for five
   minutes contribute 25 minutes. This measures total LLM work, not elapsed
   wall-clock time.
4. Existing retained Claude and Codex history is rebuilt automatically once.
5. Existing IPC request and response shapes remain unchanged.

## Source Identity and Schema

Reuse retained-source discovery's canonical, provider-qualified `source_key`.
It is derived from the configured source-root key and canonical filesystem path,
so Claude and Codex cannot collide and callers never invent identity from JSONL
contents.

Rebuild `session_events`, `response_times`, `tool_actions`, `skill_usages`, and
`hook_invocations` so every table has nullable `source_key`, non-empty
`chain_id`, nullable `parent_chain_id`, resolved-root `session_id`, and the
existing agent/project/host attribution its queries require. `session_events`
also requires a non-empty stable `event_key`. Transcript rows require a
non-empty source key. Rows produced outside retained-transcript ingestion,
notably live Codex hook observations, remain source-less and are never touched
by source replacement or inventory pruning. For every new source-less runtime
row, `chain_id` and `session_id` both fall back to the non-empty incoming
session id and `parent_chain_id` is null.

Build `event_key` from a provider-native record, message, response-item, or call
id when one exists, appending content/event ordinal when one native record emits
multiple events. Otherwise use deterministic `record_ordinal:event_ordinal`
within the canonical retained source. Source-less `/sessions/messages` events
use the required incoming message UUID plus event ordinal. Any future
source-less runtime endpoint must supply an equally stable incoming identity;
reject an event that cannot do so. Empty UUID plus timestamp, timestamp alone,
or content hashes are forbidden identity fallbacks.

Use separate partial unique indexes because SQLite treats nulls as distinct:

- `session_events`: source-owned identity is `(provider, source_key,
  event_key)`; source-less identity is `(provider, session_id, event_key)`.
  Chain remains query attribution, not row identity.
- `response_times`: `(provider, source_key, chain_id, timestamp)` for owned
  rows and `(provider, session_id, chain_id, timestamp)` for source-less rows.
- `skill_usages`: `(provider, source_key, message_id, skill_name, skill_path,
  timestamp)` for owned rows; the source-less form substitutes `session_id`.
- `hook_invocations`: `(provider, source_key, chain_id, timestamp,
  hook_identity)` for owned rows; the source-less form substitutes
  `session_id`.
- `tool_actions`: add a required `action_key`, using the provider tool-use/call
  id when present and otherwise deterministic `message_id:block_ordinal`.
  Unique identities are `(provider, source_key, action_key)` for owned rows and
  `(provider, session_id, action_key)` for source-less rows. This avoids both
  duplicate replay and accidental collapse when one message contains repeated
  calls to the same tool.

Every source-owned unique identity therefore includes `source_key`; every
source-less identity is guarded by `WHERE source_key IS NULL`. Index
`(provider, source_key)` on all five tables for replacement and pruning.

Add a provider/source registry, `transcript_analytics_sources`, keyed by
`(provider, source_key)`. It records `source_root_key`, canonical source key,
fast fingerprint (`mtime_ns`, `size_bytes`), `content_sha256`, last-good native
`chain_id`/`parent_chain_id`, resolved analytics root `session_id`, project/cwd,
hostname, last-seen inventory generation, processing status, success/attempt
timestamps, and durable suppression fingerprint/time. Successful snapshot
replacement updates this registry in the same transaction as its child rows.
Failed attempts may update diagnostics but never replace last-good identity or
fingerprint. Suppression survives inventory pruning and blocks unchanged or
changed source contents until an explicit restore workflow removes it.

Add `live_analytics_sessions`, keyed by `(provider, session_id)`, as the durable
origin mapping for source-less analytics. It stores nullable `project`, `cwd`,
and `hostname` plus `updated_at`; upserts merge non-null fields rather than
erasing earlier origin data. `/sessions/messages` writes its project and host.
`/hooks/observed` writes the Codex hook payload's `cwd`. Both mapping upserts
occur in the same transaction as their source-less analytics rows.

The Codex hook payload currently has `session_id`, event/timestamp fields, and
optional `agent_id`, tool/matcher, and `cwd`; it has no project, hostname, or
parent-chain identity. Do not infer missing values. A hook-only session can be
targeted directly by session deletion and by project deletion when its recorded
`cwd` matches that project; host deletion can target it only after another live
endpoint has populated hostname for the same `(provider, session_id)`.

`source_key` owns persistence lifecycle; `chain_id` owns runtime aggregation.
They are deliberately different concepts.

## Provider-Neutral Chain and Root Resolution

Parsing yields native `chain_id` and `parent_chain_id`; storage rows use a
resolved analytics root. Reuse model analytics' provider-neutral cross-source
chain graph, seeded from successfully parsed inventory metadata plus registry
last-known-good identity. Resolve every chain to its topmost known ancestor and
stamp that root into `session_id` on all five tables. A source's own chain id
never changes when an ancestor is discovered.

Startup stages identity for the complete inventory before writing snapshots,
making file order irrelevant. Live reconciliation initially uses the current
registry graph. When a new or corrected parent changes a resolved root, enqueue
every affected descendant for source replacement so its registry row and all
five tables are restamped to the new root. This descendant restamping is
required; sharing only the Codex parser would leave previously ingested child
rows attached to an intermediate parent.

All new `session_events` require a non-empty `chain_id`. `response_times` and
the other source-owned tables carry the same chain/root attribution so session,
sub-agent, project, host, and explicit-deletion paths agree. `is_sidechain`
comes from native chain identity, not from whether root resolution currently
finds an ancestor.

## Provider Parsing

### Claude

Keep Claude's working record-level identity extraction. Parent records use
`sessionId` as the chain; sidechain records use `agentId` as the chain and the
parent session as lineage. Preserve `isSidechain`, `agentId`, `parentUuid`, and
event UUIDs. Feed that native identity into the shared cross-source root graph.
Claude's repair is persistence, root stamping, and scheduling, not a record
parser rewrite.

### Codex

Share the proven two-pass Codex identity resolver and cross-source root graph
with model analytics rather than maintaining another interpretation of
`session_meta`:

- The forked rollout's child metadata establishes `chain_id`.
- `parent_thread_id`, falling back to `forked_from_id`, establishes
  `parent_chain_id`.
- Later metadata that restates an expected ancestor extends ancestry without
  replacing child identity.
- Unrelated metadata is an identity conflict and prevents replacement of the
  source's last-known-good snapshot.

Emit runtime events from Codex text, reasoning, tool-call, and tool-result
records. Map user text to `user_text`, assistant text to `asst_text`, reasoning
to `asst_thinking`, function/custom tool calls to `asst_tool_use`, and their
outputs to `user_tool_result`. Tool results retain that logical role even when
Codex stores them on assistant-side rollout records. Ignore administrative and
metadata records that do not represent LLM work.

## Atomic Source Replacement

Parsing first produces a complete in-memory source snapshot containing native
and resolved identity and rows for all five tables. Compute response/idle
timing in memory within that source and resolved chain; storage never derives
timing by querying rows from an earlier source generation. Only a validated,
successful source parse may enter storage replacement.

One storage entry point, `replace_transcript_analytics_snapshot`, opens one
SQLite transaction and then:

1. Deletes transcript-derived rows matching `(provider, source_key)` from all
   five tables.
2. Inserts the newly extracted rows for all five tables, including valid empty
   sets that intentionally remove stale rows.
3. Upserts the provider/source registry's last-good fingerprint, chain/root
   attribution, seen generation, and status.
4. Commits once.

Any delete or insert failure rolls back the whole transaction, preserving the
previous cross-table snapshot. Per-table delete/insert commits are forbidden
because they expose mixed generations after partial failure.

All table-specific insert helpers are pure transaction helpers: they accept the
caller's transaction, perform inserts only, and never open transactions,
delete, commit, or update the registry. No message-count gate precedes
replacement. A validated successful snapshot with zero messages or zero rows
still calls `replace_transcript_analytics_snapshot` and deletes stale owned
rows; unreadable or invalid input retains last-known-good rows.

Chain-scoped deletion is rejected. A chain is discovered only after parsing,
can have ancestor metadata restated inside its source, and is an analytics
identity rather than a filesystem lifecycle boundary. It cannot safely retain
last-known-good data on parse failure, prune renamed/deleted sources, or prevent
one source from deleting another source's rows. The canonical source is known
before parsing and is the stable replacement boundary.

## Startup, Live Updates, and Reconciliation

Startup and manual scans use the same provider-root inventory and source parser:

1. Allocate a durable generation and inventory canonical retained sources for
   each configured Claude/Codex root.
2. Mark every discovered registry source seen in the generation, including
   unchanged, suppressed, and temporarily failed sources; seen means present,
   not successfully parsed. Stage fingerprints and native identity, then
   resolve the cross-source root graph before row extraction.
3. Preserve unchanged snapshots. Atomically replace each new, changed, or
   root-restamped source snapshot and its last-good registry state.
4. After a root was inventoried completely, prune rows for active registry
   sources under that `source_root_key` whose seen generation is stale. Remove
   their child rows and active registry records in one transaction; retain
   suppressed registry tombstones.

Never prune from a root whose enumeration was incomplete or failed. A missing
file is deletion evidence only within a successful complete-root inventory.

Live notify has two independent queues. Raw/session-keyed search notification
keeps its current Tantivy behavior. Analytics work enters a separate queue only
after provider ownership, canonical root containment, and supported layout
validation produce a canonical source key; that queue coalesces by `(provider,
source_key)`. Repeated notifications for one file collapse while parent and
sibling files remain independent. Validation-unavailable work enters a bounded
retry stage keyed by provider plus the untrusted candidate path, not an
invented source key; only successful validation promotes it to the analytics
queue. Invalid candidates never enter analytics reconciliation. Search and
analytics queue success or failure do not cancel each other.

Explicit session, project, or host deletion remains root-wide. In one
transaction, resolve matching source keys from registry root/project/host
attribution, delete their rows from all five tables, and mark every registry
source durably suppressed. Also delete matching source-less rows from all five
tables. Session deletion uses its requested `(provider, session_id)` directly;
project/cwd and host deletion use only the authoritative session set captured
from `live_analytics_sessions` before any row is removed. Delete matching
origin mappings in the same transaction. Source-less rows with origin fields
the producer never supplied are not guessed into project/host scope; they
remain removable by direct session deletion. Reconciliation checks suppression
before replacement, so retained files cannot recreate deleted analytics.
Source ownership must not narrow user-requested deletion to one transcript
file.

## Failure Semantics

Unreadable files, unavailable roots, unresolved/conflicting source identity,
and source-level parse failures do not delete or replace existing rows. They
retain the last-known-good source snapshot and leave reconciliation pending for
a later retry. Malformed individual records are skipped with bounded
diagnostics when the remaining source has valid identity and forms a usable
snapshot.

The same rules apply during startup, live notify processing, and historical
backfill. No path may delete before it has a replacement snapshot or confirmed
deletion from a complete inventory.

## Migration and Historical Backfill

The migration rebuilds the five tables with source-aware schema and partial
unique indexes. It clears every legacy row from `response_times`,
`session_events`, `tool_actions`, and `skill_usages`, because none records a
reliable owning source. This also clears pre-migration source-less HTTP runtime
rows: they cannot be distinguished safely from transcript rows and the loss is
explicitly accepted. For `hook_invocations`, clear Claude rows because they are
transcript-derived; retain Codex rows because that provider's hook observations
come from the live endpoint, copying them into the rebuilt table with
`source_key = NULL`, `chain_id = session_id`, and null parent lineage.

The same migration creates `transcript_analytics_sources` and
`live_analytics_sessions`, then sets one durable transcript-derived reingest
marker. Pre-migration source-less rows have no trustworthy origin mapping, so
the migration does not fabricate mappings for them. Retained legacy Codex hook
rows are therefore removable by direct session deletion but not by
project/host deletion unless a later live endpoint supplies the missing origin
mapping. Table rebuild, legacy clearing/retention, registry/mapping creation,
marker creation, and schema-version update occur in one migration transaction.

While that marker exists, startup bypasses normal mtime short-circuiting,
inventories both retained roots, and performs source-atomic rebuilds. Each
committed source is independently durable. If Quill exits mid-run, the marker
remains and the next startup safely replays completed sources and continues the
rebuild. Clear the marker only after all available sources were handled and all
required roots completed inventory and pruning. Transient source/root failures
therefore postpone completion without destroying good data.

## Runtime Query and UI Semantics

`get_llm_runtime_stats` keeps its current IPC shape and range behavior. Runtime
reconstruction groups events by provider and resolved chain, computes active
intervals within each chain using existing gap/clamp rules, then sums every
chain. Parent and child intervals are never unioned by wall clock. Existing
parent-only filtering remains available and is defined strictly by
`is_sidechain = 0`, not by null agent or parent ids; the Now headline includes
all chains.

The LLM Runtime card tooltip states that runtime is total parent plus sub-agent
LLM work and that concurrent chains are additive. Session count, turn count,
average, sparkline, response-time breakdown, and sub-agent-tree shapes remain
compatible.

## Verification

Project policy forbids new automated test code unless explicitly requested, so
implementation verification uses existing checks and disposable retained-source
fixtures:

- Run existing Rust tests plus `cargo check` and `cargo test`.
- Run `npm run typecheck`, `npm run lint`, and `npm run build`.
- Exercise parent plus sibling Claude fixtures, a depth-three Codex fork,
  reindexing one sibling, malformed/unreadable sources, complete-root deletion,
  and failed-root non-pruning.
- Interrupt migration after several source commits, restart, and confirm the
  durable marker resumes to completion without duplication.
- Check database invariants: each transcript row has its expected source;
  sibling sources coexist; one-source replacement leaves other sources intact;
  source-owned and source-less partial identities deduplicate independently;
  session events have non-empty stable `event_key`; live message/hook writes
  update source-less origin mappings atomically; project/host deletion uses
  only recorded mapping fields;
  absent sources prune only after complete inventory; suppressed sources do not
  reappear; all Codex child rows carry chain, parent, and resolved-root lineage;
  and a late parent causes descendant restamping.
- Confirm four parallel five-minute chains report 20 minutes and the tooltip
  explains additive semantics.
- Run `lat check` after updating `lat.md/` architecture, schema, indexing, data
  flow, and UI semantics during implementation.

## Non-Goals

- Changing runtime IPC payloads or redesigning the analytics UI.
- Converting total LLM work into wall-clock elapsed time.
- Replacing Tantivy search-document lifecycle in this change; this design owns
  the five SQLite transcript-derived tables.
- Reconstructing historical Codex hooks that were never written to rollouts.
- Adding committed automated test code without a separate explicit request.
