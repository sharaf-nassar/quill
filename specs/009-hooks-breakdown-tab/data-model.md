# Phase 1 Data Model: hook_invocations

## Overview

One new SQLite table — `hook_invocations` — captures every observed hook
fire from both providers. Migration 27 creates the table and sets the
`hook_invocation_reingest_pending` settings flag so existing Claude
transcripts backfill on the next startup sweep. The table is parallel to
`skill_usages` in shape, indexing, and lifecycle. No changes to existing
tables.

## Table: `hook_invocations`

```sql
CREATE TABLE IF NOT EXISTS hook_invocations (
    provider           TEXT    NOT NULL,
    session_id         TEXT    NOT NULL,
    agent_id           TEXT,
    is_sidechain       INTEGER NOT NULL DEFAULT 0,
    timestamp          TEXT    NOT NULL,
    hook_event         TEXT    NOT NULL,
    hook_matcher       TEXT,
    tool_name          TEXT,
    hook_identity      TEXT    NOT NULL,
    script_command_raw TEXT,
    exit_code          INTEGER,
    duration_ms        INTEGER,
    cwd                TEXT,
    hostname           TEXT,
    message_id         TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS uidx_hook_invocations_identity
    ON hook_invocations(
        provider,
        session_id,
        COALESCE(agent_id, ''),
        timestamp,
        hook_identity
    );

CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_ts
    ON hook_invocations(provider, timestamp);

CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_session
    ON hook_invocations(provider, session_id);

CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_ts
    ON hook_invocations(hook_identity, timestamp);

CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_cwd
    ON hook_invocations(hook_identity, cwd);
```

### Field-by-field

| Field | Type | Source | Notes |
|-------|------|--------|-------|
| `provider` | TEXT | ingest path | `"claude"` or `"codex"` |
| `session_id` | TEXT | Claude JSONL `sessionId` (parent transcript id even for sub-agent rows) / Codex stdin `session_id` | Matches the `session_id` used by `session_events` and `skill_usages` |
| `agent_id` | TEXT NULL | sub-agent JSONL filename (`agent-*.jsonl`) | NULL for parent-transcript rows; matches the carry pattern from `session_events` |
| `is_sidechain` | INT | 1 when extracted from a sub-agent transcript, 0 otherwise | Matches `session_events` |
| `timestamp` | TEXT | Claude attachment `timestamp` / Codex stdin `ts` (ISO-8601 with offset) | Used for both query filtering and uniqueness |
| `hook_event` | TEXT | Claude `hookEvent` / Codex `hook_event` | One of `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `PreCompact`, `PostCompact`, `PermissionRequest` |
| `hook_matcher` | TEXT NULL | parsed from `hookName` (e.g., `:startup` in `SessionStart:startup`, `:Bash` in `PreToolUse:Bash`) | NULL when no matcher |
| `tool_name` | TEXT NULL | Codex stdin `tool_name` (for PreToolUse/PostToolUse) / Claude `tool_name` if present on the attachment | NULL when not applicable |
| `hook_identity` | TEXT | canonicalization per FR-003 / research R-D | The aggregation key for breakdown rows |
| `script_command_raw` | TEXT NULL | Claude attachment `command` verbatim | Preserved for forensic drilldown; NULL on older Claude transcripts where `command` is absent and on Codex (event-scoped) |
| `exit_code` | INT NULL | Claude attachment `exitCode` | NULL on Codex (event observed before exec result is known) |
| `duration_ms` | INT NULL | Claude attachment `durationMs` | NULL on Codex |
| `cwd` | TEXT NULL | Claude JSONL top-level `cwd` / Codex stdin `cwd` | Used by per-project drilldown indices and per-cwd deletion |
| `hostname` | TEXT NULL | resolved at ingest time via the same `SessionIndex#local_hostname` helper that `skill_usages` already uses | |
| `message_id` | TEXT NULL | Claude attachment `parentUuid` (the assistant message whose tool use spawned the hook, when applicable) | NULL on Codex |

### Primary key + idempotency

The UNIQUE index over `(provider, session_id, COALESCE(agent_id, ''),
timestamp, hook_identity)` makes inserts idempotent. Reingesting an entire
Claude transcript replays the same `(timestamp, hook_identity)` tuples;
`INSERT OR IGNORE` (the pattern already used by
`store_skill_usages_for_messages`) skips duplicates without error. The
Codex observer endpoint uses the same `INSERT OR IGNORE` so duplicate POSTs
during retries are absorbed silently.

### Index rationale

- `(provider, timestamp)` — powers the timeframe-scoped breakdown query
  (Now-tab 1h/24h/7d/30d).
- `(provider, session_id)` — powers per-session deletion and per-session
  drilldowns from the Sessions breakdown.
- `(hook_identity, timestamp)` — powers the per-row "last fired" relative
  timestamp without a table scan.
- `(hook_identity, cwd)` — keeps the door open for per-cwd drilldown if
  v2 adds disclosure rows (already supported by the migration-22 precedent
  on `skill_usages`).

## Migration 27

```rust
if current_version < 27 {
    let tx = conn.transaction()
        .map_err(|e| format!("Migration 27 (hook_invocations): {e}"))?;

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS hook_invocations (
            provider           TEXT NOT NULL,
            session_id         TEXT NOT NULL,
            agent_id           TEXT,
            is_sidechain       INTEGER NOT NULL DEFAULT 0,
            timestamp          TEXT NOT NULL,
            hook_event         TEXT NOT NULL,
            hook_matcher       TEXT,
            tool_name          TEXT,
            hook_identity      TEXT NOT NULL,
            script_command_raw TEXT,
            exit_code          INTEGER,
            duration_ms        INTEGER,
            cwd                TEXT,
            hostname           TEXT,
            message_id         TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_hook_invocations_identity
            ON hook_invocations(provider, session_id, COALESCE(agent_id, ''),
                                timestamp, hook_identity);
        CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_ts
            ON hook_invocations(provider, timestamp);
        CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_session
            ON hook_invocations(provider, session_id);
        CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_ts
            ON hook_invocations(hook_identity, timestamp);
        CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_cwd
            ON hook_invocations(hook_identity, cwd);"
    ).map_err(|e| format!("Migration 27 (hook_invocations DDL): {e}"))?;

    tx.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params!["hook_invocation_reingest_pending", "1"],
    ).map_err(|e| format!("Migration 27 (set reingest flag): {e}"))?;

    tx.execute("INSERT INTO schema_version (version) VALUES (27)", [])
        .map_err(|e| format!("Failed to record migration 27: {e}"))?;

    tx.commit()
        .map_err(|e| format!("Migration 27 (commit): {e}"))?;
}
```

### Reingest flag handling

The boot-time sweep at the bottom of `src-tauri/src/sessions.rs` already
reads pending reingest flags (skills, runtime events). The new flag joins
that handler:

```rust
let hook_reingest_pending = storage
    .get_setting("hook_invocation_reingest_pending")
    .ok()
    .flatten()
    .map(|v| v == "1")
    .unwrap_or(false);

// ... within the per-file extraction loop ...
if hook_reingest_pending || hook_seen_first_time(file) {
    let invocations = extract_hook_invocations_from_attachments(records);
    storage.store_hook_invocations_for_messages(provider, &invocations)?;
}

// ... after a clean sweep completes ...
if hook_reingest_pending {
    storage.set_setting("hook_invocation_reingest_pending", "")?;
}
```

The flag stays set if the sweep aborts mid-way (mirroring the existing
runtime-events flag), so the next boot retries.

## Lifecycle

| Trigger | Action |
|---------|--------|
| Migration 27 runs | Create table + indices, set `hook_invocation_reingest_pending = "1"` |
| Boot-time sweep (mtime walk) | Replay the attachment extractor for every Claude JSONL when the flag is set; clear flag on clean completion |
| Live Claude indexing (`POST /api/v1/sessions/notify` or `/messages`) | Extract attachment records inside the existing per-file batch; insert via `store_hook_invocations_for_messages` |
| Live Codex hook fire | `hook-observe.cjs` POSTs to `/api/v1/hooks/observed`; server validates, fast-acks, persists on background blocking task |
| Session deletion (`delete_session_data(provider, session_id)`) | Cascade `DELETE FROM hook_invocations WHERE provider = ?1 AND session_id = ?2` alongside the existing per-table deletions |
| Per-cwd cleanup (`delete_project_data(cwd)`) | Cascade `DELETE FROM hook_invocations WHERE cwd = ?1` alongside `skill_usages` cleanup |
| Reindex of a single file (`reindex_session_file`) | Delete-and-rebuild per the existing per-file pattern; the UNIQUE index keeps duplicates impossible |

## Entity → field mapping (spec terminology → schema)

| Spec entity / attribute | Schema column |
|-------------------------|--------------|
| Hook Invocation provider | `provider` |
| Hook Invocation session id | `session_id` (+ `agent_id`, `is_sidechain` for sub-agent rows) |
| Hook Invocation hook event name | `hook_event` |
| Hook Invocation hook matcher / tool name | `hook_matcher` + `tool_name` |
| Hook Invocation canonicalized script command | `hook_identity` (canonicalized) + `script_command_raw` (verbatim) |
| Hook Invocation timestamp | `timestamp` |
| Hook Invocation working directory | `cwd` |
| Hook Invocation host | `hostname` |
| Hook Identity | `hook_identity` |

## Validation rules (enforced at insert time)

- `provider` ∈ `{"claude", "codex"}`. Reject otherwise (400 from the HTTP
  endpoint; assertion on the Rust ingest path).
- `hook_event` MUST be one of the eight known events. Reject unknown
  values to avoid silent drift.
- `timestamp` MUST parse as ISO-8601 with offset. Reject malformed
  timestamps.
- `session_id` length ≤ 128 (matches the constraint on `skill_usages`).
- `hook_identity` length ≤ 256.
- `script_command_raw` length ≤ 2048 (truncate on insert; matches the
  truncation behavior in `observe.cjs`).

## Storage cost estimate

A heavy developer machine fires hooks roughly:

- 1× `SessionStart` per session start (~5 sessions/day) = ~150/month
- 1× `UserPromptSubmit` per user turn (~100 turns/day) = ~3000/month
- 1× `PreToolUse` + 1× `PostToolUse` per tool call (~1000 tool calls/day)
  = ~60000/month
- 1× `Stop` per session end = ~150/month

Plus the Quill-deployed multipliers (2-3× per event because Quill
registers multiple scripts per event slot on each provider). Rough total:
~200k rows/month per heavy machine. At ~120 bytes per row that's ~24 MB
per month per heavy machine — comfortably inside SQLite's range. The 90-
day rolling window the spec discusses is therefore safely under 100 MB.
