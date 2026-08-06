# Analysis: p4-backlog-sweep

Report-only pre-bead analysis (quick depth — abbreviated). Spec and plan
were clarified via delegated deep research; design decisions are fixed and
code-verified.

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|--------------------------|---------------------------|--------|
| Story 1 — count and model chips describe one snapshot (quill-qqt) | Architecture Stream A; Sequencing "Fuse merge and enrichment in the observed registry"; Backlog Refinement quill-qqt AC 1-5 | full |
| Story 2 — no phantom open agent after clock step-back (quill-hzx) | Architecture Stream B; Sequencing "Backdated-Stop root invalidation guard"; Backlog Refinement quill-hzx AC 1-5 | full |
| Story 3 — Codex per-agent identity (quill-kdx) | Architecture Stream C; Sequencing items "Codex spawn-metadata resolver ingestion" → "Backfill migration 39 and reingest sweep wiring" → "Codex analytics verification"; Backlog Refinement quill-kdx AC 1-6 | full |
| Goal — every P4 refined or superseded, none left bare | Backlog Refinement (all three dispositioned P2/P3) | full |
| Non-Goals — no registry redesign, no general clock-skew handling, no full multi-agent feature, no frontend changes | Streams stay local to server.rs / transcript_identity.rs / storage.rs; agent_path skipped; IPC contract byte-identical | full |
| Clarifications Q1-Q4 (fix shape, stop policy, identity value, backfill) | Architecture Approach + Data Model + API/Interface Changes | full |
| Test specifications | Testing Strategy (authorization-gated, listed for confirmation) | full — pending explicit test authorization (Constitution 7) |
| lat.md traceability | Affected Components (frontend.md, live-subagent-count-tests.md, backend.md) + final quality-gate item | full |

## Backlog Disposition

| Source P4 id | Plan work item(s) / non-goal | Disposition | Ready to resolve? |
|--------------|------------------------------|-------------|-------------------|
| quill-qqt | Fuse merge and enrichment in the observed registry | refine-in-place → P3 | yes |
| quill-hzx | Backdated-Stop root invalidation guard | refine-in-place → P3 | yes |
| quill-kdx | Codex spawn-metadata resolver ingestion; Backfill migration 39 and reingest sweep wiring; Codex analytics verification | split-and-supersede → P2 | yes |

## Target Epic

New epic: **"live-count edge cases + codex agent identity"**. No existing
epic fits — all three sources are unparented with no discovered-from
provenance. No ambiguity.

## Remaining Risks

- Live Codex hook `agent_id` == thread id is inferred from census (zero
  collisions), not confirmed against a captured payload — verified during
  implementation before relying on hook↔ingestion identity joins.
- Boot-time full reingest sweep (~5,435 Codex files) — matches prior
  migration profile; sweep duration logged and compared (Constitution 10
  note).
- Retention watermark must block reinsertion of re-extracted pre-watermark
  detail — explicit verification item in "Codex analytics verification".
- `merge` signature change causes mechanical test churn across server.rs
  tests + lib.rs — contained, compile-enforced.

## Unresolved Questions

None. The human approved bead creation (GO) and explicitly authorized all
listed test additions at the analyze gate (Constitution 7 satisfied):
fused-merge consistency, deadlock safety, four hzx ordering tests, two-era
Codex fixtures, restatement conflicts, backfill idempotency.

## Constitution Check

| # | Principle | Verdict |
|---|-----------|---------|
| 1 | Local source-backed truth | pass — both registry fixes prefer null over wrong numbers; identity only from source evidence |
| 2 | Established stack and boundaries | pass — all changes in existing layers, no new crates |
| 3 | Responsive execution | pass — snapshot-then-resolve keeps the registry mutex off DB work; migration off UI thread |
| 4 | Recoverable mutation | pass — migration 39 single transaction; reingest flag clears only after full success |
| 5 | Typed failure boundaries | pass — conflicts stay `IdentityError` variants |
| 6 | Zero-warning quality gates | pass — fmt/clippy/test/lat check in final gate item |
| 7 | Authorized behavior testing | tension — test additions listed, authorization pending at this gate |
| 8 | Architecture traceability | pass — three lat.md sections enumerated, `lat check` gated |
| 9 | Glass Cockpit discipline | n/a — no UI change |
| 10 | Measured performance | tension noted — reingest cost rides prior measured profile; duration logged, one-off measurement if migration-26 comparison unavailable |
| 11 | Explicit external transmission | pass — none |
| 12 | Gated delivery | pass — Beads epic/task mapping; no commit/push without authority |

## Recommendation

**GO** — every backlog source has a concrete disposition with verifiable
P2/P3 acceptance criteria, the target epic is unambiguous (new), all
clarification decisions are code-verified with file:line evidence, and the
only open item is the Constitution 7 test-authorization confirmation, which
is an approval input at this gate rather than a spec/plan defect.
