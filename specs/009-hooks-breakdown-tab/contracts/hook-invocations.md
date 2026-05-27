# Contract: Hook Invocation Extraction and Storage

This contract defines how hook fires are extracted from Claude transcripts,
how rows are inserted into the `hook_invocations` table, and the storage
interface exposed to the rest of the backend.

## 1. Claude transcript extraction

### Input

JSONL records of the shape produced by Claude Code's harness:

```json
{
  "parentUuid": "<assistant message uuid that triggered the hook, may be null>",
  "isSidechain": false,
  "attachment": {
    "type": "hook_success" | "hook_failure" | "hook_timeout" | "hook_blocked",
    "hookName": "PreToolUse:Bash",
    "toolUseID": "<tool use id when PreToolUse/PostToolUse>",
    "hookEvent": "PreToolUse",
    "stdout": "...",
    "stderr": "...",
    "exitCode": 0,
    "command": "${CLAUDE_PLUGIN_ROOT}/hooks-handlers/session-start.sh",
    "durationMs": 145
  },
  "type": "attachment",
  "uuid": "<attachment record uuid>",
  "timestamp": "2026-05-22T22:09:15.299Z",
  "sessionId": "<session uuid>",
  "cwd": "/home/mamba/work/quill"
}
```

Sub-agent records carry `isSidechain: true` and live under
`<projectSlug>/<session-uuid>/subagents/agent-*.jsonl`. The `sessionId`
field still holds the parent session id (matches `session_events` and
`skill_usages` behavior).

### Function

```rust
pub fn extract_hook_invocations_from_attachment(
    record: &TranscriptRecord,
    session_cwd: Option<&str>,
    hostname: &str,
) -> Option<HookInvocationInput>;
```

Returns `Some` exactly when `record.type == "attachment"` AND
`record.attachment.type` begins with `hook_`. Returns `None` for every
other record type (the dual-emission loop continues without action).

### Field mapping (Claude)

| Source | Destination |
|--------|-------------|
| `record.sessionId` | `session_id` |
| sub-agent filename match → `agent-(<uuid>).jsonl` | `agent_id` (NULL when missing) |
| `record.isSidechain ? 1 : 0` | `is_sidechain` |
| `record.timestamp` | `timestamp` |
| `record.attachment.hookEvent` | `hook_event` |
| portion of `record.attachment.hookName` after the first `:` | `hook_matcher` (NULL when no colon) |
| `record.attachment.tool_name` if present, else parsed from `hookName` matcher when the matcher equals a known tool | `tool_name` |
| canonicalized `record.attachment.command` per R-D | `hook_identity` |
| `record.attachment.command` verbatim, truncated to 2048 chars | `script_command_raw` |
| `record.attachment.exitCode` | `exit_code` |
| `record.attachment.durationMs` | `duration_ms` |
| `record.cwd` (top-level) | `cwd` |
| caller-provided hostname (resolved once per batch) | `hostname` |
| `record.parentUuid` | `message_id` (NULL when `parentUuid` null) |

### Canonicalization rule (R-D)

```rust
fn canonicalize_hook_identity(
    command: Option<&str>,
    hook_name: &str,
) -> String {
    let Some(raw) = command else {
        return hook_name.to_string(); // fallback
    };
    let exe = extract_executable(raw); // strip 'node', 'bash', quoting, args
    if is_quill_managed_path(&exe) {
        format!("quill:{}", basename(&exe))
    } else if exe.starts_with("${CLAUDE_PLUGIN_ROOT}/") {
        exe // verbatim (env-var preserved as the stable plugin id)
    } else {
        basename(&exe).to_string()
    }
}
```

`is_quill_managed_path` returns true when the absolute or expanded path
points inside `~/.config/quill/scripts/`, `~/.config/quill/codex/scripts/`,
or Quill's deployed-asset directories tracked in `integrations/manager.rs`.

## 2. Storage interface

### Insert path

```rust
impl Storage {
    pub fn store_hook_invocations_for_messages(
        &self,
        provider: IntegrationProvider,
        invocations: &[HookInvocationInput],
    ) -> Result<(), String>;
}
```

- Uses `INSERT OR IGNORE` against the UNIQUE index, matching the
  `store_skill_usages_for_messages` pattern.
- One transaction per call (batch of records from a single transcript
  parse).
- Returns `Ok(())` even when all rows are duplicates (idempotent).

### Codex observation path

```rust
impl Storage {
    pub fn store_codex_hook_observation(
        &self,
        obs: CodexHookObservation,
    ) -> Result<(), String>;
}
```

Called from the HTTP endpoint handler's background blocking task.

### Query path

```rust
impl Storage {
    pub fn get_hook_breakdown(
        &self,
        days: i32,
        provider: Option<IntegrationProvider>,
        all_time: bool,
        limit: Option<i64>,
    ) -> Result<Vec<HookBreakdown>, String>;
}
```

- Signature matches `get_skill_breakdown`.
- Returns rows shaped:
  ```rust
  pub struct HookBreakdown {
      pub hook_identity: String,
      pub hook_event: String,
      pub tool_name: Option<String>,
      pub is_quill: bool,             // computed from hook_identity prefix
      pub codex_count: i64,
      pub claude_count: i64,
      pub total_count: i64,
      pub last_fired_at: String,      // ISO-8601
  }
  ```
- ORDER BY `total_count DESC, hook_identity ASC` (stable secondary).
- `is_quill` is derived in SQL with
  `CASE WHEN hook_identity LIKE 'quill:%' THEN 1 ELSE 0 END`.

### Deletion paths

```rust
impl Storage {
    pub fn delete_hook_invocations_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String>;
}
```

Called from `delete_session_data` alongside the existing per-table
deletes (skill_usages, session_events, response_times, tool_actions).

Per-cwd cleanup happens inside the existing `delete_project_data(cwd)`
path:

```rust
self.conn.execute(
    "DELETE FROM hook_invocations WHERE cwd = ?1",
    params![cwd],
)?;
```

## 3. Reingest

`hook_invocation_reingest_pending` is read at the same site as
`runtime_event_reingest_pending`. When set, the boot-time sweep replays
the attachment extractor over every Claude JSONL and inserts via the
storage path above. After a clean sweep the flag is cleared.

## 4. Invariants

- Exactly one row in `hook_invocations` per `(provider, session_id,
  agent_id, timestamp, hook_identity)` tuple. Re-extraction is idempotent.
- A row in `hook_invocations` always has a matching parent row in the
  underlying transcript table (`session_events` or Codex observation) when
  one exists; orphaned rows are tolerated (e.g., when a session is
  deleted but the hook table's cascade has not yet run, the breakdown
  query simply over-counts for one refresh tick until the deletion
  completes).
- `hook_identity` is stable across machines for Quill-deployed scripts
  (always `quill:<basename>`) and across plugin installs that use the same
  `${CLAUDE_PLUGIN_ROOT}` relative path.
