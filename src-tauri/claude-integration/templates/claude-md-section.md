### Session History Search (Quill MCP)

- **Session History Tool**: Use the Quill MCP `search_history` tool to look up past
  Claude Code session history. Quill indexes all sessions into a searchable database
  with full-text search across conversation content, code changes, commands run, and
  tool usage.

- **History Tool**: `mcp__quill__search_history` — full-text search across all
  session history. Filter with `project`, `git_branch`, `role`, `host`, `date_from`,
  `date_to`, `limit`. Hits include code_changes, commands_run, tool_details, and
  files_modified metadata.

- **Working Context Tools** (large transient content stays out of the conversation):
  - `mcp__quill__quill_index_context` — Index large text, files, or command output
  - `mcp__quill__quill_search_context` — Search indexed Quill context for focused chunks
  - `mcp__quill__quill_get_context_source` — Retrieve a specific indexed source or chunk
  - `mcp__quill__quill_execute` / `mcp__quill__quill_execute_file` — Bounded shell with auto-indexed output
  - `mcp__quill__quill_batch_execute` — Multiple labeled commands; output indexed
  - `mcp__quill__quill_fetch_and_index` — Fetch web content into Quill context storage
  - `mcp__quill__quill_context_stats` — Inspect Quill context storage usage
  - `mcp__quill__quill_record_continuity_event` — Record a compact task/decision event
  - `mcp__quill__quill_create_compaction_snapshot` / `mcp__quill__quill_get_compaction_snapshot` — Save or retrieve resume snapshots

- **Context Preservation**: Quill hooks may provide a compact `<quill_continuity>` directive
  at session start with recent prompts, decisions, and tasks. The directive only injects when
  there is actual continuity to carry; treat it as resume context.

- **Routing Behavior**: Prefer Quill working-context tools over raw WebFetch dumps, raw
  `curl`/`wget`, and large Read/Grep output. When the context router denies a `curl`, the
  deny message includes a ready-to-paste `quill_fetch_and_index` or `quill_execute` call.

- **Use When**: User asks about past sessions, previous work, conversation history,
  "what did we do", or which sessions touched a specific file or branch.

- **Do NOT**: Read raw JSONL session logs from `~/.claude/projects/` — use
  `search_history` instead.
