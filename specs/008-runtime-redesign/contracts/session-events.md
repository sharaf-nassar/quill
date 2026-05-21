# Contract — Session Event Ingestion

**Feature**: 008 — Active-Time Runtime Tracking Redesign
**Owner**: `src-tauri/src/sessions.rs` (extractor) and
`src-tauri/src/storage.rs` (ingest)

This contract defines the boundary between the JSONL extractor and the
storage layer. Tests assert each obligation on this page. Any change to
the contract requires a spec amendment.

## Extractor obligations

### `extract_claude_messages_from_jsonl(path: &Path) -> ExtractedSession`

Returns the same `ExtractedSession` struct it returns today, with one
new field:

```rust
pub struct ExtractedSession {
    pub session_id: String,
    pub project_name: Option<String>,
    pub messages: Vec<ExtractedMessage>,   // unchanged
    pub events:   Vec<ExtractedEvent>,     // NEW
}

pub struct ExtractedEvent {
    pub timestamp:    String,                  // RFC3339 with ms precision; never empty
    pub kind:         SessionEventKind,
    pub is_sidechain: bool,
    pub agent_id:     Option<String>,
    pub uuid:         Option<String>,
    pub parent_uuid:  Option<String>,
}

pub enum SessionEventKind {
    UserText,
    UserToolResult,
    AsstText,
    AsstThinking,
    AsstToolUse,
}
```

Obligations:

1. **EVT-CL-1**: Every JSONL line whose top-level `type` is `"user"` or
   `"assistant"` AND whose `isMeta` is not `true` AND whose `timestamp`
   is non-empty MUST yield exactly one `ExtractedEvent`. The classifier
   uses content-block shape per Phase-0 R-C.
2. **EVT-CL-2**: A line whose content is plain string OR an array with
   any non-empty `text` block is classified `*_text`.
3. **EVT-CL-3**: A user line whose content is an array with no non-empty
   `text` block and at least one `tool_result` block is classified
   `user_tool_result`.
4. **EVT-CL-4**: An assistant line whose content is an array with no
   non-empty `text` block and at least one `tool_use` block is
   classified `asst_tool_use`. If the array also contains a
   `tool_use` AND a non-empty `text`, the line is classified
   `asst_text` (the `tool_use` is still extracted into
   `ExtractedMessage.tool_actions` as today).
5. **EVT-CL-5**: An assistant line whose content is an array with only
   `thinking` blocks is classified `asst_thinking`.
6. **EVT-CL-6**: `is_sidechain`, `agent_id`, `uuid`, and `parent_uuid`
   are read from the same JSONL fields used by `ExtractedMessage`
   (`isSidechain`, `agentId`, `uuid`, `parentUuid`). Empty strings
   become `None` for the optional fields, matching existing parser
   conventions.
7. **EVT-CL-7**: Events come out of the extractor in JSONL file order,
   which is the timestamp order for properly written transcripts.
   The storage layer sorts again before inserting to defend against
   clock skew.

### `extract_codex_messages_from_jsonl(path: &Path) -> ExtractedSession`

Same return shape. Codex transcripts emit only `user_text` and
`asst_text` event kinds; tool activity stays inside the existing
`tool_actions` enrichment pipeline (lat.md `data-flow#Session Indexing
Pipeline#Enrichment` is unchanged).

Obligations:

1. **EVT-CX-1**: Apply EVT-CL-1 with `is_sidechain = false`,
   `agent_id = None` (Codex emits no sub-agent transcripts today; the
   sub-agent attribution columns remain wired for the day OpenAI ships
   the feature).

## Storage obligations

### `Storage::ingest_session_events(provider, session_id, events: &[SessionEventInput<'_>]) -> Result<(), String>`

Where:

```rust
pub struct SessionEventInput<'a> {
    pub timestamp:    &'a str,
    pub kind:         SessionEventKind,
    pub is_sidechain: bool,
    pub agent_id:     Option<&'a str>,
    pub uuid:         Option<&'a str>,
    pub parent_uuid:  Option<&'a str>,
}
```

Obligations:

1. **ING-1**: Empty `events` returns `Ok(())` without touching the
   database.
2. **ING-2**: Rows are sorted by `timestamp` before insertion (defense
   in depth against extractor ordering bugs).
3. **ING-3**: A single transaction wraps all inserts for one
   `(provider, session_id)` batch.
4. **ING-4**: `INSERT OR IGNORE INTO session_events (...) VALUES (...)`
   is used so a re-run over the same transcript leaves the table
   byte-identical (SC-005).
5. **ING-5**: Rows where `parse_from_rfc3339(timestamp)` fails are
   skipped silently and logged at `warn`, matching existing
   `ingest_response_times` behavior.

### `Storage::delete_session_events_for_session(provider, session_id) -> Result<(), String>`

Single `DELETE FROM session_events WHERE provider = ?1 AND session_id =
?2`. Called from:

- `process_discovered_file` before re-inserting (mirrors the existing
  delete-then-insert for `tool_actions` / `response_times` /
  `skill_usages`).
- `delete_session_data` IPC handler (FR-012).
- `delete_host_data` and `delete_project_data` cascades.

## Indexer obligations (`process_discovered_file` in `sessions.rs`)

1. **IDX-1**: For each newly discovered or modified file, after
   extraction succeeds, `delete_session_events_for_session` runs once
   and `ingest_session_events` runs once. Both calls take the same
   `discovered.provider` and `extracted.session_id`.
2. **IDX-2**: Failures from either call are logged at `warn` and do
   not abort the rest of the indexing pipeline for that file.
3. **IDX-3**: On boot, when `settings.runtime_event_reingest_pending`
   reads `'1'`, the indexer clears `index_state.json::file_mtimes` for
   both providers and clears the flag, exactly mirroring the existing
   `subagent_reingest_pending` handling.

## Tests required by this contract

See `lat.md/tests.md` section *Active-time runtime tracking
(feature 008)*. The required spec entries are:

- `Extractor emits events for tool_result-only user lines`
- `Extractor emits events for each block of a multi-block assistant generation`
- `Codex extraction emits user_text and asst_text only`
- `Sub-agent transcript events carry is_sidechain=1 and the right agent_id`
- `Re-ingest is idempotent`
- `Session deletion clears session_events`
- `Migration 26 backfill flag re-arms mtime sweep`
