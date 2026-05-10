# Launcher CLI Contract — `scripts/run_quill_demo.{sh,ps1}`

Two parallel scripts that own the demo capture lifecycle. They MUST behave identically modulo platform syntax.

## Synopsis (POSIX)

```text
scripts/run_quill_demo.sh [--clean] [--bin PATH] [--keep-on-exit]
```

## Synopsis (PowerShell)

```text
scripts/run_quill_demo.ps1 [-Clean] [-Bin PATH] [-KeepOnExit]
```

## Flags

| POSIX flag       | PowerShell      | Default | Effect                                                                |
|------------------|-----------------|---------|-----------------------------------------------------------------------|
| `--clean`        | `-Clean`        | OFF     | Delete `$SANDBOX` before running, forcing a fresh seed.               |
| `--bin PATH`     | `-Bin PATH`     | auto    | Override Quill binary discovery.                                       |
| `--keep-on-exit` | `-KeepOnExit`   | OFF     | After the Quill window closes, leave the sandbox in place silently.   |

## Sandbox path discovery

| Platform | `$SANDBOX`                                                |
|----------|-----------------------------------------------------------|
| Linux    | `/tmp/quill-demo-$USER`                                   |
| macOS    | `/tmp/quill-demo-$USER`                                   |
| Windows  | `$env:TEMP\quill-demo-$env:USERNAME`                      |

## Binary auto-discovery

In order, until one resolves:

1. The path passed via `--bin` / `-Bin`, if any.
2. `quill` on `$PATH`.
3. `target/release/quill` relative to the repository root.
4. `target/debug/quill` relative to the repository root.

If none resolve, exit with code `4` and print a message instructing the maintainer to install Quill or build it from source.

## Lifecycle

1. **Discover Quill binary**. If absent, exit `4`.
2. **Resolve `$SANDBOX`**. Optionally `rm -rf $SANDBOX` (`--clean` / `-Clean`).
3. **Create `$SANDBOX/data` and `$SANDBOX/rules`** (`mkdir -p`).
4. **Set environment** for the seeder + Quill child:
   - `QUILL_DEMO_MODE=1`
   - `QUILL_DATA_DIR=$SANDBOX/data`
   - `QUILL_RULES_DIR=$SANDBOX/rules`
   - `QUILL_CLAUDE_PROJECTS_DIR=$SANDBOX/projects`
5. **Invoke seeder**: `python3 scripts/populate_dummy_data.py --data-dir "$QUILL_DATA_DIR" --rules-dir "$QUILL_RULES_DIR" --projects-dir "$QUILL_CLAUDE_PROJECTS_DIR" --no-backup`. Exit non-zero on seeder failure.
6. **Print sandbox banner** to stderr: `[demo] sandbox at /tmp/quill-demo-alex` and `[demo] launching quill ...`.
7. **Exec the Quill binary** with the prepared environment.
8. **On exit**, unless `--keep-on-exit` / `-KeepOnExit` is set, print the teardown command (`rm -rf /tmp/quill-demo-alex` or `Remove-Item -Recurse $env:TEMP\quill-demo-alex`) WITHOUT executing it. Maintainer decides.

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
- Windows: equivalent smoke-test for `.ps1`, run from a non-elevated PowerShell session.
