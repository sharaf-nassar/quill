from __future__ import annotations

import asyncio
import json
import sys
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


@asynccontextmanager
async def lifespan(app):
    try:
        yield
    finally:
        global _http_client
        if _http_client is not None:
            await _http_client.aclose()
            _http_client = None
