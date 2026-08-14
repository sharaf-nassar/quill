---
lat:
  require-code-mention: true
---
# Context HTTP API tests

These tests pin the local context API's network boundary, shared-store parity, execution policy, and pi telemetry compatibility.

## Loopback listener and mount gate

The context router is absent when its consumer setting is off and accepts connections only through a `127.0.0.1` listener when enabled.

## Authentication and size bounds

Every context request needs the shared secret, oversized bodies fail before handlers run, and serialized responses have a fixed ceiling.

## Fetch address boundary

Remote fetches reject non-global IPv4 and IPv6 destinations, including IPv4-mapped IPv6, before opening a connection.

## Pinned fetch resolution

Each hostname request connects only to its validated address set while retaining the original hostname for HTTP and TLS identity.

## Execute permission scope and cap

Execute returns 403 while context preservation is off, rejects working directories outside configured roots, and caps captured output when enabled.

## Shared-store Python parity

Rust and the existing Python tools return identical search, source, and stats references while reading and writing one WAL-mode SQLite store.

## Pi context savings ingestion

Context-savings batch validation admits provider `pi` without changing the event vocabulary or numeric bounds.
