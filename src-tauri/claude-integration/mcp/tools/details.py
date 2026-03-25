from __future__ import annotations

from typing import Annotated

from pydantic import Field

from dependencies import get_db
from server import mcp

READONLY_ANNOTATIONS = {
    "readOnlyHint": True,
    "destructiveHint": False,
    "idempotentHint": True,
    "openWorldHint": False,
}


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def get_tool_details(
    message_id: Annotated[
        str, Field(description="Message ID to retrieve tool actions for")
    ],
) -> list[dict]:
    """Full tool input/output for all tool actions in a message. Returns tool_name,
    category (code_change/command/tool_detail), file_path, summary, full_input,
    full_output, and timestamp. Use after search_history when you need the complete
    raw data of what a tool did — e.g. the exact code written, full command output,
    or complete file contents that were read."""
    db = get_db()
    rows = db.execute(
        "SELECT * FROM tool_actions WHERE message_id = ?",
        [message_id],
    ).fetchall()
    return [dict(row) for row in rows]
