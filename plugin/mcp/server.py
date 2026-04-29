from __future__ import annotations

import sys

# When run as `python server.py`, Python loads this file as __main__.
# Tool modules do `from server import mcp` which would import server.py a
# second time (as the "server" module), creating a duplicate FastMCP instance.
# Tools register on that second copy, while __main__.mcp.run() serves the
# first copy — resulting in 0 tools.  Fix: alias __main__ as "server" so
# all imports resolve to the single module instance.
if __name__ == "__main__":
    sys.modules.setdefault("server", sys.modules[__name__])

from fastmcp import FastMCP

from dependencies import lifespan

mcp = FastMCP(
    name="Quill",
    instructions="""\
Quill indexes all Claude Code session history into a searchable database.
It also stores large working-context output in a local searchable context
store. ALWAYS use Quill tools instead of reading raw JSONL session logs from
~/.claude/projects/ or dumping large command/web/file output into the
conversation. Quill returns focused snippets and stable refs instead.

## When to use Quill

- User asks about past sessions, previous work, or conversation history
- User asks "what did I/we do" or "when did I/we" questions
- You need context from a prior session (e.g. how something was implemented)
- User asks about token usage, costs, or session analytics
- You need to find which sessions touched a specific file or branch
- User asks what files were edited, what commands were run, or what tools were used
- You need to analyze large command output, files, fetched web pages, or generated logs
- A Quill hook provides a `<quill_continuity>` directive with recent task/decision context

## What Quill Indexes

Session history is enriched with:

- **code_changes** — summaries of Edit and Write tool calls (file path + what changed)
- **commands_run** — Bash commands and their truncated output
- **tool_details** — Read, Grep, Glob, and Agent tool calls with paths/queries
- **tool_actions** — full tool input/output stored in SQLite for deep inspection

Working context is indexed separately by the `quill_*` tools:

- **quill_index_context** — index large text or files
- **quill_execute / quill_execute_file / quill_batch_execute** — run bounded analysis
  and index large output
- **quill_fetch_and_index** — fetch web content into Quill context storage
- **quill_search_context / quill_get_context_source** — retrieve focused chunks by ref
- **quill_record_continuity_event / quill_create_compaction_snapshot** — preserve
  decisions, tasks, and compact resume snapshots

## Workflow: progressive disclosure (cheap → expensive)

1. **Working context** — use `quill_index_context`, `quill_execute`,
   `quill_batch_execute`, or `quill_fetch_and_index` when output may be large
2. **Focused retrieval** — use `quill_search_context` and `quill_get_context_source`
   to bring back only relevant chunks
3. **Browse history** — `list_projects` or `list_sessions` to orient
4. **Search history** — `search_history` to find messages by content, edits, commands, or tool use
5. **Cross-reference** — `get_file_history` for all actions on a file across sessions,
   `get_branch_activity` for work on a git branch, `find_related_sessions` for sessions
   that touched the same files
6. **Drill down** — `get_session_context` for surrounding messages,
   `get_tool_details` for full tool input/output (the raw data)

## Search result fields

Each search hit includes: content, code_changes, commands_run, tool_details,
tools_used, files_modified, role, project, host, timestamp, git_branch.
Use these fields to answer questions without needing to drill deeper.

## Do NOT

- Read files from ~/.claude/projects/*/*.jsonl directly — use Quill instead
- Dump full web pages, broad grep/read output, or large build logs into the transcript
- Fetch all sessions at once — use filters (project, date, branch) to narrow
- Return raw tool output to the user — summarize the relevant findings
""",
    lifespan=lifespan,
)

from tools import analytics, context, details, discovery, search  # noqa: E402, F401

if __name__ == "__main__":
    mcp.run()
