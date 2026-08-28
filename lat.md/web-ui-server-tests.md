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

A request naming a host the listener does not answer to receives an empty-body
`403` on every route class and on unrouted paths, so no handler runs and no
Quill bytes are written.

That covers the public, pairing, and authenticated classes alike, and an absent
or empty `Host` is refused the same way.

A listed name reaches the public class whatever address it arrives from, and
matching ignores case and any port, since a name is not a different name on
another port. An empty allowlist answers only to the implicit `localhost`,
`127.0.0.1`, and `[::1]`, so it is loopback-only rather than a listener that
refuses its own UI; that exemption covers the name check only — the
authenticated class still refuses without a session, and the entry document
still redirects rather than serving a chunk.

Matching is by name and never by resolution, so no suffix, prefix, or empty
value matches and nothing in this machine's `/etc/hosts` or DNS can widen an
entry. Rate limiting still keys on the socket peer, which no header can forge.

## Unpaired access reaches only the pairing bootstrap

Without a session cookie a peer receives the pairing page and nothing else.

Asset paths, the invoke route, and unrouted paths are all empty-body `403`, and
a pairing request whose body is not the contracted shape is refused exactly like
a wrong code. The root document is the one navigation among them, so it answers
`303` to `/pair` with an empty body rather than refusing — it still hands over no
bundle content, and a denied peer is refused on it like any other route.

The pairing page carries its own policy pinning its one inline script by hash,
states no Quill data, and references no bundle chunk, so an unpaired browser can
bootstrap without receiving application assets. Regenerating the credential
invalidates a session that was live before the rotation.

`/pair` must also serve the paired case, so a request carrying a live session
cookie is answered `303` to `/` rather than the form. This does not widen the
gate — the same request already passes `require_session` on the document — and
the two redirects fire on opposite session states, so they cannot cycle.

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

The listener serves at most `MAX_CONCURRENT_CONNECTIONS` live connections; a
further connection waits unserved until one is released, then completes. A
request body over the size cap is refused rather than read.

A slot is held by the connection, not the request, so the cap only bounds
anything because an idle connection is reaped: abandoned and half-closed
sockets return their slots without the peer closing them.

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
