from __future__ import annotations

from typing import Annotated

from pydantic import Field

from dependencies import get_db, get_http_client
from server import mcp

READONLY_ANNOTATIONS = {
    "readOnlyHint": True,
    "destructiveHint": False,
    "idempotentHint": True,
    "openWorldHint": False,
}


@mcp.tool(annotations=READONLY_ANNOTATIONS)
async def search_history(
    query: Annotated[str, Field(description="Full-text search query string")],
    project: Annotated[
        str | None, Field(description="Filter by project working directory (cwd)")
    ] = None,
    host: Annotated[
        str | None, Field(description="Filter by hostname")
    ] = None,
    role: Annotated[
        str | None, Field(description="Filter by message role (user or assistant)")
    ] = None,
    git_branch: Annotated[
        str | None, Field(description="Filter by git branch name")
    ] = None,
    date_from: Annotated[
        str | None, Field(description="Start date (YYYY-MM-DD)")
    ] = None,
    date_to: Annotated[
        str | None, Field(description="End date (YYYY-MM-DD)")
    ] = None,
    limit: Annotated[
        int, Field(description="Max results to return", ge=1, le=50)
    ] = 10,
) -> dict:
    """Full-text search across conversation history, code changes (Edit/Write),
    commands run (Bash), and tool details (Read/Grep/Glob/Agent). Returns
    matching messages with all indexed fields. Search for file names, commands,
    error messages, or any conversation content."""
    client = await get_http_client()
    params: dict = {"q": query, "page_size": str(limit)}
    if project is not None:
        params["project"] = project
    if host is not None:
        params["host"] = host
    if role is not None:
        params["role"] = role
    if git_branch is not None:
        params["git_branch"] = git_branch
    if date_from is not None:
        params["date_from"] = date_from
    if date_to is not None:
        params["date_to"] = date_to
    resp = await client.get("/api/v1/sessions/search", params=params)
    resp.raise_for_status()
    return resp.json()


@mcp.tool(annotations=READONLY_ANNOTATIONS)
async def get_session_context(
    session_id: Annotated[str, Field(description="Session ID to retrieve context from")],
    message_id: Annotated[str, Field(description="Message ID to center the context window on")],
    window: Annotated[
        int, Field(description="Number of messages before and after to include", ge=1, le=20)
    ] = 5,
) -> dict:
    """Retrieve the context window around a specific message in a session.
    Returns surrounding messages to understand the full conversation flow."""
    client = await get_http_client()
    params = {
        "session_id": session_id,
        "message_id": message_id,
        "window": str(window),
    }
    resp = await client.get("/api/v1/sessions/context", params=params)
    resp.raise_for_status()
    return resp.json()


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def get_file_history(
    file_path: Annotated[str, Field(description="File path (or partial path) to look up history for")],
    limit: Annotated[
        int, Field(description="Max tool actions to return", ge=1, le=100)
    ] = 50,
) -> list[dict]:
    """Get all tool actions on a file across all sessions — edits, writes, reads,
    greps, and any other tool that touched it. Returns session_id, tool_name,
    category, summary, and timestamp. Use to understand a file's history."""
    db = get_db()
    rows = db.execute(
        """SELECT session_id, tool_name, category, summary, timestamp
        FROM tool_actions
        WHERE file_path LIKE ?
        ORDER BY timestamp DESC
        LIMIT ?""",
        [f"%{file_path}%", limit],
    ).fetchall()
    return [dict(row) for row in rows]


@mcp.tool(annotations=READONLY_ANNOTATIONS)
async def get_branch_activity(
    branch_name: Annotated[str, Field(description="Git branch name to filter activity by")],
    project: Annotated[
        str | None, Field(description="Filter by project working directory (cwd)")
    ] = None,
    limit: Annotated[
        int, Field(description="Max results to return", ge=1, le=50)
    ] = 20,
) -> dict:
    """Get conversation history filtered by a specific git branch. Useful to
    review what work was done on a particular branch across sessions."""
    client = await get_http_client()
    params: dict = {"q": "", "git_branch": branch_name, "page_size": str(limit)}
    if project is not None:
        params["project"] = project
    resp = await client.get("/api/v1/sessions/search", params=params)
    resp.raise_for_status()
    return resp.json()


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def find_related_sessions(
    session_id: Annotated[str, Field(description="Session ID to find related sessions for")],
    limit: Annotated[
        int, Field(description="Max related sessions to return", ge=1, le=20)
    ] = 10,
) -> list[dict]:
    """Find sessions that share files with the given session. Returns sessions
    sorted by the number of shared files, highest first."""
    db = get_db()

    file_rows = db.execute(
        "SELECT DISTINCT file_path FROM tool_actions WHERE session_id = ? AND file_path IS NOT NULL",
        [session_id],
    ).fetchall()
    file_paths = [row["file_path"] for row in file_rows]

    if not file_paths:
        return []

    placeholders = ", ".join("?" * len(file_paths))
    rows = db.execute(
        f"""SELECT session_id, COUNT(DISTINCT file_path) as shared_files
        FROM tool_actions
        WHERE file_path IN ({placeholders})
          AND session_id != ?
        GROUP BY session_id
        ORDER BY shared_files DESC
        LIMIT ?""",
        [*file_paths, session_id, limit],
    ).fetchall()
    return [dict(row) for row in rows]
