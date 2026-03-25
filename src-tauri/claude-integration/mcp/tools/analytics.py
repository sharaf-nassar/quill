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

_PERIOD_TO_HOURS: dict[str, int] = {
    "1h": 1,
    "24h": 24,
    "7d": 168,
    "30d": 720,
}


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def get_token_usage(
    period: Annotated[
        str,
        Field(
            description="Time period to aggregate over: '1h', '24h', '7d', or '30d'",
        ),
    ] = "7d",
    project: Annotated[
        str | None,
        Field(description="Filter by project working directory (cwd)"),
    ] = None,
) -> list[dict]:
    """Token usage breakdown grouped by session for the given time period.
    Returns input, output, cache creation, and cache read tokens per session."""
    hours = _PERIOD_TO_HOURS.get(period, 168)
    db = get_db()

    query = """
        SELECT
            session_id,
            cwd,
            hostname,
            COUNT(*) as snapshot_count,
            SUM(input_tokens) as input_tokens,
            SUM(output_tokens) as output_tokens,
            SUM(cache_creation_input_tokens) as cache_creation_input_tokens,
            SUM(cache_read_input_tokens) as cache_read_input_tokens,
            SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens,
            MIN(timestamp) as first_seen,
            MAX(timestamp) as last_active
        FROM token_snapshots
        WHERE timestamp >= datetime('now', ?)
    """
    params: list = [f"-{hours} hours"]

    if project:
        query += " AND cwd LIKE ?"
        params.append(f"%{project}%")

    query += " GROUP BY session_id ORDER BY total_tokens DESC"

    rows = db.execute(query, params).fetchall()
    return [dict(row) for row in rows]


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def get_learned_rules(
    domain: Annotated[
        str | None,
        Field(description="Filter by rule domain (e.g. 'python', 'testing')"),
    ] = None,
    state: Annotated[
        str | None,
        Field(
            description="Filter by rule state (e.g. 'emerging', 'confirmed', 'suppressed')"
        ),
    ] = None,
) -> list[dict]:
    """Learned behavioral rules extracted from past sessions, ordered by
    confidence. Optionally filter by domain or state."""
    db = get_db()

    query = """
        SELECT
            id,
            name,
            domain,
            confidence,
            observation_count,
            file_path,
            state,
            project,
            is_anti_pattern,
            alpha,
            beta_param,
            last_evidence_at,
            created_at,
            updated_at
        FROM learned_rules
        WHERE 1=1
    """
    params: list = []

    if domain:
        query += " AND domain = ?"
        params.append(domain)
    if state:
        query += " AND state = ?"
        params.append(state)

    query += " ORDER BY confidence DESC"

    rows = db.execute(query, params).fetchall()
    return [dict(row) for row in rows]
