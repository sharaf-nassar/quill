from __future__ import annotations

import hashlib
import html
import ipaddress
import json
import math
import os
import re
import signal
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from html.parser import HTMLParser
from pathlib import Path
from typing import Annotated, Any, Literal
from urllib import request as urllib_request
from urllib.parse import urljoin, urlparse

import httpx
from pydantic import Field

from server import mcp

READONLY_ANNOTATIONS = {
    "readOnlyHint": True,
    "destructiveHint": False,
    "idempotentHint": True,
    "openWorldHint": False,
}

WRITE_ANNOTATIONS = {
    "readOnlyHint": False,
    "destructiveHint": False,
    "idempotentHint": False,
    "openWorldHint": False,
}

EXECUTION_ANNOTATIONS = {
    "readOnlyHint": False,
    "destructiveHint": True,
    "idempotentHint": False,
    "openWorldHint": True,
}

FETCH_ANNOTATIONS = {
    "readOnlyHint": False,
    "destructiveHint": False,
    "idempotentHint": False,
    "openWorldHint": True,
}

DESTRUCTIVE_ANNOTATIONS = {
    "readOnlyHint": False,
    "destructiveHint": True,
    "idempotentHint": False,
    "openWorldHint": False,
}

CONTEXT_DIR = Path.home() / ".config" / "quill" / "context"
CONTEXT_DB = CONTEXT_DIR / "context.db"
QUILL_CONFIG = Path.home() / ".config" / "quill" / "config.json"
CONTEXT_SAVINGS_ENDPOINT = "/api/v1/context-savings/events"
CONTEXT_SAVINGS_SCHEMA_VERSION = 1
CONTEXT_SAVINGS_TIMEOUT_SECONDS = 1.0
MAX_INDEX_BYTES = 5 * 1024 * 1024
MAX_FETCH_BYTES = 2 * 1024 * 1024
MAX_OUTPUT_BYTES = 512 * 1024
MAX_RESPONSE_PREVIEW = 4096
LARGE_OUTPUT_THRESHOLD = 12 * 1024
CHUNK_TARGET_BYTES = 8192
CHUNK_OVERLAP_LINES = 4
FETCH_TTL = timedelta(hours=24)

_db_conn: sqlite3.Connection | None = None
_db_lock = threading.RLock()
_fts_available: bool | None = None


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def _byte_len(value: Any) -> int:
    if value is None:
        return 0
    if isinstance(value, bytes):
        return len(value)
    if isinstance(value, str):
        text = value
    else:
        text = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), default=str)
    return len(text.encode("utf-8", errors="replace"))


def _json_bytes(value: Any) -> int:
    return _byte_len(json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), default=str))


def _nullable_int(value: int | float | None) -> int | None:
    if value is None:
        return None
    try:
        return max(0, int(value))
    except (TypeError, ValueError):
        return None


def _tokens_from_bytes(byte_count: int | None) -> int:
    byte_count = _nullable_int(byte_count)
    return 0 if byte_count is None else math.ceil(byte_count / 4)


def _context_savings_provider() -> str:
    if os.environ.get("QUILL_PROVIDER"):
        return str(os.environ["QUILL_PROVIDER"])
    if os.environ.get("CODEX_SESSION_ID"):
        return "codex"
    return "claude"


def _context_savings_session_id(explicit: str | None = None) -> str | None:
    if explicit:
        return explicit
    for name in (
        "QUILL_SESSION_ID",
        "CLAUDE_SESSION_ID",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_SESSION_ID",
        "SESSION_ID",
        "CONVERSATION_ID",
    ):
        value = os.environ.get(name)
        if value:
            return value
    return None


def _context_savings_hostname() -> str:
    config = _context_savings_config()
    if config and config.get("hostname"):
        return str(config["hostname"])
    return socket.gethostname()


def _context_savings_event_id(event: dict) -> str:
    raw = json.dumps(event, ensure_ascii=False, sort_keys=True, separators=(",", ":"), default=str)
    return "ctx_" + hashlib.sha256(raw.encode("utf-8", errors="replace")).hexdigest()[:32]


def _context_savings_event(
    *,
    event_type: str,
    source: str,
    decision: str | None = None,
    reason: str | None = None,
    delivered: bool | None = None,
    indexed_bytes: int | None = None,
    returned_bytes: int | None = None,
    input_bytes: int | None = None,
    source_ref: str | None = None,
    snapshot_ref: str | None = None,
    metadata: dict | None = None,
    session_id: str | None = None,
    cwd: str | None = None,
    tokens_saved_est: int | None = None,
    tokens_preserved_est: int | None = None,
) -> dict:
    indexed_bytes = _nullable_int(indexed_bytes)
    returned_bytes = _nullable_int(returned_bytes)
    input_bytes = _nullable_int(input_bytes)
    has_byte_estimate = indexed_bytes is not None or returned_bytes is not None or input_bytes is not None
    saved_bytes = None
    if input_bytes is not None and returned_bytes is not None:
        saved_bytes = max(0, input_bytes - returned_bytes)

    event = {
        "eventId": "",
        "schemaVersion": CONTEXT_SAVINGS_SCHEMA_VERSION,
        "provider": _context_savings_provider(),
        "sessionId": _context_savings_session_id(session_id),
        "hostname": _context_savings_hostname(),
        "cwd": cwd or str(_default_cwd()),
        "timestamp": _now(),
        "eventType": event_type,
        "source": source or "context",
        "decision": decision or "recorded",
        "reason": reason,
        "delivered": True if delivered is None else delivered,
        "indexedBytes": indexed_bytes,
        "returnedBytes": returned_bytes,
        "inputBytes": input_bytes,
        "tokensIndexedEst": _tokens_from_bytes(indexed_bytes),
        "tokensReturnedEst": _tokens_from_bytes(returned_bytes),
        "tokensSavedEst": tokens_saved_est if tokens_saved_est is not None else _tokens_from_bytes(saved_bytes),
        "tokensPreservedEst": (
            tokens_preserved_est if tokens_preserved_est is not None else _tokens_from_bytes(indexed_bytes)
        ),
        "estimateMethod": "ceil_bytes_div_4" if has_byte_estimate else "none",
        "estimateConfidence": 1 if has_byte_estimate else 0,
        "sourceRef": source_ref,
        "snapshotRef": snapshot_ref,
        "metadata": metadata or {},
    }
    event["eventId"] = _context_savings_event_id(event)
    return event


def _context_savings_summary(event: dict) -> dict:
    keys = (
        "eventId",
        "eventType",
        "indexedBytes",
        "returnedBytes",
        "inputBytes",
        "tokensIndexedEst",
        "tokensReturnedEst",
        "tokensSavedEst",
        "tokensPreservedEst",
        "estimateMethod",
        "estimateConfidence",
        "sourceRef",
        "snapshotRef",
    )
    return {key: event.get(key) for key in keys}


def _context_savings_config() -> dict | None:
    try:
        config = json.loads(QUILL_CONFIG.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    if not config.get("url") or not config.get("secret"):
        return None
    return config


def _post_context_savings_events_sync(events: list[dict]) -> None:
    config = _context_savings_config()
    if not config:
        return
    try:
        url = f"{str(config['url']).rstrip('/')}{CONTEXT_SAVINGS_ENDPOINT}"
        body = json.dumps({"events": events}, separators=(",", ":")).encode("utf-8")
        req = urllib_request.Request(
            url,
            data=body,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {config['secret']}",
                "Content-Length": str(len(body)),
            },
        )
        with urllib_request.urlopen(req, timeout=CONTEXT_SAVINGS_TIMEOUT_SECONDS) as resp:
            resp.read(1)
    except Exception as err:  # best-effort telemetry must never affect MCP tools
        if os.environ.get("QUILL_DEBUG"):
            print(f"context-savings telemetry error: {err}", file=sys.stderr)


def _emit_context_savings_events(events: list[dict]) -> None:
    clean_events = [event for event in events if event]
    if not clean_events:
        return
    thread = threading.Thread(target=_post_context_savings_events_sync, args=(clean_events,), daemon=True)
    thread.start()


def _attach_context_savings(response: dict, **event_kwargs: Any) -> dict:
    try:
        event = _context_savings_event(returned_bytes=0, **event_kwargs)
    except Exception as err:
        if os.environ.get("QUILL_DEBUG"):
            print(f"context-savings event error: {err}", file=sys.stderr)
        return response
    for _ in range(4):
        response["context_savings"] = _context_savings_summary(event)
        returned_bytes = _json_bytes(response)
        if event["returnedBytes"] == returned_bytes:
            break
        event["returnedBytes"] = returned_bytes
        event["tokensReturnedEst"] = _tokens_from_bytes(returned_bytes)
        if event["inputBytes"] is not None:
            event["tokensSavedEst"] = _tokens_from_bytes(max(0, event["inputBytes"] - returned_bytes))
        else:
            event["tokensSavedEst"] = 0
        event["estimateMethod"] = "ceil_bytes_div_4"
        event["estimateConfidence"] = 1
    response["context_savings"] = _context_savings_summary(event)
    try:
        _emit_context_savings_events([event])
    except Exception as err:
        if os.environ.get("QUILL_DEBUG"):
            print(f"context-savings emit error: {err}", file=sys.stderr)
    return response


def _parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def _context_db() -> sqlite3.Connection:
    global _db_conn
    with _db_lock:
        if _db_conn is None:
            CONTEXT_DIR.mkdir(parents=True, exist_ok=True)
            _db_conn = sqlite3.connect(CONTEXT_DB, check_same_thread=False)
            _db_conn.row_factory = sqlite3.Row
            _db_conn.execute("PRAGMA foreign_keys = ON")
            _db_conn.execute("PRAGMA journal_mode = WAL")
            _db_conn.execute("PRAGMA busy_timeout = 30000")
            _init_schema(_db_conn)
        return _db_conn


def _init_schema(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            label TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL,
            origin TEXT,
            file_path TEXT,
            url TEXT,
            content_hash TEXT,
            content_bytes INTEGER NOT NULL DEFAULT 0,
            chunk_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            metadata_json TEXT
        );

        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            content_type TEXT NOT NULL,
            start_line INTEGER,
            end_line INTEGER,
            byte_length INTEGER NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_id, chunk_index);
        CREATE INDEX IF NOT EXISTS idx_sources_updated ON sources(updated_at DESC);

        CREATE TABLE IF NOT EXISTS executions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            command TEXT NOT NULL,
            cwd TEXT NOT NULL,
            exit_code INTEGER,
            timed_out INTEGER NOT NULL DEFAULT 0,
            duration_ms INTEGER NOT NULL,
            stdout_bytes INTEGER NOT NULL DEFAULT 0,
            stderr_bytes INTEGER NOT NULL DEFAULT 0,
            stdout_truncated INTEGER NOT NULL DEFAULT 0,
            stderr_truncated INTEGER NOT NULL DEFAULT 0,
            output_source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS continuity_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            summary TEXT NOT NULL,
            details TEXT,
            source_refs_json TEXT,
            priority INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_continuity_session
            ON continuity_events(session_id, priority DESC, created_at DESC);

        CREATE TABLE IF NOT EXISTS compaction_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            snapshot TEXT NOT NULL,
            event_count INTEGER NOT NULL DEFAULT 0,
            source_refs_json TEXT,
            created_at TEXT NOT NULL,
            metadata_json TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_snapshots_session
            ON compaction_snapshots(session_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS fetch_cache (
            url TEXT PRIMARY KEY,
            source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL,
            label TEXT NOT NULL,
            content_type TEXT,
            status_code INTEGER,
            etag TEXT,
            last_modified TEXT,
            fetched_at TEXT NOT NULL,
            content_hash TEXT
        );
        """
    )
    _init_fts(conn)
    conn.commit()


def _init_fts(conn: sqlite3.Connection) -> bool:
    global _fts_available
    if _fts_available is not None:
        return _fts_available
    try:
        conn.execute(
            """
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                title,
                content,
                source_id UNINDEXED,
                content_type UNINDEXED,
                tokenize='porter unicode61'
            )
            """
        )
        _fts_available = True
    except sqlite3.Error:
        conn.rollback()
        _fts_available = False
    return _fts_available


def _has_fts(conn: sqlite3.Connection) -> bool:
    return _init_fts(conn)


def _source_ref(source_id: int) -> str:
    return f"source:{source_id}"


def _chunk_ref(chunk_id: int) -> str:
    return f"chunk:{chunk_id}"


def _parse_ref(value: str | int | None, prefix: str) -> int | None:
    if value is None:
        return None
    if isinstance(value, int):
        return value
    text = str(value).strip()
    if text.startswith(f"{prefix}:"):
        text = text.split(":", 1)[1]
    if text.isdigit():
        return int(text)
    return None


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8", errors="replace")).hexdigest()


def _preview(text: str, limit: int = MAX_RESPONSE_PREVIEW) -> dict:
    truncated = len(text.encode("utf-8", errors="replace")) > limit
    raw = text.encode("utf-8", errors="replace")[:limit]
    return {
        "text": raw.decode("utf-8", errors="replace"),
        "truncated": truncated,
    }


def _title_from_lines(lines: list[str], fallback: str) -> str:
    for line in lines:
        stripped = line.strip()
        if stripped:
            return stripped[:120]
    return fallback


def _content_type_for(text: str, requested: str = "auto") -> str:
    if requested != "auto":
        return requested
    code_markers = ("```", "def ", "class ", "function ", "import ", "const ", "let ")
    if any(marker in text for marker in code_markers):
        return "code"
    return "prose"


def _chunk_text(
    text: str,
    content_type: str = "auto",
    target_bytes: int = CHUNK_TARGET_BYTES,
    overlap_lines: int = CHUNK_OVERLAP_LINES,
) -> list[dict]:
    if not text.strip():
        return []

    lines = text.splitlines()
    chunks: list[dict] = []
    start = 0

    while start < len(lines):
        end = start
        size = 0
        while end < len(lines):
            line_bytes = len((lines[end] + "\n").encode("utf-8", errors="replace"))
            if end > start and size + line_bytes > target_bytes:
                break
            size += line_bytes
            end += 1
            if size >= target_bytes:
                break

        if end == start:
            end += 1

        chunk_lines = lines[start:end]
        content = "\n".join(chunk_lines).strip()
        if content:
            title = _title_from_lines(chunk_lines, f"Lines {start + 1}-{end}")
            chunks.append(
                {
                    "chunk_index": len(chunks),
                    "title": title,
                    "content": content,
                    "content_type": _content_type_for(content, content_type),
                    "start_line": start + 1,
                    "end_line": end,
                    "byte_length": len(content.encode("utf-8", errors="replace")),
                }
            )

        if end >= len(lines):
            break
        start = max(end - overlap_lines, start + 1)

    return chunks


def _read_text_file(path: Path, max_bytes: int) -> tuple[str, bool, int]:
    with path.open("rb") as f:
        raw = f.read(max_bytes + 1)
    truncated = len(raw) > max_bytes
    if truncated:
        raw = raw[:max_bytes]
    return raw.decode("utf-8", errors="replace"), truncated, len(raw)


def _delete_sources(conn: sqlite3.Connection, source_ids: list[int]) -> None:
    if not source_ids:
        return
    placeholders = ",".join("?" for _ in source_ids)
    if _has_fts(conn):
        conn.execute(
            f"DELETE FROM chunks_fts WHERE rowid IN "
            f"(SELECT id FROM chunks WHERE source_id IN ({placeholders}))",
            source_ids,
        )
    conn.execute(f"DELETE FROM chunks WHERE source_id IN ({placeholders})", source_ids)
    conn.execute(f"DELETE FROM sources WHERE id IN ({placeholders})", source_ids)


def _insert_source(
    *,
    label: str,
    kind: str,
    content: str,
    origin: str | None = None,
    file_path: str | None = None,
    url: str | None = None,
    content_type: str = "auto",
    metadata: dict | None = None,
) -> dict:
    conn = _context_db()
    chunks = _chunk_text(content, content_type=content_type)
    now = _now()
    content_bytes = len(content.encode("utf-8", errors="replace"))
    content_hash = _sha256_text(content)

    with _db_lock:
        rows = conn.execute("SELECT id FROM sources WHERE label = ?", [label]).fetchall()
        _delete_sources(conn, [int(row["id"]) for row in rows])

        cur = conn.execute(
            """
            INSERT INTO sources (
                label, kind, origin, file_path, url, content_hash, content_bytes,
                chunk_count, created_at, updated_at, metadata_json
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                label,
                kind,
                origin,
                file_path,
                url,
                content_hash,
                content_bytes,
                len(chunks),
                now,
                now,
                json.dumps(metadata or {}, sort_keys=True),
            ],
        )
        source_id = int(cur.lastrowid)
        for chunk in chunks:
            chunk_cur = conn.execute(
                """
                INSERT INTO chunks (
                    source_id, chunk_index, title, content, content_type,
                    start_line, end_line, byte_length, created_at
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    source_id,
                    chunk["chunk_index"],
                    chunk["title"],
                    chunk["content"],
                    chunk["content_type"],
                    chunk["start_line"],
                    chunk["end_line"],
                    chunk["byte_length"],
                    now,
                ],
            )
            chunk_id = int(chunk_cur.lastrowid)
            if _has_fts(conn):
                conn.execute(
                    """
                    INSERT INTO chunks_fts(rowid, title, content, source_id, content_type)
                    VALUES (?, ?, ?, ?, ?)
                    """,
                    [
                        chunk_id,
                        chunk["title"],
                        chunk["content"],
                        source_id,
                        chunk["content_type"],
                    ],
                )
        conn.commit()

    return {
        "source_id": source_id,
        "source_ref": _source_ref(source_id),
        "label": label,
        "kind": kind,
        "content_bytes": content_bytes,
        "chunk_count": len(chunks),
        "content_hash": content_hash,
        "chunks": _chunk_inventory(source_id, limit=5),
    }


def _chunk_inventory(source_id: int, limit: int = 20) -> list[dict]:
    conn = _context_db()
    rows = conn.execute(
        """
        SELECT id, chunk_index, title, content, content_type, byte_length, start_line, end_line
        FROM chunks
        WHERE source_id = ?
        ORDER BY chunk_index
        LIMIT ?
        """,
        [source_id, limit],
    ).fetchall()
    return [
        {
            "chunk_ref": _chunk_ref(int(row["id"])),
            "index": row["chunk_index"],
            "title": row["title"],
            "content_type": row["content_type"],
            "bytes": row["byte_length"],
            "lines": [row["start_line"], row["end_line"]],
            "preview": _preview(row["content"], 400)["text"],
        }
        for row in rows
    ]


def _tokens(query: str) -> list[str]:
    return [
        token.lower()
        for token in re.findall(r"[\w./:-]+", query)
        if len(token.strip()) > 1
    ]


def _fts_query(query: str) -> str:
    words = [re.sub(r"[^\w]", " ", token).strip() for token in _tokens(query)]
    words = [word for word in words if word]
    return " ".join(f'"{word}"' for word in words[:12])


def _like_escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def _source_filter(source: str | None) -> tuple[str, list[Any]]:
    if not source:
        return "", []
    source_id = _parse_ref(source, "source")
    if source_id is not None:
        return " AND s.id = ?", [source_id]
    return " AND s.label LIKE ? ESCAPE '\\'", [f"%{_like_escape(source)}%"]


def _snippet(content: str, query: str, limit: int = 700) -> str:
    terms = _tokens(query)
    lower = content.lower()
    positions = [lower.find(term) for term in terms if lower.find(term) >= 0]
    pos = min(positions) if positions else 0
    start = max(0, pos - limit // 3)
    end = min(len(content), start + limit)
    start = max(0, end - limit)
    snippet = content[start:end].strip()
    snippet = re.sub(r"\s+", " ", snippet)
    if start > 0:
        snippet = "..." + snippet
    if end < len(content):
        snippet += "..."
    return snippet


def _search_context(query: str, limit: int, source: str | None = None) -> dict:
    conn = _context_db()
    effective_limit = max(1, min(limit, 20))
    fts_used = False
    rows: list[sqlite3.Row] = []

    if _has_fts(conn):
        match = _fts_query(query)
        if match:
            clause, params = _source_filter(source)
            try:
                rows = conn.execute(
                    f"""
                    SELECT
                        c.id, c.source_id, s.label, c.title, c.content,
                        c.content_type, c.byte_length, bm25(chunks_fts) AS rank
                    FROM chunks_fts
                    JOIN chunks c ON c.id = chunks_fts.rowid
                    JOIN sources s ON s.id = c.source_id
                    WHERE chunks_fts MATCH ? {clause}
                    ORDER BY rank
                    LIMIT ?
                    """,
                    [match, *params, effective_limit],
                ).fetchall()
                fts_used = True
            except sqlite3.Error:
                rows = []
                fts_used = False

    if not rows:
        tokens = _tokens(query)
        clause, params = _source_filter(source)
        where = "1=1"
        like_params: list[Any] = []
        for token in tokens[:8]:
            pattern = f"%{_like_escape(token)}%"
            where += (
                " AND (LOWER(c.title) LIKE ? ESCAPE '\\' "
                "OR LOWER(c.content) LIKE ? ESCAPE '\\')"
            )
            like_params.extend([pattern, pattern])
        rows = conn.execute(
            f"""
            SELECT c.id, c.source_id, s.label, c.title, c.content,
                   c.content_type, c.byte_length, 0.0 AS rank
            FROM chunks c
            JOIN sources s ON s.id = c.source_id
            WHERE {where} {clause}
            ORDER BY s.updated_at DESC, c.chunk_index
            LIMIT ?
            """,
            [*like_params, *params, effective_limit * 3],
        ).fetchall()
        rows = sorted(
            rows,
            key=lambda row: sum(
                row["content"].lower().count(token) + row["title"].lower().count(token)
                for token in tokens
            ),
            reverse=True,
        )[:effective_limit]

    return {
        "query": query,
        "fts_used": fts_used,
        "results": [
            {
                "source_ref": _source_ref(int(row["source_id"])),
                "chunk_ref": _chunk_ref(int(row["id"])),
                "source": row["label"],
                "title": row["title"],
                "content_type": row["content_type"],
                "bytes": row["byte_length"],
                "snippet": _snippet(row["content"], query),
            }
            for row in rows
        ],
    }


def _default_cwd() -> Path:
    for name in (
        "CLAUDE_PROJECT_DIR",
        "CODEX_PROJECT_DIR",
        "GEMINI_PROJECT_DIR",
        "CONTEXT_MODE_PROJECT_DIR",
        "PWD",
    ):
        value = os.environ.get(name)
        if value:
            path = Path(value).expanduser()
            if path.exists() and path.is_dir():
                return path.resolve()
    return Path.cwd().resolve()


def _resolve_cwd(cwd: str | None) -> Path:
    base = _default_cwd()
    target = Path(cwd).expanduser() if cwd else base
    if not target.is_absolute():
        target = base / target
    target = target.resolve()
    if not target.exists() or not target.is_dir():
        raise ValueError(f"cwd does not exist or is not a directory: {target}")

    home = Path.home().resolve()
    tmp = Path(tempfile.gettempdir()).resolve()
    allowed_roots = (home, tmp)
    if not any(target == root or root in target.parents for root in allowed_roots):
        raise ValueError(f"cwd must be under {home} or {tmp}: {target}")

    blocked = {Path("/"), Path("/bin"), Path("/sbin"), Path("/usr"), Path("/etc"), Path("/dev"), Path("/proc"), Path("/sys")}
    if target in blocked:
        raise ValueError(f"unsafe cwd: {target}")
    return target


def _resolve_context_file_path(file_path: str, cwd: str | None) -> tuple[Path, Path]:
    resolved_cwd = _resolve_cwd(cwd)
    path = Path(file_path).expanduser()
    if not path.is_absolute():
        path = (resolved_cwd / path).resolve()
    else:
        path = path.resolve()

    if not (path == resolved_cwd or resolved_cwd in path.parents):
        raise ValueError(f"file_path must be under cwd {resolved_cwd}: {path}")
    if not path.exists() or not path.is_file():
        raise ValueError(f"file_path does not exist or is not a file: {path}")
    return path, resolved_cwd


def _safe_env(extra: dict[str, str] | None = None) -> dict[str, str]:
    keep = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TERM", "TMPDIR"]
    env = {key: os.environ[key] for key in keep if key in os.environ}
    env.setdefault("PATH", "/usr/local/bin:/usr/bin:/bin")
    env["QUILL_CONTEXT"] = "1"
    if extra:
        env.update(extra)
    return env


_DANGEROUS_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (
        re.compile(
            r"(?is)\brm\s+[^;&|]*(-rf|-fr|--recursive\s+--force|--force\s+--recursive)"
            r"[^;&|]*(\s/(\s|$)|\s/\*|\s~(\s|/|$)|\s\$HOME(\s|/|$)|--no-preserve-root)"
        ),
        "refusing rm -rf against root or home",
    ),
    (re.compile(r"(?is)(^|[;&|])\s*(sudo\s+)?(shutdown|reboot|halt|poweroff)\b"), "refusing shutdown/reboot command"),
    (re.compile(r"(?is)\b(curl|wget)\b[^;&]*\|\s*(sudo\s+)?(sh|bash|zsh|fish|python|perl|ruby)\b"), "refusing curl/wget piped to an interpreter"),
    (re.compile(r":\s*\(\s*\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:"), "refusing fork bomb pattern"),
    (re.compile(r"(?is)(^|[;&|])\s*(sudo\s+)?mkfs(\.\w+)?\b"), "refusing filesystem formatting command"),
    (re.compile(r"(?is)\bdd\s+[^;&|]*\bof=/dev/"), "refusing dd write to device"),
    (re.compile(r"(?is)(^|[;&|])\s*(cd|pushd|popd)\b"), "pass cwd instead of changing directories inside the command"),
]


def _validate_command(command: str) -> None:
    normalized = command.strip()
    if not normalized:
        raise ValueError("command must not be empty")
    for pattern, message in _DANGEROUS_PATTERNS:
        if pattern.search(normalized):
            raise ValueError(message)


def _reader(stream: Any, limit: int, out: dict) -> None:
    chunks: list[bytes] = []
    total = 0
    kept = 0
    try:
        while True:
            chunk = stream.read(8192)
            if not chunk:
                break
            total += len(chunk)
            if kept < limit:
                take = chunk[: limit - kept]
                chunks.append(take)
                kept += len(take)
    finally:
        out["data"] = b"".join(chunks)
        out["bytes"] = total
        out["truncated"] = total > kept


def _run_command(
    command: str,
    cwd: Path,
    timeout_ms: int,
    max_output_bytes: int,
    stdin_data: bytes | None = None,
    extra_env: dict[str, str] | None = None,
) -> dict:
    _validate_command(command)
    timeout = max(0.1, min(timeout_ms, 120_000) / 1000)
    output_limit = max(1024, min(max_output_bytes, MAX_OUTPUT_BYTES))
    stdin_file = None
    start = time.monotonic()
    timed_out = False

    try:
        if stdin_data is not None:
            stdin_file = tempfile.TemporaryFile()
            stdin_file.write(stdin_data)
            stdin_file.seek(0)

        proc = subprocess.Popen(
            command,
            shell=True,
            cwd=str(cwd),
            env=_safe_env(extra_env),
            stdin=stdin_file,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=(os.name == "posix"),
        )

        stdout_state: dict[str, Any] = {}
        stderr_state: dict[str, Any] = {}
        stdout_thread = threading.Thread(target=_reader, args=(proc.stdout, output_limit, stdout_state))
        stderr_thread = threading.Thread(target=_reader, args=(proc.stderr, output_limit, stderr_state))
        stdout_thread.start()
        stderr_thread.start()

        try:
            exit_code = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            if os.name == "posix":
                os.killpg(proc.pid, signal.SIGKILL)
            else:
                proc.kill()
            exit_code = proc.wait(timeout=5)

        stdout_thread.join(timeout=2)
        stderr_thread.join(timeout=2)

        duration_ms = int((time.monotonic() - start) * 1000)
        stdout = stdout_state.get("data", b"").decode("utf-8", errors="replace")
        stderr = stderr_state.get("data", b"").decode("utf-8", errors="replace")

        return {
            "command": command,
            "cwd": str(cwd),
            "exit_code": exit_code,
            "timed_out": timed_out,
            "duration_ms": duration_ms,
            "stdout": stdout,
            "stderr": stderr,
            "stdout_bytes": int(stdout_state.get("bytes", 0)),
            "stderr_bytes": int(stderr_state.get("bytes", 0)),
            "stdout_truncated": bool(stdout_state.get("truncated", False)),
            "stderr_truncated": bool(stderr_state.get("truncated", False)),
        }
    finally:
        if stdin_file is not None:
            stdin_file.close()


def _execution_output(result: dict) -> str:
    parts = [f"$ {result['command']}", f"cwd: {result['cwd']}"]
    if result["stdout"]:
        parts.extend(["", "STDOUT:", result["stdout"]])
    if result["stderr"]:
        parts.extend(["", "STDERR:", result["stderr"]])
    if result["stdout_truncated"] or result["stderr_truncated"]:
        parts.extend(["", "[output truncated at Quill capture cap]"])
    return "\n".join(parts)


def _record_execution(result: dict, source_id: int | None) -> int:
    conn = _context_db()
    with _db_lock:
        cur = conn.execute(
            """
            INSERT INTO executions (
                command, cwd, exit_code, timed_out, duration_ms, stdout_bytes,
                stderr_bytes, stdout_truncated, stderr_truncated, output_source_id, created_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                result["command"],
                result["cwd"],
                result["exit_code"],
                1 if result["timed_out"] else 0,
                result["duration_ms"],
                result["stdout_bytes"],
                result["stderr_bytes"],
                1 if result["stdout_truncated"] else 0,
                1 if result["stderr_truncated"] else 0,
                source_id,
                _now(),
            ],
        )
        conn.commit()
        return int(cur.lastrowid)


def _execution_response(
    result: dict,
    source_label: str,
    index_output: bool = True,
    *,
    telemetry_source: str = "quill_execute",
    telemetry_event_type: str = "mcp.execute",
    telemetry_input_bytes: int | None = None,
    telemetry_metadata: dict | None = None,
    extra_response: dict | None = None,
) -> dict:
    output = _execution_output(result)
    output_bytes = len(output.encode("utf-8", errors="replace"))
    should_index = (
        index_output
        and (
            output_bytes > LARGE_OUTPUT_THRESHOLD
            or result["stdout_truncated"]
            or result["stderr_truncated"]
        )
    )
    source: dict | None = None
    if should_index:
        source = _insert_source(
            label=source_label,
            kind="execution",
            origin="quill_execute",
            content=output,
            content_type="text",
            metadata={
                "command": result["command"],
                "cwd": result["cwd"],
                "timed_out": result["timed_out"],
                "exit_code": result["exit_code"],
            },
        )

    execution_id = _record_execution(result, source["source_id"] if source else None)
    response = {
        "execution_ref": f"execution:{execution_id}",
        "exit_code": result["exit_code"],
        "timed_out": result["timed_out"],
        "duration_ms": result["duration_ms"],
        "stdout_bytes": result["stdout_bytes"],
        "stderr_bytes": result["stderr_bytes"],
        "stdout_truncated": result["stdout_truncated"],
        "stderr_truncated": result["stderr_truncated"],
    }
    if source:
        response["output_source"] = {
            "source_ref": source["source_ref"],
            "label": source["label"],
            "chunk_count": source["chunk_count"],
            "preview": _preview(output, 1600)["text"],
        }
    else:
        response["stdout"] = _preview(result["stdout"] or "", MAX_RESPONSE_PREVIEW)
        response["stderr"] = _preview(result["stderr"] or "", 2000)
    if extra_response:
        response.update(extra_response)
    raw_output_bytes = result["stdout_bytes"] + result["stderr_bytes"]
    return _attach_context_savings(
        response,
        event_type=telemetry_event_type,
        source=telemetry_source,
        decision="indexed" if source else "returned",
        reason="large_output" if source else "bounded_output",
        delivered=True,
        input_bytes=telemetry_input_bytes if telemetry_input_bytes is not None else raw_output_bytes,
        indexed_bytes=source["content_bytes"] if source else 0,
        source_ref=source["source_ref"] if source else None,
        cwd=result["cwd"],
        metadata={
            "commandBytes": _byte_len(result["command"]),
            "stdoutBytes": result["stdout_bytes"],
            "stderrBytes": result["stderr_bytes"],
            "stdoutTruncated": result["stdout_truncated"],
            "stderrTruncated": result["stderr_truncated"],
            "timedOut": result["timed_out"],
            "exitCode": result["exit_code"],
            **(telemetry_metadata or {}),
        },
    )


class _HTMLToText(HTMLParser):
    block_tags = {"p", "div", "section", "article", "br", "li", "tr", "table", "pre", "blockquote"}
    heading_tags = {"h1", "h2", "h3", "h4", "h5", "h6"}
    skip_tags = {"script", "style", "noscript", "nav", "header", "footer"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.skip_depth = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        tag = tag.lower()
        if tag in self.skip_tags:
            self.skip_depth += 1
            return
        if self.skip_depth:
            return
        if tag in self.heading_tags:
            level = int(tag[1])
            self.parts.append("\n" + ("#" * min(level, 4)) + " ")
        elif tag in self.block_tags:
            self.parts.append("\n")

    def handle_endtag(self, tag: str) -> None:
        tag = tag.lower()
        if tag in self.skip_tags and self.skip_depth:
            self.skip_depth -= 1
            return
        if not self.skip_depth and (tag in self.block_tags or tag in self.heading_tags):
            self.parts.append("\n")

    def handle_data(self, data: str) -> None:
        if not self.skip_depth:
            text = html.unescape(data).strip()
            if text:
                self.parts.append(text + " ")

    def text(self) -> str:
        raw = "".join(self.parts)
        raw = re.sub(r"[ \t]+\n", "\n", raw)
        raw = re.sub(r"\n{3,}", "\n\n", raw)
        raw = re.sub(r"[ \t]{2,}", " ", raw)
        return raw.strip()


def _normalize_fetched_content(content: str, content_type: str) -> tuple[str, str]:
    lower = content_type.lower()
    if "json" in lower:
        try:
            return json.dumps(json.loads(content), indent=2, sort_keys=True), "json"
        except json.JSONDecodeError:
            return content, "text"
    if "html" in lower or "xhtml" in lower:
        parser = _HTMLToText()
        parser.feed(content)
        return parser.text(), "markdown"
    return content, "text"


def _validate_public_http_url(url: str) -> None:
    parsed = urlparse(url)
    if parsed.scheme not in {"http", "https"}:
        raise ValueError("only http and https URLs are supported")
    if not parsed.hostname:
        raise ValueError("URL must include a hostname")

    host = parsed.hostname.strip().lower()
    if host == "localhost" or host.endswith(".localhost"):
        raise ValueError("refusing to fetch localhost URLs")

    try:
        addresses = {ipaddress.ip_address(host)}
    except ValueError:
        try:
            infos = socket.getaddrinfo(
                host,
                parsed.port or (443 if parsed.scheme == "https" else 80),
                type=socket.SOCK_STREAM,
            )
        except socket.gaierror as err:
            raise ValueError(f"could not resolve URL hostname: {host}") from err
        addresses = {ipaddress.ip_address(info[4][0]) for info in infos}

    if not addresses:
        raise ValueError(f"could not resolve URL hostname: {host}")

    blocked = [str(addr) for addr in addresses if not addr.is_global]
    if blocked:
        raise ValueError(
            "refusing to fetch non-public URL address(es): " + ", ".join(blocked[:3])
        )


async def _fetch_public_url(url: str, max_bytes: int) -> dict:
    headers = {"User-Agent": "Quill-MCP/0.1"}
    current_url = url
    redirect_count = 0

    async with httpx.AsyncClient(timeout=30.0, follow_redirects=False, headers=headers) as client:
        while True:
            _validate_public_http_url(current_url)
            chunks: list[bytes] = []
            total = 0

            async with client.stream("GET", current_url) as resp:
                if 300 <= resp.status_code < 400 and resp.headers.get("location"):
                    redirect_count += 1
                    if redirect_count > 5:
                        raise ValueError("too many redirects while fetching URL")
                    current_url = urljoin(str(resp.url), resp.headers["location"])
                    continue

                resp.raise_for_status()
                content_type = resp.headers.get("content-type", "text/plain")
                async for chunk in resp.aiter_bytes():
                    total += len(chunk)
                    kept = sum(len(part) for part in chunks)
                    if kept < max_bytes:
                        remaining = max_bytes - kept
                        chunks.append(chunk[:remaining])
                    if total > max_bytes:
                        break

                return {
                    "final_url": str(resp.url),
                    "raw": b"".join(chunks),
                    "total": total,
                    "content_type": content_type,
                    "status_code": resp.status_code,
                    "etag": resp.headers.get("etag"),
                    "last_modified": resp.headers.get("last-modified"),
                    "encoding": resp.encoding or "utf-8",
                }


def _purge_context_files() -> list[str]:
    removed: list[str] = []
    for name in ("continuity", "markers"):
        target = CONTEXT_DIR / name
        if not target.exists():
            continue
        if target.is_dir():
            shutil.rmtree(target)
        else:
            target.unlink()
        removed.append(str(target))
    return removed


@mcp.tool(annotations=WRITE_ANNOTATIONS)
def quill_index_context(
    content: Annotated[
        str | None,
        Field(description="Raw text to index. Provide content or file_path, not both."),
    ] = None,
    file_path: Annotated[
        str | None,
        Field(description="File path to index without returning full content."),
    ] = None,
    cwd: Annotated[
        str | None,
        Field(description="Working directory used to resolve and constrain file_path."),
    ] = None,
    source: Annotated[
        str | None,
        Field(description="Human-readable source label. Re-indexing the same label replaces prior chunks."),
    ] = None,
    content_type: Annotated[
        Literal["auto", "text", "markdown", "json", "code"],
        Field(description="Content type hint for chunk metadata."),
    ] = "auto",
    max_bytes: Annotated[
        int,
        Field(description="Maximum bytes to read from a file or content payload.", ge=1024, le=MAX_INDEX_BYTES),
    ] = MAX_INDEX_BYTES,
) -> dict:
    """Index working-context text or a file into Quill's local context store."""
    if bool(content) == bool(file_path):
        raise ValueError("provide exactly one of content or file_path")

    truncated = False
    file_resolved: str | None = None
    input_bytes = 0
    metadata: dict[str, Any] = {}
    if file_path:
        path, _ = _resolve_context_file_path(file_path, cwd)
        text, truncated, _ = _read_text_file(path, max_bytes)
        input_bytes = min(path.stat().st_size, max_bytes)
        file_resolved = str(path)
        label = source or str(path)
        kind = "file"
        metadata["fileBytes"] = path.stat().st_size
    else:
        raw_content = content or ""
        raw_content_bytes = raw_content.encode("utf-8", errors="replace")
        input_bytes = len(raw_content_bytes)
        text = raw_content_bytes[: max_bytes + 1].decode("utf-8", errors="replace")
        truncated = len(raw_content_bytes) > max_bytes
        if truncated:
            text = text.encode("utf-8", errors="replace")[:max_bytes].decode("utf-8", errors="replace")
        label = source or f"content:{_sha256_text(text)[:12]}"
        kind = "content"

    if truncated:
        text += "\n\n[truncated at Quill indexing cap]"

    indexed = _insert_source(
        label=label,
        kind=kind,
        origin="quill_index_context",
        file_path=file_resolved,
        content=text,
        content_type=content_type,
        metadata={"truncated": truncated},
    )
    response = {"indexed": indexed, "truncated": truncated}
    return _attach_context_savings(
        response,
        event_type="mcp.index",
        source="quill_index_context",
        decision="indexed",
        reason=kind,
        delivered=True,
        input_bytes=input_bytes,
        indexed_bytes=indexed["content_bytes"],
        source_ref=indexed["source_ref"],
        metadata={
            "kind": kind,
            "chunkCount": indexed["chunk_count"],
            "truncated": truncated,
            **metadata,
        },
    )


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def quill_search_context(
    query: Annotated[str, Field(description="Search query for indexed working-context content.")],
    source: Annotated[
        str | None,
        Field(description="Optional source label substring or source:N ref to restrict search."),
    ] = None,
    limit: Annotated[int, Field(description="Maximum results to return.", ge=1, le=20)] = 5,
) -> dict:
    """Search Quill working-context sources and return bounded snippets with refs."""
    response = _search_context(query, limit=limit, source=source)
    matched_bytes = sum(int(result.get("bytes") or 0) for result in response["results"])
    source_filter_id = _parse_ref(source, "source")
    return _attach_context_savings(
        response,
        event_type="mcp.search",
        source="quill_search_context",
        decision="returned",
        reason="search",
        delivered=True,
        input_bytes=matched_bytes,
        source_ref=_source_ref(source_filter_id) if source_filter_id is not None else None,
        metadata={
            "queryBytes": _byte_len(query),
            "resultCount": len(response["results"]),
            "ftsUsed": response["fts_used"],
            "sourceFilter": source,
        },
    )


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def quill_get_context_source(
    source_ref: Annotated[
        str | None,
        Field(description="source:N ref or numeric source id."),
    ] = None,
    chunk_ref: Annotated[
        str | None,
        Field(description="Optional chunk:N ref to retrieve a single bounded chunk."),
    ] = None,
    source: Annotated[
        str | None,
        Field(description="Optional source label substring when source_ref is not known."),
    ] = None,
    include_content: Annotated[
        bool,
        Field(description="Return bounded chunk content instead of only previews."),
    ] = False,
    limit: Annotated[int, Field(description="Maximum chunks to list.", ge=1, le=100)] = 20,
) -> dict:
    """Retrieve source metadata, chunk inventory, or one bounded chunk by ref."""
    conn = _context_db()
    chunk_id = _parse_ref(chunk_ref, "chunk")
    if chunk_id is not None:
        row = conn.execute(
            """
            SELECT c.*, s.label, s.kind
            FROM chunks c
            JOIN sources s ON s.id = c.source_id
            WHERE c.id = ?
            """,
            [chunk_id],
        ).fetchone()
        if row is None:
            return {"error": f"chunk not found: {chunk_ref}"}
        content_preview = _preview(row["content"], 16 * 1024 if include_content else 1200)
        response = {
            "chunk_ref": _chunk_ref(int(row["id"])),
            "source_ref": _source_ref(int(row["source_id"])),
            "source": row["label"],
            "title": row["title"],
            "content_type": row["content_type"],
            "bytes": row["byte_length"],
            "content": content_preview if include_content else None,
            "preview": None if include_content else content_preview,
        }
        return _attach_context_savings(
            response,
            event_type="mcp.source_read",
            source="quill_get_context_source",
            decision="chunk",
            reason="include_content" if include_content else "preview",
            delivered=True,
            input_bytes=row["byte_length"],
            source_ref=_source_ref(int(row["source_id"])),
            metadata={
                "chunkRef": _chunk_ref(int(row["id"])),
                "includeContent": include_content,
                "sourceKind": row["kind"],
            },
        )

    source_id = _parse_ref(source_ref, "source")
    params: list[Any] = []
    where = "1=1"
    if source_id is not None:
        where += " AND id = ?"
        params.append(source_id)
    elif source:
        where += " AND label LIKE ? ESCAPE '\\'"
        params.append(f"%{_like_escape(source)}%")

    row = conn.execute(
        f"""
        SELECT *
        FROM sources
        WHERE {where}
        ORDER BY updated_at DESC
        LIMIT 1
        """,
        params,
    ).fetchone()
    if row is None:
        return {"error": "source not found"}

    response = {
        "source_ref": _source_ref(int(row["id"])),
        "label": row["label"],
        "kind": row["kind"],
        "origin": row["origin"],
        "file_path": row["file_path"],
        "url": row["url"],
        "content_bytes": row["content_bytes"],
        "chunk_count": row["chunk_count"],
        "updated_at": row["updated_at"],
        "chunks": _chunk_inventory(int(row["id"]), limit=limit),
    }
    return _attach_context_savings(
        response,
        event_type="mcp.source_read",
        source="quill_get_context_source",
        decision="source",
        reason="inventory",
        delivered=True,
        input_bytes=row["content_bytes"],
        indexed_bytes=row["content_bytes"],
        source_ref=_source_ref(int(row["id"])),
        metadata={
            "chunkCount": row["chunk_count"],
            "kind": row["kind"],
            "limit": limit,
        },
    )


@mcp.tool(annotations=EXECUTION_ANNOTATIONS)
def quill_execute(
    command: Annotated[str, Field(description="Shell command to execute in a sanitized environment.")],
    cwd: Annotated[
        str | None,
        Field(description="Working directory. Defaults to the provider project dir when available."),
    ] = None,
    timeout_ms: Annotated[int, Field(description="Execution timeout in milliseconds.", ge=100, le=120000)] = 30000,
    max_output_bytes: Annotated[
        int,
        Field(description="Maximum stdout and stderr bytes to capture each.", ge=1024, le=MAX_OUTPUT_BYTES),
    ] = MAX_OUTPUT_BYTES,
    index_output: Annotated[
        bool,
        Field(description="Index large/truncated output and return refs instead of full output."),
    ] = True,
) -> dict:
    """Execute a bounded shell command and preserve large output as indexed context."""
    resolved_cwd = _resolve_cwd(cwd)
    result = _run_command(command, resolved_cwd, timeout_ms, max_output_bytes)
    label = f"execute:{_sha256_text(command + _now())[:12]}"
    return _execution_response(result, label, index_output=index_output)


@mcp.tool(annotations=EXECUTION_ANNOTATIONS)
def quill_execute_file(
    file_path: Annotated[
        str,
        Field(description="File path whose bounded contents should be provided to command stdin."),
    ],
    command: Annotated[
        str,
        Field(description="Shell command that reads the file content from stdin."),
    ],
    cwd: Annotated[
        str | None,
        Field(description="Working directory. Defaults to the provider project dir when available."),
    ] = None,
    timeout_ms: Annotated[int, Field(description="Execution timeout in milliseconds.", ge=100, le=120000)] = 30000,
    max_file_bytes: Annotated[
        int,
        Field(description="Maximum file bytes to feed to stdin.", ge=1024, le=MAX_INDEX_BYTES),
    ] = MAX_INDEX_BYTES,
    max_output_bytes: Annotated[
        int,
        Field(description="Maximum stdout and stderr bytes to capture each.", ge=1024, le=MAX_OUTPUT_BYTES),
    ] = MAX_OUTPUT_BYTES,
) -> dict:
    """Run a command over a bounded file payload without returning the file dump."""
    path, resolved_cwd = _resolve_context_file_path(file_path, cwd)
    content, truncated, byte_count = _read_text_file(path, max_file_bytes)
    stdin = content.encode("utf-8", errors="replace")
    if truncated:
        stdin += b"\n\n[truncated at Quill file execution cap]\n"
    result = _run_command(
        command,
        resolved_cwd,
        timeout_ms,
        max_output_bytes,
        stdin_data=stdin,
        extra_env={"QUILL_CONTEXT_FILE": str(path)},
    )
    label = f"execute_file:{path.name}:{_sha256_text(command + str(path) + _now())[:12]}"
    response = _execution_response(
        result,
        label,
        index_output=True,
        telemetry_source="quill_execute_file",
        telemetry_input_bytes=byte_count + result["stdout_bytes"] + result["stderr_bytes"],
        telemetry_metadata={
            "fileBytesRead": byte_count,
            "fileTruncated": truncated,
        },
        extra_response={
            "file": {
                "path": str(path),
                "bytes_read": byte_count,
                "truncated": truncated,
                "content_delivery": "stdin",
            },
        },
    )
    return response


@mcp.tool(annotations=EXECUTION_ANNOTATIONS)
def quill_batch_execute(
    commands: Annotated[
        list[dict],
        Field(description="Commands to run sequentially. Each item has label and command keys."),
    ],
    queries: Annotated[
        list[str] | None,
        Field(description="Optional search queries to run against indexed batch output."),
    ] = None,
    cwd: Annotated[
        str | None,
        Field(description="Working directory. Defaults to the provider project dir when available."),
    ] = None,
    timeout_ms: Annotated[int, Field(description="Total batch timeout in milliseconds.", ge=100, le=120000)] = 60000,
    max_output_bytes: Annotated[
        int,
        Field(description="Maximum stdout and stderr bytes to capture per command.", ge=1024, le=MAX_OUTPUT_BYTES),
    ] = 128 * 1024,
) -> dict:
    """Execute several bounded commands, index combined output, and optionally search it."""
    if not commands:
        raise ValueError("commands must not be empty")
    resolved_cwd = _resolve_cwd(cwd)
    start = time.monotonic()
    sections: list[str] = []
    command_summaries: list[dict] = []

    for index, item in enumerate(commands, start=1):
        label = str(item.get("label") or f"command {index}")
        command = str(item.get("command") or "").strip()
        if not command:
            raise ValueError(f"command {index} is empty")
        elapsed_ms = int((time.monotonic() - start) * 1000)
        remaining = max(100, timeout_ms - elapsed_ms)
        if elapsed_ms >= timeout_ms:
            sections.append(f"# {label}\n\n[skipped: batch timeout exceeded]")
            command_summaries.append({"label": label, "skipped": True})
            continue
        result = _run_command(command, resolved_cwd, remaining, max_output_bytes)
        sections.append(f"# {label}\n\n{_execution_output(result)}")
        command_summaries.append(
            {
                "label": label,
                "exit_code": result["exit_code"],
                "timed_out": result["timed_out"],
                "stdout_bytes": result["stdout_bytes"],
                "stderr_bytes": result["stderr_bytes"],
            }
        )
        if result["timed_out"]:
            break

    combined = "\n\n".join(sections)
    source = _insert_source(
        label=f"batch:{_sha256_text(combined + _now())[:12]}",
        kind="execution",
        origin="quill_batch_execute",
        content=combined,
        content_type="text",
        metadata={"command_count": len(commands), "cwd": str(resolved_cwd)},
    )
    search_results = [
        _search_context(query, limit=5, source=source["source_ref"])
        for query in (queries or [])
        if query.strip()
    ]
    response = {
        "output_source": {
            "source_ref": source["source_ref"],
            "label": source["label"],
            "chunk_count": source["chunk_count"],
            "preview": _preview(combined, 1600)["text"],
        },
        "commands": command_summaries,
        "search": search_results,
    }
    return _attach_context_savings(
        response,
        event_type="mcp.execute",
        source="quill_batch_execute",
        decision="indexed",
        reason="batch",
        delivered=True,
        input_bytes=_byte_len(combined),
        indexed_bytes=source["content_bytes"],
        source_ref=source["source_ref"],
        cwd=str(resolved_cwd),
        metadata={
            "commandCount": len(commands),
            "executedCount": len([item for item in command_summaries if not item.get("skipped")]),
            "queryCount": len(search_results),
        },
    )


@mcp.tool(annotations=FETCH_ANNOTATIONS)
async def quill_fetch_and_index(
    url: Annotated[str, Field(description="HTTP(S) URL to fetch and index.")],
    source: Annotated[
        str | None,
        Field(description="Optional source label. Defaults to the URL."),
    ] = None,
    force: Annotated[
        bool,
        Field(description="Bypass the 24 hour fetch cache and refetch."),
    ] = False,
    max_bytes: Annotated[
        int,
        Field(description="Maximum response bytes to fetch.", ge=1024, le=MAX_FETCH_BYTES),
    ] = MAX_FETCH_BYTES,
) -> dict:
    """Fetch URL content with a 24 hour TTL cache, index it, and return refs."""
    _validate_public_http_url(url)

    conn = _context_db()
    label = source or url
    if not force:
        row = conn.execute("SELECT * FROM fetch_cache WHERE url = ?", [url]).fetchone()
        fetched_at = _parse_time(row["fetched_at"]) if row else None
        if row and fetched_at and datetime.now(timezone.utc) - fetched_at < FETCH_TTL:
            source_row = None
            if row["source_id"] is not None:
                source_row = conn.execute("SELECT * FROM sources WHERE id = ?", [row["source_id"]]).fetchone()
            if source_row is not None:
                response = {
                    "cached": True,
                    "source_ref": _source_ref(int(source_row["id"])),
                    "label": source_row["label"],
                    "chunk_count": source_row["chunk_count"],
                    "fetched_at": row["fetched_at"],
                    "next": "Use quill_search_context with this source_ref for details.",
                }
                return _attach_context_savings(
                    response,
                    event_type="mcp.fetch",
                    source="quill_fetch_and_index",
                    decision="cache_hit",
                    reason="fetch_cache",
                    delivered=True,
                    input_bytes=source_row["content_bytes"],
                    indexed_bytes=source_row["content_bytes"],
                    source_ref=_source_ref(int(source_row["id"])),
                    metadata={
                        "urlBytes": _byte_len(url),
                        "chunkCount": source_row["chunk_count"],
                        "cached": True,
                    },
                )

    fetched = await _fetch_public_url(url, max_bytes)
    fetched_text = fetched["raw"].decode(fetched["encoding"], errors="replace")
    truncated = fetched["total"] > max_bytes
    content_type = fetched["content_type"]
    normalized, normalized_type = _normalize_fetched_content(fetched_text, content_type)
    if truncated:
        normalized += "\n\n[truncated at Quill fetch cap]"

    indexed = _insert_source(
        label=label,
        kind="fetch",
        origin="quill_fetch_and_index",
        url=url,
        content=normalized,
        content_type=normalized_type,
        metadata={
            "truncated": truncated,
            "status_code": fetched["status_code"],
            "content_type": content_type,
            "final_url": fetched["final_url"],
        },
    )
    with _db_lock:
        conn.execute(
            """
            INSERT INTO fetch_cache (
                url, source_id, label, content_type, status_code, etag,
                last_modified, fetched_at, content_hash
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(url) DO UPDATE SET
                source_id = excluded.source_id,
                label = excluded.label,
                content_type = excluded.content_type,
                status_code = excluded.status_code,
                etag = excluded.etag,
                last_modified = excluded.last_modified,
                fetched_at = excluded.fetched_at,
                content_hash = excluded.content_hash
            """,
            [
                url,
                indexed["source_id"],
                label,
                content_type,
                fetched["status_code"],
                fetched["etag"],
                fetched["last_modified"],
                _now(),
                indexed["content_hash"],
            ],
        )
        conn.commit()

    response = {
        "cached": False,
        "indexed": {
            "source_ref": indexed["source_ref"],
            "label": indexed["label"],
            "chunk_count": indexed["chunk_count"],
            "content_bytes": indexed["content_bytes"],
            "truncated": truncated,
        },
        "preview": _preview(normalized, 3000),
        "next": "Use quill_search_context with this source_ref for specific lookups.",
    }
    return _attach_context_savings(
        response,
        event_type="mcp.fetch",
        source="quill_fetch_and_index",
        decision="indexed",
        reason="network_fetch",
        delivered=True,
        input_bytes=len(fetched["raw"]),
        indexed_bytes=indexed["content_bytes"],
        source_ref=indexed["source_ref"],
        metadata={
            "urlBytes": _byte_len(url),
            "statusCode": fetched["status_code"],
            "contentType": content_type,
            "finalUrl": fetched["final_url"],
            "truncated": truncated,
            "observedBytes": fetched["total"],
            "chunkCount": indexed["chunk_count"],
        },
    )


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def quill_context_stats() -> dict:
    """Return compact stats for the Quill working-context store."""
    return _context_stats()


def _context_stats() -> dict:
    conn = _context_db()
    source_count = conn.execute("SELECT COUNT(*) AS c FROM sources").fetchone()["c"]
    chunk_count = conn.execute("SELECT COUNT(*) AS c FROM chunks").fetchone()["c"]
    execution_count = conn.execute("SELECT COUNT(*) AS c FROM executions").fetchone()["c"]
    event_count = conn.execute("SELECT COUNT(*) AS c FROM continuity_events").fetchone()["c"]
    snapshot_count = conn.execute("SELECT COUNT(*) AS c FROM compaction_snapshots").fetchone()["c"]
    cache_count = conn.execute("SELECT COUNT(*) AS c FROM fetch_cache").fetchone()["c"]
    bytes_row = conn.execute("SELECT COALESCE(SUM(content_bytes), 0) AS b FROM sources").fetchone()
    return {
        "db_path": str(CONTEXT_DB),
        "fts_available": _has_fts(conn),
        "sources": source_count,
        "chunks": chunk_count,
        "executions": execution_count,
        "continuity_events": event_count,
        "compaction_snapshots": snapshot_count,
        "fetch_cache_entries": cache_count,
        "indexed_bytes": bytes_row["b"],
    }


@mcp.tool(annotations=DESTRUCTIVE_ANNOTATIONS)
def quill_purge_context(
    confirm: Annotated[bool, Field(description="Must be true to purge context data.")] = False,
    source_ref: Annotated[
        str | None,
        Field(description="Optional source:N ref or id to purge one source instead of all context data."),
    ] = None,
) -> dict:
    """Purge one source or all Quill working-context data."""
    if not confirm:
        return {"purged": False, "message": "Pass confirm=true to purge context data."}
    conn = _context_db()
    with _db_lock:
        if source_ref:
            source_id = _parse_ref(source_ref, "source")
            if source_id is None:
                return {"purged": False, "error": f"invalid source_ref: {source_ref}"}
            _delete_sources(conn, [source_id])
            conn.execute("DELETE FROM fetch_cache WHERE source_id = ?", [source_id])
            conn.commit()
            return {"purged": True, "scope": _source_ref(source_id)}

        counts = _context_stats()
        if _has_fts(conn):
            conn.execute("DELETE FROM chunks_fts")
        for table in (
            "fetch_cache",
            "executions",
            "continuity_events",
            "compaction_snapshots",
            "chunks",
            "sources",
        ):
            conn.execute(f"DELETE FROM {table}")
        conn.commit()
    removed_files = _purge_context_files()
    return {
        "purged": True,
        "scope": "all",
        "previous_counts": counts,
        "removed_files": removed_files,
    }


@mcp.tool(annotations=WRITE_ANNOTATIONS)
def quill_record_continuity_event(
    session_id: Annotated[str, Field(description="Provider session id for this continuity event.")],
    event_type: Annotated[
        str,
        Field(description="Event category, such as decision, task, blocker, file, or handoff."),
    ],
    summary: Annotated[str, Field(description="Short durable summary of the event.")],
    details: Annotated[
        str | None,
        Field(description="Optional bounded details to preserve for later compaction snapshots."),
    ] = None,
    source_refs: Annotated[
        list[str] | None,
        Field(description="Optional source:N or chunk:N refs related to this event."),
    ] = None,
    priority: Annotated[int, Field(description="Higher priority events are selected first.", ge=0, le=100)] = 50,
) -> dict:
    """Record a durable continuity event for later compaction snapshots."""
    conn = _context_db()
    with _db_lock:
        cur = conn.execute(
            """
            INSERT INTO continuity_events (
                session_id, event_type, summary, details, source_refs_json, priority, created_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            [
                session_id,
                event_type,
                summary[:1000],
                (details or "")[:32000],
                json.dumps(source_refs or []),
                priority,
                _now(),
            ],
        )
        conn.commit()
        event_id = int(cur.lastrowid)
    response = {"event_ref": f"continuity:{event_id}", "session_id": session_id, "priority": priority}
    stored_bytes = _byte_len(summary[:1000]) + _byte_len((details or "")[:32000]) + _byte_len(source_refs or [])
    return _attach_context_savings(
        response,
        event_type="mcp.continuity",
        source="quill_record_continuity_event",
        decision="recorded",
        reason=event_type,
        delivered=True,
        input_bytes=stored_bytes,
        indexed_bytes=stored_bytes,
        session_id=session_id,
        metadata={
            "priority": priority,
            "sourceRefCount": len(source_refs or []),
        },
    )


def _resolve_source_refs(refs: list[str]) -> list[dict]:
    conn = _context_db()
    resolved: list[dict] = []
    for ref in refs:
        source_id = _parse_ref(ref, "source")
        chunk_id = _parse_ref(ref, "chunk")
        if source_id is not None:
            row = conn.execute("SELECT id, label, chunk_count FROM sources WHERE id = ?", [source_id]).fetchone()
            if row:
                resolved.append({"ref": _source_ref(int(row["id"])), "label": row["label"], "chunks": row["chunk_count"]})
        elif chunk_id is not None:
            row = conn.execute(
                """
                SELECT c.id, c.title, s.id AS source_id, s.label
                FROM chunks c
                JOIN sources s ON s.id = c.source_id
                WHERE c.id = ?
                """,
                [chunk_id],
            ).fetchone()
            if row:
                resolved.append(
                    {
                        "ref": _chunk_ref(int(row["id"])),
                        "title": row["title"],
                        "source_ref": _source_ref(int(row["source_id"])),
                        "source": row["label"],
                    }
                )
    return resolved


@mcp.tool(annotations=WRITE_ANNOTATIONS)
def quill_create_compaction_snapshot(
    session_id: Annotated[str, Field(description="Provider session id to snapshot.")],
    max_events: Annotated[int, Field(description="Maximum continuity events to include.", ge=1, le=200)] = 50,
    max_chars: Annotated[int, Field(description="Maximum snapshot characters to store.", ge=1000, le=64000)] = 16000,
) -> dict:
    """Create a compact, reference-based snapshot from continuity events."""
    conn = _context_db()
    rows = conn.execute(
        """
        SELECT *
        FROM continuity_events
        WHERE session_id = ?
        ORDER BY priority DESC, created_at DESC
        LIMIT ?
        """,
        [session_id, max_events],
    ).fetchall()
    ordered = sorted(rows, key=lambda row: row["created_at"])
    refs: list[str] = []
    lines = [
        f"<quill_working_context session_id={json.dumps(session_id)} generated_at={json.dumps(_now())}>",
        "",
        "## Continuity Events",
    ]
    for row in ordered:
        event_refs = json.loads(row["source_refs_json"] or "[]")
        refs.extend(event_refs)
        lines.append(
            f"- continuity:{row['id']} [{row['event_type']}] priority={row['priority']} "
            f"at={row['created_at']}: {row['summary']}"
        )
        if row["details"]:
            detail = re.sub(r"\s+", " ", row["details"]).strip()[:600]
            lines.append(f"  details: {detail}")
        if event_refs:
            lines.append(f"  refs: {', '.join(event_refs)}")

    unique_refs = list(dict.fromkeys(refs))
    resolved = _resolve_source_refs(unique_refs)
    if resolved:
        lines.extend(["", "## Referenced Context"])
        for item in resolved[:50]:
            if item["ref"].startswith("source:"):
                lines.append(f"- {item['ref']} {item['label']} ({item['chunks']} chunks)")
            else:
                lines.append(f"- {item['ref']} {item['title']} from {item['source_ref']} {item['source']}")
    lines.extend(["", "</quill_working_context>"])
    snapshot = "\n".join(lines)
    truncated = len(snapshot) > max_chars
    if truncated:
        snapshot = snapshot[: max_chars - 38].rstrip() + "\n[truncated at snapshot cap]\n</quill_working_context>"

    with _db_lock:
        cur = conn.execute(
            """
            INSERT INTO compaction_snapshots (
                session_id, snapshot, event_count, source_refs_json, created_at, metadata_json
            )
            VALUES (?, ?, ?, ?, ?, ?)
            """,
            [
                session_id,
                snapshot,
                len(ordered),
                json.dumps(unique_refs),
                _now(),
                json.dumps({"truncated": truncated, "max_chars": max_chars}),
            ],
        )
        conn.commit()
        snapshot_id = int(cur.lastrowid)
    response = {
        "snapshot_ref": f"snapshot:{snapshot_id}",
        "session_id": session_id,
        "event_count": len(ordered),
        "source_refs": unique_refs[:50],
        "truncated": truncated,
        "preview": _preview(snapshot, 3000),
    }
    input_bytes = sum(
        _byte_len(row["summary"]) + _byte_len(row["details"] or "") + _byte_len(row["source_refs_json"] or "[]")
        for row in ordered
    )
    return _attach_context_savings(
        response,
        event_type="mcp.snapshot",
        source="quill_create_compaction_snapshot",
        decision="created",
        reason="compaction",
        delivered=True,
        input_bytes=input_bytes,
        indexed_bytes=_byte_len(snapshot),
        session_id=session_id,
        snapshot_ref=f"snapshot:{snapshot_id}",
        metadata={
            "eventCount": len(ordered),
            "sourceRefCount": len(unique_refs),
            "truncated": truncated,
            "maxChars": max_chars,
        },
    )


@mcp.tool(annotations=READONLY_ANNOTATIONS)
def quill_get_compaction_snapshot(
    snapshot_ref: Annotated[
        str | None,
        Field(description="snapshot:N ref or id. If omitted, session_id is used."),
    ] = None,
    session_id: Annotated[
        str | None,
        Field(description="Session id whose latest snapshot should be returned."),
    ] = None,
    max_chars: Annotated[int, Field(description="Maximum snapshot characters to return.", ge=1000, le=64000)] = 16000,
) -> dict:
    """Retrieve the latest or referenced compaction snapshot with a response cap."""
    conn = _context_db()
    snapshot_id = _parse_ref(snapshot_ref, "snapshot")
    if snapshot_id is not None:
        row = conn.execute("SELECT * FROM compaction_snapshots WHERE id = ?", [snapshot_id]).fetchone()
    elif session_id:
        row = conn.execute(
            """
            SELECT *
            FROM compaction_snapshots
            WHERE session_id = ?
            ORDER BY created_at DESC
            LIMIT 1
            """,
            [session_id],
        ).fetchone()
    else:
        raise ValueError("provide snapshot_ref or session_id")

    if row is None:
        return {"error": "snapshot not found"}
    snapshot = row["snapshot"]
    truncated = len(snapshot) > max_chars
    response = {
        "snapshot_ref": f"snapshot:{row['id']}",
        "session_id": row["session_id"],
        "event_count": row["event_count"],
        "created_at": row["created_at"],
        "source_refs": json.loads(row["source_refs_json"] or "[]"),
        "snapshot": snapshot[:max_chars],
        "truncated": truncated,
    }
    return _attach_context_savings(
        response,
        event_type="mcp.snapshot",
        source="quill_get_compaction_snapshot",
        decision="returned",
        reason="snapshot_read",
        delivered=True,
        input_bytes=_byte_len(snapshot),
        session_id=row["session_id"],
        snapshot_ref=f"snapshot:{row['id']}",
        metadata={
            "eventCount": row["event_count"],
            "sourceRefCount": len(response["source_refs"]),
            "truncated": truncated,
            "maxChars": max_chars,
        },
    )
