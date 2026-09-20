---
lat:
  require-code-mention: true
---
# Python context pipeline tests

Focused synthetic regressions for the Python MCP context store and remote-fetch boundary. Tests use temporary SQLite databases, patched DNS, and HTTPX's in-memory transport; they do not access production data or the network.

## Pinned fetch hops

Every request resolves and pins its logical hostname while preserving logical HTTP, TLS, redirect, and result identity.

The request retains the logical authority in `Host` and the logical hostname in HTTPcore's `sni_hostname` extension. Each hop uses a fresh HTTPX client with environment proxies disabled, so neither proxy routing nor pooled cross-host TLS connections can bypass the pinned peer. Explicit ports and IPv6 peers remain intact.

`FetchSecurityTests.test_fetch_pins_host_sni_and_redirects_without_environment_proxy` verifies these invariants with patched IPv4/IPv6 DNS answers and an in-memory redirect.

## Fetch boundary rejection

URLs containing credentials and DNS answer sets mixing public and private addresses fail before connection. Redirect destinations are independently resolved and rejected before their request is sent.

`FetchSecurityTests.test_fetch_rejects_credentials_mixed_answers_and_private_redirect` pins these refusals without external calls.

## Replacement rollback and serialized reads

Source replacement is one locked SQLite transaction, and shared reads cannot observe its intermediate state.

Any insert failure rolls back deletion of the prior source and chunks. Once admitted after a writer, readers observe committed replacement state.

`ContextDatabaseTests.test_replacement_failure_rolls_back_and_reads_wait_for_commit` injects a trigger failure mid-replacement and pauses a later replacement between delete and insert.

## Transactional cache purge

Targeted and full purges remove cache and source data in recoverable SQLite transactions.

A targeted purge removes `fetch_cache` rows before deleting their source, avoiding `ON DELETE SET NULL` orphan rows. Any SQL failure during full purge rolls back all preceding deletes and preserves the last committed counts.

`ContextDatabaseTests.test_source_and_full_purge_remove_cache_rows` verifies targeted cache removal, injected full-purge rollback, and successful full-purge counts.

Upstream references: HTTPcore 1.0.9's documented `sni_hostname` request extension, HTTPX 0.28.1's public `build_request`/`send` extension path, Python `sqlite3.Connection` context-manager semantics, and SQLite serialized connection access guidance.
