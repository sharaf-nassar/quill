from __future__ import annotations

import sys
import os

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

CONTEXT_PRESERVATION_ENABLED = os.environ.get("QUILL_CONTEXT_PRESERVATION") == "1"

BASE_INSTRUCTIONS = """\
Quill indexes all Claude Code and Codex session history into a searchable
database. ALWAYS use Quill tools instead of reading raw JSONL session logs from
~/.claude/projects/ or ~/.codex/sessions/ directly. Quill returns focused
snippets and stable session references.

## When to use Quill

- User asks about past sessions, previous work, or conversation history
- User asks "what did I/we do" or "when did I/we" questions
- You need context from a prior session, such as how something was implemented
- User asks about token usage, costs, or session analytics
- You need to find which sessions touched a specific file or branch
- User asks what files were edited, what commands were run, or what tools were used

## What Quill Indexes

Session history is enriched with:

- code_changes: summaries of Edit and Write tool calls
- commands_run: Bash commands and truncated output
- tool_details: Read, Grep, Glob, and Agent tool calls with paths/queries
- tool_actions: full tool input/output stored in SQLite for deep inspection

## Workflow: progressive disclosure

1. Browse history with list_projects or list_sessions to orient
2. Search history with search_history to find messages by content, edits,
   commands, or tool use
3. Cross-reference with get_file_history, get_branch_activity, or
   find_related_sessions
4. Drill down with get_session_context or get_tool_details only when needed

## Do NOT

- Read raw session logs directly when Quill is available
- Fetch all sessions at once; use filters such as project, date, and branch
- Return raw tool output to the user; summarize the relevant findings
"""

CONTEXT_INSTRUCTIONS = """\

Quill also stores large working-context output in a local searchable context
store. Use Quill tools instead of dumping large command, web, file, or build
output into the conversation.

## Working context tools

- quill_index_context: index large text, files, or command output
- quill_execute / quill_execute_file / quill_batch_execute: run bounded
  analysis and index large output
- quill_fetch_and_index: fetch web content into Quill context storage
- quill_search_context / quill_get_context_source: retrieve focused chunks by ref
- quill_record_continuity_event / quill_create_compaction_snapshot: preserve
  decisions, tasks, and compact resume snapshots

Use working context tools before broad Read/Grep dumps, WebFetch page dumps, or
unbounded curl/wget output.
"""

mcp = FastMCP(
    name="Quill",
    instructions=BASE_INSTRUCTIONS
    + (CONTEXT_INSTRUCTIONS if CONTEXT_PRESERVATION_ENABLED else ""),
    lifespan=lifespan,
)

from tools import analytics, details, discovery, search  # noqa: E402, F401

if CONTEXT_PRESERVATION_ENABLED:
    from tools import context  # noqa: E402, F401

if __name__ == "__main__":
    mcp.run()
