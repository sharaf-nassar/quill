# Environment Variable Contract — Demo Mode Path Override

This contract defines the environment variables Quill reads at startup to redirect its data and learned-rules directories away from the platform default. These exist solely to support the marketing-site screenshot capture workflow and MUST NOT be relied on for production use.

## Variables

| Name                         | Type    | Required when active? | Effect when unset                       |
|------------------------------|---------|-----------------------|------------------------------------------|
| `QUILL_DEMO_MODE`            | `0`/`1` | Required to opt in    | Demo mode disabled; all overrides ignored. |
| `QUILL_DATA_DIR`             | path    | Optional              | Falls back to `app_data_dir()`.          |
| `QUILL_RULES_DIR`            | path    | Optional              | Falls back to default learned-rules dir. |
| `QUILL_CLAUDE_PROJECTS_DIR`  | path    | Optional              | Falls back to `~/.claude/projects/` for the session indexer. |

## Resolution rules

These rules are implemented in `src-tauri/src/data_paths.rs` and called by every Quill code path that previously called `app_data_dir()` or referenced a learned-rules path directly.

```text
fn resolve_data_dir(app):
    if env("QUILL_DEMO_MODE") != "1":
        return app.path().app_data_dir()           # production behavior, unchanged
    if env("QUILL_DATA_DIR") is set:
        ensure dir exists; log demo-mode banner; return it
    log demo-mode warning ("QUILL_DEMO_MODE=1 but QUILL_DATA_DIR unset")
    return app.path().app_data_dir()
```

The same logic applies to `resolve_rules_dir()` for `QUILL_RULES_DIR`.

## Safety guarantees (MUST hold)

1. **Production safety**: when `QUILL_DEMO_MODE` is anything other than the literal string `1`, `QUILL_DATA_DIR` and `QUILL_RULES_DIR` are ignored. A stray env var in a maintainer's shell or in an inherited environment MUST NOT redirect a real Quill installation.
2. **Loud activation**: the very first time a Quill process detects `QUILL_DEMO_MODE=1`, it MUST print to stderr (and to its tracing log) a banner showing the resolved data and rules paths. The banner makes it impossible to confuse a demo run with a real one.
3. **Unset is safe**: with all three variables unset, behavior MUST be byte-identical to today.
4. **No path traversal**: the resolver MUST canonicalize the override paths before use; any error in canonicalization is fatal (process exits with a non-zero code) — better to refuse to start than to silently fall back to the real data dir under a confused launcher.

## Examples

```bash
# Production (no override): Quill writes to ~/.local/share/com.quilltoolkit.app/
quill

# Stray env var, no opt-in: same as production (no override)
QUILL_DATA_DIR=/tmp/wat quill                       # writes to ~/.local/share/...

# Opt in but no overrides: same as production, plus a stderr warning
QUILL_DEMO_MODE=1 quill                             # writes to ~/.local/share/...

# Full demo activation: writes only to /tmp/quill-demo-alex/
QUILL_DEMO_MODE=1 \
  QUILL_DATA_DIR=/tmp/quill-demo-alex/data \
  QUILL_RULES_DIR=/tmp/quill-demo-alex/rules \
  quill
```

## Out-of-scope (future, separate clarification)

- A `QUILL_HOOKS_DIR` override for the deployed-hook directory under `~/.config/quill/`. Initial release covers data and rules only because those are the dirs the seeder writes; hook installation is not part of the screenshot pipeline.
- A `QUILL_CACHE_DIR` override for `~/.cache/quill/`. The instance-state files are restart-related and not relevant to screenshots.
