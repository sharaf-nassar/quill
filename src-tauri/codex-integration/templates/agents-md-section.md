<!-- quill-managed:codex:start -->
## Quill Session History

- Past-session lookups go through the Quill MCP `search_history` tool. Never read raw JSONL
  session logs from `~/.codex/sessions/` directly.
- Route large transient output (command, web, file, or build dumps) through the Quill
  working-context tools (`quill_execute`, `quill_index_context`, `quill_fetch_and_index`,
  `quill_search_context`) instead of pasting it into the conversation.
- Quill hooks may inject a `<quill_continuity>` directive at session start; treat it as
  resume context instead of asking the user to repeat recent work.
<!-- quill-managed:codex:end -->
