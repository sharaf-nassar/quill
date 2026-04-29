### Session History Search (Quill MCP)

- **Session History Tool**: Use the Quill MCP tools to search past Claude Code session history.
  Quill indexes all sessions into a searchable database with full-text search across conversation
  content, code changes, commands run, and tool usage.

- **MCP Tools** (use these directly):
  - `mcp__quill__quill_index_context` — Index large text, files, or command output into Quill context storage
  - `mcp__quill__quill_search_context` — Search indexed Quill context without dumping raw content
  - `mcp__quill__quill_get_context_source` — Retrieve a focused indexed context source or chunk
  - `mcp__quill__quill_execute` / `mcp__quill__quill_execute_file` — Run analysis while returning only compact results
  - `mcp__quill__quill_batch_execute` — Run multiple labeled commands and index large outputs
  - `mcp__quill__quill_fetch_and_index` — Fetch web content into Quill context storage instead of dumping pages
  - `mcp__quill__quill_context_stats` — Inspect Quill context storage usage
  - `mcp__quill__quill_record_continuity_event` — Record a compact task/decision continuity event
  - `mcp__quill__quill_create_compaction_snapshot` / `mcp__quill__quill_get_compaction_snapshot` — Save or retrieve compact resume snapshots
  - `mcp__quill__list_projects` — List all projects with session counts
  - `mcp__quill__list_sessions` — List sessions with metadata (filter by project, date)
  - `mcp__quill__get_session_overview` — Preview a session (first message, tools, files)
  - `mcp__quill__search_history` — Full-text search across all session history
  - `mcp__quill__get_session_context` — Get surrounding messages around a search hit
  - `mcp__quill__get_file_history` — All tool actions on a file across sessions
  - `mcp__quill__get_branch_activity` — Work done on a specific git branch
  - `mcp__quill__find_related_sessions` — Sessions that share files with a given session
  - `mcp__quill__get_token_usage` — Token usage analytics by period (1h/24h/7d/30d)
  - `mcp__quill__get_learned_rules` — Learned behavioral rules from past sessions
  - `mcp__quill__get_tool_details` — Full tool input/output for a specific message
  - `mcp__quill__get_index_status` — Index and database health stats

- **Workflow**: For large current-context work, index or execute with the `quill_*` context tools,
  then search/retrieve focused chunks. For session history, browse (`list_projects`/`list_sessions`)
  → search (`search_history`) → cross-reference (`get_file_history`/`get_branch_activity`) → drill
  down (`get_session_context`/`get_tool_details`).

- **Context Preservation**: Quill hooks may provide a compact `<quill_continuity>` directive
  at session start with recent prompts, decisions, tasks, and the best Quill MCP tools to use.
  Treat it as resume context; continue the current task when it is relevant.

- **Routing Behavior**: Prefer Quill MCP tools for prior work, transcript details, token usage,
  file/session history, web fetches, and large command/file analysis. Avoid raw transcript reads,
  broad Read/Grep dumps, WebFetch page dumps, and unbounded `curl`/`wget`; summarize large
  shell/build output before bringing it into context.

- **Use When**: User asks about past sessions, previous work, conversation history, "what did we do",
  token usage/costs, or which sessions touched a specific file or branch.

- **Do NOT**: Read raw JSONL session logs from `~/.claude/projects/` — use Quill instead.
