<!-- quill-v1.11.0 -->
### Session History Search (Quill MCP)

- **Session History Tool**: Use the Quill MCP tools to search past Claude Code session history.
  Quill indexes all sessions into a searchable database with full-text search across conversation
  content, code changes, commands run, and tool usage.

- **MCP Tools** (use these directly):
  - `mcp__plugin_quill-hook_quill__list_projects` — List all projects with session counts
  - `mcp__plugin_quill-hook_quill__list_sessions` — List sessions with metadata (filter by project, date)
  - `mcp__plugin_quill-hook_quill__get_session_overview` — Preview a session (first message, tools, files)
  - `mcp__plugin_quill-hook_quill__search_history` — Full-text search across all session history
  - `mcp__plugin_quill-hook_quill__get_session_context` — Get surrounding messages around a search hit
  - `mcp__plugin_quill-hook_quill__get_file_history` — All tool actions on a file across sessions
  - `mcp__plugin_quill-hook_quill__get_branch_activity` — Work done on a specific git branch
  - `mcp__plugin_quill-hook_quill__find_related_sessions` — Sessions that share files with a given session
  - `mcp__plugin_quill-hook_quill__get_token_usage` — Token usage analytics by period (1h/24h/7d/30d)
  - `mcp__plugin_quill-hook_quill__get_learned_rules` — Learned behavioral rules from past sessions
  - `mcp__plugin_quill-hook_quill__get_tool_details` — Full tool input/output for a specific message
  - `mcp__plugin_quill-hook_quill__get_index_status` — Index and database health stats

- **Workflow**: Use progressive disclosure — browse (`list_projects`/`list_sessions`) → search
  (`search_history`) → cross-reference (`get_file_history`/`get_branch_activity`) → drill down
  (`get_session_context`/`get_tool_details`).

- **Use When**: User asks about past sessions, previous work, conversation history, "what did we do",
  token usage/costs, or which sessions touched a specific file or branch.

- **Do NOT**: Read raw JSONL session logs from `~/.claude/projects/` — use Quill instead.
