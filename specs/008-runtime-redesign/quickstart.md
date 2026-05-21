# Quickstart — Maintainer Verification

**Feature**: 008 — Active-Time Runtime Tracking Redesign
**Audience**: Quill maintainers verifying a local build before merge.

This walkthrough exercises the redesign end-to-end. It assumes a Linux
or macOS machine with the standard Quill dev toolchain (Rust 2024,
Node 20+, the Tauri 2 CLI) already installed.

## Prerequisites

- Active CC and/or Codex history on disk under `~/.claude/projects/`
  and/or `~/.codex/sessions/`.
- A clean working tree on branch `008-runtime-redesign`.
- The Quill widget closed.

## Step 1 — Snapshot the pre-change baseline

Before applying the migration, capture what the old card said. From a
running build on `main`:

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
    "SELECT COUNT(*) AS rows, SUM(response_secs) AS sum_secs
     FROM response_times WHERE timestamp >= datetime('now','-1 hour');"
```

Note the row count and total — the redesign must produce a strictly
larger active-time number than this for the same window.

## Step 2 — Apply the change and launch

```bash
git switch 008-runtime-redesign
npm run tauri dev
```

On first launch the app should:

1. Run migration 26: create `session_events` and set
   `runtime_event_reingest_pending = '1'`.
2. The session indexer reads the flag on boot, clears
   `index_state.json::file_mtimes`, and clears the flag.
3. The next mtime sweep re-reads every transcript and inserts rows
   into `session_events`.

Confirm via the database:

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
    "SELECT name FROM sqlite_master WHERE type='table' AND name='session_events';
     SELECT COUNT(*) FROM session_events;
     SELECT value FROM settings WHERE key='runtime_event_reingest_pending';"
```

Expect: the table exists; the count is non-zero (it grows as the sweep
progresses); the flag has been cleared (`'0'` or absent).

## Step 3 — Verify the headline card

Open the widget's Now tab. The **LLM Runtime** card should report a
total comfortably larger than the Step-1 baseline for the same range.
Hover the `?` help icon and confirm the description reads:

> "Total active time across CC and Codex sessions in this window —
> model generation, reasoning, and tool execution counted together.
> User-idle gaps over 5 minutes are excluded; tool waits are not."

Cycle the range selector (1h → 24h → 7d → 30d) and confirm the total
scales monotonically, the sparkline updates, and the subtitle still
reads "{sessions} sessions · {turns} turns · avg {avg}".

## Step 4 — Verify the new semantics against transcripts

Pick a CC session you know had a long tool execution (a multi-minute
build, a `/qbuild` orchestration). Confirm the card's total grew by
roughly the duration of that tool wait — not by a few seconds for the
"time to first token" the old card recorded.

For a quick sanity check from the command line:

```bash
python3 - <<'PY'
import sqlite3, datetime
db = sqlite3.connect("~/.local/share/com.quilltoolkit.app/usage.db".replace("~", "/home/$USER"))
cur = db.cursor()
cur.execute("""
SELECT provider, session_id,
       MIN(timestamp) AS first_ts,
       MAX(timestamp) AS last_ts,
       COUNT(*) AS events
FROM session_events
WHERE timestamp >= datetime('now','-1 hour')
GROUP BY provider, session_id
ORDER BY events DESC LIMIT 5;
""")
for r in cur.fetchall():
    print(r)
PY
```

Expect: one row per active session, with event counts in the dozens to
hundreds.

## Step 5 — Sessions breakdown sanity check

In the Now tab, expand the Sessions breakdown. Open a session that has
sub-agents (the disclosure caret appears). Confirm:

- Parent row's `turn_count` is non-zero (it reads from
  `response_times`, which is unchanged by this feature).
- Sub-agent rows render with their own turn counts and activity ranges.
- The headline card's total does not double-count parent + sub-agent
  time (the redesign sums per-chain turns; the chain key includes
  `agent_id`).

## Step 6 — Idempotency

Force a re-run of the indexer:

```bash
rm ~/.local/share/com.quilltoolkit.app/session-index/.tantivy-writer.lock 2>/dev/null
# Inside the running app, trigger a manual session-search sync, or
# touch a transcript file and wait for notify-driven ingest.
touch ~/.claude/projects/*/*.jsonl
```

After the sweep settles, run:

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
    "SELECT COUNT(*) FROM session_events;"
```

Compare to Step 2's count. It must be identical (FR-009 + SC-005).

## Step 7 — Session-delete cleanup

In the Sessions breakdown, delete a session you can spare. Confirm:

```bash
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db \
    "SELECT COUNT(*) FROM session_events WHERE session_id = '<the-deleted-id>';"
```

Expect: `0`.

## Step 8 — Run the test suite and lat check

```bash
cd src-tauri && cargo test storage::tests::session_events
cd src-tauri && cargo test sessions::tests::events_emission
cd .. && lat check
```

All three must succeed. `lat check` validates that the lat.md updates
to `backend.md`, `data-flow.md`, `features.md`, and `tests.md` are
consistent with the code refs.

## Rollback

If something goes wrong:

```bash
# 1. Inside the app: not exposed — there is no UI rollback.
# 2. To roll back the data side, drop the new table and clear the flag.
sqlite3 ~/.local/share/com.quilltoolkit.app/usage.db <<'SQL'
DROP TABLE IF EXISTS session_events;
DELETE FROM settings WHERE key = 'runtime_event_reingest_pending';
UPDATE settings SET value = '25' WHERE key = 'schema_version';
SQL
# 3. Reinstall an older build.
```

The transcripts themselves are untouched, so the new model can be
rebuilt from scratch on any future build.

## Pass criteria

- All Step-2 through Step-7 expectations met.
- Step-8 commands all return success.
- The LLM Runtime card total for the 1h window has grown materially
  (typically 10× or more) compared to the Step-1 baseline on a
  machine with active CC/Codex usage in the past hour.
