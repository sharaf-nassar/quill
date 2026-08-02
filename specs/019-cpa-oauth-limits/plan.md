# Plan: cpa-oauth-limits

Implementation plan for `specs/019-cpa-oauth-limits/spec.md`. All seven clarify
decisions (Q1–Q7) are binding and each is traced to a work item in
`## Sequencing`. Constitution principles are cited inline as P1–P12.

## Architecture Approach

CPA is integrated as a **cross-provider usage source**, not a provider. All CPA
HTTP traffic lives in a new Rust module; the frontend receives typed snapshots
only (P2). The existing `UsageData` pipeline carries CPA-sourced rows end to
end by adding an account dimension to `UsageBucket` rather than adding new
identity types.

**Core decisions:**

1. **No new `IntegrationProvider` variant** (Clarifications Q7 — binding).
   `UsageBucket.provider` stays `Claude`/`Codex` for CPA-sourced rows; a new
   `source` discriminator (`direct` | `cpa`, `#[serde(default)]` = `direct`)
   plus optional `account_id`/`account_label` fields distinguish them. The
   hundreds of usage sites (~527 occurrences across 21 Rust files) stay
   untouched.
2. **CpaConnection as settings-backed state, outside `ProviderStatus`.** CPA
   follows the MiniMax service-only template for its *lifecycle* (no CLI/home
   detection; enable = save url+key, disable = delete them; mutations under
   `integration_mutation_guard`), but it deliberately does NOT get a
   `ProviderStatus` entry: `ProviderStatus.provider` is the closed enum, and
   CPA is not a row identity — it is a source that produces Claude- and
   Codex-flavored rows. Connection state = presence of
   `integration.cpa.base_url` + `integration.cpa.management_key` settings rows,
   read by `refresh_usage_cache` each cycle. An unconfigured CPA costs exactly
   nothing: no probe, no poll time, no UI (spec "null-impact guarantee", P3).
3. **CPA-sourced rows flow through the existing pipeline.** Per-account window
   buckets enter `UsageData.buckets`, persist via `store_snapshot`, restore via
   the cached-snapshot fallback, and reuse the existing cooldown machinery
   (`compute_network_backoff`, `ProviderCooldownKeys`) with new `usage.cpa.*`
   keys. Degradation is therefore the existing ladder for free (Goal 5, US-5).
4. **The pool aggregate is derived, never stored** (Clarifications Q1). A pure
   function in Rust computes per-provider aggregate buckets + healthy/total
   counts from per-account buckets and health flags; it is unit-testable and
   lat.md-traceable (US-4, P1: gaps stay explicit, no invented data).
5. **Errors carry the source too.** `UsageProviderError` gains the same
   `#[serde(default)] source` field so a CPA outage marks only CPA-sourced rows
   stale/offline while the native Claude/Codex rows stay live, and the titlebar
   pill can keep its existing vocabulary. (Spec gap — Q7 text covers only
   `UsageBucket`; without this the degraded ladder conflates native and CPA
   state. Recorded as a plan-level resolution.)

**Alternatives considered and rejected:**

- *New `IntegrationProvider::Cpa` variant (or per-account variants):* rejected
  per Clarifications Q7 — touches hundreds of usage sites (~527 occurrences
  across 21 Rust files), breaks persisted snapshot
  strings and the TS union, and mismodels CPA (it holds Claude/Codex accounts,
  it is not a fourth provider identity).
- *Frontend-side fetching (webview calls CPA directly):* rejected — violates
  the Rust/strict-TS boundary (P2) and the constitution's "all CPA HTTP calls
  live in the Rust backend" constraint; also would leak the management key into
  the webview.
- *CPA as a full `ProviderStatus` pseudo-provider:* rejected — would require
  widening the enum through `ProviderStatus`, `detect_all`, `enabled_providers`
  and every status consumer; the settings-presence model gives the same
  enable/disable semantics with zero type surgery.
- *Storing computed aggregates in `usage_snapshots`:* rejected — aggregates are
  a function of health flags that change between polls; storing them would
  create stale derived data and double bookkeeping (P1).
- *Consuming `GET /usage-queue` for per-account data:* rejected — destructive
  pop that races other consumers (spec Non-Goal).

## Affected Components

- **`src-tauri/src/cpa/` (new module):**
  - `client.rs` — `CpaClient` over the shared `config::http_client()` (5s
    connect / 15s total timeouts): `auth_files()` (GET
    `/v0/management/auth-files`, Bearer key), `api_call()` (POST
    `/v0/management/api-call` with `{authIndex, method, url, header}` and
    `$TOKEN$` substitution), typed `CpaError` enum (`Unreachable`,
    `Unauthorized`, `UnsupportedVersion`, `InvalidResponse`,
    `AccountCall { auth_index, .. }`) — P5, all display-safe, no emails or
    `status_message` in error strings. Feature-detects `/auth-files` fields
    (`auth_index`, `status`, `unavailable`) → `UnsupportedVersion` (OQ16
    default).
  - `quota.rs` — per-account window fetchers built on `api_call()`:
    Claude via `GET https://api.anthropic.com/api/oauth/usage`
    (`anthropic-beta: oauth-2025-04-20`), Codex via
    `GET https://chatgpt.com/backend-api/wham/usage` (`Chatgpt-Account-Id` from
    the auth-file record, Codex CLI User-Agent); serde response parsers mapping
    to `UsageBucket` (reusing the existing Claude bucket key/label conventions
    and the Codex primary/secondary window mapping in `fetcher.rs`).
  - `aggregate.rs` — the documented pure function (see Data Model).
- **`src-tauri/src/integrations/cpa.rs` (new, `minimax.rs` template):**
  `save_connection` / `load_connection` / `delete_connection` over
  `storage.set_setting`/`get_setting`/`delete_setting`; loopback-only URL
  validation; connect-time smoke test orchestration.
- **`src-tauri/src/integrations/manager.rs`:** `set_cpa_connection` /
  `clear_cpa_connection` entry points under `integration_mutation_guard()`
  (P4), mirroring `set_minimax_api_key` / `confirm_disable` shape; clear also
  purges CPA snapshot rows and `usage.cpa.*` settings (PII cleanup) and bumps
  the usage cache epoch.
- **`src-tauri/src/models.rs`:** `UsageBucket` gains `account_id:
  Option<String>`, `account_label: Option<String>`, `source: UsageSource`
  (`#[serde(default)]`); `UsageProviderError` gains `source`; `UsageData` gains
  `#[serde(default)]`-safe additive fields `cpa_accounts:
  Vec<CpaAccountHealth>` and `cpa_pools: Vec<CpaPoolAggregate>` (Serialize-only
  toward the FE, so additive is backward compatible).
- **`src-tauri/src/lib.rs`:** new CPA branch at the end of the
  `refresh_usage_cache` provider loop (runs last so its 5s connect-timeout
  worst case never delays native providers); `CPA_COOLDOWN_KEYS`
  (`usage.cpa.cooldown_until`, `usage.cpa.network_cooldown_until`,
  `usage.cpa.network_failures`) driven by the existing
  `check_provider_cooldown` / `record_network_failure` /
  `compute_network_backoff` helpers; `provider_status_key` extended with a CPA
  config fingerprint (enabled flag + url hash) so connect/disconnect
  invalidates the usage cache; cached-fallback path restores CPA rows plus the
  last persisted account-health snapshot (`usage.cpa.last_accounts` settings
  row) and recomputes the aggregate marked stale/offline.
- **`src-tauri/src/storage.rs`:** migration 36 (three nullable columns on
  `usage_snapshots`); `store_snapshot` writes the new columns; retrieval
  splits by source: `get_latest_usage_buckets(provider)` adds
  `AND (source IS NULL OR source = 'direct')` so native rows never absorb CPA
  rows, and a new `get_latest_cpa_usage_buckets()` selects `source = 'cpa'`
  keyed by (provider, account_id, bucket_key); `delete_cpa_usage_snapshots()`
  for disable cleanup. `usage_hourly` and `get_usage_history` are untouched
  because CPA bucket keys are account-qualified (see Data Model).
- **`src/types.ts`:** hand-maintained mirrors updated in lockstep —
  `UsageBucket` (`account_id?`, `account_label?`, `source?`),
  `UsageProviderError.source?`, `CpaAccountHealth`, `CpaPoolAggregate`,
  `UsageData` additions, settings-command payload types.
- **`src/components/widget/LimitsSection.tsx`:** row model grows a group
  concept: CPA pool rows keyed `(provider, 'cpa')`, rendered after the native
  row for the same provider with a visible `CPA` source tag (Q3 labeling);
  click/Enter/Space toggles indented per-account sub-rows (cap 6, then
  "…and N more"), `aria-expanded` on the aggregate row toggle following the
  `ViewSwitcher.tsx` chevron precedent; per-account placeholder (`—`, never a
  fabricated 0%) when windows are missing; health badges reuse the existing
  SETUP/UNAVAILABLE lamp treatment; a trailing muted count line
  "+N other accounts" for non-native CPA providers (Q4). Flat Polish (P9): no
  cards, hairline separation, existing swatches only, severity ramp reserved
  for the 50/80 buckets, keyboard focus visible.
- **`src/components/widget/WidgetTitleBar.tsx`:** sync-pill derivation reads
  `source`-tagged provider errors so CPA offline/stale/paused reuses the
  existing `offline`/`cached`/`paused` slate vocabulary — no new pill states
  (Goal 5).
- **`src/components/settings/IntegrationsTab.tsx`:** minimal CPA card in the
  Manage window (current density — DESIGN.md §6 exception): base URL (default
  `http://127.0.0.1:8317`), management key, Connect (runs validation + smoke
  test, shows the typed result), Disconnect (confirms, deletes key + rows).
- **`lat.md/features.md` "Live Usage View" and `lat.md/data-flow.md` "Usage
  Bucket Fetching":** extended with the CPA source, the aggregate function
  section (US-4 traceability target), and the degraded ladder; new test-spec
  sections referenced by `@lat:` comments (P7, P8).

## Data Model

**`UsageBucket` account dimension (Q7):**

```rust
pub struct UsageBucket {
    pub provider: IntegrationProvider,
    pub key: String,          // CPA rows: "cpa/{auth_index}/{window_key}"
    pub label: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
    #[serde(default)] pub sort_order: u32,
    #[serde(default)] pub source: UsageSource,        // Direct | Cpa
    #[serde(default)] pub account_id: Option<String>,  // CPA auth_index
    #[serde(default)] pub account_label: Option<String>, // email or label
}
```

CPA bucket keys are **account-qualified** (`cpa/{auth_index}/five_hour`). This
keeps `usage_hourly`'s `UNIQUE(hour, provider, bucket_key)` and every
`GROUP BY bucket_key` query correct without touching them, and gives history
queries a stable per-account key. (Spec gap resolved at plan time: unqualified
keys would collide with native rows in `usage_hourly`.)

**`usage_snapshots` migration (number 36, after current 35):** plain nullable
`ALTER TABLE ADD COLUMN account_id TEXT NULL, account_label TEXT NULL,
source TEXT NULL` (three statements), following the additive-ALTER precedent of
migration 33 rather than migration 14's rename-rebuild — no data rewrite is
needed for nullable additions. `NULL` source reads as `direct`.
`MAX_SUPPORTED_SCHEMA_VERSION` (src-tauri/src/storage.rs:100, currently 35) is
bumped to 36 alongside the migration. Downgrade is blocked by that
schema-version gate exactly like every prior migration — an older build
refuses to open a version-36 DB entirely. The nullable columns matter for
forward compatibility of code paths reading pre-36 rows (`NULL` ⇒ `direct`),
not for downgrade (P4; spec "no breaking changes to persisted data" covers the
forward migration path).

**Settings rows:** `integration.cpa.base_url`,
`integration.cpa.management_key` (plain SQLite settings rows — MiniMax parity,
Q6, risk documented in Risks), plus runtime keys `usage.cpa.cooldown_until`,
`usage.cpa.network_cooldown_until`, `usage.cpa.network_failures`,
`usage.cpa.last_attempt_at`, `usage.cpa.last_accounts` (JSON health snapshot
for offline fallback), `usage.cpa.window_smoke.{claude,codex}` (smoke-test
verdicts gating window polling per provider, US-3). Flat single-instance keys;
a future multi-instance list migrates to an indexed JSON row without
constraining this shape (OQ17 default).

**Loopback-only URL rule (v1, P11):** accepted iff the URL parses, scheme is
`http` or `https`, and host is exactly `127.0.0.1`, `localhost`, or `::1`.
Anything else is a typed `InvalidUrl` rejection at save time — no request is
ever issued to a non-loopback host.

**Aggregate — derived, not stored (Q1).** Pure function in
`src-tauri/src/cpa/aggregate.rs`:

```rust
fn compute_cpa_pools(accounts: &[CpaAccountSnapshot]) -> Vec<CpaPoolAggregate>
// CpaAccountSnapshot: provider, auth_index, label, status, status_message,
//                     disabled, unavailable, buckets: Option<Vec<UsageBucket>>
// CpaPoolAggregate:   provider, healthy, total, buckets (aggregate windows),
//                     nearest reset comes from the row logic below
```

Semantics (documented in lat.md, unit-tested): healthy = `status == "ready"` ∧
`!disabled` ∧ `!unavailable` ∧ not cooling; per window, aggregate utilization =
**max** across healthy accounts that have that bucket; healthy accounts with a
missing/failed bucket fetch are excluded from the max and counted as a
surfaced gap; if ALL healthy accounts lack buckets the aggregate window is
`None` → the UI shows a non-numeric placeholder; `healthy/total` uses the full
denominator including disabled/unavailable/cooling; `runtime_only` entries
included (OQ13 default). The aggregate window's `resets_at` is that of the
account owning the max; the row countdown is the nearest upcoming reset across
the aggregate windows (resolves the spec's "nearest relevant reset"
ambiguity). Note: CPA exposes no standalone "cooling" field — cooldown is
folded into `status`/`unavailable` ("mirror the runtime auth manager"), so
"cooling" is operationally `status != "ready"`; recorded as a spec
inconsistency below and in lat.md.

`CpaAccountHealth` — the FE-facing mirror of `CpaAccountSnapshot` (minus
buckets) — carries the same fields including `status_message`.
`status_message` surfaces only in the expanded per-account sub-row UI (item
5, e.g. title/tooltip text); it never appears in logs or in typed error
display strings (P5).

## API / Interface Changes

- **Tauri IPC (backward compatible, no breaking changes):** `UsageData` grows
  additive fields (`cpa_accounts`, `cpa_pools`); `UsageBucket` and
  `UsageProviderError` grow optional fields. All Serialize-only toward the FE;
  Rust-side deserialization sites (`UsageBucket` from cache paths) use
  `#[serde(default)]` so old persisted data loads cleanly.
- **New Tauri commands:** `set_cpa_connection { base_url, management_key } ->
  CpaConnectResult` (validates loopback URL, calls `auth_files`, runs one
  smoke `api_call` per present native provider, returns typed verdicts —
  distinct unreachable / 401 / unsupported-version / unexpected-response
  messages per US-1), `clear_cpa_connection` (delete key + url, purge CPA
  snapshot rows and `usage.cpa.*` keys, epoch bump → rows disappear on next
  emit), `get_cpa_connection_status` (masked: url + configured flag only — the
  key is never returned to the webview).
- **No external API changes.** Quill only *consumes* CPA's documented
  Management API and, through it, the two verified upstream quota endpoints.
  Induced upstream transmission is documented (P11, see Risks).
- **TS mirrors:** every shape above lands in `src/types.ts` in the same work
  item as its Rust change (strict TS, zero-warning gates — P6).

## Testing Strategy

P7: automated tests require explicit authorization. The spec itself grants it
narrowly — US-4 mandates "a documented pure function with unit-testable
semantics" — and the plan treats acceptance of this plan as authorizing only
the tests enumerated here, each linked one-to-one to a lat.md spec section via
`@lat:` comments (P8):

- **Unit — aggregate function** (`cpa/aggregate.rs`): max-across-healthy per
  window; disabled/unavailable excluded from math but in denominator;
  missing-bucket account excluded from max; all-healthy-missing ⇒ `None`
  placeholder; empty pool; `runtime_only` inclusion. No live CPA needed.
- **Unit — response parsers** (`cpa/client.rs`, `cpa/quota.rs`): serde
  fixtures built from the research log's verified field lists (`/auth-files`
  entry, `api-call` envelope `{status_code, header, body}`, Anthropic
  `oauth/usage` windows, Codex `wham/usage` `rate_limit` shape); malformed and
  missing-field fixtures drive the typed-error paths, including the
  `UnsupportedVersion` feature-detect. No live CPA needed.
- **Unit — migration 36**: following the existing `storage.rs` `#[cfg(test)]`
  precedent — open a pre-migration schema, migrate, assert nullable columns
  exist, old rows read as `source = direct`, and `get_latest_usage_buckets`
  excludes a seeded `source='cpa'` row. No live CPA needed.
- **Manual acceptance checklist** (mapped to Goal 6 and US acceptance
  criteria; **requires the user's live CPA instance + management key** — CPA
  v7.2.113 on 127.0.0.1:8317 with ≥2 Claude accounts; Codex M≥2 as available):
  US-1 connect success + each typed failure (wrong key → 401 message, wrong
  port → unreachable, disable deletes key and rows); US-2 all N+M accounts
  visible, a disabled/unavailable account visually distinct within ≤3 min;
  US-3 per-account windows render with severity + countdown, forced fetch
  failure shows the placeholder; US-4 aggregate equals hand-computed max and
  `healthy/total`; US-5 kill CPA → cached/offline treatment + pill within one
  poll + backoff cycle (bounded 4–33 min per spec note), restart → automatic
  recovery; fan-out timing measurement (below). Zero-warning gates: `cargo
  clippy`/`cargo test`, `tsc`, existing lint (P6); `lat check` (P8).
- **Performance evidence (P10):** the CPA branch logs a `cpa_phase_ms`
  duration. The 12-account budget check is a fixture harness within item 3's
  unit-test scope: a synthetic `CpaAccountSnapshot` set drives the fan-out
  scheduler with a stubbed transport, asserting the scheduled fan-out stays
  inside the stated budget. The [live] complement records `cpa_phase_ms` for
  the user's real pool during item 7's acceptance pass.

## Risks

- **Management key blast radius (Q6, P11):** the key authorizes `api-call`
  with every pooled token, yet sits in a plain SQLite settings row (0600 DB,
  MiniMax parity). Accepted for v1 and documented; never logged, never in
  telemetry, never returned to the webview. The key is also unrecoverable from
  CPA's disk (bcrypt hash only) — the user must paste it; a connect-time smoke
  test catches paste errors immediately. Mitigation follow-up: OS-keyring
  migration filed as a separate P3 issue (see Target Epic).
- **Upstream response drift:** `oauth/usage` and `wham/usage` are unversioned,
  undocumented endpoints. Parsers are lenient (`#[serde(default)]`, unknown
  fields ignored), failures are per-account typed errors that degrade that
  account to health-only (Q2) — never a whole-source failure, never invented
  data (P1, P5).
- **Fan-out cost (P3, P10, P11) — explicit budget:** per 3-min poll: 1×
  `auth-files` + up to **16** accounts' window calls (beyond 16, deterministic
  order by `auth_index`, remainder is health-only that cycle), concurrency
  bounded by a 3-permit semaphore with 250 ms launch stagger, per-request
  timeouts from the shared `http_client` (5 s connect / 15 s total). Worst
  case ≈ ceil(16/3) × 15 s ≈ 90 s, typical < 5 s — inside the 180 s cadence;
  CPA runs last in the provider loop so native providers are never delayed
  (US-5). Upstream load ≈ 20 requests/hour/account — orders below interactive
  client traffic; a per-account 429 simply skips that account until the next
  poll. This is Quill-*induced* off-device transmission via CPA to Anthropic/
  OpenAI quota endpoints — documented here and in lat.md (P11); `api-call` is
  verified side-effect-free on CPA counters/cooldowns.
- **CPA version drift (OQ16):** older builds lack `auth_index`/`unavailable`.
  Feature-detect on the `/auth-files` payload; a typed `UnsupportedVersion`
  error distinct from unreachable/bad-key, with its own settings-surface
  message, so old builds do not present as mystery outages (P5).
- **Expansion UI is a new widget pattern (P9):** no expand/collapse exists in
  the widget today. Risk of violating Flat Polish or keyboard access —
  mitigated by reusing the `ViewSwitcher` chevron/`aria-expanded` precedent,
  hairline-only structure, existing swatches/severity ramp, focus-visible
  toggle, and a design spot-check against DESIGN.md before merge.
- **Double-count confusion (Q3):** a direct credential may also live in CPA;
  both rows show. Mitigations: the pool row carries a visible `CPA` source
  tag, sub-rows show account emails/labels, and the settings card states the
  overlap explicitly. No dedup in v1 (spec Non-Goal).
- **Rollback:** disconnect/disable deletes the key and url, purges
  `source='cpa'` snapshot rows and `usage_hourly` rows
  `WHERE bucket_key LIKE 'cpa/%'` (removing stored emails/labels — PII
  hygiene), clears `usage.cpa.*` state, and bumps the cache epoch so rows
  vanish on next emit. App-version downgrade is blocked by the schema-version
  gate — `MAX_SUPPORTED_SCHEMA_VERSION` moves to 36, so an older build
  refuses to open a version-36 DB, exactly like every prior migration (P4).
  The nullable migration-36 columns aid forward compatibility of code paths
  reading pre-36 rows, not downgrade.

## Sequencing

Ordered phases; blockers are explicit and become the bead DAG.

1. **Usage bucket account dimension and snapshot migration** — P1, blocked by:
   none. Add `source`/`account_id`/`account_label` to `UsageBucket` and
   `source` to `UsageProviderError` in `src-tauri/src/models.rs` (all
   `#[serde(default)]`); migration 36 nullable columns; bump
   `MAX_SUPPORTED_SCHEMA_VERSION` (src-tauri/src/storage.rs:100, currently
   35) to 36 alongside migration 36; update `store_snapshot`,
   `get_latest_usage_buckets` (direct-only predicate), add
   `get_latest_cpa_usage_buckets` and `delete_cpa_usage_snapshots` (which
   also deletes `usage_hourly` rows `WHERE bucket_key LIKE 'cpa/%'`); mirror
   in `src/types.ts`. *Acceptance:* zero-warning build both sides; migration
   unit test passes and `init` reopens the freshly migrated DB; existing
   snapshots load with `source = direct`; native bucket queries exclude
   seeded CPA rows; `delete_cpa_usage_snapshots` removes CPA rows from both
   `usage_snapshots` and `usage_hourly`.
2. **CPA management client module** — P1, blocked by: none (parallel with 1).
   `src-tauri/src/cpa/{client,quota}.rs`: `auth_files()`, `api_call()`,
   Claude/Codex window fetchers with verified headers, typed `CpaError` incl.
   `UnsupportedVersion` feature-detect, loopback URL validator. *Acceptance:*
   parser unit tests green on research-log fixtures; error strings display-safe
   (no emails/status_message); no call path reachable without a configured
   loopback URL.
3. **Poll-loop integration, caching, and pool aggregation** — P1, blocked by:
   1, 2. CPA branch in `refresh_usage_cache` (runs last; `CPA_COOLDOWN_KEYS`;
   16-account cap, 3-permit semaphore, 250 ms stagger; per-account failure ⇒
   health-only, connection failure ⇒ `record_network_failure`; reads the
   `usage.cpa.window_smoke.{claude,codex}` verdicts written at connect time
   by item 4 to gate per-provider window polling, US-3);
   `compute_cpa_pools` pure function; `UsageData.cpa_accounts`/`cpa_pools`;
   `src/types.ts` mirrors for the types introduced here
   (`UsageData.cpa_accounts`/`cpa_pools`, `CpaAccountHealth`,
   `CpaPoolAggregate`), honoring the API section's "TS mirror lands with its
   Rust change" promise; `usage.cpa.last_accounts` persistence;
   `provider_status_key` CPA fingerprint; `cpa_phase_ms` log; a fan-out
   fixture harness in unit-test scope (synthetic `CpaAccountSnapshot` set
   driving the fan-out scheduler with a stubbed transport) for the
   12-account `cpa_phase_ms` budget check. *Acceptance* ([live] = requires
   the user's real CPA instance + management key; [live] bullets defer to
   item 7's acceptance checklist, so this item closes on [auto] bullets
   alone): [auto] aggregate unit tests green (Q1 semantics incl. placeholder
   case); [live] with CPA configured, one poll emits per-account buckets +
   pools; [live] with CPA unreachable, cached rows return marked offline and
   native providers are unaffected; [auto] a failed or absent
   `usage.cpa.window_smoke.*` verdict suppresses that provider's window
   calls (health-only); [auto] the 12-account fixture harness keeps the
   scheduled fan-out inside the stated budget; [auto] unconfigured CPA ⇒ no
   HTTP request issued (asserted via the loopback gate in unit scope),
   `cpa_accounts`/`cpa_pools` empty/absent in emitted `UsageData`, and no
   `cpa_phase_ms` log line.
4. **CPA settings surface and connection lifecycle** — P1, blocked by: 1, 2
   (calls `delete_cpa_usage_snapshots()`, which item 1 creates).
   `integrations/cpa.rs`, `manager.rs` mutations under
   `integration_mutation_guard`, Tauri commands (`set_cpa_connection` with
   auth-files validation + per-provider smoke `api_call`, writing the
   `usage.cpa.window_smoke.{claude,codex}` verdicts at connect time for item
   3 to read, `clear_cpa_connection` with full purge,
   `get_cpa_connection_status`), `IntegrationsTab.tsx` card with default
   URL, typed failure messages, and overlap/double-count copy. *Acceptance:*
   US-1 criteria — distinct unreachable/401/unsupported/unexpected messages;
   key never sent to webview; connect persists the per-provider
   `usage.cpa.window_smoke.*` verdicts; disconnect deletes key, purges CPA
   rows/settings including `usage_hourly` rows
   `WHERE bucket_key LIKE 'cpa/%'`, bumps epoch.
5. **LIMITS aggregate row and expandable account sub-rows** — P1, blocked by:
   3. `LimitsSection.tsx` group model: CPA pool row per provider with `CPA`
   tag, healthy/total count, aggregate windows with 50/80 ramp + stale
   handling, nearest-reset countdown; `aria-expanded` toggle
   (click/Enter/Space) revealing ≤6 indented sub-rows then "…and N more";
   per-account health badges and non-numeric placeholders; per-account
   failure detail (including `status_message`) surfaces in the expanded
   sub-row only (e.g. title/tooltip text) — never in logs or typed error
   display strings (P5); the `LimitsSection` empty-state mapping for CPA
   rows (401/config ⇒ SETUP, connection-refused on a configured integration
   ⇒ UNAVAILABLE — resolves OQ10; moved here from item 6 since it is a
   `LimitsSection.tsx` change); "+N other accounts" trailing line (Q4); Flat
   Polish audit at 360 px. *Acceptance* ([live] = requires the user's real
   CPA instance + management key; [live] bullets defer to item 7's
   acceptance checklist, so this item closes on [auto] bullets alone):
   [auto] CPA health-state → badge mapping, kept within the existing
   lamp/badge vocabulary (SETUP amber / UNAVAILABLE slate precedent): ready
   ⇒ normal row, no badge; disabled ⇒ muted slate "DISABLED";
   unavailable/cooldown (status != ready) ⇒ amber "COOLING" (or
   "UNAVAILABLE") — visually distinct from disabled; disabled and cooling
   can never render identically; [live] US-2/US-3/US-4 UI criteria against
   the real pool; [auto] keyboard-only operation; [auto] no new colors;
   [auto] stale `resets_at` renders neutral; [auto] placeholder never
   renders as 0%; [auto] empty-state mapping renders SETUP/UNAVAILABLE per
   the OQ10 resolution.
6. **Sync pill and degraded-state wiring** — P2, blocked by: 3.
   Titlebar/sync-pill only: `WidgetTitleBar.tsx` consumes source-tagged
   provider errors so CPA offline/stale/paused maps onto the existing pill
   vocabulary; recovery clears state on next successful poll. (The
   `LimitsSection` empty-state mapping moved to item 5.) *Acceptance*
   ([live] = requires the user's real CPA instance + management key; [live]
   bullets defer to item 7's acceptance checklist, so this item closes on
   [auto] bullets alone): [auto] pill derivation maps source-tagged CPA
   errors onto the existing `offline`/`cached`/`paused` vocabulary with no
   new pill states; [live] US-5 criteria — kill CPA ⇒ cached/offline pill
   within one poll + backoff cycle; [live] restart ⇒ automatic recovery;
   [live] native rows keep `live`.
7. **Documentation, tests, and acceptance pass** — P1, blocked by: 1, 2, 3, 4,
   5, 6. lat.md updates (features "Live Usage View", data-flow "Usage Bucket
   Fetching", new aggregate-semantics and CPA test-spec sections with `@lat:`
   links; induced-transmission note per P11), `lat check`, full zero-warning
   gates, manual acceptance checklist against the live CPA instance (Goal 6),
   recorded `cpa_phase_ms` measurements vs. budget (P10). *Acceptance:* `lat
   check` clean; every US criterion checked off with evidence; measurements
   recorded in the bead.

Traceability: US-1→4; US-2→3,5; US-3→2,3,5; US-4→3,5,7; US-5→3,6. Q1→3; Q2→2,
3,4; Q3→4,5; Q4→3,5; Q5→5; Q6→4 + follow-up issue; Q7→1.

## Backlog Refinement

None — no P4 backlog sources were supplied; nothing to refine.

## Target Epic

New epic to be created at create-beads (no existing epic). Additionally, file
one approved follow-up as a separate P3 issue (not part of this epic's DAG):
**OS-keyring migration for high-sensitivity secrets** — move
`integration.cpa.management_key` (and optionally the MiniMax key) from plain
SQLite settings rows to the OS keyring, per Clarifications Q6.

## Alignment fixes applied

- [B-must] Item 1 scope + acceptance now include bumping
  `MAX_SUPPORTED_SCHEMA_VERSION` (src-tauri/src/storage.rs:100, 35 → 36)
  alongside migration 36, with acceptance that `init` reopens the freshly
  migrated DB; also stated in the Data Model migration passage.
- [B-must] Corrected the false downgrade claim in Data Model and
  Risks/Rollback: downgrade is blocked by the schema-version gate exactly
  like every prior migration; nullable columns only aid forward
  compatibility of code paths, not downgrade.
- [B-must] Item 4's blockers changed from "2" to "1, 2" because it calls
  `delete_cpa_usage_snapshots()`, which item 1 creates.
- [B-should] Moved the `LimitsSection` empty-state mapping for CPA rows
  (OQ10) from item 6 into item 5's scope; item 6 is now
  titlebar/sync-pill-only (blockers unchanged: 3).
- [B-should] Gave the `usage.cpa.window_smoke.{claude,codex}` keys owners:
  item 4 writes the smoke-test verdicts at connect time; item 3 reads them
  to gate per-provider window polling — both in scope and acceptance.
- [B-should] Extended CPA purge to `usage_hourly`:
  `delete_cpa_usage_snapshots` also deletes rows
  `WHERE bucket_key LIKE 'cpa/%'`; reflected in item 1 and item 4
  acceptance and in the Rollback passage.
- [B-should] Rewrote the "simulated 12-account fixture" measurement as a
  fixture harness within item 3's unit-test scope (synthetic
  `CpaAccountSnapshot` set driving the fan-out scheduler with a stubbed
  transport), keeping the live-pool measurement as the [live] complement in
  item 7.
- [B-should] Tagged every acceptance bullet in items 3, 5, and 6 as [auto]
  or [live] (live = requires the user's real CPA instance + management
  key); [live] bullets defer to item 7's acceptance checklist so items
  3/5/6 close on [auto] bullets alone.
- [B-should] Replaced item 3's vague "zero fields of note" acceptance with
  concrete criteria: unconfigured CPA ⇒ no HTTP request issued (asserted
  via the loopback gate in unit scope), `cpa_accounts`/`cpa_pools`
  empty/absent in emitted `UsageData`, and no `cpa_phase_ms` log line.
- [B-should] Corrected the enum usage-site count in Architecture Approach
  (both occurrences) from "~273 enum usage sites across 16 files" to
  "hundreds of usage sites (~527 occurrences across 21 Rust files)".
- [B-should] Item 3 scope now explicitly includes the `src/types.ts`
  mirrors for the types it introduces (`UsageData.cpa_accounts`/`cpa_pools`,
  `CpaAccountHealth`, `CpaPoolAggregate`), honoring the API section's "TS
  mirror lands with its Rust change" promise.
- [A-should] Added `status_message` to the
  `CpaAccountHealth`/`CpaAccountSnapshot` field list in Data Model; item 5
  specifies that per-account failure detail (including `status_message`)
  surfaces only in the expanded sub-row (e.g. title/tooltip text), staying
  out of logs and typed error display strings per constitution P5.
- [A-should] Item 5 acceptance now states an explicit CPA
  health-state → badge mapping within the existing lamp/badge vocabulary:
  ready ⇒ normal row; disabled ⇒ muted slate "DISABLED";
  unavailable/cooldown (status != ready) ⇒ amber "COOLING"/"UNAVAILABLE",
  visually distinct from disabled — disabled and cooling can never render
  identically.
