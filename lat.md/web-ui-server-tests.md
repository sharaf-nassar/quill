---
lat:
  require-code-mention: true
---
# Web UI Server Test Specs

These tests pin the security invariants the browser-facing listener depends on. Authorization for this surface is recorded in `specs/029-web-ui-server.md` (Clarifications Q4 and the analyze-gate ratification).

## Browser command default-deny

The permitted table admits exactly the sixteen monitor reads and refuses everything else.

Refusals cover unknown names, every registered setter and mutation, `fetch_usage_data`, `refresh_usage_data`, retry/backfill and maintenance commands, the whole `plugin:*` namespace, and prefix, suffix, whitespace, or case variants of a permitted name.

## Host denial precedes any data

A peer outside the pinned allowlist receives an empty-body `403` on the public,
pairing, and authenticated route classes and on unrouted paths, so no handler
runs and no Quill bytes are written.

An allowed peer reaches the public class. An empty allowlist and an allowlist
whose only entry fails to resolve both admit nobody but loopback, and loopback's
exemption covers host filtering only — the authenticated class still refuses it
without a session. A resolved hostname entry admits exactly the addresses it
pinned, and only `host_policy=all` skips the pinned set.

## Unpaired access reaches only the pairing bootstrap

Without a session cookie a peer receives the pairing page and nothing else.

The root document, asset paths, the invoke route, and unrouted paths are all
empty-body `403`, and a pairing request whose body is not the contracted shape
is refused exactly like a wrong code.

The pairing page carries its own policy pinning its one inline script by hash,
states no Quill data, and references no bundle chunk, so an unpaired browser can
bootstrap without receiving application assets. Regenerating the credential
invalidates a session that was live before the rotation.

## Only the isolated monitor bundle is servable

The web bundle's served asset set is exactly the transitive chunk graph of the
`web.html` entry, which is the entry the build emits.

No chunk key, source module, chunk name, or emitted filename in that graph names
a Manage or Release Notes module, and the build directory holds no file outside
the graph, so no desktop-only chunk is present to be served.

## Request classes carry bounded per-peer budgets

Each peer gets its own sliding windows: 120 general requests per minute and a
stricter separate pairing window.

Exhausting one class neither borrows from nor grants capacity in the other, one
peer's exhaustion does not affect another, and the tracked-peer table never
exceeds its cap however many peers appear.

## Connection and body caps bound one client

The listener serves at most eight live connections; a further connection waits
unserved until one is released, then completes. A request body over the size cap
is refused rather than read.

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
