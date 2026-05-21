### Session History Search (Quill MCP)

- **Session History Tool**: Use the Quill MCP `search_history` tool to look up past
  Claude Code session history. Quill indexes all sessions into a searchable database
  with full-text search across conversation content, code changes, commands run, and
  tool usage.

- **Tool**: `mcp__quill__search_history` — full-text search across all session
  history. Filter with `project`, `git_branch`, `role`, `host`, `date_from`,
  `date_to`, `limit`. Search hits include code_changes, commands_run, tool_details,
  and files_modified metadata.

- **Use When**: User asks about past sessions, previous work, conversation history,
  "what did we do", or which sessions touched a specific file, branch, or command.

- **Do NOT**: Read raw JSONL session logs from `~/.claude/projects/` — use
  `search_history` instead.
