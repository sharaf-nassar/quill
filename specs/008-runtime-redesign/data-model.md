# Phase 1 — Data Model

**Feature**: 008 — Active-Time Runtime Tracking Redesign
**Date**: 2026-05-20

## Overview

One new table, `session_events`, holds the per-event timeline that the
redesigned LLM Runtime card reads. No existing table is altered or
dropped. One migration step (26) creates the table and arms a one-shot
reingest flag.

## Table: `session_events`

Stores one row per non-meta `user` or `assistant` JSONL line from CC and
Codex transcripts. Provider-attributed, sub-agent-aware.

```sql
CREATE TABLE session_events (
    provider     TEXT    NOT NULL,             -- 'claude' | 'codex'
    session_id   TEXT    NOT NULL,             -- parent session UUID
    agent_id     TEXT,                         -- sub-agent id; NULL on parent rows
    is_sidechain INTEGER NOT NULL DEFAULT 0,   -- 0 = parent, 1 = sub-agent
    timestamp    TEXT    NOT NULL,             -- RFC3339 with millisecond precision
    kind         TEXT    NOT NULL,             -- 'user_text' | 'user_tool_result'
                                               -- | 'asst_text' | 'asst_thinking'
                                               -- | 'asst_tool_use'
    uuid         TEXT,                         -- JSONL record uuid (idempotency key)
    parent_uuid  TEXT,                         -- prior message uuid in the chain
    PRIMARY KEY (provider, session_id, COALESCE(agent_id, ''), timestamp, kind)
);

CREATE INDEX idx_se_timestamp
    ON session_events(timestamp);

CREATE INDEX idx_se_chain
    ON session_events(provider, session_id, agent_id, timestamp);

CREATE INDEX idx_se_provider_session_sidechain
    ON session_events(provider, session_id, is_sidechain, timestamp);
```

### Field-by-field

- **provider** — `'claude'` or `'codex'`, mirroring the existing
  `response_times.provider` convention. Required.
- **session_id** — Parent transcript session UUID. For sub-agent
  transcript rows this is the parent's session id, matching the
  on-disk `sessionId` field (the same convention `response_times` and
  `tool_actions` use after migration 20). Required.
- **agent_id** — Sub-agent identifier or `NULL` for parent-transcript
  rows. Distinguishes sibling sub-agent chains so the active-interval
  computation can scope them independently.
- **is_sidechain** — `0` for parent-transcript rows, `1` for sub-agent
  rows. Materially redundant with `agent_id IS NOT NULL` but kept as a
  fast filter to match the existing `parent_only` scope.
- **timestamp** — RFC3339 ISO 8601 with millisecond precision and `Z`
  suffix, identical to the format stored in `response_times.timestamp`.
  Required.
- **kind** — Event classification per R-C. Required. One of the five
  values listed above.
- **uuid** — The JSONL record's `uuid` field, when present. Stored to
  give a stable per-event handle for future use; not currently read by
  any query.
- **parent_uuid** — The JSONL record's `parentUuid` field, when present.
  Lets the sub-agent tree be reconstructed if `response_times` is ever
  retired without losing the parent linkage data.

### Primary key + idempotency

`(provider, session_id, COALESCE(agent_id, ''), timestamp, kind)` is
unique. Combined with `INSERT OR IGNORE` in the ingestion path this
makes a second walk over the same transcript a no-op (FR-009, SC-005).
Including `kind` in the key handles the rare case where two content
blocks of different kinds share a sub-second timestamp (e.g.,
back-to-back streamed assistant blocks).

### Index rationale

- `idx_se_timestamp` covers the `WHERE timestamp >= ?1` lower bound on
  the headline query.
- `idx_se_chain` covers the `ORDER BY provider, session_id, agent_id,
  timestamp` walk used by the active-interval computation.
- `idx_se_provider_session_sidechain` covers the `parent_only` scope
  filter (FR-007), matching the existing
  `idx_rt_provider_session_sidechain` index on `response_times`.

## Migration 26

```text
Migration 26 — session_events
  CREATE TABLE session_events (...);
  CREATE INDEX idx_se_timestamp ...;
  CREATE INDEX idx_se_chain ...;
  CREATE INDEX idx_se_provider_session_sidechain ...;
  INSERT OR REPLACE INTO settings(key, value)
    VALUES ('runtime_event_reingest_pending', '1');
```

Schema version advances from 25 to 26.

### Reingest flag handling

The session indexer reads the flag on startup. When `1`:

1. Walk every project directory under `~/.claude/projects/` and
   `~/.codex/sessions/`. For each JSONL file, drop its entry from
   `index_state.json::file_mtimes`.
2. Clear the flag (`settings.runtime_event_reingest_pending = '0'`).
3. Trigger the existing mtime sweep, which now sees every file as
   newly modified and re-runs `process_discovered_file`. Each file
   ingests one batch of `session_events` rows via `INSERT OR IGNORE`,
   keyed to its session_id.

The flag handler is added to the same `sessions.rs` boot path that
already honors `subagent_reingest_pending` (migration 20) and
`skill_usage_reingest_pending` (migrations 21-22).

## Lifecycle

- **Create**: rows are inserted by `Storage::ingest_session_events`
  after the per-file delete in `process_discovered_file`. Single
  transaction per file, `INSERT OR IGNORE`.
- **Update**: events are immutable. A "change" to a transcript is a
  full re-walk: existing rows for that `(provider, session_id)` are
  deleted first via `delete_session_events_for_session`, then the
  fresh set is inserted.
- **Delete**: session deletion calls
  `delete_session_events_for_session(provider, session_id)`. The
  existing `delete_session_data` IPC command and the
  `delete_host_data`/`delete_project_data` cascades each add this call
  to their fan-out so deletion stays atomic with `tool_actions`,
  `skill_usages`, `response_times`, and `token_snapshots` (FR-012).

## Entity → field mapping (spec terminology → schema)

| Spec entity (key)             | Schema fields                                                          |
|-------------------------------|-------------------------------------------------------------------------|
| Session event                 | one row in `session_events`                                            |
| Chain                         | `(provider, session_id, COALESCE(agent_id, ''))`                       |
| Logical turn                  | computed at query time from a contiguous run of rows on a chain        |
| Idle threshold (5 min)        | constant `IDLE_THRESHOLD_SECS = 300.0` in `storage.rs`                 |
| Tool-wait safety ceiling (6h) | constant `TOOL_WAIT_MAX_SECS = 21_600.0` in `storage.rs`               |

## Validation rules (enforced at insert time)

- `timestamp` MUST parse via `chrono::DateTime::parse_from_rfc3339`.
  Rows that fail parsing are dropped at extraction time (matches the
  existing `response_times` ingestion behavior).
- `kind` MUST be one of the five enum values. The extractor produces no
  other value; an `INSERT OR IGNORE` with an unexpected value is
  treated as a developer error caught by `cargo test`.
- `is_sidechain` MUST be `0` or `1`. The extractor reads the JSONL
  `isSidechain` boolean and stores `1`/`0` accordingly.

## Storage cost estimate

Per-row overhead in SQLite WAL mode for the column shapes above is
roughly 80-110 bytes (SQLite varint encoding + index entries). A heavy
session (1 000 events) lands around 100 KB. A developer machine with
2 000 sessions averaging 200 events is ~32 MB — well under existing
table footprints and inside the spec's stated tolerance (assumption #7).
