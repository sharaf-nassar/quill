# Quill — repo guide for agents

Local-first Tauri + React 19 desktop companion for Claude Code, Codex, Pi,
and MiniMax: usage/limits tracking, session search, learning, and agent
tools. Rust backend in `src-tauri/`, React frontend in `src/`, Python MCP
server in `src-tauri/claude-integration/mcp/`.

## Ground rules

- `constitution.md` governs engineering decisions (12 principles). Notable:
  adding automated tests requires explicit user authorization (P7);
  zero-warning gates before completion (P6); UI changes follow `PRODUCT.md`
  and `DESIGN.md` (P9); commit or push only with explicit authority (P12).
- Task tracking is Beads: `bd ready`, `bd show <id>`, `bd close <id>`;
  run `bd prime` for full workflow context when it is missing or stale.
- Architecture, design intent, and test specs live in `lat.md/`. Search it
  before coding, update it after behavior changes, run `lat check` before
  declaring done.
- Reusable debugging learnings live in `docs/solutions/` (conventions,
  environment, runtime-errors). Check it before re-deriving a fix; add new
  ones there.
- Feature specs live in `specs/` (speckit pipeline reads `constitution.md`).

## Build, test, gates

Frontend, from repo root:

```bash
npm run typecheck   # tsc --noEmit
npm run lint        # eslint src/
npm test            # node --test scripts/*.test.mjs — serial, keep concurrency 1
npm run knip        # unused files/exports/deps
```

Backend, from `src-tauri/`:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
uv run --locked --project claude-integration/mcp cargo test
```

The `uv run` wrapper is required: Rust tests spawn the Python MCP server.
Pre-commit hooks add shellcheck plus the fmt/clippy/eslint/typecheck set;
CI (`.github/workflows/ci.yml`) enforces the same and a macOS check.

## Dev runs

- Use `npm run tauri -- dev`. Never `cargo tauri dev` — it skips
  `tauri.dev.conf.json` and writes production state.
- Only one Quill runs at a time (fixed provider ports 19876/19877); stop an
  installed Quill first. `QUILL_PORT`/`QUILL_CONTEXT_PORT` override.
- Sandboxed demo against dummy data: `scripts/run_quill_demo.sh`.
