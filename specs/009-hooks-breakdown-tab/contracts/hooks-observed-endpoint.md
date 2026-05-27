# Contract: `POST /api/v1/hooks/observed`

New HTTP endpoint that accepts hook fire observations from
Quill-deployed Codex hook scripts (and any future producer that wants to
report a hook fire). Sits alongside the existing 14 endpoints in
`src-tauri/src/server.rs`.

## 1. Wire format

### Request

```http
POST /api/v1/hooks/observed HTTP/1.1
Host: 127.0.0.1:19876
Authorization: Bearer <quill-secret>
Content-Type: application/json
Content-Length: <n>

{
  "provider": "codex",
  "session_id": "019e51ba-ee61-7b53-b4be-fd5a96ee0a26",
  "hook_event": "PreToolUse",
  "tool_name": "Bash",
  "cwd": "/home/mamba/work/quill",
  "ts": "2026-05-22T15:07:45.123Z",
  "hook_matcher": "Bash"
}
```

| Field | Type | Required | Constraint |
|-------|------|----------|------------|
| `provider` | string | yes | Exactly `"codex"` for now. Claude continues to ingest from transcripts; sending `"claude"` is accepted but logged as a misconfiguration and persisted normally. |
| `session_id` | string | yes | ≤ 128 chars. |
| `hook_event` | string | yes | One of the eight known events. |
| `tool_name` | string | no | ≤ 64 chars. Present for PreToolUse / PostToolUse. |
| `cwd` | string | no | ≤ 1024 chars. |
| `ts` | string | yes | ISO-8601 with offset. |
| `hook_matcher` | string | no | ≤ 64 chars. |

### Successful response

```http
HTTP/1.1 202 Accepted
Content-Type: application/json

{ "status": "accepted" }
```

Status `202 Accepted` matches the fast-ack semantics of
`/api/v1/learning/observations` and `/api/v1/sessions/notify`: validation
happens synchronously, persistence happens on a background blocking task.

### Error responses

| Code | Reason |
|------|--------|
| 400  | Validation failure (missing field, unknown `hook_event`, malformed `ts`). Body: `{ "error": "<reason>" }`. |
| 401  | Bearer token missing or wrong. Body: `{ "error": "unauthorized" }`. |
| 413  | Payload exceeds 4096 bytes (enforced by the existing body-size limit). |
| 429  | Rate limit (existing per-endpoint limiter applies). |

## 2. Handler responsibilities

```rust
async fn post_hook_observed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<HookObservationPayload>,
) -> Result<impl IntoResponse, HookObservationError>;
```

1. Verify bearer token (constant-time comparison via existing helper).
2. Validate fields per the table above.
3. Reject `hook_event` not in the eight-event whitelist.
4. Build a `CodexHookObservation` struct.
5. Spawn a blocking task to call
   `storage.store_codex_hook_observation(obs)`.
6. Return `202 Accepted` immediately.

The handler MUST NOT block on storage I/O — the calling Codex hook script
has a 1.5-second local timeout (matching `observe.cjs`).

## 3. Idempotency

The endpoint accepts duplicate POSTs (Codex may retry on transient
errors); the storage layer's `INSERT OR IGNORE` on the unique index
absorbs duplicates.

## 4. Producer: `hook-observe.cjs`

Deployed at `~/.config/quill/codex/scripts/hook-observe.cjs`, registered
by the installer onto each of the eight Codex hook events as a separate
`[[hooks.<Event>]]` block with no matcher. Behavior:

```js
#!/usr/bin/env node
"use strict";
const fs = require("fs");
const path = require("path");
const http = require("http");
const https = require("https");

function loadConfig() { /* same as observe.cjs */ }
function postJSON(/* same shape as observe.cjs */) {}

function main() {
  try {
    const input = JSON.parse(fs.readFileSync(0, "utf8"));
    const event = input.hook_event_name;
    if (!event) return;
    const config = loadConfig();
    postJSON(config, "/api/v1/hooks/observed", {
      provider: "codex",
      session_id: input.session_id,
      hook_event: event,
      tool_name: input.tool_name ?? null,
      cwd: input.cwd ?? null,
      ts: new Date().toISOString(),
      hook_matcher: input.matcher ?? null,
    }, "codex hook-observe");
  } catch {
    // swallow; never block the hook chain
  }
}

main();
```

Constraints:

- ≤ 80 lines of code. No new dependencies beyond Node stdlib.
- Reads `~/.config/quill/config.json` (same loader as `observe.cjs`).
- Reuses the `LOCAL_TIMEOUT_MS = 1500` constant pattern.
- Exits with code 0 even on error so it never blocks the hook chain.
- Honors `QUILL_DEBUG` env var for stderr diagnostics (matches existing
  scripts).

## 5. Installer wiring

`src-tauri/src/integrations/codex.rs` is extended:

- `ALL_MANAGED_SCRIPT_FILES` gains `"hook-observe.cjs"`.
- A new helper `hook_observation_scripts_for(features) -> Vec<&str>`
  returns `vec!["hook-observe.cjs"]` when
  `features.activity_tracking` is true, empty otherwise.
- The hook-group builder produces eight `CodexHookGroup` entries (one
  per event from `CODEX_HOOK_EVENTS`), each with a single
  `CodexHookCommand { command: "node \"<absolute path to
  hook-observe.cjs>\"", timeout: 3 }`.
- The installer's reinstall path detects orphaned entries by the
  `quill-codex-setup` marker comment (existing mechanism) so toggling
  `activity_tracking` cleanly removes/re-adds the eight blocks.

## 6. Disable / opt-out

When `activity_tracking` is set false in `IntegrationFeatures`, the
installer:

1. Removes the eight `hook-observe.cjs` entries from `~/.codex/config.toml`.
2. Removes `hook-observe.cjs` from `~/.config/quill/codex/scripts/`.

The endpoint stays available (other producers may exist in the future),
but no Codex producer will be POSTing to it. Pre-existing rows in
`hook_invocations` are untouched.
