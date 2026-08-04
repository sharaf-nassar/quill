# Seeder CLI Contract — `scripts/populate_dummy_data.py`

The seeder writes deterministic dummy content into a Quill data directory plus an optional learned-rules directory. This contract documents the CLI surface AFTER the marketing-site feature lands.

## Synopsis

```text
python3 scripts/populate_dummy_data.py
       [--bin PATH]
       [--data-dir PATH]
       [--rules-dir PATH]
       [--projects-dir PATH]
       [--codex-sessions-dir PATH]
       [--home-dir PATH]
       [--no-projects]
       [--no-backup]
       [--seed INT]
       [--quiet]
```

## Flags

| Flag             | Type | Default                                                    | Effect                                                                    |
|------------------|------|------------------------------------------------------------|----------------------------------------------------------------------------|
| `--bin`          | path | `quill` on `PATH`, then repo release/debug build            | Executable whose hidden initializer creates or migrates `usage.db`.        |
| `--data-dir`     | path | platform `app_data_dir()` for `com.quilltoolkit.app`       | Directory the seeder writes `usage.db` into. Created if it does not exist. |
| `--rules-dir`    | path | `~/.claude/rules/learned/` (legacy; today's seeder writes here) | Directory the seeder writes sample learned-rule `.md` files into.          |
| `--projects-dir` | path | `~/.claude/projects/`                                      | Directory the seeder writes fictional Claude session JSONL files into (one subdir per project, two `<sessionId>.jsonl` files per subdir). Created if it does not exist. |
| `--codex-sessions-dir` | path | unset | Isolated directory for fictional Codex rollout JSONLs. Supplying it with explicit data and Claude paths enables coherent dual-provider model fixtures. |
| `--home-dir`     | path | unset | Isolated HOME the seeder writes per-project memory documents into (`<home>/.claude/projects/<slug>/memory/<name>.md`, one per seeded project, with `type:`/`description:` frontmatter). Omitted by default because the app resolves memory files from the real home directory. |
| `--no-projects`  | flag | OFF                                                        | Skip writing session JSONL files (Session Search demo data omitted).      |
| `--no-backup`    | flag | OFF                                                        | Skip the existing-DB backup. Used by the launcher when seeding a fresh sandbox. |
| `--seed`         | int  | `42`                                                       | RNG seed for reproducibility.                                              |
| `--quiet`        | flag | OFF                                                        | Suppress per-step progress output; emit only the final summary.            |

## Behavior changes vs. today

1. **Path arguments override the hard-coded `~/.local/share/com.quilltoolkit.app/usage.db`** (and the hard-coded `~/.claude/rules/learned/`). The existing default is preserved when the flags are not passed.
2. **`--data-dir PATH/usage.db` is the seeded DB**. If the directory does not exist, the seeder MUST create it.
3. **`--no-backup` skips the WAL/SHM cleanup + `.bak` copy** because a fresh sandbox has nothing to back up. Production callers (no flags) get backup as today.
4. **`check_quill_not_running()` keeps applying** to the legacy default-path call only. When `--data-dir` is passed, the running-process check is skipped (a sandboxed demo can run while a personal Quill is open, since they target different files).
5. **`--seed` exposed for forward compatibility** — same default produces same byte-output (regression-safe). Legacy Claude-only reruns retain their regular-file replacement behavior, while symlink/junction parents and targets remain forbidden.
6. **Complete model fixtures require an isolated triple override** — `--data-dir`, `--projects-dir`, and `--codex-sessions-dir`. This mode writes ownership-marked Claude and Codex JSONLs, exact migration-28 source fingerprints/keys and observations, plus root-complete state only when runtime discovery exactly matches seeded sources. Reruns remove only marker-owned JSONLs. Every target must remain beneath its canonical configured root through ordinary directories: symlink/junction parents and targets are refused, and exclusive creation never truncates an unmarked collision. Production Claude/Codex roots cannot be used.
7. **Post-core failures attempt migration-safe recovery** — after the core schema/data commit, a cleanup, JSONL, fingerprint, observation, or model-state failure attempts to restore the migration-28 singleton to `pending` with incomplete/zero progress in a separate transaction, warns if recovery itself fails, and preserves the original error.
8. **The selected Quill binary owns schema initialization** — after backup, the seeder invokes `quill --init-database PATH`, which applies the same Rust migrations through version 37 as app startup without launching Tauri or running unrelated cleanup. Python never creates tables, drops stale shapes, or writes `schema_version`. Legitimate old databases migrate; newer or inconsistent schemas fail. Reruns clear fixture and hourly rows, then reset `rollup_meta` to pending with null bookmarks so rollups rebuild from current evidence.
9. **Canonical source keys mirror Rust bytes** — Unix uses canonical path bytes. Windows restores the verbatim `\\?\` drive or `\\?\UNC\` form returned by `std::fs::canonicalize`, then hex-encodes each UTF-16 code unit's big-endian bytes exactly like `sessions.rs`.

## Exit codes

| Code | Meaning                                                                 |
|------|-------------------------------------------------------------------------|
| `0`  | Seeded successfully.                                                     |
| `1`  | Personal Quill is running and `--data-dir` was NOT passed (legacy guard). |
| `2`  | Argument validation failure (e.g., `--data-dir` is not a writable path).  |
| `3`  | DB error during seeding (e.g., schema migration failed).                  |

## Side effects

- Writes / overwrites `usage.db` (+ WAL/SHM at runtime by Quill).
- Writes a small set of sample rule `.md` files into the rules dir, under the `claude/`, `codex/` and `shared/` scope subdirectories the app scans; a rule that is meant to read as a discovered candidate gets no file and an empty `file_path`.
- When `--home-dir` is passed, writes one memory document per seeded project under `<home>/.claude/projects/<slug>/memory/`.
- When `--data-dir` is unset, also writes a `usage.db.bak` next to the DB before mutating it (current behavior, preserved).
- In complete isolated mode, writes retained Claude JSONLs under `--projects-dir` and Codex JSONLs under `--codex-sessions-dir`; never writes a model catalog or provider credential/config file. Cleanup is limited to JSONLs carrying the exact ownership marker inside canonical roots. Writes refuse existing targets and symlink/junction descendants; arbitrary files and unmarked transcripts are never changed or deleted.

## Backwards compatibility

Callers that run `python3 scripts/populate_dummy_data.py` with no flags still write to `~/.local/share/com.quilltoolkit.app/usage.db`, back up the existing DB, and refuse to run while Quill is alive. They must have `quill` on `PATH` or a repo release/debug build; `--bin` selects another executable explicitly.

## Test surface

A small integration test (manual: shell command + `sqlite3 .schema`) verifies:
- `--data-dir /tmp/x` creates `/tmp/x/usage.db` with the expected schema.
- Fresh and rerun databases contain one version-37 record and reset `rollup_meta` to pending with null bookmarks.
- Default invocation still writes to the legacy path.
- `--seed 42` (default) produces identical row counts on re-run.
- Fresh and different-seed rerun isolated modes each expose exactly the same runtime-discoverable JSONL set as the DB source inventory, use success/completion timestamps at or after file mtimes, and record 2/2 complete roots.
- An unmarked JSONL added to either isolated root survives reruns and leaves backfill `pending` with `inventory_complete=0` until Quill reconciles it.
- Unmarked random/deterministic target collisions and child symlink escapes fail without modifying the collision or writing outside the configured root; the DB retains one pending/incomplete backfill row.
- Static Windows drive, UNC, spaces, Unicode, and surrogate-pair fixtures match `sessions.rs`'s verbatim-path UTF-16BE hex algorithm.
