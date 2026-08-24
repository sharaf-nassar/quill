---
lat:
  require-code-mention: true
---
# Web UI Server Test Specs

These tests pin the security invariants the browser-facing listener depends on. Authorization for this surface is recorded in `specs/029-web-ui-server.md` (Clarifications Q4 and the analyze-gate ratification).

## Browser command default-deny

The permitted table admits exactly the sixteen monitor reads and refuses everything else.

Refusals cover unknown names, every registered setter and mutation, `fetch_usage_data`, `refresh_usage_data`, retry/backfill and maintenance commands, the whole `plugin:*` namespace, and prefix, suffix, whitespace, or case variants of a permitted name.
