from __future__ import annotations

from typing import Annotated

from pydantic import Field

from dependencies import get_http_client
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
