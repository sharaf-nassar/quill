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
ALWAYS use these tools instead of reading raw JSONL session logs from
~/.claude/projects/. The Quill index is faster, pre-processed, and returns
structured results — raw logs are large, unindexed, and waste context window.

## When to use Quill

- User asks about past sessions, previous work, or conversation history
- User asks "what did I/we do" or "when did I/we" questions
- You need context from a prior session (e.g. how something was implemented)
- User asks about token usage, costs, or session analytics
- You need to find which sessions touched a specific file or branch
- User asks what files were edited, what commands were run, or what tools were used

## What Quill indexes

Quill doesn't just index conversation text. Every message is enriched with:

- **code_changes** — summaries of Edit and Write tool calls (file path + what changed)
- **commands_run** — Bash commands and their truncated output
- **tool_details** — Read, Grep, Glob, and Agent tool calls with paths/queries
- **tool_actions** — full tool input/output stored in SQLite for deep inspection

All of these are full-text searchable. Searching for "edit server.py" finds
messages where server.py was edited. Searching for "cargo build" finds
messages where that command was run. Searching for "grep auth" finds
messages where auth-related searches happened.

## Workflow: progressive disclosure (cheap → expensive)

1. **Browse** — `list_projects` or `list_sessions` to orient
2. **Search** — `search_history` to find messages by content, edits, commands, or tool use
3. **Cross-reference** — `get_file_history` for all actions on a file across sessions,
   `get_branch_activity` for work on a git branch, `find_related_sessions` for sessions
   that touched the same files
4. **Drill down** — `get_session_context` for surrounding messages,
   `get_tool_details` for full tool input/output (the raw data)

## Search result fields

Each search hit includes: content, code_changes, commands_run, tool_details,
tools_used, files_modified, role, project, host, timestamp, git_branch.
Use these fields to answer questions without needing to drill deeper.

## Do NOT

- Read files from ~/.claude/projects/*/*.jsonl directly — use Quill instead
- Fetch all sessions at once — use filters (project, date, branch) to narrow
- Return raw tool output to the user — summarize the relevant findings
""",
    lifespan=lifespan,
)

from tools import analytics, details, discovery, search  # noqa: E402, F401

if __name__ == "__main__":
    mcp.run()
