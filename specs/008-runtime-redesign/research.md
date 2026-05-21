# Phase 0 — Research & Design Decisions

**Feature**: 008 — Active-Time Runtime Tracking Redesign
**Date**: 2026-05-20
**Status**: Decisions locked. No `NEEDS CLARIFICATION` markers remain.

This document records the six design decisions (R-A through R-F) the
implementation must follow. Each entry states the choice, why it was made,
and which alternatives were considered and rejected.

---

## R-A — Data model: per-event table, additive to response_times

**Decision**: Introduce a new `session_events` table in the SQLite usage
database at migration 26. Keep the existing `response_times` table
unchanged. The new LLM Runtime card queries `session_events`; the existing
`get_session_breakdown` and `get_session_subagent_tree` queries continue
to read from `response_times` so the breakdown view and sub-agent tree
do not regress (FR-011).

**Rationale**: The investigation that motivated this feature established
two root issues that the user→assistant pair model cannot represent
faithfully:

1. The session indexer's message extractor filters out
   `tool_result`-only user messages before the runtime pipeline sees
   them. Surgically un-filtering them inside the parser would also
   require teaching the search index to skip them again — a back-and-
   forth across the same boundary that signals the wrong abstraction.
2. A single API generation arrives as N JSONL lines (typically
   `thinking` + `text` + `tool_use` in quick succession), each of which
   should contribute its timestamp to the active interval. The
   `pending_user.take()` pairing in `ingest_response_times` consumes
   the user_ts on the FIRST of those N lines, so even with tool_results
   restored we would still under-count the within-generation streaming
   gap.

A per-event timeline avoids both issues at the source: every JSONL line
contributes its own timestamp and event kind, and the active-interval
computation is "walk events, split on idle gaps".

The choice to keep `response_times` in place (rather than retiring it
in the same change) caps the blast radius of the migration to one
consumer — the headline card — and lets the Sessions breakdown and
sub-agent tree continue working without any query rewrite in this
feature. A follow-up migration can repoint or retire `response_times`
once the new pipeline has been observed in production.

**Alternatives considered**:

- **Surgical patch to `response_times`**: un-filter `tool_result` in the
  extractor, add a `kind` column to `response_times`, distinguish tool-
  wait from idle gaps in the query. Rejected: leaves the within-
  generation under-count unsolved; requires plumbing `kind` through
  both ingestion and query while still owning a pairing model that
  fundamentally mis-shapes the data.
- **Replace `response_times` outright in this feature**: rewrite the
  three other consumers (`get_session_breakdown`, `get_session_subagent_tree`,
  delete-session flow) in the same change. Rejected: larger diff, more
  regression surface, and the breakdown's `turn_count` aggregate would
  need a new derivation that doesn't add value to this feature's
  headline goal.
- **In-memory recomputation at query time directly from JSONL**: skip
  the SQL table entirely and read transcripts on demand. Rejected: the
  1h query already needs sub-second performance, JSONL is unindexed,
  and the existing architecture funnels transcript work through the
  indexer for good reason.

---

## R-B — Logical-turn semantics: idle vs tool-wait

**Decision**: A "logical turn" is a contiguous run of events on a single
chain `(provider, session_id, agent_id)` where every between-event gap
satisfies at least one of:

- The gap is at most the **idle threshold of 300 seconds (5 minutes)**, OR
- The gap is a **tool-loop gap**: the prior event is `asst_tool_use` and
  the next event is `user_tool_result`, in which case the gap counts
  toward active time up to a **safety ceiling of 6 hours (21 600 s)**.

A gap that fails both conditions ends the current logical turn and
starts a new one. Each turn's contribution to the total is
`last_event_ts - first_event_ts` (millisecond precision, computed in
seconds with floating-point).

**Rationale**: The user expects active time to include long tool waits
(spec acceptance scenario 1.3) and exclude idle gaps (1.2). The only
reliable signal that distinguishes the two in a transcript is the kind
of events that bracket the gap. Five minutes is short enough to
de-couple genuine user-idle from "I was reading the output" pauses, and
long enough to absorb typical between-prompt thought time. The six-hour
ceiling on tool-wait gaps guards against clock skew and stuck
processes; the longest legitimate single tool call observed in the
target environment (a multi-hour `/qbuild` orchestration) fits inside
this ceiling.

**Alternatives considered**:

- **Single uniform threshold across all gaps**: any gap over T seconds
  splits, regardless of kind. Rejected: a 15-minute Bash command would
  incorrectly split a single agent loop into two turns, violating
  acceptance scenario 1.3.
- **No ceiling on tool waits**: count any tool gap fully. Rejected:
  clock skew or a stuck process would inject an unbounded value into
  the sum. The 6-hour ceiling matches the existing Codex
  `response_max_secs` cap and is empirically sufficient.
- **300 s vs 600 s idle threshold**: the existing `idle_secs` cap in
  `response_times` is 600 s. We chose 300 s for the new semantics
  because the spec asks the metric to track active engagement, and a
  5-minute idle gap is already past the point where the user has
  context-switched. Both values pass the success criteria; 300 s gives
  tighter "active" semantics.

---

## R-C — Event kind classification

**Decision**: Events are classified into exactly five kinds at extraction
time, derived from JSONL message type plus content-block shape:

| Kind                 | JSONL `type` | Content shape                                |
|----------------------|--------------|----------------------------------------------|
| `user_text`          | `user`       | Plain string OR array containing a non-empty `text` block |
| `user_tool_result`   | `user`       | Array whose blocks include `tool_result` and no non-empty `text` block |
| `asst_text`          | `assistant`  | Plain string OR array containing a non-empty `text` block |
| `asst_thinking`      | `assistant`  | Array whose blocks include `thinking` and no `text`/`tool_use` block |
| `asst_tool_use`      | `assistant`  | Array whose blocks include `tool_use` and no non-empty `text` block |

When an assistant line carries both `text` and `tool_use` (rare but
seen — a final summary alongside the next tool call), it is classified
`asst_text`; the `tool_use` is still extracted into `tool_actions` as
today. Codex transcripts emit `user_text` and `asst_text` only (its
tool activity lives on assistant-side records, treated as `asst_text`
for runtime purposes; the existing `tool_actions` enrichment is
unaffected).

**Rationale**: Five kinds are the minimum needed for the R-B
discrimination (tool-loop vs idle gap). A simpler `{user, asst}` would
not distinguish the gap kinds; a finer taxonomy (separating `thinking`
from `text`) would not change any metric the spec asks for. The
classification is stable under the existing extractor's enrichment
pipeline — `tool_actions` and `skill_usages` continue to read from the
same parse pass and continue to work as today.

**Alternatives considered**:

- **Two kinds, `user` and `asst`**: rejected, breaks R-B's ability to
  distinguish tool waits from idle gaps.
- **Cross-link tool_use IDs to their tool_result**: useful for future
  analytics but not required by this feature. Postponed.

---

## R-D — Ingestion path: dual emission from the extractor

**Decision**: `extract_claude_messages_from_jsonl` and
`extract_codex_messages_from_jsonl` each return an extended
`ExtractedSession` carrying both `messages: Vec<ExtractedMessage>`
(unchanged — filtered for Tantivy search index) and
`events: Vec<ExtractedEvent>` (new — every non-meta `user`/`assistant`
line with a non-empty `timestamp`, classified by R-C). The session
indexer (`process_discovered_file` in `sessions.rs`) now also calls
`storage.delete_session_events_for_session` and
`storage.ingest_session_events` alongside its existing wipe-and-reinsert
of `tool_actions`, `skill_usages`, and `response_times`. `INSERT OR
IGNORE` matches the existing `response_times` ingestion idempotency
contract.

**Rationale**: Reusing the existing parse pass keeps the indexer's hot
path single-traversal. Separating the two output streams lets the
search index stay lean (it can keep skipping tool_results and isMeta
records) while the runtime pipeline gets the full event log.
Hooking into `process_discovered_file` reuses the existing notify-
driven incremental update path with no new background machinery.

**Alternatives considered**:

- **Two parse passes**: rejected, doubles disk I/O on the hot path.
- **Compute events from `messages` post-extraction**: rejected, the
  filter at sessions.rs line 1941-1943 has already discarded
  tool_result-only records by then.

---

## R-E — Backfill mechanism

**Decision**: Migration 26 creates the table and sets a
`runtime_event_reingest_pending` flag in `settings` (matching the
pattern from migrations 20-22). On app start, the existing session
indexer reads this flag; when set, it clears `file_mtimes` in
`index_state.json` for both providers so every transcript is re-read
on the next mtime sweep. The reingest path is unchanged — it just
runs once on the fresh table.

**Rationale**: This is the same backfill mechanism Quill already uses
for response_times (migration 20), tool_actions (migration 20), and
skill_usages (migrations 21 and 22). It is incremental (one file per
mtime-sweep iteration), idempotent (`INSERT OR IGNORE`), and uses no
new threads or background tasks (FR-008 + assumption #6).

For users with a large transcript history, the existing notify-driven
ingest path tops off freshly active sessions ahead of the catch-up
sweep, so the LLM Runtime card lights up for "this week" within minutes
of upgrade and fully fills in shortly after (SC-004).

**Alternatives considered**:

- **One-shot synchronous backfill at migration time**: rejected, would
  block app start on machines with hundreds of transcripts.
- **Spawn a dedicated background backfill thread**: rejected, the
  existing indexer is already the right place; adding another thread
  duplicates work and risks contention on the SQLite writer lock.

---

## R-F — Documentation, copy, and lat.md updates

**Decision**: The same change set updates the InsightCard description
on `NowTab.tsx` and the four lat.md sections that describe the metric
or the schema:

- `lat.md/backend.md` Schema — add a `session_events` bullet between
  Code/Runtime Metrics and Metadata; update the Metadata bullet to
  note migration 26 and the `runtime_event_reingest_pending` flag.
- `lat.md/backend.md` Tauri IPC Commands #Code and Response Stats —
  update `get_llm_runtime_stats` description to source from
  `session_events` with R-B semantics; keep the `(range, scope)`
  contract note.
- `lat.md/data-flow.md` Session Indexing Pipeline — describe dual
  emission (messages + events) from the parser.
- `lat.md/features.md` Now Tab — update the LLM Runtime sentence to
  describe active time (model + tool execution) and exclude user-idle.

`lat check` is run before commit; any code refs in `@lat:` comments
introduced by the redesign point at the new functions in
`storage.rs` and `sessions.rs`.

**Rationale**: The CLAUDE.md project rules require lat.md to stay in
sync with the codebase, with `lat check` passing. Coupling the doc
update to the code change in one diff prevents documentation drift
(spec SC-007).

**Alternatives considered**:

- **Update docs in a follow-up PR**: rejected, contradicts CLAUDE.md.

---

## Open questions

None. All decisions are locked.
