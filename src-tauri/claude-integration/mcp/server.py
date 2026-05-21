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
Quill indexes Claude Code and Codex session history into a searchable database.
Use `search_history` to find prior conversation content, code changes, commands,
or tool calls — do not read raw JSONL session logs from ~/.claude/projects/ or
~/.codex/sessions/ directly.

## When to use search_history

- User asks about past sessions, previous work, or conversation history
- User asks "what did I/we do" or "when did I/we" questions
- You need context from a prior session, such as how something was implemented
- You need to find which sessions touched a specific file, command, or error

Filter with `project`, `git_branch`, `role`, or date range to narrow results.
Summarize hits rather than returning raw output to the user.
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

from tools import search  # noqa: E402, F401

if CONTEXT_PRESERVATION_ENABLED:
    from tools import context  # noqa: E402, F401

if __name__ == "__main__":
    mcp.run()
