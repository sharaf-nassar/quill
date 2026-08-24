---
lat:
  require-code-mention: true
---
# Web UI Server Test Specs

These tests pin the security invariants the browser-facing listener depends on. Authorization for this surface is recorded in `specs/029-web-ui-server.md` (Clarifications Q4 and the analyze-gate ratification).

## Browser command default-deny

The permitted table admits exactly the sixteen monitor reads and refuses everything else.

Refusals cover unknown names, every registered setter and mutation, `fetch_usage_data`, `refresh_usage_data`, retry/backfill and maintenance commands, the whole `plugin:*` namespace, and prefix, suffix, whitespace, or case variants of a permitted name.

## Disabled listener owns no socket

Loading a disabled configuration leaves `running=false`, returns no bound
address or reachable URL, and accepts no TCP connection on its configured port.

## Failed listener transitions preserve last known good

A failed runtime reconfiguration leaves both durable settings and the serving
socket at the previous working state.

### Different-port rollback

When an enabled listener cannot bind a candidate port, the old port continues
accepting connections and the candidate configuration is not persisted.

### Same-port address rollback

When a loopback listener stops for a same-port wildcard transition and the new
bind fails, the previous loopback address is rebound before the error returns;
the old configuration remains both live and durable.
