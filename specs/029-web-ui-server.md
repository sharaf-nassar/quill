# web-ui-server

## Problem Statement

Quill's UI is reachable only from the desktop window on the machine running the
app. The one browser-reachable surface today is the Vite dev server on `:8181`,
which is development-only and serves fixture data through
[[src/mocks/installBrowserMock.ts]] — it renders the real components against
fake evidence, so it answers "does this layout work" and never "what is my usage
right now".

Users who run Quill on one machine (a desktop that hosts their coding agents, a
home server, a second workstation) have no way to look at live Quill data from
another device. They want the monitor half of the product — the widget's usage,
limits, and activity — in a browser, served by the already-running app, without
exposing the management/settings half.

Why now: the widget redesign settled the monitor-vs-manage split
([[lat.md/frontend#Manage Workspace]]), so "the main app without settings" is
now a coherent, already-separated surface rather than a subset that would have
to be invented.

## Goals

- The running Quill app can serve its real main UI over HTTP to a browser on the
  local network, showing the same live data the desktop widget shows.
- The served surface is the monitor experience only: the widget shell and its
  content bands. The Manage workspace (`?view=manage`), Settings, and every
  window-management affordance are not reachable from the web UI.
- Users control the feature entirely from Settings: an enable toggle, a
  configurable listen port, and a host-acceptance policy that is either "accept
  all hosts" or an explicit allowlist of IPs/hostnames.
- Disabled is the default and the shipped state; no listener binds until the
  user enables it. Toggling off stops the listener without restarting the app.
- The feature is honest about its own exposure: the settings surface states what
  becomes reachable, and an enabled server is discoverable in the UI (not a
  silent background listener).
- Constitution alignment: P11 (explicit external transmission) is the governing
  principle — this is a new network boundary that must be opt-in, minimal, and
  user-controlled. P3 (responsive execution) bounds the serving work off UI
  threads; P5 (typed failure boundaries) covers bind/port failures.

## Non-Goals

- Serving the Manage workspace, Settings, Sessions, Learning, Memory, or Runs
  surfaces. Monitor only.
- Any mutation of Quill state from the browser. Read-only unless a later
  iteration explicitly adds writes.
- Managed Internet publication: TLS termination, reverse-proxy config, tunnels,
  or authentication federation. The scope is a locally-reachable HTTP listener;
  accept-all on a local network is a supported user choice, and anyone who wants
  Internet reach brings their own proxy. (Narrowed at the analyze gate — the
  original "no Internet exposure" wording read as forbidding the accept-all
  policy the feature exists to offer.)
- Multi-user accounts, per-user views, or session isolation. One Quill instance,
  one dataset, whoever is allowed in sees it.
- A separate mobile-specific UI. Responsive behavior of the existing widget is
  in scope; a redesign is not.
- Replacing or changing the dev-only Vite `:8181` mock mode. It stays as-is for
  `/impeccable live`.
- Headless, daemon, or service operation. The listener lives inside the running
  GUI process: it requires a logged-in desktop session with Quill running, and
  dies with the app. Autostart-before-login, sleep/wake availability, and
  unattended server operation are separate product scope.
- Changing the existing ingestion server (`:19876`) or context server
  (`:19877`) contracts.

## Backlog Inputs

None. No `source_backlog` was supplied and no P4 backlog issues were named as
input to this run.

## Target Epic

Resolved: this run creates a new feature epic for `web-ui-server`. No `epic` or
`epic_candidates` was supplied and no provenance closure yielded a candidate.

## Source Authority

None. No visual artifact (mock, screenshot, prototype, design file) was
referenced by the problem statement or context. The existing widget UI is the
de-facto visual reference; `DESIGN.md` and `PRODUCT.md` remain normative for any
UI change per constitution P9.

## User Stories

### Story 1 — Enable the web UI

As a Quill user running the app on my desktop, I want to turn on a web UI from
Settings, so that I can open Quill's monitor view in a browser on another
device.

Acceptance Criteria:
- A Settings control enables/disables the web UI. Default is disabled.
- Enabling binds a listener without restarting the app; disabling releases it
  without restarting the app.
- While enabled, Settings displays the URL(s) the UI is reachable at.
- The enabled/disabled state persists across app restarts.
- With the feature disabled, no socket is bound on the configured port
  (verifiable: the port is connection-refused).

### Story 2 — Choose the port

As a user whose machine already runs other services, I want to choose the port
the web UI listens on, so that it does not collide with something else.

Acceptance Criteria:
- The port is user-configurable and persists.
- An invalid port (out of range, or a reserved/privileged value the app refuses)
  is rejected at the settings boundary with a display-safe message; the previous
  working configuration is preserved (P4, P5).
- A bind failure (port already in use, permission denied) surfaces as a visible,
  typed error in Settings rather than a silent no-op, and leaves the feature in
  a recoverable state.
- Changing the port while enabled rebinds to the new port and releases the old
  one.

### Story 3 — Control which hosts may connect

As a user exposing Quill data on my network, I want to choose between accepting
all hosts and an explicit allowlist, so that I decide who can read my data.

Acceptance Criteria:
- The host policy is a two-mode choice: accept-all, or allowlist.
- In allowlist mode the user can add, view, and remove entries; entries may be
  IP addresses or hostnames.
- A request from a non-allowed source in allowlist mode is refused, and the
  refusal is not a partial render — no Quill data reaches the client.
- Switching to accept-all is an explicit user action with the exposure stated in
  the UI (P11).
- Malformed allowlist entries are rejected at entry time with a display-safe
  message.
- The allowlist persists across restarts.

### Story 4 — See real data in the browser

As a user viewing Quill in a browser, I want the page to show the same live
usage, limits, and activity as the desktop widget, so that the web view is
trustworthy.

Acceptance Criteria:
- The browser view renders live data from the running app's storage, never the
  DEV fixture layer.
- Data that updates in the desktop widget becomes visible in the browser view
  without a manual full-page reload (mechanism to be decided — see Open
  Questions).
- Data unavailable to the web transport degrades to an explicit gap, not
  invented values (P1).
- The `MOCK DATA` badge never appears in the web UI.

### Story 5 — Settings stays desktop-only

As a user, I want the web UI to be view-only monitoring, so that nobody on my
network can change my Quill configuration.

Acceptance Criteria:
- No route, link, keyboard accelerator, or command-palette entry in the web UI
  reaches Manage/Settings.
- Requesting a management route directly by URL does not serve the management
  UI.
- Commands that mutate settings/state are not reachable through the web
  transport, enforced server-side rather than only by hiding UI affordances.

## Constraints

- **Existing servers.** [[src-tauri/src/server.rs]] runs Axum on `:19876`
  bound `0.0.0.0` for hook ingestion with bearer-token auth (constant-time
  compare) and per-endpoint rate limiting; [[src-tauri/src/context_store.rs]]
  runs a separate loopback listener on `:19877` gated by
  `context_http.enabled`. The new web UI is a third listener; whether it reuses
  the `server.rs` router, the `context_store` gated-listener pattern, or a new
  module is a plan decision. The `context_http.enabled` gate is the closest
  existing precedent for a user-gated listener.
- **Auth secret.** [[src-tauri/src/auth.rs]] already generates a bearer token at
  `~/.local/share/com.quilltoolkit.app/auth_secret` (mode 0o600). Whether the
  web UI uses it, uses a separate credential, or uses none is unresolved.
- **Settings storage.** Settings are key/value rows in the `settings` table
  ([[src-tauri/src/storage.rs]]) using dotted keys (`context_http.enabled`,
  `pi_reporter.enabled`). The allowlist is a list, so it needs an encoding
  decision (JSON blob in one key vs. a new table).
- **Settings UI.** Tabs live in `src/components/settings/`
  (`GeneralTab`, `ContextTab`, `IntegrationsTab`, `LearningTab`,
  `PerformanceTab`) inside the Manage workspace. New controls need a tab home;
  a list editor for allowlist entries has no existing component precedent
  (`SettingRow` + `Toggle` are the current primitives).
- **The IPC seam.** The React app talks to the backend through
  `invoke()`/`listen()` against `window.__TAURI_INTERNALS__`, with 88 registered
  Tauri commands ([[src-tauri/src/lib.rs]]) and push events (`usage-updated`,
  `tokens-updated`, `learning-updated`, …). `mockIPC` already proves the seam is
  replaceable without touching call sites — a browser transport can install a
  `__TAURI_INTERNALS__` shim the same way. This is the cheapest known path and
  is the presumed direction unless review rejects it; the alternative is a
  purpose-built read-only REST surface.
- **Command exposure is a security boundary.** If the transport is a generic
  invoke bridge, an allowlist of permitted command names must be enforced
  server-side (Story 5). A deny-list or an unfiltered bridge would expose all 82
  commands, including mutations.
- **Build/assets.** The frontend is a Vite SPA; production assets are bundled
  into the Tauri binary today. Serving them over HTTP needs a decision on
  embedding (e.g. `rust-embed`) vs. reading from the app bundle, and the
  production CSP in `index.html` is written for `tauri://` + the Sentry DSN
  origin ([[lat.md/infrastructure#Crash Transport CSP]]) — an HTTP origin needs
  its own policy, and `scripts/csp.test.mjs` pins the current allowlist.
- **Host validation.** Host acceptance can be enforced at peer-IP level, at
  `Host` header level, or both. Hostname allowlist entries require resolution
  (forward or reverse DNS), which is a network call with failure and
  rebinding-attack considerations. `context_store.rs` already contains
  DNS-pinning precedent for its fetch path.
- **Responsiveness.** Serving must not block Tauri setup or UI threads (P3), and
  bind work must not stall window creation.
- **Zero-warning gates** (P6): `npm run typecheck|lint|test|knip`, `cargo fmt
  --check`, `cargo clippy --all-targets -D warnings`, `uv run --locked --project
  claude-integration/mcp cargo test`.
- **Tests need authorization** (P7). Any new automated test in the plan requires
  an explicit user decision at a gate.
- **lat.md** must be updated for the new listener, transport, and settings
  (P8): at minimum `backend`, `frontend`, `architecture#Communication Layers`,
  and `infrastructure`.

## Open Questions

1. **Transport shape.** Generic `invoke`-over-HTTP bridge (reuses the whole
   React app, needs a server-side command allowlist) vs. a narrow read-only REST
   API for widget data (smaller attack surface, duplicates data shaping and
   diverges over time)? This is the single largest plan fork.
2. **Authentication.** Does the web UI require a credential at all, or is host
   acceptance the only gate? Options: reuse the existing bearer secret, a
   separate web password/token, a pairing code shown in Settings, or none.
   "Accept all hosts" with no auth means anyone who can route to the port reads
   the user's full usage history — is that an acceptable user-chosen state, or
   must auth be mandatory when accept-all is selected?
3. **Live updates.** The desktop UI receives push events. In the browser:
   polling, SSE, WebSocket, or no live updates in v1?
4. **Host matching semantics.** Is the allowlist matched against the peer IP,
   the `Host` header, or both? How are hostname entries resolved, and what
   happens when resolution fails or changes? Is `localhost`/loopback always
   allowed regardless of policy?
5. **Bind address.** Does the listener bind `0.0.0.0`, or is the bind address
   derived from the host policy (loopback-only when the allowlist is empty)?
6. **Which surface exactly.** "The main quill app without settings" — does the
   web UI include the widget titlebar and its controls (tray, close, settings
   key)? What replaces the settings key? What happens to the app-scoped ⌘M
   accelerator and the command palette in a browser?
7. **Non-provider / empty states.** The widget's empty state action opens Manage
   at `settings:integrations`, which does not exist in the web UI. What does the
   web empty state offer instead?
8. **Asset serving.** Embed the built SPA in the binary or read it from the app
   bundle at runtime? How does this interact with `tauri dev`, where assets are
   served by Vite rather than built?
9. **CSP.** What policy does the HTTP-served page carry, and does
   `scripts/csp.test.mjs` need to pin a second production policy?
10. **Rate limiting / abuse.** Does the web listener need the rate limiter that
    guards `:19876`, and what are its budgets?
11. **Discoverability and honesty.** Does an enabled web server show an
    indicator in the desktop UI or tray? Does the app log/surface connections?
12. **Port default.** What is the default port, and how does it avoid colliding
    with `19876`/`19877` and with the dev `:8181`?
13. **Platform/firewall.** Binding a non-loopback listener triggers firewall
    prompts on macOS and Windows. Is that acceptable, documented, or mitigated?
14. **Testing authorization** (P7). Which invariants — host refusal, command
    allowlist, disabled-means-no-socket — warrant automated tests, and does the
    user authorize adding them?

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility, scope,
stakeholders) against the draft and the codebase. Findings below are merged;
cross-dimension hits are marked. Line references were verified against source.

### Critical Questions (answer before planning)

1. **Access-control posture: is a web credential mandatory, or is the host
   policy the only gate?** — flagged by: all six dimensions. The draft listed
   "reuse the existing bearer secret" as a neutral option; it is not. One
   secret ([[src-tauri/src/auth.rs]]) authenticates the *writable* ingestion
   API bound on `0.0.0.0:19876` **and** `/api/v1/context/execute`
   ([[src-tauri/src/server.rs]], [[src-tauri/src/context_store.rs]]), so any
   credential handed to a browser is a write credential readable by that
   device's user and its extensions. Separately, "accept ALL hosts" with no
   credential means anyone who can route to the port reads the user's full
   usage history in plaintext — over VPNs, bridged interfaces, and public
   Wi-Fi, not only a trusted LAN. This is the P11 risk-acceptance decision and
   it also decides the bind address.

2. **Off-device data fidelity: full widget data, or redacted?** — flagged by:
   stakeholders, gaps, scope. P11 requires transmission to be "minimal,
   scrubbed". The Usage view is not totals: it renders project names and paths,
   hostnames, session identifiers, raw model IDs, skills, hook identities, and
   live lineage ([[src/components/widget/views/UsageView.tsx]]). "Same data as
   the widget" is therefore an operational-identity disclosure, and shipping it
   unredacted needs an explicit approval, not an inference from Story 4.

3. **Which widget controls survive in the browser?** — flagged by: gaps,
   ambiguity, feasibility, stakeholders. The monitor surface is *not* read-only
   as built: [[src/App.tsx]] invokes `install_app_update`, `hide_window`,
   `quit_app`, and `refresh_usage_data`; the titlebar writes
   `set_runtime_settings` ([[src/components/widget/WidgetTitleBar.tsx]]); the
   Models view triggers backfill retry; the empty state and rail footer link to
   Manage. Goals say "monitor only" and Non-Goals say "no mutation" — those two
   statements delete real controls, and which ones go is a product call. Note
   that `refresh_usage_data` and even `fetch_usage_data` spend the user's own
   provider quota and write snapshots ([[src-tauri/src/lib.rs]]), so N browser
   viewers multiply upstream egress unless reads are cache-only.

4. **Is an always-visible exposure indicator part of MVP?** — flagged by:
   requirements, gaps, stakeholders. The Goals make honesty-about-exposure
   governing (P11), but no story requires more than a URL shown inside Settings
   — invisible unless the user opens Manage. Decide whether a persistent
   indicator (widget titlebar or tray) ships in v1, and whether Settings must
   also show connected-client visibility and a "revoke access" action.

5. **Is phone/tablet viewport support an MVP requirement?** — flagged by:
   requirements, scope, gaps. The draft puts "responsive behavior of the
   existing widget" in scope with zero acceptance criteria, and the app has
   exactly one breakpoint (`src/styles/index.css`). Either name a supported
   viewport range with a no-clip/no-horizontal-scroll assertion (P9 makes
   `DESIGN.md` normative) or move it to Non-Goals. "View Quill from my phone"
   is the most likely day-after-launch request.

6. **P7: are automated tests for the security invariants authorized?** —
   flagged by: requirements, feasibility, stakeholders. The constitution
   reserves this for the human. The invariants worth pinning are:
   disabled-means-no-socket, host-denied-before-any-data, denied-command
   default-deny, management-bundle-not-served, failed-rebind rollback, and the
   web CSP. Several ACs in Stories 1-3 are written as tests and cannot be
   demonstrated otherwise.

### Technical Decisions (self-resolved — veto at the gate to override)

- **Never transmit `auth_secret` to a browser.** Any web credential is separate,
  web-scoped, and revocable — the shared secret carries ingestion-write and
  context-execute authority.
- **Transport: `invoke`-shaped bridge over same-origin HTTP.** The web bundle
  installs a `window.__TAURI_INTERNALS__` shim the way
  [[src/mocks/installBrowserMock.ts]] already proves is possible, so widget call
  sites and data shaping are reused unchanged. Rejected: a parallel REST surface
  (duplicates shaping, diverges), and generic dispatch into Tauri's handler.
- **Default-deny allowlist matched on the full invoke command string**, as one
  explicit Rust `match` over monitor-read commands. It must cover the `plugin:`
  namespace too — `listen()` is `plugin:event|listen`, and `plugin:updater|*`,
  `plugin:window|*`, `plugin:webview|set_zoom` all ride the same seam. Permit
  `plugin:event|listen`/`unlisten`; deny every other `plugin:*`; stub window and
  webview calls as client-side no-ops. Never key security on a command count:
  the draft's "82" was stale (actual 88; `lat.md/architecture.md` says 89 — a
  documented drift to fix under P8).
- **Cache-only reads.** `fetch_usage_data`, `refresh_usage_data`, backfill
  retry, and every setter stay off the allowlist; a pure cached-usage read is
  added instead. The desktop process remains the sole producer of upstream
  provider traffic, so viewer count cannot multiply egress or burn quota.
- **Live updates: visibility-aware polling, no SSE or WebSocket in v1.** Reuses
  the existing `cachedInvokeStore.refreshStaleSubscribers()` seam at 60s with an
  immediate refresh on focus. Budget: data visible in the desktop widget appears
  in the browser within 60s p95. A push transport is a later upgrade if that
  budget proves wrong.
- **Separate Vite entry (`web.html` + `web-main.tsx`)** that installs the HTTP
  transport *before* importing shared widget code and never imports
  `ManageWindowView` or `ReleaseNotesWindow`. `?view=manage` is resolved
  client-side after bundle load ([[src/main.tsx]]), so a server refusing that URL
  is defeated by `history.replaceState` — Story 5's URL-refusal criterion is
  reworded to "the management bundle is not served", with the server-side
  command allowlist as the real boundary.
- **The web bundle hard-disables crash reporting and the updater.** Otherwise a
  phone on the LAN transmits *its own* Sentry telemetry under the desktop
  owner's consent ([[src/main.tsx]], `src/lib/crashReporting.ts`) — a second
  off-device boundary P11 never authorized.
- **Separate HTTP CSP** carrying `connect-src 'self'` and no Sentry origin. The
  desktop policy omits `'self'` entirely (`index.html`) so serving it verbatim
  blocks the bridge, and `scripts/csp.test.mjs` pins that array exactly — it
  gets extended to pin two named policies.
- **Assets: `rust-embed` the web entry's build output**, SPA fallback for
  monitor routes only. `dist` is not currently a bundle resource
  (`tauri.conf.json`), and dev needs a prebuilt-`dist` path since
  `beforeDevCommand` only starts Vite.
- **Listener lifecycle: a dedicated `web_server` module with a managed
  controller**, not the `context_http.enabled` pattern — that gate is read once
  at startup and its handle only `abort()`s on drop, which neither reconfigures
  nor deterministically releases the socket. Use
  `axum::serve(..).with_graceful_shutdown(token)` and *await* the old task
  before binding the new port. Transitions are serialized bind-new → persist →
  swap → drop-old; any failure preserves the last-known-good config and listener
  (P4, P5).
- **Settings keys** `web_ui.enabled`, `web_ui.port`, `web_ui.host_policy`,
  `web_ui.allowlist` (JSON array), `web_ui.last_error`, written through
  `set_settings_atomically` — dotted key/value needs no migration. Default port
  **19878**; accept 1024-65535; reject collisions with the resolved
  `QUILL_PORT` and context port. Bind failures surface as a typed error from the
  toggle command and persist to `web_ui.last_error` for startup failures.
- **Host matching on the socket peer IP only** (`ConnectInfo`). Hostname entries
  are forward-resolved when the configuration is applied and the resulting
  addresses are pinned until restart or re-save; resolution failure fails
  closed. `Host` is attacker-controlled and names the destination, and reverse
  DNS is not client authentication — neither is ever trusted. Grammar: IPv4/IPv6
  literal, CIDR, or RFC-1123 hostname; no wildcards; deduplicated; cap 64
  entries. Empty allowlist denies all non-loopback. Loopback bypasses host
  filtering, never authentication.
- **Refusal contract: `403` with an empty body on every path**, static assets
  included, decided before any data is read — so "not a partial render" becomes
  `status == 403 && body.len() == 0`.
- **Bounds:** per-peer-IP token bucket over a size-capped LRU (an uncapped peer
  map is itself the DoS), 120 req/min/peer, ≤8 concurrent connections, 1 MiB
  body cap, 10s request timeout. The existing limiter is one global deque per
  endpoint with no peer identity ([[src-tauri/src/server.rs]]), so one hostile
  client would starve every other; it is not reused as-is.
- **Settings home: a dedicated Web section** rather than a row in General. The
  allowlist needs a list editor, which `SettingRow`/`Toggle` do not cover.
- **The web bundle drops the Ctrl+F and Ctrl+`±`/`0` interceptors**
  ([[src/main.tsx]]). They are correct for a Tauri webview and hostile in a
  browser, where they kill find-in-page and native zoom.

### Non-Blocking Observations

- No audit surface is specified: no accepted/refused peer log, no view of who is
  currently connected, and no defined behavior for an already-open browser tab
  when the toggle flips, the port changes, or the app quits. Worth a follow-up
  bead even if cut from MVP.
- Firewall prompts on macOS and Windows are acknowledged but unhandled. Decide
  support copy and whether Settings distinguishes "listener bound" from
  "reachable through the firewall" before release.
- `ingest_is_quiesced()` gates writes only; read-side behavior during retention
  and compaction windows is undefined. Under P1 that should surface as an
  explicit gap rather than stale numbers.
- Checked and dismissed as non-gaps: **i18n** (the app has no framework and is
  English-only throughout) and **settings migrations** (dotted key/value table
  with `set_settings_atomically` already in place).

## Clarifications

**Q1: Access-control posture — what gates a browser connection?**

A: **Web credential required, plus the host policy as a second gate.** A
separate, revocable, web-only credential is always mandatory — a pairing code
displayed in Settings with a Regenerate action. The browser enters it once and
holds a cookie-scoped session thereafter. `auth_secret` never leaves the
desktop under any configuration. The bind address derives from the host policy:
loopback-only until the user allows a non-local host.

Reflected in: Goals (opt-in exposure), Story 1 and Story 3 acceptance criteria,
and a new Story 6.

**Q2: Off-device data fidelity — how much does the browser see?**

A: **Full fidelity, with disclosure in Settings.** The browser sees exactly what
the desktop widget sees, including project paths, hostnames, and session
identifiers. Settings states plainly what becomes readable *before* the user
enables the feature. P11's "minimal" is satisfied by informed opt-in and the
loopback-by-default bind rather than by field redaction; no redaction layer is
built.

Reflected in: Goals, Story 1 acceptance criteria, Non-Goals (redaction).

**Q3: Which widget controls survive in the browser build?**

A: **Bands and view controls only.** Client-local controls stay: range
selection, view switching, breakdown expansion. Removed from the web build:
titlebar app controls (pin/always-on-top, settings key, update, close, quit),
the Limits manual-refresh control, the Models backfill-retry control, the Manage
footer and empty-state Settings action, the right-click Refresh/Quit menu, and
the ⌘M/Ctrl+M accelerator and command palette. No inert or disabled affordances
are rendered — removed controls are absent, not greyed out.

Reflected in: Story 5 acceptance criteria and Non-Goals (no new story — this
is a subtraction from the existing surface).

**Q4: MVP scope selections.**

A: **In scope** — phone/tablet viewport support (with acceptance criteria), and
P7 authorization for the security-invariant tests.
**Deferred** — the always-visible exposure indicator (Settings disclosure and
the reachable-URL display carry the honesty requirement in v1), and the
connected-client peer log / active-client view. The credential's own Regenerate
action ships as part of Q1's pairing UI and is the v1 revocation path; the
broader connection-visibility surface is a follow-up.

Reflected in: Goals, Non-Goals, Story 7 (new), Constraints (test
authorization).

### Resulting changes to earlier sections

**Goals** — add: a mandatory web credential gates every connection; the bind
address is loopback-only until a non-local host is allowed; Settings discloses
exactly what data becomes readable before enablement; the browser view is
usable on a phone or tablet viewport.

**Non-Goals** — add: field-level redaction of monitor data; an always-visible
tray or titlebar exposure indicator; a connected-client peer log or access
history; inert/disabled renderings of removed desktop controls.

**Story 1** (enable) — add acceptance criteria: Settings displays the
disclosure of readable data before the toggle can be enabled; enabling with the
allowlist empty binds loopback only.

**Story 3** (host policy) — add: allowing the first non-local host rebinds the
listener off loopback; the credential requirement is independent of and
unaffected by the host policy.

**Story 5** (settings stays desktop-only) — the URL-refusal criterion is
replaced: the management bundle is never served, enforced by a separate Vite
entry that does not import it, with the server-side command allowlist as the
authoritative boundary. Client-side routing makes URL refusal alone
unimplementable.

**New Story 6 — Pair a browser.** As a user, I want to authorize a specific
browser once, so that casual network neighbours cannot read my data and I can
cut off access later.
Acceptance criteria: Settings displays a pairing code and a Regenerate action;
an unpaired browser receives the `403` empty-body refusal on every path,
including static assets; a paired browser holds a session across reloads;
Regenerate invalidates every existing session; `auth_secret` is never
transmitted to any client; pairing attempts are rate-limited per peer.

**New Story 7 — View on a phone.** As a user, I want the web view to be usable
on my phone, so that I can check Quill away from my desk.
Acceptance criteria: the monitor surface renders without clipping or horizontal
scroll from 360px viewport width upward; touch targets meet the `DESIGN.md`
minimum; the layout degrades by reflow rather than by hiding data (P1, P9).

**Constraints** — add: P7 authorization is granted for automated tests covering
disabled-means-no-socket, host-denied-before-any-data, denied-command
default-deny, management-bundle-not-served, failed-rebind rollback, and the web
CSP. No other new test surface is authorized.

### Open Questions — resolved or deferred

Q2 (authentication) → Q1 above. Q5 (bind address) → derived from host policy,
loopback until a non-local host is allowed. Q6 (which surface) → Q3 above.
Q7 (empty states) → the web empty state states that a provider must be enabled
on the desktop; it carries no action. Q11 (discoverability) → Settings
disclosure plus reachable-URL display; indicator and connection log deferred.
Q14 (test authorization) → granted, scoped as above.

Q1, Q3, Q4, Q8, Q9, Q10, Q12 were resolved in Technical Decisions. Q13
(firewall prompts) remains a release-readiness item in Non-Blocking
Observations.

## Architecture Approach

A third Axum listener, owned by a new `src-tauri/src/web_server/` module tree,
serves a **web-only** Vite bundle over same-origin HTTP. The bundle installs an
HTTP-backed `window.__TAURI_INTERNALS__` shim before importing any widget code,
so existing `invoke()` call sites, hooks, and data-shaping paths are reused; the
server answers those invokes from a default-deny allowlist of monitor-read
commands.

**Why this shape.** [[src/mocks/installBrowserMock.ts]] already proves the
`__TAURI_INTERNALS__` seam is replaceable without touching call sites — the dev
mock does exactly this with fixtures. Swapping fixtures for HTTP is the smallest
change that yields a real web UI. The security boundary lives entirely
server-side (allowlist + credential + peer-IP filter), so a modified client gains
nothing the allowlist does not already grant.

**Three request classes, not one gate.** Pairing must be able to bootstrap, so
the gate graph is route-specific rather than a single session check:

| Class | Routes | Gates |
| --- | --- | --- |
| Public, data-free | `GET /pair` (inline minimal page + its inline assets) | peer-IP filter, rate limit |
| Pairing | `POST /api/web/pair` | peer-IP filter, stricter rate limit, constant-time code compare |
| Authenticated | `/`, `/assets/*`, `POST /api/web/invoke` | peer-IP filter, rate limit, session cookie |

The `/pair` page carries no Quill data and links to no bundle chunk. Desktop
liveness uses the `get_web_ui_status` Tauri command, not an HTTP route, so
Settings never needs a session against its own app.

**Rejected: a parallel read-only REST API for widget data.** It duplicates the
shaping ~20 hooks already do, and the two surfaces diverge on the first schema
change.

**Rejected: reusing the `context_http.enabled` listener pattern.** That gate is
read once at startup and its handle only `abort()`s on drop — it can neither
toggle nor rebind at runtime, and `abort()` does not deterministically release
the socket.

**Rejected: nesting the web listener inside `start_server`.** Ingestion binds
first and returns early on collision, which would silently suppress the web
listener whenever `:19876` is taken.

**Rejected: serving the existing bundle with server-side route refusal.**
`?view=manage` resolves client-side after the bundle loads ([[src/main.tsx]]), so
`history.replaceState` defeats any URL check. Worse, a single multi-entry Rollup
output still emits the Manage and Release Notes chunks that `src/main.tsx`
imports, and a generic `/assets/*` handler would serve them. The web entry
therefore builds to its **own output directory** (`dist-web`) from `web.html`
only, and only that directory is embedded.

**Rejected: reusing `LimitsSection` / `ViewRegion` / `UsageView` / `ModelsView`
unchanged.** Those components unconditionally render the manual-refresh, Manage,
and backfill-retry controls that Clarifications Q3 removes, and the usage state
they consume is owned by [[src/App.tsx]]. They gain an explicit web-surface prop
that *omits* those controls (not disables them — Q3 forbids inert affordances),
and a new `useWebMonitorData` hook supplies what `App.tsx` supplies on desktop.

**Constitution check.** P11 governs and was **ratified at the analyze gate**:
the principle requires off-device transmission be "minimal, scrubbed", and
Clarifications Q2 chose full fidelity. The human ruled P11's intent satisfied by
informed opt-in, a mandatory credential, the loopback-by-default bind, and
pre-enablement disclosure — "scrubbed" is knowingly waived for a surface the
user deliberately points at their own data. No constitution edit; the waiver is
recorded here and no scrubbing layer is built.
P1: cache-only reads mean the web view shows the evidence the desktop has, never
a fresher invented one. P2: extends existing Axum/Tauri/React layers; adds two
crates. P3: the listener is a detached tokio task off Tauri setup's critical
path. P4: config transitions preserve last-known-good on failure. P5: bind and
validation failures are typed and display-safe. P6: full gate set runs, listed in
Testing Strategy. P7: automated scope is the six surfaces Q4 authorized plus the
pairing/session surface authorized at the analyze gate. P9: `DESIGN.md` governs the new
Web settings section, and the phone work requires a numeric hit-area rule that
`DESIGN.md` does not yet define. P10: freshness budget with a reproducible
measurement procedure.

**Learnings store.** `docs/solutions/` has no entry for HTTP serving, listener
lifecycle, or CSP. Nothing here re-attempts a documented failure.

## Affected Components

| Component | Change |
| --- | --- |
| [[src-tauri/src/lib.rs]] | Module declarations, the four new command registrations, and controller spawn at setup. **Single-writer file — owned by the scaffold work item** |
| `src-tauri/src/web_config.rs` (new) | Typed config struct, validation, canonical allowlist grammar parser |
| `src-tauri/src/web_pairing.rs` (new) | Pairing secret file, HMAC session issue/verify, constant-time compare, atomic rotation |
| `src-tauri/src/web_allowlist.rs` (new) | Default-deny command `match`; the single security-critical list |
| `src-tauri/src/web_server/mod.rs` (new) | Router and shared state scaffold |
| `src-tauri/src/web_server/controller.rs` (new) | Bind, graceful shutdown, transition state machine, rollback |
| `src-tauri/src/web_server/gates.rs` (new) | Peer-IP filter, DNS pin, session check, per-peer rate limiter |
| `src-tauri/src/web_server/router.rs` (new) | `/pair`, `/api/web/pair`, `/api/web/invoke` handlers |
| `src-tauri/src/web_server/assets.rs` (new) | `rust-embed` of `dist-web`, SPA fallback for monitor routes |
| [[src-tauri/src/storage.rs]] | No schema change — `web_ui.*` keys use the existing dotted key/value `settings` table |
| [[src-tauri/src/auth.rs]] | Untouched. The web credential is separate |
| `src-tauri/src/data_paths.rs` | Identity-scoped path for the web credential. `data_paths.rs` reserves the shared production path for provider auth only, so the web secret must not sit beside `auth_secret` |
| `src-tauri/Cargo.toml` + `Cargo.lock` | Add `rust-embed` and `hmac`; enable axum `ConnectInfo` |
| `web.html`, `src/web-main.tsx` (new) | Second Vite entry; installs transport before importing widget code; crash reporting and updater hard-disabled; Ctrl+F and Ctrl+zoom interceptors dropped |
| `src/web/httpTransport.ts` (new) | `__TAURI_INTERNALS__` shim implementing the wire contract |
| `src/web/WebShell.tsx` (new) | Brand-only header + monitor bands |
| `src/web/useWebMonitorData.ts` (new) | Cached usage/status/focus polling — the web equivalent of what `App.tsx` owns |
| [[src/App.tsx]] | Untouched |
| `src/components/widget/LimitsSection.tsx`, `views/UsageView.tsx`, `views/ModelsView.tsx` | Add a web-surface prop that omits refresh, retry, Manage, and empty-state Settings affordances |
| [[vite.config.ts]] | Web build config emitting `dist-web` from `web.html` only |
| `package.json` | `build:web` script; `tauri.conf.json` `beforeDevCommand`/`beforeBuildCommand` run it |
| `.gitignore` | `dist-web/` |
| `src/windows/SettingsWindowView.tsx` | Renders tab content — must import and render `WebTab` when `active === "web"` |
| [[src/components/settings/SettingsTabs.tsx]] | Add the `web` tab id and label |
| `src/components/settings/WebTab.tsx` (new) | Enable toggle, port, host policy, pairing code + Regenerate, disclosure, reachable URL, status, last error |
| `src/components/settings/AllowlistEditor.tsx` (new) | Add/remove rows; submits candidates and displays typed backend rejection — parsing stays canonical in Rust |
| `src/hooks/useWebUiSettings.ts` (new) | Typed read/write of `web_ui.*` |
| `scripts/csp.test.mjs` | Pin two named policies |
| `DESIGN.md` | Define the numeric mobile hit-area minimum the phone work needs |
| `README.md` | Web build/dev instructions |
| `.github/workflows/ci.yml` | Frontend web build before the Rust build that embeds it |
| `lat.md/` | `backend`, `frontend`, `architecture#Communication Layers`, `infrastructure`; correct the stale command count |

## Data Model

No schema migration. Five new rows in the existing `settings` table:

| Key | Type | Default | Validation |
| --- | --- | --- | --- |
| `web_ui.enabled` | bool | `false` | — |
| `web_ui.port` | int | `19878` | 1024-65535; rejects the resolved `QUILL_PORT` and context port |
| `web_ui.host_policy` | `"all"` \| `"allowlist"` | `"allowlist"` | — |
| `web_ui.allowlist` | JSON array | `[]` | IPv4/IPv6 literal, CIDR, or RFC-1123 hostname; no wildcards; deduped; ≤64 entries |
| `web_ui.last_error` | string \| null | `null` | Set on startup bind failure so Settings can show it when opened later |

**Pairing credential.** A 160-bit random secret in an identity-scoped app-data
file, mode 0o600 — *not* beside `auth_secret`, whose shared production path
`data_paths.rs` reserves for provider auth. Rotation writes to a temp file and
renames, so a crash mid-rotation leaves exactly one valid secret. The
user-facing pairing code is a short display encoding of that secret; comparison
is constant-time.

**Session.** An HMAC-SHA256 of the secret over `(issued_at, nonce)`, carried in
the contracted `quill_web_session` cookie with `Path=/`, `HttpOnly`,
`SameSite=Strict`, and `Max-Age=2592000`; `Domain`, `Secure`, and `Expires` are
omitted. Verification is constant-time. Because every session derives from the
secret, Regenerate invalidates all of them with no session table.

**In-memory only.** Resolved allowlist addresses are pinned at config-apply time
and recomputed on restart or re-save — never persisted, so a stale DNS answer
cannot outlive a restart.

## API / Interface Changes

**New HTTP routes** (web listener only; never mounted on `:19876`). Gate columns
are the three request classes from Architecture Approach:

| Method | Route | Peer filter | Rate limit | Session |
| --- | --- | --- | --- | --- |
| GET | `/pair` | yes | yes | **no** |
| POST | `/api/web/pair` | yes | stricter | **no** |
| GET | `/`, `/assets/*` | yes | yes | yes |
| POST | `/api/web/invoke` | yes | yes | yes |

A refusal at any applicable gate is `403` with an empty body, decided before any
Quill data is read. The `/pair` page is inline-rendered and references no bundle
chunk, so an unpaired peer never receives application assets.

### Web transport protocol contract

This subsection is the single normative Rust/TypeScript wire contract. The Rust
types in `src-tauri/src/web_server/mod.rs` and the TypeScript types in
`src/web/httpTransport.ts` use these names and payloads verbatim; neither side
applies a casing conversion. Object fields are closed at the Rust boundary so a
schema mismatch fails before command dispatch.

#### Invoke envelope and status mapping

`POST /api/web/invoke` accepts exactly:

```json
{"cmd":"get_provider_statuses","args":{}}
```

`cmd` is the complete Tauri invoke command string. `args` is required and must
be a JSON object; calls without arguments send `{}`. Responses are exactly one
of these discriminated envelopes:

| HTTP status | Body | Client result |
| --- | --- | --- |
| `200` | `{"ok":true,"value":<command result>}` | resolve `invoke()` with `value` |
| `200` | `{"ok":false,"code":"command_error","message":string}` | reject `invoke()` with the exact message string |
| `403` | `{"ok":false,"code":"command_denied"}` | reject with `Command is not available in the web UI.` |

Malformed JSON or a request that does not match the request shape is `400` with
an empty body. Host, rate-limit, or session refusal remains `403` with an empty
body and the shim rejects with `Web UI access denied.`; this is distinct from an
authenticated command denial, whose JSON body is safe to return. Any other HTTP
status rejects with `Web UI request failed (HTTP <status>).`, an invalid JSON or
envelope rejects with `Web UI returned an invalid invoke response.`, and a
network failure rejects with `Web UI is unavailable.`

#### Exact permitted-command table

The authenticated invoke route is default-deny. Only these complete command
strings reach dispatch; each is a cache or local-storage read used by the
monitor surface:

| Command | Monitor use |
| --- | --- |
| `get_activity_series` | Session/project sparklines |
| `get_cached_usage_data` | Pure cache-only Limits snapshot; never polls a provider |
| `get_code_stats` | Range code totals |
| `get_code_stats_history` | Code history and insights |
| `get_context_savings_analytics` | Context view and Usage insight |
| `get_cpa_connection_status` | Whether CPA is a configured usage source |
| `get_hook_breakdown` | Hooks breakdown |
| `get_host_breakdown` | Hosts breakdown |
| `get_llm_runtime_stats` | Runtime totals and insights |
| `get_model_usage_overview` | Usage chart and Models view |
| `get_project_breakdown` | Projects readout and breakdown |
| `get_provider_statuses` | Enabled/detected provider state |
| `get_retention_policy` | Read-only retention disclosure |
| `get_session_breakdown` | Sessions breakdown and live overlay |
| `get_skill_breakdown` | Skills breakdown |
| `get_token_history` | Token/code comparison insight |

Every other command is denied, including `fetch_usage_data`,
`refresh_usage_data`, every setter or maintenance command, and every `plugin:*`
command. No prefix, suffix, alias, or command count participates in the check.

#### Desktop config and status shapes

The desktop-only commands remain excluded from the web allowlist. Their
serialized shapes are:

```text
get_web_ui_config() -> {
  config: {
    enabled: boolean,
    port: integer,
    host_policy: "all" | "allowlist",
    allowlist: string[]
  },
  pairing_code: string
}

set_web_ui_config({ config }) -> same shape as get_web_ui_config
get_web_ui_status() -> {
  running: boolean,
  bound_addr: string | null,
  reachable_urls: string[],
  last_error: string | null
}

regenerate_web_pairing_code() -> { pairing_code: string }
```

`last_error` is status, not writable config. Persistent storage uses exactly
`web_ui.enabled`, `web_ui.port`, `web_ui.host_policy`, `web_ui.allowlist`, and
`web_ui.last_error`; no camelCase aliases exist.

When `running` is false, `bound_addr` is `null` and `reachable_urls` is empty.
When running, `bound_addr` is canonical `SocketAddr` text. Reachable URLs are
server-produced from concrete local interface IP addresses, never wildcard
bind addresses or hostname aliases: `http://<IPv4>:<port>/` or
`http://[<IPv6>]:<port>/`. The explicit port and trailing slash are mandatory;
values are deduplicated and sorted lexicographically before serialization.

#### Pairing and session cookie

`POST /api/web/pair` accepts exactly `{"code":string}`. A correct code returns
`204` with no body and:

```text
Set-Cookie: quill_web_session=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000
```

The cookie has no `Domain`, `Secure`, or `Expires` attribute. `Secure` is omitted
because this feature serves plain HTTP by design; `SameSite=Strict`, `HttpOnly`,
and the root path are mandatory. A wrong code is `403` with an empty body.

#### Cross-language fixtures

The following values are the round-trip fixtures. Rust serde tests and the
TypeScript exported fixtures use these payloads without field renaming:

```json
{"request":{"cmd":"get_provider_statuses","args":{}},"status":200,"response":{"ok":true,"value":[]}}
{"request":{"cmd":"set_runtime_settings","args":{"settings":{}}},"status":403,"response":{"ok":false,"code":"command_denied"}}
{"request":{"cmd":"get_model_usage_overview","args":{"range":"24h","provider":null}},"status":200,"response":{"ok":false,"code":"command_error","message":"Model analytics unavailable."}}
{"request":{"code":"fixture-pair-code"},"success_status":204}
```

**Events.** No push transport exists in v1. `plugin:event|listen` and
`plugin:event|unlisten` are client-side no-ops that return a disposer; window
and webview plugin calls are also client-side no-ops. All are denied server-side
along with every other `plugin:*` command.

**Breaking changes:** none. The desktop path, `:19876`, `:19877`, and the dev
`:8181` mock are untouched.

## Testing Strategy

Automated scope is the six surfaces Clarifications Q4 authorized plus the
pairing/session surface authorized at the analyze gate. Nothing beyond these
seven.

| Invariant | Layer | Oracle |
| --- | --- | --- |
| Disabled means no socket | Rust integration | Connect to the configured port → refused |
| Host denied before any data | Rust integration | Non-allowed peer gets `403`, empty body, on `/`, `/assets/*`, and `/api/web/invoke` |
| Unpaired denied before any data | Rust integration | No cookie → `403` empty body on `/`, `/assets/*`, `/api/web/invoke`; `/pair` still reachable; Regenerate invalidates a live session |
| Command default-deny | Rust unit | Unknown name, every setter, `fetch_usage_data`, `refresh_usage_data`, and every `plugin:*` are refused |
| Management bundle not served | Node | The `dist-web` build manifest and its transitive chunk graph contain no Manage or Release Notes module — asserted against the served asset set, not string presence in shared output |
| Failed rebind rolls back | Rust unit | Both transition paths: different-port bind conflict, and same-port address change whose new bind fails → old listener still serving, config unchanged, typed error returned |
| Web CSP | `scripts/csp.test.mjs` | Both policies pinned; the web policy has `connect-src 'self'` and no Sentry origin |

**Manual acceptance matrix** — every acceptance criterion not covered above is
verified by hand and recorded in the implementing bead:

| Story | Manual checks |
| --- | --- |
| 1 | Disclosure renders before the toggle can be enabled; enable/disable without restart; state survives restart; empty allowlist binds loopback only |
| 2 | Invalid port rejected with a display-safe message and prior config intact; bind failure visible in Settings; port change rebinds and releases the old port |
| 3 | Add/view/remove entries; malformed entry rejected at entry time; first non-local entry rebinds off loopback; accept-all states its exposure |
| 4 | Browser matches desktop values; updates appear without manual reload; unavailable data shows an explicit gap; no `MOCK DATA` badge |
| 5 | No Manage/Settings route, link, accelerator, or palette entry exists in the web build |
| 6 | Pairing code entry succeeds; session survives reload (refusal and invalidation are automated above) |
| 7 | 360px–430px viewports: no clip, no horizontal scroll, hit areas meet the new `DESIGN.md` rule |

**P10 freshness measurement.** A recorded procedure, not a claim: write a known
token count on the desktop, timestamp it, poll the browser DOM for the value,
record delta; 20 samples; pass when p95 ≤ 60s. Note the existing cache TTL is 45s
with 5s coalescing (`cachedInvokeStore`), so the poll cadence is tuned against
this measurement rather than assumed.

**P6 gates**, all must be green: `npm run typecheck`, `npm run lint`,
`npm test`, `npm run knip`, `cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, `uv run --locked --project claude-integration/mcp cargo test`.

## Risks

| Risk | Mitigation |
| --- | --- |
| The allowlist is the whole security boundary — one wrong entry leaks a mutation | One explicit `match` in a dedicated module, default-deny, unit-tested against every setter and the whole `plugin:*` namespace. Never keyed on a command count |
| A read command turns out to do network I/O or writes | `fetch_usage_data` and `refresh_usage_data` are already known to; a pure cached read replaces them. Each allowlisted command is individually verified storage-read-only during the allowlist item |
| Pairing bootstrap deadlock (an unpaired browser cannot reach the page that pairs it) | Explicit three-class gate graph; `/pair` is public and data-free; covered by the manual matrix |
| Same-port rebind cannot bind-new-first | Two transition paths, both with rollback, both unit-tested |
| Embedded assets leak management chunks | Separate `dist-web` output built from `web.html` only; asserted against the served asset set |
| Two new crates (`rust-embed`, `hmac`) | Both small, widely used, compile-time or pure-compute. The Tauri-resource alternative needs a runtime `resource_dir` lookup that differs per packaging target |
| Clean-clone dev breaks because `dist-web` is gitignored and `tauri dev` only starts Vite | Build orchestration is its own work item: `build:web` wired into `beforeDevCommand`/`beforeBuildCommand`, CI builds the frontend before the Rust build, README documents it |
| Graceful shutdown hangs on a live connection | Shutdown token plus a bounded await; connections are short-lived since there is no push stream |
| Firewall prompts make an enabled listener look broken | `get_web_ui_status` distinguishes "bound" from "externally reachable"; a same-host check cannot prove remote reachability, so the UI must not claim it |
| Phone work touches shared widget CSS and could regress the desktop widget | Desktop widget checked at its shipped 360px width as part of the same item |
| Full-fidelity data over plaintext HTTP | Recorded user decision (Q2) with disclosure; loopback-by-default limits blast radius; carried to analyze as a P11 tension |

## Sequencing

Order is expressed by dependency edges. Two items are left unordered only when
their files are disjoint **and** they share no new primitive.

**Foundation — must land first (P0, strictly serial with its dependents):**

- **Web module scaffold** — declares the new modules in [[src-tauri/src/lib.rs]],
  registers the four Tauri commands, adds `rust-embed` and `hmac` to
  `Cargo.toml`/`Cargo.lock`, and lands `web_server/mod.rs` with the router and
  shared-state skeleton. Exists because `lib.rs` and `Cargo.toml` are
  single-writer files that every other Rust item would otherwise edit
  concurrently. Acceptance: the app builds and runs with the modules registered
  and every command returning a typed not-yet-implemented error; all P6 gates
  green.
- **Protocol contract** — one document-and-types item fixing the invoke request
  and response envelopes, the error/status code mapping, the exact permitted-
  command table, the serialized `web_ui.*` config and status field names, the
  cookie attributes, and reachable-URL formatting. Exists because the Rust
  router and the TypeScript transport are written by different workers who
  cannot see each other's unlanded code. Acceptance: Rust types and TS types
  both derive from this section and round-trip a fixture payload.

**Foundation part two (P0, parallel with each other — disjoint files, both
depend on the scaffold and the protocol contract):**

- **Config model and storage keys** — `web_config.rs`: the typed struct, port
  validation and collision rejection, and the **canonical** allowlist grammar
  parser (IPv4/IPv6/CIDR/RFC-1123, no wildcards, dedup, ≤64). Also
  `get_web_ui_config` / `set_web_ui_config`. Acceptance: invalid ports and
  malformed entries return typed display-safe errors; valid config round-trips
  through the settings table.
- **Pairing credential** — `web_pairing.rs`: identity-scoped secret file at
  0o600, atomic rotation, constant-time code compare, HMAC session issue and
  verify with the contracted cookie attributes, `regenerate_web_pairing_code`.
  Acceptance: rotation invalidates a previously valid session; a wrong code
  fails in constant time; the secret file is never under the provider-auth path.
- **Command allowlist** — `web_allowlist.rs`: the explicit `match`, plus the
  pure cached-usage read command that replaces `fetch_usage_data` for web
  callers. Acceptance: the unit test in Testing Strategy passes; every
  allowlisted command is documented as storage-read-only.

**Listener (P1, serial within `web_server/` — each item owns its own file and
depends on the previous landing):**

- **Controller and lifecycle** — `controller.rs`: bind, graceful shutdown, the
  two transition paths (different-port → bind-new-first; same-port address
  change → stop old, bind new, rebind old on failure), last-known-good rollback,
  typed errors, `web_ui.last_error`, loopback-vs-policy bind address,
  `get_web_ui_status`. Acceptance: the rollback unit test covers both paths;
  enable/disable/rebind need no restart.
- **Request gates** — `gates.rs`: peer-IP filter via `ConnectInfo`,
  forward-resolve and pin hostname entries at config-apply, fail-closed,
  per-class rate limits over a size-capped LRU (120 req/min/peer, ≤8
  connections, 1 MiB body, 10s timeout), `403` empty-body refusal. Acceptance:
  the host-denial integration test passes on all three route classes.
- **Router and handlers** — `router.rs`: `/pair`, `/api/web/pair`,
  `/api/web/invoke` per the contract. Acceptance: an unpaired peer reaches
  `/pair` and nothing else; a paired peer's allowlisted invoke returns real
  data; a denied command returns `403`.
- **Asset embedding** — `assets.rs`: `rust-embed` of `dist-web`, SPA fallback for
  monitor routes only, debug-build prebuilt path. Depends additionally on the
  web build orchestration item. Acceptance: the management-bundle test passes.

**Client (P1, parallel with the listener items — disjoint files; all depend on
the protocol contract):**

- **Web build orchestration** — `vite.config.ts` web build emitting `dist-web`
  from `web.html` only, `build:web` script, `beforeDevCommand` and
  `beforeBuildCommand` wiring, `.gitignore`, CI ordering, README instructions.
  Acceptance: a clean clone runs `npm run tauri -- dev` and a packaged build
  serves the web UI, both without manual steps.
- **Web entry and HTTP transport** — `web.html`, `src/web-main.tsx`,
  `src/web/httpTransport.ts`; shim installed before widget imports; crash
  reporting and updater hard-disabled; Ctrl+F and Ctrl+zoom interceptors
  dropped. Acceptance: the built bundle contains no Sentry init and no updater
  call; `invoke()` round-trips through the contracted envelope.
- **Web-surface props** — the `LimitsSection` / `UsageView` / `ModelsView` prop
  that omits refresh, retry, Manage, and empty-state Settings affordances.
  Single-writer over those three widget files. Acceptance: with the prop set,
  those controls are absent from the DOM, not disabled; the desktop path renders
  unchanged.
- **Web shell and monitor data** — `WebShell.tsx` (brand-only header, no context
  menu, palette, or ⌘M) and `useWebMonitorData.ts` (cached usage/status supply
  plus visibility-aware 60s polling through
  `cachedInvokeStore.refreshStaleSubscribers()`, immediate refresh on focus).
  Depends on the web entry and the web-surface props. Acceptance: the browser
  renders live desktop-matching values; the empty state states a provider must
  be enabled on the desktop and carries no action.
- **Web CSP** — separate HTTP policy with `connect-src 'self'` and no Sentry
  origin; `csp.test.mjs` pins both. Depends on the web entry. Acceptance: both
  policies pinned; the CSP test passes.

**Settings UI (P1, serial — both touch the settings tree):**

- **Web settings section** — `WebTab.tsx`, the `web` tab id in `SettingsTabs`,
  **and the content branch in `src/windows/SettingsWindowView.tsx`**, plus
  `useWebUiSettings`. Renders the enable toggle, port field, host policy radio,
  pairing code with Regenerate, the disclosure copy shown before enablement, the
  reachable URL, bound-vs-reachable status, and the last error. Depends on the
  config model, pairing credential, and controller. Acceptance: every Story 1-3
  and Story 6 manual check is performable from this tab.
- **Allowlist editor** — `AllowlistEditor.tsx`, submitting candidates to the Rust
  parser and rendering its typed rejection. No parsing logic in TypeScript.
  Depends on the settings section. Acceptance: a malformed entry shows the
  backend's message; valid entries persist and reorder deterministically.

**Closeout (P2, last):**

- **Mobile hit-area rule** — define the numeric minimum in `DESIGN.md` (none
  exists today; controls reach 18px in `src/styles/index.css`). Blocks the phone
  work because it is its oracle. Acceptance: `DESIGN.md` states a testable
  minimum.
- **Phone viewport support** — reflow the monitor bands from 360px up: no clip,
  no horizontal scroll, hit areas meeting the new rule, desktop widget unchanged
  at 360px. Depends on the web shell and the hit-area rule.
- **Freshness measurement** — run the P10 procedure and record the p95 in the
  bead; adjust the poll cadence if the budget misses. Depends on the web shell.
- **Authorized tests** — the seven invariants. Depends on every item they cover.
- **lat.md sync** — `backend`, `frontend`, `architecture#Communication Layers`,
  `infrastructure`; correct the stale command count; `lat check` green. Depends
  on everything.

## Analyze-gate ratifications

All three items raised at the analyze gate were decided by the human; none
remains open.

1. **P11 tension — ratified as satisfied by opt-in.** Informed opt-in, the
   mandatory credential, the loopback-by-default bind, and pre-enablement
   disclosure carry P11's intent; "scrubbed" is knowingly waived. No constitution
   edit, no scrubbing layer.
2. **Non-Goal wording — narrowed** to "no managed Internet publication: TLS
   termination, reverse-proxy config, tunnels, or authentication federation".
   Accept-all on a local network is a supported user choice.
3. **Seventh test surface — authorized.** Unpaired-denied-before-any-data and
   Regenerate-invalidates-live-sessions move from the manual matrix into
   automated Rust integration coverage.

## Normative Visual Coverage

None (0 rows). Source Authority names no normative visual artifact, confirmed by
rebuilding the inventory from Source Authority rather than from the table. The
ASCII layout the user selected at the clarify gate is a decision record for
Clarifications Q3, not a normative design artifact; `DESIGN.md` and `PRODUCT.md`
govern the new surfaces under P9.

## Backlog Refinement

None. No P4 backlog sources were supplied or discovered — this run creates a new
feature epic with all work items above at P0-P2. No item is P4.

## Alignment fixes applied

- **must (A+B)** — Pairing could not bootstrap: session gating applied to every
  path including `/api/web/pair` and static assets. Replaced the single gate with
  a three-class gate graph; `/pair` and `POST /api/web/pair` are public and
  data-free; desktop liveness moved from `/api/web/health` to
  `get_web_ui_status`. Story 6's "every path" wording now carries the exception.
- **must (A+B)** — Rebind sequencing was internally impossible: a same-port
  loopback→wildcard change cannot bind-new-first. Split into two transition
  paths, both with rollback, both in the rollback test.
- **must (A+B)** — A single multi-entry Rollup output still emits the Manage and
  Release Notes chunks. The web entry now builds to its own `dist-web`, only that
  is embedded, and the test asserts the served asset set rather than string
  presence.
- **must (B)** — The three "parallel" P0 foundations all had to edit `lib.rs` and
  `Cargo.toml`. Added a serial **Web module scaffold** item owning both files.
- **must (B)** — Four P1 items were unordered over one `web_server.rs`. Split into
  `web_server/{mod,controller,gates,router,assets}.rs`, serialized behind the
  scaffold.
- **must (B)** — Undefined shared contracts (invoke envelope, error mapping,
  status fields, allowlist grammar seam) would have been invented twice. Added a
  P0 **Protocol contract** item; allowlist parsing is canonical in Rust and the
  editor renders typed backend rejections.
- **must (A+B)** — Testing Strategy exceeded the P7 authorization by adding
  pairing/session tests. Escalated rather than self-authorized; the human granted
  the seventh surface at the analyze gate, and it is now automated by permission.
- **must (B)** — Clean-clone dev, CI ordering, `.gitignore`, `Cargo.lock`, and
  README fallout were undefined. Added a **Web build orchestration** work item.
- **must (B)** — P10 claimed a budget with no measurement. Added a reproducible
  procedure (20 samples, p95 ≤ 60s) and a closeout item, noting the 45s TTL and
  5s coalescing already in `cachedInvokeStore`.
- **must (B)** — Pairing storage and dependencies were wrong: `hmac` was not
  counted, and "beside `auth_secret`" would use the path `data_paths.rs` reserves
  for provider auth. Now identity-scoped, with atomic rotation and full cookie
  attributes specified.
- **must (A)** — `LimitsSection` / `UsageView` / `ModelsView` unconditionally
  render the controls Q3 removes, and `App.tsx` owns the usage state. Added the
  web-surface prop item and `useWebMonitorData`.
- **must (A)** — `SettingsWindowView.tsx` owns tab-content rendering; adding a tab
  label alone would render nothing. Added to Affected Components and the
  settings work item.
- **must (A)** — Story 7 cited a `DESIGN.md` touch-target minimum that does not
  exist. Added a closeout item defining the numeric rule, blocking the phone work
  as its oracle.
- **must (A)** — P11 "minimal, scrubbed" versus the Q2 full-fidelity choice, and
  the "no Internet exposure" Non-Goal versus an accept-all non-loopback bind.
  Escalated rather than resolved unilaterally; both were ratified by the human at
  the analyze gate and the wording is now applied.
- **should (A)** — Technical Decision 3 permitted `plugin:event|listen`
  server-side while the client no-ops events. With no push transport in v1, the
  whole `plugin:*` namespace is now denied server-side.
- **should (A)** — Acceptance coverage: added a manual matrix for every criterion
  outside the authorized automated set, plus the exact P6 gate commands.
- **should (A)** — Clarifications Q3 said "reflected in Story 3 (new)"; the new
  stories are 6 and 7. Corrected below.
