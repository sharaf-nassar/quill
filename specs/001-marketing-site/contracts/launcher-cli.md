# Launcher CLI Contract — `scripts/run_quill_demo.sh`

One POSIX script that owns the demo capture lifecycle.

## Synopsis

```text
scripts/run_quill_demo.sh [--clean] [--bin PATH] [--keep-on-exit]
```

## Flags

| Flag             | Default | Effect                                                                |
|------------------|---------|-----------------------------------------------------------------------|
| `--clean`        | OFF     | Delete `$SANDBOX` before running, forcing a fresh seed.               |
| `--bin PATH`     | auto    | Override Quill binary discovery.                                       |
| `--keep-on-exit` | OFF     | After the Quill window closes, leave the sandbox in place silently.   |

## Sandbox path discovery

| Platform | `$SANDBOX`                                                |
|----------|-----------------------------------------------------------|
| Linux    | `/tmp/quill-demo-$USER`                                   |
| macOS    | `/tmp/quill-demo-$USER`                                   |

## Binary auto-discovery

In order, until one resolves:

1. The path passed via `--bin`, if any.
2. `quill` on `$PATH`.
3. `target/release/quill` relative to the repository root.
4. `target/debug/quill` relative to the repository root.

If none resolve, exit with code `4` and print a message instructing the maintainer to install Quill or build it from source.

## Lifecycle

1. **Discover Quill binary**. If absent, exit `4`.
2. **Resolve `$SANDBOX`**. Optionally `rm -rf $SANDBOX` (`--clean`).
3. **Create `$SANDBOX/data`, `$SANDBOX/rules`, `$SANDBOX/projects`, `$SANDBOX/codex-sessions`, and `$SANDBOX/home`** (`mkdir -p`).
4. **Set environment** for the seeder + Quill child:
   - `QUILL_DEMO_MODE=1`
   - `QUILL_DATA_DIR=$SANDBOX/data`
   - `QUILL_RULES_DIR=$SANDBOX/rules`
   - `QUILL_CLAUDE_PROJECTS_DIR=$SANDBOX/projects`
   - `QUILL_CODEX_SESSIONS_DIR=$SANDBOX/codex-sessions`
   - `HOME=$SANDBOX/home`. The app resolves memory documents and its own context assets from the home directory rather than from the `QUILL_*` overrides, so isolating it is what keeps the demo's writes out of the maintainer's `~/.claude` and what makes the Memories panel show the seeded documents.
5. **Invoke seeder**: `python3 scripts/populate_dummy_data.py --bin "$QUILL_BIN" --data-dir "$QUILL_DATA_DIR" --rules-dir "$QUILL_RULES_DIR" --projects-dir "$QUILL_CLAUDE_PROJECTS_DIR" --codex-sessions-dir "$QUILL_CODEX_SESSIONS_DIR" --home-dir "$HOME" --no-backup`. The same executable initializes the schema and launches the demo, preventing version skew. Exit `3` on seeder failure.
6. **Print sandbox banner** to stderr: `[demo] sandbox at /tmp/quill-demo-alex` and `[demo] launching quill ...`.
7. **Exec the Quill binary** with the prepared environment. This runs under `dbus-run-session` when that binary exists, so the demo owns a private session bus and therefore its own `tauri-plugin-single-instance` lock and can run beside a personal Quill; without it the launcher warns that the personal instance must be closed first. A bounded background watcher removes `<home>/.claude/CLAUDE.md` as soon as the app recreates it, because the Memories panel counts that one global file once per project and it would otherwise double every project count.
8. **On exit**, unless `--keep-on-exit` is set, print the teardown command (`rm -rf /tmp/quill-demo-alex`) WITHOUT executing it. Maintainer decides.

## Exit codes

| Code | Meaning                                                                 |
|------|-------------------------------------------------------------------------|
| `0`  | Demo Quill exited cleanly.                                               |
| `1`  | Bad argument(s).                                                         |
| `2`  | Sandbox setup failed (could not create directories).                     |
| `3`  | Seeder failed (forwarded from `populate_dummy_data.py`).                 |
| `4`  | No Quill binary found.                                                   |
| `>4` | Forwarded from the Quill child process exit code.                        |

## Out-of-scope (deliberately)

- The launcher does NOT capture screenshots. That remains `scripts/take_screenshots.sh`'s job; the maintainer runs it manually after Quill is on screen with the demo data loaded.
- The launcher does NOT publish or copy assets. The maintainer is responsible for moving captured PNGs into `marketing-site/assets/screenshots/`.
- The launcher does NOT install Quill. It assumes the binary already exists (see auto-discovery).

## Test surface

- Linux + macOS: smoke-test by running `scripts/run_quill_demo.sh --clean`, observing the demo window with seeded data, killing it, observing the printed teardown command.
