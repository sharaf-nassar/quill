# Quill Context Preservation Implementation Plan

## Goal

Add context-mode-inspired context preservation to Quill without vendoring context-mode directly. Quill should provide the same practical benefits through its existing Claude/Codex provider integrations: searchable large-output context, safer routed execution, prompt/compaction continuity capture, provider guidance, and idempotent install/repair for new and existing enabled providers.

## Constraints

- Keep the feature installable through the existing provider enablement paths.
- Repair already-enabled Claude and Codex providers on app startup when assets or hooks are missing or stale.
- Do not overwrite user-owned Claude/Codex config outside Quill managed blocks and marked hooks.
- Keep local Claude and local Codex on the same MCP tool implementation by updating the shared local MCP source.
- Mirror the MCP surface for the remote Claude plugin.
- Do not add test files unless explicitly requested; verify with existing type/build checks.

## Design

1. Add MCP working-context tools in Python.
   - Store large context in a Quill-owned SQLite database under `~/.config/quill/context/context.db`.
   - Use SQLite FTS5 when available for chunk search, with LIKE fallback for older SQLite builds.
   - Provide tools for command execution with large output indexing, file/content indexing, search, fetch-and-index with TTL cache, stats, and purge.
   - Return bounded previews and stable `quill-context://source/chunk` references instead of dumping large output back into LLM context.

2. Add lifecycle hook scripts for Claude and Codex.
   - PreToolUse routing warns or blocks high-risk raw-output patterns and points the assistant at Quill context tools.
   - UserPromptSubmit captures prompts and lightweight decision/task hints.
   - SessionStart emits recent continuity guidance.
   - PreCompact records a compact resume snapshot where supported.
   - Stop keeps existing token/session sync and records a final continuity event.

3. Update provider installers.
   - Deploy new scripts and templates.
   - Register new Claude hooks for SessionStart, UserPromptSubmit, PreCompact, and routing.
   - Register Codex-compatible hooks without relying on unavailable PreCompact support.
   - Verify new assets, hooks, and managed instruction blocks.
   - Keep hooks idempotent by removing prior Quill-marked entries before adding current entries.

4. Repair existing enabled providers.
   - Extend startup refresh to run provider install/verify for enabled Claude/Codex providers that are still detected.
   - Persist `last_verified_at`, `last_error`, and setup state after repair.
   - Emit updated provider status so the UI reflects repair failures.

5. Update Quill architecture docs.
   - Document the context MCP tools, hook lifecycle, provider repair behavior, and install/update expectations in `lat.md/`.

## Verification

- `npm run typecheck`
- `cargo check` in `src-tauri`
- Python syntax/import checks for both local and plugin MCP packages
- `lat check`
