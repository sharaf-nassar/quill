# Seeder CLI Contract — `scripts/populate_dummy_data.py`

The seeder writes deterministic dummy content into a Quill data directory plus an optional learned-rules directory. This contract documents the CLI surface AFTER the marketing-site feature lands.

## Synopsis

```text
python3 scripts/populate_dummy_data.py
       [--data-dir PATH]
       [--rules-dir PATH]
       [--no-backup]
       [--seed INT]
       [--quiet]
```

## Flags

| Flag             | Type | Default                                                    | Effect                                                                    |
|------------------|------|------------------------------------------------------------|----------------------------------------------------------------------------|
| `--data-dir`     | path | platform `app_data_dir()` for `com.quilltoolkit.app`       | Directory the seeder writes `usage.db` into. Created if it does not exist. |
| `--rules-dir`    | path | `~/.claude/rules/learned/` (legacy; today's seeder writes here) | Directory the seeder writes sample learned-rule `.md` files into.          |
| `--projects-dir` | path | `~/.claude/projects/`                                      | Directory the seeder writes fictional Claude session JSONL files into (one subdir per project, two `<sessionId>.jsonl` files per subdir). Created if it does not exist. |
| `--no-projects`  | flag | OFF                                                        | Skip writing session JSONL files (Session Search demo data omitted).      |
| `--no-backup`    | flag | OFF                                                        | Skip the existing-DB backup. Used by the launcher when seeding a fresh sandbox. |
| `--seed`         | int  | `42`                                                       | RNG seed for reproducibility.                                              |
| `--quiet`        | flag | OFF                                                        | Suppress per-step progress output; emit only the final summary.            |

## Behavior changes vs. today

1. **Path arguments override the hard-coded `~/.local/share/com.quilltoolkit.app/usage.db`** (and the hard-coded `~/.claude/rules/learned/`). The existing default is preserved when the flags are not passed.
2. **`--data-dir PATH/usage.db` is the seeded DB**. If the directory does not exist, the seeder MUST create it.
3. **`--no-backup` skips the WAL/SHM cleanup + `.bak` copy** because a fresh sandbox has nothing to back up. Production callers (no flags) get backup as today.
4. **`check_quill_not_running()` keeps applying** to the legacy default-path call only. When `--data-dir` is passed, the running-process check is skipped (a sandboxed demo can run while a personal Quill is open, since they target different files).
5. **`--seed` exposed for forward compatibility** — same default produces same byte-output (regression-safe).

## Exit codes

| Code | Meaning                                                                 |
|------|-------------------------------------------------------------------------|
| `0`  | Seeded successfully.                                                     |
| `1`  | Personal Quill is running and `--data-dir` was NOT passed (legacy guard). |
| `2`  | Argument validation failure (e.g., `--data-dir` is not a writable path).  |
| `3`  | DB error during seeding (e.g., schema migration failed).                  |

## Side effects

- Writes / overwrites `usage.db` (+ WAL/SHM at runtime by Quill).
- Writes a small set of sample rule `.md` files into the rules dir.
- When `--data-dir` is unset, also writes a `usage.db.bak` next to the DB before mutating it (current behavior, preserved).

## Backwards compatibility

Callers that run `python3 scripts/populate_dummy_data.py` with no flags MUST observe the existing v0 behavior: writes to `~/.local/share/com.quilltoolkit.app/usage.db`, backs the existing DB up, refuses to run while Quill is alive. The new flags are strictly additive.

## Test surface

A small integration test (manual: shell command + `sqlite3 .schema`) verifies:
- `--data-dir /tmp/x` creates `/tmp/x/usage.db` with the expected schema.
- Default invocation still writes to the legacy path.
- `--seed 42` (default) produces identical row counts on re-run.
