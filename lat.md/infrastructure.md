# Infrastructure

Build tooling, CI/CD pipeline, release automation, and code quality enforcement for the Quill desktop application.

## Build Configuration

The frontend uses Vite with the React plugin; the backend uses Cargo with Tauri.

### Frontend Build

Vite serves on port 8181 in dev mode and ignores `src-tauri/**` to avoid extra frontend reloads during Rust rebuilds.

Production builds target ES2020 with esbuild minification and sourcemaps. TypeScript is strict mode with ESNext modules and bundler resolution. See `vite.config.ts` and `tsconfig.json`.

### Backend Build

Rust edition 2024. Crate types: `lib`, `cdylib`, `staticlib`. `build.rs` calls `tauri_build::build()`.

The bundled SQLite driver (`rusqlite` with `bundled` feature) avoids system dependency issues. Tauri bundles `claude-integration/**/*` as app resources.

### Tauri Configuration

`src-tauri/tauri.conf.json` defines product name "Quill", identifier `com.quilltoolkit.app`, with a borderless transparent main window (280x340px, min 240x200).

Bundle targets: DMG, NSIS, AppImage, DEB. Auto-updater uses GitHub releases endpoint with minisign public key verification.

## CI/CD Pipeline

GitHub Actions workflow (`.github/workflows/release.yml`) triggers on `v*` tags or manual dispatch.

### Draft Release Pre-Creation

A `create-release` job runs before all builds to create a single draft release. This prevents a race condition where parallel `tauri-action` instances each create their own draft, splitting assets across multiple releases and breaking the updater.

### Build Matrix

Four parallel builds (fail-fast disabled), all depending on `create-release` so `tauri-action` finds the existing draft.

Platforms: Linux (Ubuntu 22.04, AppImage + DEB), macOS Intel (x86_64), macOS ARM (aarch64), Windows (NSIS). Each installs Node.js LTS, Rust stable, and platform-specific system dependencies.

### Version Injection

Parses version from git tag (e.g., `v0.2.1` -> `0.2.1`). Updates `src-tauri/Cargo.toml` via sed and `package.json` via Node.js before build.

### macOS Code Signing

Imports APPLE_CERTIFICATE from secrets into a temporary build keychain and extracts CERT_ID for codesigning.

After build, submits DMG to Apple notary service (35-minute timeout), staples the notarization ticket, and re-uploads the notarized DMG to the release.

### Release Publishing

A third job (`publish`) waits for all builds, finds the draft release, and renames assets with platform labels (e.g., `Quill_0.3.1_macOS_amd64.dmg`).

It retries the draft lookup for API eventual consistency, updates `latest.json` for the auto-updater, and publishes the release.

### Required Secrets

`GITHUB_TOKEN`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.

## Release Process

`release.sh` automates version bumping, release note generation, and tagging.

### Commands

Available subcommands for the release script.

- `./release.sh [--ai auto|codex|claude] bump <major|minor|patch>` — Bump version, auto-select a release-notes CLI, create annotated git tag, and push to trigger CI
- `./release.sh [--ai auto|codex|claude] retag [version]` — Re-point existing tag to HEAD and optionally regenerate notes with the selected CLI
- `./release.sh latest` — Show current version

### AI Release Notes

Uses `codex` when installed, otherwise falls back to `claude`; `--ai claude` or `--ai codex` overrides the default selection.

The Codex path pins `gpt-5.4`, `model_reasoning_effort="xhigh"`, and `service_tier="fast"` in non-interactive mode, forces `-C` to the git repo root, and leaves Claude on the existing Haiku-based path.

When release notes are generated through Codex in an interactive terminal, `release.sh` shows a live spinner plus a framed tail of the last 20 user-meaningful Codex progress lines. Internal hook, MCP, router, and sandbox-noise lines stay hidden unless the run fails.

Prompt instructs the model to focus on user-visible features only, omitting refactors, dependency updates, and CI changes. Output format: "## What's New" section with bold feature headings.

## Code Quality

Linting, formatting, and pre-commit enforcement for both frontend and backend code.

### ESLint

Flat config format (v9+) in `eslint.config.js`. Base: `@eslint/js` recommended + `typescript-eslint` strict. Unused vars starting with `_` are allowed. Scope: `src/**/*.{ts,tsx}`. Max warnings: 0 (enforced by pre-commit).

### Pre-Commit Hooks

`.pre-commit-config.yaml` runs on every commit:

| Hook | Scope | Purpose |
|------|-------|---------|
| trailing-whitespace | All | Strip trailing spaces |
| end-of-file-fixer | All | Ensure newline at EOF |
| check-yaml, check-json | All | Syntax validation |
| check-merge-conflict | All | Detect unresolved conflicts |
| check-added-large-files | All | Flag files >500 KB |
| detect-private-key | All | Catch hardcoded secrets |
| shellcheck | `*.sh` | Shell script linting |
| cargo fmt | `src-tauri/**` | Rust formatting |
| cargo clippy | `src-tauri/**` | Rust linting (`-D warnings`) |
| eslint | `src/**/*.{ts,tsx}` | TypeScript linting |
| tsc --noEmit | `src/**/*.{ts,tsx}` | Type checking |

## Scripts

Utility scripts for development, testing, and documentation tasks.

### Screenshot Capture

`scripts/take_screenshots.sh` uses xdotool and ImageMagick to capture screenshots of all UI views for the README. Navigates between tabs and windows by clicking titlebar buttons, generating 6 PNG files.

### Dummy Data Seeder

`scripts/populate_dummy_data.py` seeds the SQLite database with deterministic sample data (seed 42) across all 16 tables.

Checks that Quill is not running before modifying the DB to prevent WAL corruption. Creates a backup before seeding. Populates observations, rules, memory files, tool actions, and writes sample rule files to `~/.claude/rules/learned/`.

## Claude Integration Deployment

Claude integration now has a dedicated adapter shell in
[[src-tauri/src/integrations/claude.rs]] plus manifest-aware install/uninstall wrappers in
[[src-tauri/src/claude_setup.rs]], while startup detection remains in
[[src-tauri/src/integrations/manager.rs]].

`Task 3A` introduces manifest generation during setup so uninstall can remove only owned artifacts:
hook entries marked with [[src-tauri/src/claude_setup.rs#HOOK_MARKER]],
`mcpServers.quill` from `~/.claude.json`, and the `[[src-tauri/src/claude_setup.rs#BLOCK_START]]`/`[[src-tauri/src/claude_setup.rs#BLOCK_END]]` managed block in `CLAUDE.md`.
Migration from legacy heading-based detection to block markers happens automatically on first run.
`confirm_enable_provider` now routes Claude through that adapter install path, and
[[src-tauri/src/restart.rs#uninstall_claude_restart_assets]] removes Quill-owned restart hooks,
shell integration lines, and cache files when Claude uninstall runs.

### Deployed Assets

Files and directories created during first-launch auto-deployment.

| Target | Content |
|--------|---------|
| `~/.config/quill/scripts/` | Hook scripts: token reporting, observation capture, session sync, session-end learning |
| `~/.config/quill/mcp/` | Python MCP server for session querying tools |
| `~/.claude/commands/` | Custom CLI commands (if applicable) |
| `~/.claude/settings.json` | Hook registrations (marked with `_source: "quill-setup"`) |
| `~/.claude.json` | MCP server registration |
| `~/.claude/CLAUDE.md` | Quill MCP usage instructions injected as a managed block between `<!-- quill-managed:claude:start -->` / `<!-- quill-managed:claude:end -->` markers |

Scripts get 0o755 permissions; auth files get 0o600. Existing hook entries are detected to avoid duplication. Original `settings.json` is backed up before patching.

## Codex Integration Deployment

Codex integration lives in [[src-tauri/src/integrations/codex.rs]] and deploys provider-specific assets under `~/.config/quill/codex/` plus Quill-managed entries in the user's Codex home.

### Deployed Assets

Files and config entries created when the Codex provider is enabled. Deployment is allowlisted to the observation, token, sync, and learning scripts; the Claude-only `qbuild-guard.sh` is never copied into Codex assets.

| Target | Content |
|--------|---------|
| `~/.config/quill/codex/scripts/` | Hook scripts for observations, token reporting, session sync, and session-end learning |
| `~/.config/quill/codex/mcp/` | Python MCP server copied from the bundled Quill MCP assets |
| `~/.config/quill/codex/templates/` | Managed AGENTS template block |
| `~/.codex/hooks.json` | Hook registrations marked with `_source: "quill-codex-setup"` |
| `~/.codex/config.toml` | `codex_hooks = true` plus a Quill-managed `mcp_servers.quill` block when no manual entry exists |
| `~/.codex/AGENTS.md` | Managed Quill session-history guidance block |

Codex uninstall removes only Quill-marked hooks, config blocks, AGENTS blocks, and the provider-owned asset directories.

## Remote Plugin

The `plugin/` directory contains a Claude Code plugin for remote host connectivity.

### Plugin Manifest

`plugin/.claude-plugin/plugin.json`: name "quill", version 2.0.0, provides `/quill:setup`, `/quill:learn`, `/quill:qbuild` commands. Installed via marketplace on remote machines.

### Plugin MCP Server

`plugin/mcp/server.py` runs a FastMCP server (via `uv`) with session query tools mirroring the local MCP.

Provides search_history, list_sessions, get_session_context, get_branch_activity, get_token_usage, and more. Communicates back to the desktop widget's HTTP server.

## Dependencies

Key runtime and dev dependencies for both frontend and backend.

### Frontend Runtime

React 19, React DOM, Tauri API v2, Tauri plugins (updater, window-state, process), Recharts 3.7, DOMPurify 3.3.

### Frontend Dev

TypeScript 5.9, Vite 6.0, ESLint 9.39, @vitejs/plugin-react.

### Backend

Tauri 2, Axum 0.8, Tokio 1, rusqlite 0.31 (bundled), Tantivy 0.25, reqwest 0.13, rig-core 0.32, serde/serde_json, chrono, sha2, parking_lot 0.12, similar 2, regex, walkdir, dirs, nix (unix). Full list in `src-tauri/Cargo.toml`.
