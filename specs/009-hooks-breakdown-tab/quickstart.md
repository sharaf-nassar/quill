# Phase 1 Quickstart: Hooks Breakdown Tab

Maintainer walkthrough for verifying the feature on a development machine.
Mirrors the structure of `specs/008-runtime-redesign/quickstart.md`.

## Prerequisites

- Local clone with the `009-hooks-breakdown-tab` branch checked out.
- A Quill build that includes migration 27 (i.e., this branch).
- A Codex install (≥ the version that supports the `[hooks]` config
  block; verified working on the version recorded in
  `~/.codex/config.toml` at the time of writing).
- A Claude Code install (any version that emits `type:"attachment"`
  records with `attachment.type: "hook_*"` — confirmed present in current
  Claude Code transcripts).
- `~/.config/quill/config.json` exists with `url` and `secret` set (the
  managed scripts read this).
- `activity_tracking` feature flag is ON in Quill Settings → Integration
  Features (the Codex producer is gated on this).

## Step 1 — Snapshot the pre-change baseline

Before applying the change, run Quill once on `main` to capture baseline
analytics behavior:

```bash
git checkout main
npm run tauri dev
```

Open Analytics → Now tab → cycle through the four existing breakdown
modes (Sessions, Projects, Hosts, Skills). Confirm everything renders.
Quit the app.

## Step 2 — Apply the change and launch

```bash
git checkout 009-hooks-breakdown-tab
cargo check --manifest-path src-tauri/Cargo.toml
npm install
npm run tauri dev
```

Expected boot-log lines (verify with `tail -f ~/.local/share/com.quilltoolkit.app/logs/quill.log`):

```
storage: applied migration 27 (hook_invocations)
storage: hook_invocation_reingest_pending = 1
sessions: reingest sweep begin (flags: skill_usage, runtime_event,
  hook_invocation)
sessions: reingest sweep complete; cleared hook_invocation_reingest_pending
```

Migration 27 should run exactly once. Subsequent boots see
`current_version == 27` and skip the migration block. The
reingest-pending flag is cleared after the post-boot sweep.

## Step 3 — Verify the Hooks breakdown renders

In the running app:

1. Open Analytics → Now tab.
2. Confirm the breakdown selector now shows: Sessions, Projects, Hosts,
   Skills, **Hooks**.
3. Click **Hooks**.
4. Expect at least one row to appear (the `quill:context-capture.cjs`
   hook fires on every SessionStart of the running Quill itself).
5. Verify rows starting with `quill:` display that prefix in the identity
   text without an additional QUILL badge.
6. Verify the All / Codex / Claude filter strip and the `∞ ALL TIME` chip
   are present and behave as on the Skills breakdown.

## Step 4 — Verify Claude hook ingestion against a known transcript

Pick a recent Claude session that fired hooks (i.e., any session in
`~/.claude/projects/-home-mamba-work-quill/`):

```bash
RECENT=$(find ~/.claude/projects/-home-mamba-work-quill -name "*.jsonl" \
  -printf '%T@ %p\n' | sort -rn | head -1 | cut -d' ' -f2-)

python3 - "$RECENT" <<'PY'
import json, collections, sys
fn = sys.argv[1]
events = collections.Counter()
identities = collections.Counter()
for line in open(fn):
    d = json.loads(line)
    att = d.get('attachment')
    if isinstance(att, dict) and att.get('type', '').startswith('hook_'):
        events[att.get('hookEvent', '?')] += 1
        identities[att.get('command', '?')] += 1
print('hook events:', dict(events))
print('script commands:', dict(identities))
PY
```

Now open the Hooks breakdown filtered to **Claude**. The total fire
counts for each canonicalized identity must match the per-script counts
returned by the script (`quill:context-capture.cjs` should equal the
total of all `node /...quill/scripts/context-capture.cjs` plus
`node /...quill/scripts/context-capture.cjs` records; plugin entries
should equal the `${CLAUDE_PLUGIN_ROOT}/...` records verbatim).

## Step 5 — Verify Codex hook ingestion live

In a second terminal:

```bash
codex
```

Send any user prompt (e.g. `hi`). Wait for the response to start.

Back in Quill: switch the Hooks breakdown provider filter to **Codex**.
Within 1-2 seconds, expect at least:

- `SessionStart` row with count ≥ 1
- `UserPromptSubmit` row with count ≥ 1

If you ran a Bash tool via Codex:

- `PreToolUse · Bash` row with count ≥ 1
- `PostToolUse · Bash` row with count ≥ 1

Verify the rows do **not** carry a separate QUILL badge (the observer is a
generic event-observer, not a Quill telemetry script in the script-row
sense; Codex rows are event-scoped).

Verify the inline header help affordance (the `?` button next to the
breakdown selector) opens a tooltip that explains the Claude vs Codex
asymmetry.

## Step 6 — Verify Codex producer disable on `activity_tracking` toggle

In Quill Settings → Integration Features, toggle **Activity tracking**
off. Confirm:

```bash
grep -A 3 "hook-observe" ~/.codex/config.toml
```

returns nothing (the eight Quill-managed observer blocks are removed),
and:

```bash
ls ~/.config/quill/codex/scripts/hook-observe.cjs 2>&1
```

returns "No such file". Toggle back on; confirm both come back.

## Step 7 — Verify idempotency

While the app is running, manually retrigger the reingest:

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
  "INSERT OR REPLACE INTO settings (key, value) VALUES ('hook_invocation_reingest_pending', '1');"
```

Restart Quill. Confirm the reingest sweep runs again and that the
`hook_invocations` row count does NOT grow (idempotency held — the
UNIQUE index absorbed all replays):

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
  "SELECT COUNT(*) FROM hook_invocations;"
```

before and after must match.

## Step 8 — Verify session-delete cleanup

Pick a session id from the Sessions breakdown. Right-click → Delete.
Confirm:

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
  "SELECT COUNT(*) FROM hook_invocations WHERE session_id = '<id>';"
```

returns 0.

## Step 9 — Run the test suite and `lat check`

```bash
cargo test --manifest-path src-tauri/Cargo.toml
npm run lint
npx lat check
```

All three must pass. `lat check` validates that the new sections in
`lat.md/` resolve cleanly and that the new `@lat:` code references in
`storage.rs`, `sessions.rs`, `server.rs`, and `integrations/codex.rs`
point at existing sections.

## Rollback

If the feature needs to be backed out before merge:

1. Drop the `hook_invocations` table:
   ```sql
   DROP TABLE IF EXISTS hook_invocations;
   ```
2. Reset the schema version:
   ```sql
   DELETE FROM schema_version WHERE version = 27;
   ```
3. Clear the reingest flag:
   ```sql
   DELETE FROM settings WHERE key = 'hook_invocation_reingest_pending';
   ```
4. Remove the eight Codex `[[hooks.<Event>]]` blocks whose `command`
   contains `hook-observe.cjs` from `~/.codex/config.toml`.
5. Remove `~/.config/quill/codex/scripts/hook-observe.cjs`.

Backing out is safe because the table is additive and shares no schema
with prior tables.

## Pass criteria

This quickstart passes when:

- Migration 27 runs once on first launch and the reingest sweep clears
  the flag on clean exit.
- The Hooks breakdown renders rows for both providers under the cases in
  Steps 3-5.
- Quill-deployed Claude rows keep their `quill:` identity prefix without a
  separate QUILL badge; plugin/personal rows do not show that prefix.
- Toggling `activity_tracking` cleanly adds/removes the Codex producer.
- Idempotent reingest and session-delete cleanup both behave as
  specified.
- `cargo test`, lint, and `lat check` all pass.
