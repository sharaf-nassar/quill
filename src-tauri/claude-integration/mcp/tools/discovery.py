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
async def list_projects() -> dict:
    """List all projects with session counts. Use this first to discover
    available projects before searching."""
    client = await get_http_client()
    resp = await client.get("/api/v1/sessions/facets")
    resp.raise_for_status()
    return resp.json()


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def list_sessions(
    project: Annotated[
        str | None, Field(description="Filter by project working directory (cwd)")
    ] = None,
    date_from: Annotated[
        str | None, Field(description="Start date (YYYY-MM-DD)")
    ] = None,
    date_to: Annotated[
        str | None, Field(description="End date (YYYY-MM-DD)")
    ] = None,
    limit: Annotated[
        int, Field(description="Max sessions to return", ge=1, le=100)
    ] = 20,
) -> list[dict]:
    """List sessions with metadata: ID, hostname, time range, turn count,
    total tokens, and working directory. Use to browse before drilling in."""
    db = get_db()
    query = """
        SELECT
            session_id,
            hostname,
            cwd,
            MIN(timestamp) as first_seen,
            MAX(timestamp) as last_active,
            COUNT(*) as turn_count,
            SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens
        FROM token_snapshots
        WHERE 1=1
    """
    params: list = []

    if project:
        query += " AND cwd LIKE ?"
        params.append(f"%{project}%")
    if date_from:
        query += " AND timestamp >= ?"
        params.append(date_from)
    if date_to:
        query += " AND timestamp <= ?"
        params.append(date_to)

    query += " GROUP BY session_id ORDER BY last_active DESC LIMIT ?"
    params.append(limit)

    rows = db.execute(query, params).fetchall()
    return [dict(row) for row in rows]


@mcp.tool(annotations=READONLY_ANNOTATIONS)
async def get_session_overview(
    session_id: Annotated[str, Field(description="Session ID to get overview for")],
) -> dict:
    """Get a session overview: first user message, tools used, files touched,
    duration, and token totals. Use after list_sessions to preview a session."""
    client = await get_http_client()
    db = get_db()

    # 1. Get first user message via Tantivy search (filtered by session_id)
    resp = await client.get(
        "/api/v1/sessions/search",
        params={
            "q": "",
            "session_id": session_id,
            "role": "user",
            "page_size": "1",
        },
    )
    first_message = None
    if resp.status_code == 200:
        data = resp.json()
        hits = data.get("hits", [])
        if hits:
            first_message = hits[0].get("content", "")

    # 2. Get tools and files from tool_actions
    actions = db.execute(
        "SELECT DISTINCT tool_name FROM tool_actions WHERE session_id = ?",
        [session_id],
    ).fetchall()
    tools = [row["tool_name"] for row in actions]

    files = db.execute(
        "SELECT DISTINCT file_path FROM tool_actions WHERE session_id = ? AND file_path IS NOT NULL",
        [session_id],
    ).fetchall()
    files_touched = [row["file_path"] for row in files]

    # 3. Get token totals
    stats = db.execute(
        """SELECT
            MIN(timestamp) as first_seen,
            MAX(timestamp) as last_active,
            COUNT(*) as turn_count,
            SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens
        FROM token_snapshots WHERE session_id = ?""",
        [session_id],
    ).fetchone()

    return {
        "session_id": session_id,
        "first_message": first_message,
        "tools_used": tools,
        "files_touched": files_touched,
        "first_seen": stats["first_seen"] if stats else None,
        "last_active": stats["last_active"] if stats else None,
        "turn_count": stats["turn_count"] if stats else 0,
        "total_tokens": stats["total_tokens"] if stats else 0,
    }


@mcp.tool(annotations=READONLY_ANNOTATIONS)
async def get_index_status() -> dict:
    """Get the status of the session index and database: total indexed messages,
    projects, sessions, and token snapshot counts."""
    client = await get_http_client()
    db = get_db()

    # Facets give us project/host counts from Tantivy
    resp = await client.get("/api/v1/sessions/facets")
    facets = resp.json() if resp.status_code == 200 else {}

    total_projects = len(facets.get("projects", []))
    total_indexed_messages = sum(
        p.get("count", 0) for p in facets.get("projects", [])
    )

    # DB stats
    session_count = db.execute(
        "SELECT COUNT(DISTINCT session_id) as c FROM token_snapshots"
    ).fetchone()
    snapshot_count = db.execute(
        "SELECT COUNT(*) as c FROM token_snapshots"
    ).fetchone()
    action_count = db.execute(
        "SELECT COUNT(*) as c FROM tool_actions"
    ).fetchone()

    return {
        "indexed_messages": total_indexed_messages,
        "projects": total_projects,
        "sessions": session_count["c"] if session_count else 0,
        "token_snapshots": snapshot_count["c"] if snapshot_count else 0,
        "tool_actions": action_count["c"] if action_count else 0,
    }
