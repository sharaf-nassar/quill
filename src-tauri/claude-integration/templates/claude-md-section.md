### Session History (Quill)

- Past-session lookups go through the Quill MCP `search_history` tool. Never read raw JSONL
  session logs from `~/.claude/projects/` directly.
- Route large transient output (command, web, file, or build dumps) through the Quill
  working-context tools instead of pasting it into the conversation; the MCP server's own
  instructions cover usage.
