from __future__ import annotations

import socket
import sys
import tempfile
import threading
import types
import unittest
from pathlib import Path
from unittest import mock

import httpx


class _McpStub:
    def tool(self, **_kwargs):
        return lambda function: function


server = types.ModuleType("server")
server.mcp = _McpStub()
sys.modules.setdefault("server", server)
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools import context  # noqa: E402


_REAL_ASYNC_CLIENT = httpx.AsyncClient


def _dns_answer(address: str, port: int) -> tuple:
    family = socket.AF_INET6 if ":" in address else socket.AF_INET
    sockaddr = (address, port, 0, 0) if family == socket.AF_INET6 else (address, port)
    return family, socket.SOCK_STREAM, socket.IPPROTO_TCP, "", sockaddr


class FetchSecurityTests(unittest.IsolatedAsyncioTestCase):
    # @lat: [[pipeline-context-python-tests#Pinned fetch hops]]
    async def test_fetch_pins_host_sni_and_redirects_without_environment_proxy(self) -> None:
        requests: list[httpx.Request] = []
        client_options: list[dict] = []

        async def handler(request: httpx.Request) -> httpx.Response:
            requests.append(request)
            if len(requests) == 1:
                return httpx.Response(302, headers={"location": "../middle"})
            if len(requests) == 2:
                return httpx.Response(302, headers={"location": "https://next.test:9443/final"})
            return httpx.Response(200, headers={"content-type": "text/plain"}, content=b"safe")

        def resolver(host: str, port: int, **_kwargs):
            addresses = {
                "start.test": "93.184.216.34",
                "next.test": "2001:4860:4860::8888",
            }
            return [_dns_answer(addresses[host], port)]

        def client_factory(**kwargs):
            client_options.append(kwargs.copy())
            return _REAL_ASYNC_CLIENT(transport=httpx.MockTransport(handler), **kwargs)

        with (
            mock.patch.object(context.socket, "getaddrinfo", side_effect=resolver),
            mock.patch.object(context.httpx, "AsyncClient", side_effect=client_factory),
        ):
            result = await context._fetch_public_url("https://start.test:8443/path", 1024)

        self.assertEqual(result["final_url"], "https://next.test:9443/final")
        self.assertEqual([str(request.url) for request in requests], [
            "https://93.184.216.34:8443/path",
            "https://93.184.216.34:8443/middle",
            "https://[2001:4860:4860::8888]:9443/final",
        ])
        self.assertEqual([request.headers["host"] for request in requests], [
            "start.test:8443",
            "start.test:8443",
            "next.test:9443",
        ])
        self.assertEqual([request.extensions["sni_hostname"] for request in requests], [
            "start.test",
            "start.test",
            "next.test",
        ])
        self.assertTrue(client_options)
        self.assertTrue(all(options["trust_env"] is False for options in client_options))

    # @lat: [[pipeline-context-python-tests#Fetch boundary rejection]]
    async def test_fetch_rejects_credentials_mixed_answers_and_private_redirect(self) -> None:
        sent: list[httpx.Request] = []

        async def handler(request: httpx.Request) -> httpx.Response:
            sent.append(request)
            return httpx.Response(302, headers={"location": "http://private.test/secret"})

        def client_factory(**kwargs):
            return _REAL_ASYNC_CLIENT(transport=httpx.MockTransport(handler), **kwargs)

        with self.assertRaisesRegex(ValueError, "credentials"):
            context._validate_public_http_url("https://user:secret@example.test/")

        with mock.patch.object(
            context.socket,
            "getaddrinfo",
            return_value=[
                _dns_answer("93.184.216.34", 443),
                _dns_answer("127.0.0.1", 443),
            ],
        ):
            with self.assertRaisesRegex(ValueError, "non-public"):
                context._validate_public_http_url("https://mixed.test/")

        def resolver(host: str, port: int, **_kwargs):
            address = "93.184.216.34" if host == "public.test" else "127.0.0.1"
            return [_dns_answer(address, port)]

        with (
            mock.patch.object(context.socket, "getaddrinfo", side_effect=resolver),
            mock.patch.object(context.httpx, "AsyncClient", side_effect=client_factory),
        ):
            with self.assertRaisesRegex(ValueError, "non-public"):
                await context._fetch_public_url("https://public.test/start", 1024)

        self.assertEqual(len(sent), 1)
        self.assertEqual(str(sent[0].url), "https://93.184.216.34/start")


class ContextDatabaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        with context._db_lock:
            if context._db_conn is not None:
                context._db_conn.close()
            context.CONTEXT_DIR = Path(self.temp.name)
            context.CONTEXT_DB = context.CONTEXT_DIR / "context.db"
            context._db_conn = None
            context._fts_available = None

    def tearDown(self) -> None:
        with context._db_lock:
            if context._db_conn is not None:
                context._db_conn.close()
            context._db_conn = None
            context._fts_available = None
        self.temp.cleanup()

    # @lat: [[pipeline-context-python-tests#Replacement rollback and serialized reads]]
    def test_replacement_failure_rolls_back_and_reads_wait_for_commit(self) -> None:
        original = context._insert_source(
            label="replace-me", kind="content", content="last good content"
        )
        conn = context._context_db()
        with context._db_lock, conn:
            conn.execute(
                """
                CREATE TRIGGER fail_replacement BEFORE INSERT ON chunks
                WHEN NEW.content LIKE '%replacement%'
                BEGIN SELECT RAISE(ABORT, 'injected replacement failure'); END
                """
            )

        with self.assertRaisesRegex(Exception, "injected replacement failure"):
            context._insert_source(
                label="replace-me", kind="content", content="broken replacement"
            )

        retained = context.quill_get_context_source(source_ref=original["source_ref"])
        self.assertEqual(retained["label"], "replace-me")
        self.assertIn("last good content", retained["chunks"][0]["preview"])

        deleted = threading.Event()
        release = threading.Event()
        read_finished = threading.Event()
        read_result: dict = {}
        original_delete = context._delete_sources

        def pausing_delete(connection, source_ids):
            original_delete(connection, source_ids)
            deleted.set()
            self.assertTrue(release.wait(timeout=2))

        def replace_source() -> None:
            context._insert_source(label="replace-me", kind="content", content="committed content")

        def read_stats() -> None:
            read_result.update(context._context_stats())
            read_finished.set()

        with mock.patch.object(context, "_delete_sources", side_effect=pausing_delete):
            writer = threading.Thread(target=replace_source)
            writer.start()
            self.assertTrue(deleted.wait(timeout=2))
            reader = threading.Thread(target=read_stats)
            reader.start()
            self.assertFalse(read_finished.wait(timeout=0.1))
            release.set()
            writer.join(timeout=2)
            reader.join(timeout=2)

        self.assertFalse(writer.is_alive())
        self.assertFalse(reader.is_alive())
        self.assertEqual(read_result["sources"], 1)
        self.assertEqual(read_result["chunks"], 1)

    # @lat: [[pipeline-context-python-tests#Transactional cache purge]]
    def test_source_and_full_purge_remove_cache_rows(self) -> None:
        source = context._insert_source(label="fetched", kind="fetch", content="payload")
        conn = context._context_db()
        with context._db_lock, conn:
            conn.execute(
                """INSERT INTO fetch_cache
                   (url, source_id, label, fetched_at)
                   VALUES (?, ?, ?, ?)""",
                ["https://example.test/", source["source_id"], source["label"], context._now()],
            )

        result = context.quill_purge_context(confirm=True, source_ref=source["source_ref"])
        self.assertEqual(result, {"purged": True, "scope": source["source_ref"]})
        self.assertEqual(context._context_stats()["fetch_cache_entries"], 0)

        second = context._insert_source(label="second", kind="fetch", content="payload")
        with context._db_lock, conn:
            conn.execute(
                """INSERT INTO fetch_cache
                   (url, source_id, label, fetched_at)
                   VALUES (?, ?, ?, ?)""",
                ["https://second.test/", second["source_id"], second["label"], context._now()],
            )
            conn.execute(
                """
                CREATE TRIGGER fail_full_purge BEFORE DELETE ON sources
                BEGIN SELECT RAISE(ABORT, 'injected purge failure'); END
                """
            )

        with self.assertRaisesRegex(Exception, "injected purge failure"):
            context.quill_purge_context(confirm=True)
        self.assertEqual(context._context_stats()["sources"], 1)
        self.assertEqual(context._context_stats()["fetch_cache_entries"], 1)

        with context._db_lock, conn:
            conn.execute("DROP TRIGGER fail_full_purge")
        result = context.quill_purge_context(confirm=True)
        self.assertEqual(result["previous_counts"]["sources"], 1)
        self.assertEqual(result["previous_counts"]["fetch_cache_entries"], 1)
        self.assertEqual(context._context_stats()["sources"], 0)
        self.assertEqual(context._context_stats()["fetch_cache_entries"], 0)


if __name__ == "__main__":
    unittest.main()
