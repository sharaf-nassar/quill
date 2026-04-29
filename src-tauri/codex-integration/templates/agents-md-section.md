<!-- quill-managed:codex:start -->
## Quill Session History

Use the Quill MCP tools to search and inspect previous Codex sessions instead of reading raw
`~/.codex/sessions/*` files directly.

- Prefer Quill MCP history/search tools when the user asks about past sessions, prior work, tool
  usage, or token history.
- Do not read raw Codex JSONL session logs directly when Quill is available.
- Quill context hooks may inject a compact `<quill_continuity>` directive at session start with
  recent prompts, decisions, and task hints. Use it to resume relevant work without asking the user
  to repeat context.
- Prefer `mcp__quill__quill_search_context`, `mcp__quill__quill_execute`,
  `mcp__quill__quill_execute_file`, `mcp__quill__quill_batch_execute`,
  `mcp__quill__quill_fetch_and_index`, `mcp__quill__search_history`,
  `mcp__quill__get_session_context`, and `mcp__quill__get_tool_details` before broad
  raw file/session inspection.
- Avoid WebFetch page dumps and unbounded `curl`/`wget`; keep build, Bash, Read, and Grep output
  summarized when it may flood the conversation.
<!-- quill-managed:codex:end -->
