---
lat:
  require-code-mention: true
---
# Crash Reporting Test Specs

These tests verify Rust crash events preserve privacy and use the same release identity as frontend events and uploaded artifacts.

## Shared tagged release identifier

Rust prefixes the CI-injected Cargo package version with `v`, matching the GitHub tag used by the frontend SDK and source-map upload.

## Rust deny-by-default payload boundary

The Rust scrubber removes host identity, dynamic content, arbitrary tags, disallowed contexts, and full paths while preserving approved runtime, release, environment, and stack-frame metadata.

## Release matrix symbolication contract

Every release runner authenticates the Sentry Vite plugin, uploads its exact debug-ID bundle and source map without managing the shared release, and rejects missing debug IDs or packaged maps.
