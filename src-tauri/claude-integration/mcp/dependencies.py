from __future__ import annotations

import asyncio
import json
import sqlite3
import sys
import threading
from contextlib import asynccontextmanager
from functools import lru_cache
from pathlib import Path

import httpx

APP_ID = "com.quilltoolkit.app"


def get_app_data_dir() -> Path:
    """Platform-aware Quill app data directory.
    Mirrors the Rust backend's use of dirs::data_local_dir()."""
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / APP_ID
    return Path.home() / ".local" / "share" / APP_ID


@lru_cache(maxsize=1)
def get_config() -> dict:
    config_path = Path.home() / ".config/quill/config.json"
    if not config_path.exists():
        raise RuntimeError(
            "Quill config not found at ~/.config/quill/config.json. "
            "If the Quill widget runs on this machine, restart Claude Code to auto-configure. "
            "For a remote widget, run /quill-setup to configure the connection."
        )
    with open(config_path) as f:
        return json.load(f)


_http_client: httpx.AsyncClient | None = None
_http_lock = asyncio.Lock()


async def get_http_client() -> httpx.AsyncClient:
    global _http_client
    async with _http_lock:
        if _http_client is None:
            config = get_config()
            _http_client = httpx.AsyncClient(
                base_url=config["url"],
                headers={"Authorization": f"Bearer {config['secret']}"},
                timeout=30.0,
            )
    return _http_client


_db_conn: sqlite3.Connection | None = None
_db_lock = threading.Lock()


def get_db() -> sqlite3.Connection:
    global _db_conn
    with _db_lock:
        if _db_conn is None:
            db_path = get_app_data_dir() / "usage.db"
            _db_conn = sqlite3.connect(
                f"file:{db_path}?mode=ro", uri=True, check_same_thread=False
            )
            _db_conn.row_factory = sqlite3.Row
    return _db_conn


@asynccontextmanager
async def lifespan(app):
    try:
        yield
    finally:
        global _http_client, _db_conn
        if _http_client is not None:
            await _http_client.aclose()
            _http_client = None
        if _db_conn is not None:
            _db_conn.close()
            _db_conn = None
