# Data Model: Session Model Analytics

The model stores replayable source evidence, reconciliation state, and one
application-level backfill state without creating a model catalog.

## Design invariants

- A model identity is the binary, case-sensitive pair `(provider, raw_model_id)`.
- `raw_model_id` is nullable on observations; null represents missing or rejected
  evidence and never becomes an `unknown` model row.
- Token dimensions are independently nullable. Null means unavailable; zero is an
  observed zero.
- Existing token and session tables remain unchanged and authoritative for their
  current views.
- Every source replacement is atomic and scoped by `(provider, source_key)`.
- A session can own any number of provider-qualified observations; no
  authoritative single-model session column or model-count cap exists.
- Observation lifetime follows retained source/session lifecycle. The `30D` range
  is a query boundary, not a storage TTL.
- Derived switch and primary-model values are queried, not persisted.

## Entity: `model_usage_observations`

One row represents a normalized turn or token-usage fact from one retained source
record.

| Field | SQLite type | Null | Meaning |
|---|---|---:|---|
| `id` | INTEGER PRIMARY KEY | No | Internal row key |
| `provider` | TEXT | No | Existing Quill session-provider vocabulary |
| `source_key` | TEXT | No | Stable normalized source identity, normally canonical transcript path |
| `source_record_key` | TEXT | No | Deterministic record identity within one complete parse |
| `source_ordinal` | INTEGER | No | Monotonic source order used after timestamp |
| `observation_kind` | TEXT | No | `turn` or `token` |
| `source_session_id` | TEXT | No | Native session/thread identifier from this source |
| `analytics_session_id` | TEXT | No | Resolved root session used for session aggregates |
| `chain_id` | TEXT | No | Parent or subagent/thread chain partition |
| `parent_chain_id` | TEXT | Yes | Immediate parent when available |
| `is_sidechain` | INTEGER | No | Boolean parent/subagent distinction |
| `agent_id` | TEXT | Yes | Provider-native subagent identity when available |
| `turn_id` | TEXT | Yes | Provider-native turn identity when available |
| `observed_at_ms` | INTEGER | No | UTC Unix milliseconds |
| `raw_model_id` | TEXT | Yes | Accepted exact model ID after surrounding trim |
| `input_tokens` | INTEGER | Yes | Reliable nonnegative input count |
| `output_tokens` | INTEGER | Yes | Reliable nonnegative output count |
| `cache_creation_tokens` | INTEGER | Yes | Reliable nonnegative cache-write count |
| `cache_read_tokens` | INTEGER | Yes | Reliable nonnegative cache-read count |
| `model_evidence` | TEXT | No | `explicit`, `missing`, or `invalid` |
| `token_evidence` | TEXT | No | `direct`, `cumulative_delta`, or `unavailable` |
| `cwd` | TEXT | Yes | Session project context for display/deletion scope |
| `hostname` | TEXT | Yes | Host context for display/deletion scope |

Unique constraint: `(provider, source_key, source_record_key)`.

`source_record_key` uses provider shape plus line/record position and a
within-record observation index. It only needs to be stable for repeated parsing
of identical content because a changed source is replaced as a unit.

### Observation validation

- Trim surrounding Unicode whitespace from a model string.
- Accept 1–256 Unicode scalar values and reject any control character.
- Preserve accepted case, punctuation, separators, aliases, and version text.
- Accept a token dimension only when it is an integer in `0..=100_000_000`.
- Preserve valid dimensions when sibling dimensions are absent or invalid.
- Require a valid timestamp and source/session/chain identity for a row. Skip one
  malformed record without aborting the source.
- A recognized turn with missing/invalid model evidence still emits a null-model
  `turn` so it can break switch adjacency.
- A reliable token record without model evidence emits a null-model `token` so it
  remains in coverage.

### Codex cumulative normalization

Maintain a prior valid baseline independently for input, output, cache-creation,
and cache-read totals. The first valid value and any value below its baseline
contribute their full current amount from zero; a monotonic value contributes its
difference. Missing/invalid dimensions emit null and preserve their baselines.
Emit a row only when at least one delta is positive, retaining valid zero siblings
when another dimension changes. Reset warnings are local diagnostics only.

### Indexes

- `(observed_at_ms, provider)` for range and provider scope.
- `(provider, raw_model_id, observed_at_ms)` for model totals and history.
- `(provider, analytics_session_id, observed_at_ms)` for session summaries.
- `(provider, analytics_session_id, chain_id, observed_at_ms, source_ordinal)` for
  ordered chain history.
- `(provider, source_key)` for source-scoped replacement and removal.

## Entity: `model_observation_sources`

One row tracks discovery, ownership, fingerprints, and last-known-good state for
one transcript source, including sources with no accepted model IDs.

| Field | SQLite type | Null | Meaning |
|---|---|---:|---|
| `provider` | TEXT | No | Source provider; part of primary key |
| `source_root_key` | TEXT | No | Stable owning provider-root identity used for root-scoped pruning |
| `source_key` | TEXT | No | Stable source identity; part of primary key |
| `source_path` | TEXT | No | Local path used for discovery and diagnostics |
| `source_session_id` | TEXT | Yes | Native session/thread identifier |
| `analytics_session_id` | TEXT | Yes | Resolved root session |
| `chain_id` | TEXT | Yes | Native parent/subagent chain |
| `parent_chain_id` | TEXT | Yes | Immediate parent chain when known |
| `is_sidechain` | INTEGER | No | Boolean parent/subagent distinction |
| `agent_id` | TEXT | Yes | Provider-native subagent identity |
| `cwd` | TEXT | Yes | Project context |
| `hostname` | TEXT | Yes | Host context |
| `first_activity_at_ms` | INTEGER | Yes | Earliest supported session activity |
| `last_activity_at_ms` | INTEGER | Yes | Latest supported session activity |
| `mtime_ns` | INTEGER | Yes | Fast file-change fingerprint |
| `size_bytes` | INTEGER | Yes | Fast file-change fingerprint |
| `content_sha256` | TEXT | Yes | Successful full-content fingerprint |
| `seen_generation` | INTEGER | No | Latest complete/in-progress inventory generation |
| `processing_status` | TEXT | No | `pending`, `ok`, `stale`, `failed`, or `suppressed` |
| `observation_count` | INTEGER | No | Rows from last successful replacement |
| `last_attempt_at_ms` | INTEGER | Yes | Most recent processing attempt |
| `last_success_at_ms` | INTEGER | Yes | Most recent committed replacement |
| `last_error` | TEXT | Yes | Bounded diagnostic safe for display/logging |
| `suppressed_sha256` | TEXT | Yes | Deleted analytics fingerprint that must not reappear unchanged |
| `suppressed_at_ms` | INTEGER | Yes | Existing deletion/retention action time |

Primary key: `(provider, source_key)`.

Indexes cover `(provider, first_activity_at_ms, last_activity_at_ms)`,
`analytics_session_id`, `cwd`, `hostname`, and
`(provider, source_root_key, seen_generation)`.

### Fingerprint and suppression rules

1. Equal mtime/size on an `ok` source is an unchanged fast-path.
2. Changed fast metadata triggers a full read and SHA-256 calculation.
3. Equal content hash remains a no-op while fast metadata is refreshed.
4. Deletion removes observations and records the last successful hash as
   `suppressed_sha256`.
5. A scan whose hash still equals `suppressed_sha256` keeps the source suppressed.
6. A different hash is parsed while the source remains suppressed. Only a
   successful replacement transaction clears suppression and returns the source
   to analytics scope.
7. A missing source is removed only within its persisted `source_root_key` after
   that exact provider root was enumerated completely; incomplete discovery never
   proves deletion for that root.

## Entity: `model_backfill_state`

One singleton row (`id = 1`) describes initial history recovery and later retries.

| Field | SQLite type | Null | Meaning |
|---|---|---:|---|
| `id` | INTEGER PRIMARY KEY CHECK(id = 1) | No | Singleton identity |
| `generation` | INTEGER | No | Monotonic inventory generation |
| `trigger` | TEXT | No | `migration`, `startup_resume`, `retry`, or `reconcile` |
| `status` | TEXT | No | `pending`, `running`, `complete`, `partial`, or `failed` |
| `total_roots` | INTEGER | No | Configured supported transcript roots attempted this run |
| `completed_roots` | INTEGER | No | Roots whose enumeration completed; an absent optional root counts as empty/completed |
| `failed_roots` | INTEGER | No | Roots whose contents could not be enumerated |
| `inventory_complete` | INTEGER | No | Boolean; terminal run enumerated every configured root and attempted every discovered source |
| `total_sources` | INTEGER | No | Sources discovered for this run |
| `source_total_published` | INTEGER | No | Internal boolean distinguishing an authoritative zero-source inventory from a total not yet published |
| `processed_sources` | INTEGER | No | Changed sources committed successfully |
| `failed_sources` | INTEGER | No | Sources unreadable or invalid at source level |
| `skipped_sources` | INTEGER | No | Unchanged or unchanged-suppressed sources |
| `remaining_sources` | INTEGER | No | Discovered sources not yet attempted |
| `observations_written` | INTEGER | No | Rows committed during this run |
| `started_at_ms` | INTEGER | Yes | Current run start |
| `updated_at_ms` | INTEGER | No | Last committed progress update |
| `finished_at_ms` | INTEGER | Yes | Terminal-state time |
| `last_error` | TEXT | Yes | Bounded run-level diagnostic |

Migration 28 creates the row as `pending` with `inventory_complete = 0`. Setup
converts an interrupted `running` row to `pending` before scheduling. Pending,
running, and interrupted states are never inventory-complete. Only one process
worker can move the row to `running`; retry during a running pass returns the
current row. A terminal inventory-complete claim requires at least one configured
root and `source_total_published = 1`; publishing zero sources is valid and
idempotent, but it cannot later change within the same generation.

`failed_sources` counts discovered sources that could not be read or parsed.
`failed_roots` counts roots that could not be enumerated, whose undiscovered
source count is unknowable. A terminal `partial` state may still have
`inventory_complete = 1` when every root completed but some discovered sources
failed. Retry resets run counters and completeness, never observations or source
rows.

### State transitions

```text
pending ──start──> running ──all readable──> complete
   ^                  ├────some failure────> partial
   │                  └────no progress─────> failed
   └──retry/resume──── complete | partial | failed
```

Previously committed observations remain queryable in every state.

## Relationships and ownership

```text
model_observation_sources (provider, source_key)
    1 ─────────── * model_usage_observations (provider, source_key)

analytics_session_id
    1 ─────────── * chain_id
    1 ─────────── * provider-qualified model identities
```

There is no foreign-key model entity. `raw_model_id` values become discoverable
through `SELECT DISTINCT provider, raw_model_id` over accepted observations.

The source/observation relationship is logical rather than dependent on SQLite
foreign-key enforcement. Migration 28 does not change the connection-wide foreign
key pragma. Source replacement and pruning explicitly delete matching observation
children before updating or deleting the source row in the same transaction.

Session/project/host deletion finds matching source rows, explicitly deletes
their observations, and leaves fingerprinted suppression rows when retained files
still exist. Physical source pruning deletes observations before the source row.

## Source replacement transaction

Parsing and hashing happen before a write transaction. A successful transaction:

1. Delete observations for exactly `(provider, source_key)`.
2. Insert the complete normalized replacement set.
3. Upsert source metadata, fingerprints, activity range, generation, and counts.
4. Clear any obsolete error and, only now, changed-content suppression.
5. Commit, then publish progress/data notification.

Read or parse failure before the transaction preserves prior observations, marks
the source `stale` or `failed`, and contributes to partial backfill status.

## Derived analytics

### Observation token amount

For a token-bearing row, sum each present token dimension. A row is token-bearing
when at least one dimension is non-null. Null dimensions contribute no invented
zero, while explicit zeros remain observed values.

### Attribution coverage

```text
total = sum(token amount for every token-bearing row in scope)
attributed = sum(token amount where raw_model_id is non-null)
unattributed = total - attributed
coverage = null when total = 0, else 100 * attributed / total
```

Codex token observations normally remain unattributed even when tokenless
turn-context rows name a model. This can yield model turns and sessions with zero
attributed tokens and 0% provider coverage; that result is intentional.

### Cache-read share

For one model, require input, cache-creation, and cache-read dimensions on every
token observation contributing to the share. If any required dimension is absent,
or their aggregate denominator is zero, return null. Otherwise:

```text
cache_read / (input + cache_creation + cache_read)
```

### Model switches

Use only `turn` rows partitioned by provider, analytics session, and chain.
Order by `observed_at_ms`, `source_ordinal`, and `source_record_key`. A null model
resets the previous identity. Count a switch only when the current and previous
non-null provider-qualified identities differ. Token rows do not participate.

### Primary model

Within the active range, rank session identities by:

1. Attributed token amount descending.
2. Observed model turns descending.
3. Provider ascending under SQLite `BINARY` collation.
4. Raw model ID ascending under SQLite `BINARY` collation.

Valid UTF-8 preserves Unicode scalar lexicographic order under `BINARY`, matching
the specification's case-sensitive, locale-independent identifier order.

### Scope and empty-state facts

Global session count is the number of distinct `(provider, analytics_session_id)`
pairs across all unsuppressed retained source activity, independent of the active
range/provider. Scoped session count uses distinct pairs with a normalized
observation timestamp inside the half-open active interval, joined to unsuppressed
source ownership. Scoped evidence count uses accepted non-null observations in
that same scope. Represented providers use any in-range normalized observation,
including null-model observations, so an entirely unattributed provider remains
filterable. A source's first/last interval overlap is not treated as continuous
activity. Stale/failed last-known-good sources remain included; suppressed sources
contribute no scope, filter, row, or empty-state facts.

The response reads persisted `inventory_complete`; it never derives completeness
from terminal status because `partial` can represent either complete discovery
with failed sources or incomplete root discovery.

Final empty-state claims additionally require `status = complete`, zero failed
roots/sources, and zero remaining sources. Any other combination exposes recovered
data with provisional/incomplete scope rather than a final empty claim.
