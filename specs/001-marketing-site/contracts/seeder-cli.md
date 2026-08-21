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
8. **The selected Quill binary owns schema initialization** — after backup, the seeder invokes `quill --init-database PATH`, which applies the current Rust migration chain without launching Tauri or unrelated cleanup. Python never creates tables, drops stale shapes, or writes `schema_version`. Legitimate old databases migrate; newer or inconsistent schemas fail. Reruns clear fixture and hourly rows, then reset `rollup_meta` to pending with null bookmarks so rollups rebuild from current evidence.
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
- Writes current sample rule `.md` files under `claude/`, `codex`, and `shared`, DB-only candidates with empty `file_path`, and `legacy_rules_archived=1` so startup does not treat those fresh fixtures as pre-governance rules.
- When `--home-dir` is passed, writes one memory document per seeded project under `<home>/.claude/projects/<slug>/memory/` and creates a fictional `<home>/.pi/agent/sessions/` root used by the container's harmless Pi CLI stub.
- When `--data-dir` is unset, also writes a `usage.db.bak` next to the DB before mutating it (current behavior, preserved).
- In complete isolated mode, writes retained Claude JSONLs under `--projects-dir` and Codex JSONLs under `--codex-sessions-dir`; never writes a model catalog or provider credential/config file. Model observations carry production-style per-chain attribution, while watcher-side Pi rows seed explicit model evidence without inventing a retained Pi parser. The 1,001-id stress source sits ten days old; photographed ranges use a curated Claude/Codex/Pi model mix. Cleanup is limited to marked JSONLs inside canonical roots.
- Seeds four skill identities with explicit Claude/Codex/Pi counts, enables those three agent integrations, and disables MiniMax for the published screenshot composition.

## Backwards compatibility

Callers that run `python3 scripts/populate_dummy_data.py` with no flags still write to `~/.local/share/com.quilltoolkit.app/usage.db`, back up the existing DB, and refuse to run while Quill is alive. They must have `quill` on `PATH` or a repo release/debug build; `--bin` selects another executable explicitly.

## Test surface

A small integration test (manual: shell command + `sqlite3 .schema`) verifies:
- `--data-dir /tmp/x` creates `/tmp/x/usage.db` with the expected schema.
- Fresh and rerun databases contain the current binary-owned schema version and reset `rollup_meta` to pending with null bookmarks.
- Default invocation still writes to the legacy path.
- `--seed 42` (default) produces identical row counts on re-run.
- Fresh and different-seed rerun isolated modes each expose exactly the same runtime-discoverable Claude/Codex JSONL set as the DB source inventory, record 2/2 complete roots, retain non-null derived model ids, and preserve watcher-side Pi model rows after rollup rebuild.
- The seeder fails before commit unless the six-hour model corpus spans Claude/Codex/Pi and at least six 45-minute buckets, the stress ids stay outside seven days, Skills spans all three providers, and only those providers are enabled.
- Active fixture rules survive app startup because the demo archive sentinel is present; DB-only candidates remain fileless.
- An unmarked JSONL added to either isolated root survives reruns and leaves backfill `pending` with `inventory_complete=0` until Quill reconciles it.
- Unmarked random/deterministic target collisions and child symlink escapes fail without modifying the collision or writing outside the configured root; the DB retains one pending/incomplete backfill row.
- Static Windows drive, UNC, spaces, Unicode, and surrogate-pair fixtures match `sessions.rs`'s verbatim-path UTF-16BE hex algorithm.
