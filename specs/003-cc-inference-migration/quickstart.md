# Quickstart: Verify the CC Inference Migration

This is a maintainer's walkthrough for sanity-checking the migration end-to-end after the implementation tasks land. It is **not** a user-facing document.

## Prerequisites

- `claude` CLI installed and on PATH (`which claude` returns a path).
- User signed in (`claude auth` or `claude /login` previously run; `~/.claude/.credentials.json` exists).
- A Quill development build (`pnpm tauri dev` or equivalent).
- A project with at least a few committed observations in the learning database (e.g., run a `claude` session with the observe hooks for a minute to seed observations).

## Verification 1 — Happy-path learning analyze (Success Criterion SC-001, SC-002)

1. Open Quill, navigate to Learning → Analyze.
2. Trigger an on-demand analysis.
3. While it runs, monitor with `lsof -p <quill-pid> -nP -iTCP | grep anthropic.com` (or platform equivalent). You should see **zero** connections from the Quill process to `api.anthropic.com:443` for inference. (You may see one connection from a separate poller process for `/api/oauth/usage` — that is the live-usage scraper and is allowed by FR-015.)
4. Confirm the run completes with status `success`, produces at least zero (and typically some) rules, and is recorded in RUN HISTORY.

Pass when: connections-during-inference check is clean, the run finishes successfully, and the run record matches the pre-migration shape (status, observations analyzed, rules created/updated, duration, error).

## Verification 2 — Per-call metadata is persisted (SC-009, FR-016)

1. After Verification 1 completes, query the SQLite database directly:

   ```bash
   sqlite3 ~/.local/share/com.quilltoolkit.app/quill.db \
     "SELECT inference_metadata FROM learning_runs ORDER BY id DESC LIMIT 1;"
   ```

2. The output is a non-NULL JSON array. For a full-mode run, expect 4 entries (`stream_a`, `stream_b`, `stream_c`, `synthesis`). For a micro-mode run, expect 1 entry (`stream_a`). Each entry has the fields enumerated in `data-model.md` § "Inference Call Metadata".

Pass when: the JSON parses, lengths match expectations, every required field is present and reasonable (token counts > 0, `model` is a concrete Claude model id, `success: true` for all entries in a successful run).

## Verification 3 — Stream-by-stream text logs preserved (FR-016 (a))

1. From the same query as above, also fetch the `logs` column.
2. Confirm the textual lines mirror the pre-migration shape: `Stream A: extracted N patterns, K verdicts`, `Stream B: ...`, `Synthesis: prompt size X chars, calling Sonnet`, etc.

Pass when: every text log line that would have appeared pre-migration is still there, verbatim or with equivalent meaning.

## Verification 4 — Missing-Claude-Code error UX (SC-003, FR-010)

1. Temporarily rename or PATH-mask the `claude` binary (`PATH=/usr/bin pnpm tauri dev`, or rename `~/.local/bin/claude` for the duration of this test). Make sure the new Quill process doesn't see it.
2. Restart Quill so the cached `which` lookup is invalidated.
3. Trigger an analyze.
4. Confirm the UI shows a specific message naming Claude Code as missing, not a generic "Anthropic API error".
5. Restore PATH / rename when done.

Pass when: the error message is specific and actionable.

## Verification 5 — Rate-limit failure surfacing (SC-004, FR-011)

This one is hard to deterministically trigger. Two options:

- **Natural:** Run an analyze while a heavy Claude Code session is active in the same account. Repeat until you hit the 429 path.
- **Synthetic:** Temporarily replace the `claude` binary on PATH with a script that prints a known rate-limit envelope:

  ```bash
  #!/usr/bin/env bash
  cat <<'EOF'
  {"type":"result","subtype":"error","is_error":true,"api_error_status":"429","result":"Anthropic rate limit","duration_ms":0,"duration_api_ms":0,"ttft_ms":0,"usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"service_tier":"standard"},"total_cost_usd":0,"stop_reason":null,"session_id":"","permission_denials":[],"terminal_reason":"error","modelUsage":{}}
  EOF
  exit 0
  ```

  Save as `~/tmp/fake-claude/claude`, `chmod +x`, prepend to PATH, restart Quill.

Pass when: the UI failure message identifies the failure as rate-limit-related and the user can tell to wait and retry. The run record has `failure_kind: rate_limited` in the metadata.

## Verification 6 — Timeout firing (FR-009)

1. Replace `claude` on PATH with a script that sleeps indefinitely:

   ```bash
   #!/usr/bin/env bash
   sleep 600
   ```

2. Trigger an analyze.
3. After 300 seconds the run should fail with a clear timeout message, and the subprocess should be killed.

Pass when: the run errors at ~300s (not 600s, not indefinitely), and no orphan `sleep` process remains.

## Verification 7 — Concurrency preserved (FR-014, SC-005)

1. Add a small `dprintln!` or log line at the start and end of each `cc_client::invoke_typed` call (temporary, for verification only).
2. Trigger a full-mode analyze with enough observations to ensure Stream A runs and a project with git history so Stream B runs.
3. Read the temporary log: Stream A, B, and C `start` lines should appear within tens of milliseconds of each other; the Synthesis `start` line should appear only after all three `end` lines.
4. Remove the temporary logging.

Pass when: parallel dispatch is visible in the timing, and Synthesis is strictly sequential after.

## Verification 8 — Live Usage View untouched (FR-015, SC-008)

1. Open the main window's Live Usage View.
2. Confirm the bars update on the same cadence as before the migration.
3. Confirm `lsof` still shows the OAuth-usage poller making HTTPS connections to `api.anthropic.com` for the `/api/oauth/usage` path (one connection per poll interval).

Pass when: the Live Usage View is indistinguishable from pre-migration behavior.

## Verification 9 — Codebase cleanliness (SC-006, SC-007)

```bash
rg -n "rig::|rig_core|rig-core|AnthropicRateLimitMiddleware|OAuthHeaderMiddleware|ai_client::analyze_typed|ai_client::complete_text" src-tauri/
```

Pass when: zero matches in `src-tauri/src/` except in comments / changelog / removed-line context. The only acceptable `read_access_token()` callers are `fetcher.rs` (live-usage poller) and `auth.rs` (if applicable).

```bash
rg -n "tokio::time::sleep|retry|Retry-After|backoff" src-tauri/src/cc_client.rs src-tauri/src/learning.rs src-tauri/src/memory_optimizer.rs
```

Pass when: zero matches related to inference retries; SC-007 enforced.

## Done

After all nine verifications pass, the migration is functionally complete and ready for QA / release. The `lat.md/` files should also be updated to reflect the removal of the rig-core path and the addition of `cc_client.rs`; run `lat check` before merging.
