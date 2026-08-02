# Analysis: cpa-oauth-limits

Report-only cross-check of `spec.md` (clarified, Q1–Q7 decided) against
`plan.md` (7 sequenced work items, alignment fixes applied) and
`constitution.md` (P1–P12). No spec or plan edits were made.

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|---|---|---|
| Goal 1 — CPA as first-class source (service-only, MiniMax pattern) | Architecture Approach decision 2 (settings-backed `CpaConnection`, no `ProviderStatus` entry, null-impact guarantee); items 2, 4 | full |
| Goal 2 — Per-account visibility (identity, health, Claude/Codex windows, 50/80 ramp, health-only degradation) | Items 2 (client + verified window fetchers), 3 (poll fan-out, health mapping), 5 (sub-rows, badges, placeholders) | full |
| Goal 3 — Worst-case aggregate + healthy/total count | Item 3 (`compute_cpa_pools` pure function, Q1 semantics incl. all-missing placeholder); Data Model aggregate section; item 5 rendering | full |
| Goal 4 — Fits 360px Flat Polish (aggregate-first, expand-on-demand, cap ~6 + "…and N more") | Item 5 (group model, `aria-expanded` toggle per ViewSwitcher precedent, Flat Polish audit at 360px) | full |
| Goal 5 — Degrades exactly like existing providers (backoff, cached snapshot, stale/offline/paused, sync pill) | Architecture decisions 3 and 5 (reused pipeline + source-tagged `UsageProviderError`); items 3, 6 | full |
| Goal 6 — Measurable acceptance (N+M accounts visible, unavailable distinct ≤3 min, kill-CPA flip within one poll + backoff) | Item 7 manual acceptance checklist mapped to Goal 6; flip window bounded 4–33 min per spec note. Full plan coverage; execution is gated on the user's live CPA instance + pasted management key (all [live] bullets defer to item 7) | full |
| US-1 — Connect Quill to CPA (settings surface, typed connect verdicts, disable purge) | Item 4 (`set_cpa_connection` with distinct unreachable/401/unsupported/unexpected messages, smoke tests, `clear_cpa_connection` full purge incl. `usage_hourly`); loopback URL rule in Data Model | full |
| US-2 — Every pooled account's health (3-min poll, unavailable/disabled distinct, grouped under provider, healthy/total formula) | Items 3 (auth-files poll, health snapshot persistence) and 5 (badge mapping: DISABLED vs COOLING never identical); `runtime_only` included per OQ13 default in aggregate semantics | full |
| US-3 — Per-account rate-limit windows (verified `api-call` mechanism, smoke-test gate, non-numeric placeholder, `resets_at` semantics) | Items 2 (Claude `oauth/usage` + Codex `wham/usage` fetchers with verified headers), 4 (writes `usage.cpa.window_smoke.*` verdicts), 3 (reads verdicts to gate polling), 5 (placeholder never 0%, stale renders neutral) | full |
| US-4 — Aggregate pool headroom (one aggregate row, worst-case math, documented pure unit-testable function, lat.md-traceable) | Items 3 (function + unit tests), 5 (row rendering, nearest-reset), 7 (lat.md aggregate-semantics section + `@lat:` links) | full |
| US-5 — CPA outage degrades gracefully (backoff, independent providers, cached + pill, auto-recovery) | Items 3 (`CPA_COOLDOWN_KEYS`, runs last so natives never delayed, cached fallback) and 6 (source-tagged pill, no new states, auto-clear on success) | full |
| Q1 — Aggregate = worst-case max across healthy; excluded-but-counted denominator; missing-bucket gap; all-missing placeholder | Item 3; Data Model aggregate semantics (verbatim Q1 rules, incl. "cooling ≡ status != ready" note recorded as a spec inconsistency) | full |
| Q2 — Full per-account windows for Claude AND Codex via verified `api-call`; smoke test before window polling; health-only fallback | Items 2, 3, 4 (traceability line Q2→2,3,4) | full |
| Q3 — Both rows shown, no dedup; CPA row visibly labeled | Items 4 (settings-card overlap copy) and 5 (`CPA` source tag); Risks "double-count confusion" | full |
| Q4 — Claude + Codex only; others as neutral "+N other accounts" line | Items 3 and 5 (trailing muted count line, no new identities/colors) | full |
| Q5 — Aggregate row + expandable sub-rows, cap ~6, keyboard/`aria-expanded` | Item 5 (click/Enter/Space toggle, ViewSwitcher precedent, [auto] keyboard-only acceptance) | full |
| Q6 — Plain SQLite settings row for the key; keyring as separate follow-up | Item 4 + Data Model settings rows; Risks (blast radius accepted, documented); Target Epic files the P3 keyring follow-up | full |
| Q7 — Keep enum; optional `#[serde(default)]` account fields; nullable SQLite columns via migration | Item 1 (models, migration 36, `MAX_SUPPORTED_SCHEMA_VERSION` 35→36, direct-only predicate, TS mirror); account-qualified bucket keys close the `usage_hourly` collision gap | full |
| Plan-time default — fan-out budget (OQ12) | Risks: explicit budget (16-account cap, 3-permit semaphore, 250 ms stagger, ≈90 s worst case vs 180 s cadence, ~20 req/h/account); item 3 fixture harness asserts it; item 7 records live `cpa_phase_ms` | full |
| Plan-time default — feature-detect / unsupported version (OQ16) | Item 2 (`UnsupportedVersion` typed error via `/auth-files` field detection); item 4 (distinct settings-surface message) | full |
| Plan-time default — single-instance settings shape (OQ17) | Data Model: flat `integration.cpa.*` keys with an explicit migration path to an indexed JSON list; item 4 | full |

No partially covered acceptance criteria were found: every US bullet maps to a
named work item, and the plan's own traceability line (US-1→4; US-2→3,5;
US-3→2,3,5; US-4→3,5,7; US-5→3,6; Q1→3; Q2→2,3,4; Q3→4,5; Q4→3,5; Q5→5;
Q6→4+follow-up; Q7→1) checks out against the item scopes. The one caveat worth
naming: all [live] acceptance bullets (Goal 6, parts of US-2/3/4/5) are
plan-covered but cannot be *executed* until the user pastes the management key
— tracked as a risk, not a coverage gap.

## Backlog Disposition

| Source P4 id | Plan work item(s) / non-goal | Disposition | Ready to resolve? |
|---|---|---|---|
| None — no P4 backlog sources supplied | — | — | n/a |

## Target Epic

New epic — created at the create-beads step (no existing epic; resolved, not
ambiguous). Additionally, one approved follow-up is filed as a separate P3
issue outside this epic's DAG: **OS-keyring migration for the CPA management
key** (and optionally the MiniMax key), per Clarifications Q6.

## Remaining Risks

- **Upstream schema drift.** `api.anthropic.com/api/oauth/usage` and
  `chatgpt.com/backend-api/wham/usage` are unversioned, undocumented
  endpoints. *Mitigation:* lenient parsers (`#[serde(default)]`, unknown
  fields ignored); per-account typed failures degrade that account to
  health-only — never whole-source failure, never invented data (P1, P5).
- **Management key handling.** The key authorizes `api-call` with every
  pooled token yet sits in a plain SQLite row; it is unrecoverable from CPA's
  disk (bcrypt hash only), so the user must paste the plaintext. *Mitigation:*
  never logged/telemetered/returned to webview; connect-time smoke test
  catches paste errors immediately; P3 keyring follow-up filed.
- **Live end-to-end verification is impossible until the key is entered.**
  All [live] acceptance bullets defer to item 7's checklist against the real
  CPA v7.2.113 instance. *Mitigation:* items 3/5/6 close on [auto] bullets
  alone; the smoke-test gate keeps window polling off until connect-time
  validation passes, so nothing ships un-exercised into the poll loop.
- **New expansion UI pattern at 360px.** No expand/collapse exists in the
  widget today — Flat Polish and keyboard-access risk (P9). *Mitigation:*
  reuse the ViewSwitcher chevron/`aria-expanded` precedent, hairline-only
  structure, existing swatches/severity ramp, focus-visible toggle, design
  spot-check against DESIGN.md before merge.
- **Fan-out budget assumptions vs real upstream tolerance.** Anthropic/OpenAI
  tolerance of ~20 requests/hour/account is asserted, not verified (spec
  OQ12 remains UNVERIFIED until live). *Mitigation:* 16-account cap,
  3-permit semaphore, 250 ms stagger, per-account 429 skip-until-next-poll,
  fixture-harness budget assertion plus live `cpa_phase_ms` measurement in
  item 7 (P10).
- **CPA version drift.** Older builds lack `auth_index`/`unavailable` and
  would otherwise present as mystery outages. *Mitigation:* feature-detect on
  the `/auth-files` payload with a typed `UnsupportedVersion` error distinct
  from unreachable/bad-key, plus its own settings-surface message (P5).
- **Accepted double-count.** The same underlying account reachable via a
  direct credential and a CPA auth file appears (and is polled) twice; the
  aggregate can overstate the pool. *Mitigation:* explicitly labeled — `CPA`
  source tag on the pool row, account emails/labels in sub-rows, overlap copy
  in the settings card; dedup-by-email deferred by decision (Q3, Non-Goal).
- **Rollback surface.** Disconnect must purge snapshots, `usage_hourly`
  `cpa/%` rows (stored emails/labels — PII), and `usage.cpa.*` keys; app
  downgrade is blocked by the schema-version gate (35→36), same as every
  prior migration. *Mitigation:* purge is in item 1/4 scope and acceptance;
  the plan's alignment pass already corrected the earlier false downgrade
  claim (P4).

## Unresolved Questions

- **OQ11 — faster health-only poll.** 3-minute cadence decided for v1; a
  cheaper `auth-files`-only fast poll is deferred to a possible v2. CPA's
  health state can lag pool-exhaustion events mid-session by up to a cycle.
- **OQ16 — minimum CPA version floor.** Resolved by feature-detection rather
  than a version number; there is deliberately no documented "CPA ≥ X.Y"
  statement. Acceptable, but support questions will be answered by the typed
  error, not a compatibility table.
- **Upstream rate-limit tolerance (OQ12 residue).** The exact tolerance of
  Anthropic/OpenAI quota endpoints to per-account polling is only measurable
  live; the budget is a defensible estimate until item 7's measurements land.
- **OQ2 — port config key.** Never verified upstream; moot for v1 (manual URL
  entry), but would resurface if zero-config discovery is ever picked up.

## Constitution Check

| # | Principle | Verdict | Note |
|---|---|---|---|
| P1 | Local source-backed truth | pass | Aggregate is derived, never stored; missing buckets are surfaced gaps, all-missing ⇒ non-numeric placeholder; no fabricated 0%. |
| P2 | Established stack and boundaries | pass | All CPA HTTP in a new Rust module; frontend gets typed snapshots; hand-maintained TS mirrors land with each Rust change; webview-fetch alternative explicitly rejected. |
| P3 | Responsive execution | pass | CPA joins the async refresh, runs last, bounded semaphore + stagger + cap; unconfigured CPA costs exactly nothing (no probe, no poll time). |
| P4 | Recoverable mutation | pass | Mutations under `integration_mutation_guard`; additive nullable migration 36 with schema-version gate; disconnect fully purges and epoch-bumps. |
| P5 | Typed failure boundaries | pass | `CpaError` enum incl. `UnsupportedVersion` and per-account `AccountCall`; display-safe strings (no emails/`status_message`); per-account failure never fails the provider snapshot. |
| P6 | Zero-warning quality gates | pass | clippy/cargo test/tsc/lint gates in item acceptance and item 7. |
| P7 | Authorized behavior testing | tension | Spec explicitly authorizes only US-4's unit-testable aggregate; the plan enumerates parser, migration, and fixture-harness tests and treats plan acceptance as the authorization. Flagged honestly in Testing Strategy — plan approval must be understood as granting exactly that enumerated set. |
| P8 | Architecture traceability | pass | lat.md sections for source, aggregate semantics, degraded ladder, induced transmission; `@lat:` links one-to-one; `lat check` in item 7 acceptance. |
| P9 | Glass Cockpit discipline | pass | Flat Polish audit at 360px, existing swatches/severity ramp only, keyboard + `aria-expanded`, no new colors or pill states; the new expansion pattern is a tracked risk with a named precedent, not a violation. |
| P10 | Measured performance | tension | Budget is explicit (16-cap / 3-permit / 250 ms / ≈90 s worst case vs 180 s) and the fixture harness asserts it reproducibly, but the live numbers (`cpa_phase_ms` on the real pool, upstream tolerance) are pending item 7 — acceptable staging, incomplete evidence today. |
| P11 | Explicit external transmission | tension | Localhost-only boundary is enforced (loopback URL validation), but window polling is Quill-*induced* off-device transmission via CPA to Anthropic/OpenAI quota endpoints. Documented in spec, plan Risks, and a planned lat.md note; opt-in via explicit user configuration; key never leaves device. Documented tension, not a violation. |
| P12 | Gated delivery | pass | Sequencing becomes the bead DAG; gates precede completion; epic + P3 follow-up created at create-beads; no commit/push authority assumed. |

## Recommendation

**GO** — Every Goal, user story acceptance criterion, clarification decision
(Q1–Q7), and plan-time default traces to a named work item with concrete
acceptance bullets; the coverage table above found 21/21 full with zero
partial or uncovered rows, and the plan's "Alignment fixes applied" log shows
the gaps this analysis would otherwise flag (schema-version bump, downgrade
claim, item-4 blocker, smoke-verdict ownership, `usage_hourly` purge) were
already caught and fixed. The three constitution tensions (P7 test
authorization scope, P10 pending live measurements, P11 induced upstream
transmission) are each explicitly documented in the plan with a defined
resolution path rather than left implicit, and the residual risks all carry
concrete mitigations — the largest practical dependency is that the [live]
half of acceptance waits on the user pasting the CPA management key, which
the smoke-test gate and [auto]-closable items 3/5/6 are specifically designed
to absorb. No fixes are required before creating beads.
