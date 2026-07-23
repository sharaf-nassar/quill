# Spec: continuity-hook-improvements

## Problem Statement

Quill's continuity hook injects a `<quill_continuity>` block (cwd, last_prompt,
task_hints, decision_hints) into new Claude Code / Codex sessions at
SessionStart, sourced from JSONL records the same hook captures on
UserPromptSubmit / Stop / PreCompact. An empirical effectiveness analysis
(2026-07-23) over 1,283 injections across 992 sessions (~43% session coverage,
≥15 repos) found the mechanism genuinely useful — documented wins include
pre-existing-failure recall, prior-session-suspect debugging, and
duplicate-work avoidance — but with four systematic quality problems:

1. **Two disconnected continuity systems.** The SessionStart injection reads
   only the hook's own auto-captured JSONL
   (`~/.config/quill/context/continuity/`). The richer, model-curated MCP path
   (`quill_record_continuity_event` → SQLite `continuity_events` /
   `compaction_snapshots` in `~/.config/quill/context/context.db`) is never
   read by the injection and is effectively dead on the write side too
   (13 rows ever, none since 2026-05-19).
2. **Trivial last_prompt.** The newest prompt-bearing record is echoed
   verbatim (220-char compact) with no triviality filter, so shorthand aliases
   like `ctc` surface as the "last prompt" and carry zero resume signal.
3. **Mixed/stale hints.** Hints are selected by pure recency across the whole
   7-day project window with no thread awareness. Documented failure
   (2026-05-18): last_prompt referenced one work thread while task hints
   described another, forcing the assistant to distrust the block and rebuild
   context manually.
4. **Raw events over snapshots.** Stop/PreCompact snapshots already aggregate
   and dedupe up to 12 same-session records (≤5 prompt_summaries, ≤5
   decisions, ≤5 tasks) but the directive builder treats events and snapshots
   uniformly, wasting the higher-quality aggregate.

The users are the assistants (Claude Code / Codex sessions) consuming the
block and, transitively, the human who avoids re-explaining context. Why now:
the analysis shows the hook is firing at its highest rate since May
(W29 = 160 injections/week) so quality defects now propagate widely.

## Goals

- Every injected `last_prompt` carries real resume signal: trivial prompts
  (known shorthand aliases, very short or low-information prompts) are never
  selected as `last_prompt`.
- The injected block is thread-coherent: task/decision hints preferentially
  come from the same session as the chosen `last_prompt`, so the block never
  mixes two unrelated work threads without saying so.
- Snapshots become the primary injection source; raw events are fallback only
  (e.g. when no snapshot exists in the window for the scoped project).
- One continuity system, not two: the MCP SQLite path is either wired into
  the SessionStart injection or explicitly retired, so there is no
  write-only/dead store.
- No regression of the two fixes already shipped 2026-05-21: per-project
  scoping and the empty-gate (suppress directive when last_prompt, tasks, and
  decisions are all empty).
- Measurable: no emitted directive carries a trivial `last_prompt`, and new
  `capture.guidance` telemetry fields (`source`, `trivialSkipped`,
  `coherent`) show the new selection paths exercising. A coverage drop from
  the stricter filters is accepted (see Clarifications Q5).
- The once-daily JSONL prune is concurrency-safe: no lost appends, no torn
  reads (see Clarifications Q6).

## Non-Goals

- Replacing regex hint extraction (`extractHints`) with semantic/LLM
  classification. Known-noisy, but out of scope for this round; a triviality
  filter and snapshot-first sourcing already reduce the noise floor.
- Changing the injected block's format/shape (`<quill_continuity>` tag,
  field names, caps of 3 hints × 180 chars) — consumers and docs assume it.
- Changing capture cadence (which hook events fire) or retention windows
  (7-day read window, 30-day prune) except where snapshot-first selection
  requires reading them differently.
- Building UI for continuity inspection in the Quill app.
- Improving Codex-side parity beyond keeping the shared script byte-identical
  across both deploy dirs.
- Backfill/migration/drop of existing SQLite continuity rows — the dead
  tables stay in place as inert historical data.
- Cross-provider or cross-repo continuity merge (anticipated day-after ask).
- User-configurable trivial-alias lists or any deployed config surface for
  the triviality rule.
- Editing the user's personal `~/.claude/CLAUDE.md` (references to retired
  tools there are a post-ship follow-up for the user).

## User Stories

### 1. As an assistant starting a session, I want `last_prompt` to be a substantive prompt, so that the resume hint tells me what work was actually in flight.

Acceptance criteria:
- A prompt matching the triviality rule (after trimming: length < 12 chars
  OR a single whitespace-free token — no alias list; see Clarifications Q4)
  is never chosen as `last_prompt`.
- When the newest record is trivial, selection falls back to the newest
  non-trivial prompt (or snapshot prompt_summary) within scope.
- If no non-trivial prompt exists in scope, the directive omits `last_prompt`
  rather than emitting a trivial one; the empty-gate still applies.
- Capture is unchanged: trivial prompts are still recorded to JSONL (they may
  matter for session files); only directive selection filters them.

### 2. As an assistant, I want the hints in one block to describe one work thread, so that I don't have to distrust and rebuild context.

Acceptance criteria:
- Anchor selection is coherence-first per Clarifications Q3: the newest
  in-scope session having BOTH a non-trivial prompt signal and ≥1 hint is
  the anchor; `last_prompt` and hints come from it, snapshots preferred
  over raw events within the session.
- Fallback order when no session qualifies is deterministic: newest session
  with a non-trivial prompt (hints filled from other sessions newest-first),
  then hints-by-recency with `last_prompt` omitted, then the empty-gate.
- Given the reproduced 2026-05-18 shape (newest prompt from thread A, richer
  hints only in thread B), the emitted block anchors on whichever thread
  satisfies the coherence rule — never an arbitrary interleave of A and B.

### 3. As an assistant, I want the directive built from Stop/PreCompact snapshots when available, so that hints reflect deduped whole-session aggregates instead of single-event regex hits.

Acceptance criteria:
- If ≥1 in-scope snapshot exists in the 7-day window, `last_prompt` and hints
  are sourced from snapshots (newest-first), with raw events used only to
  fill empty fields.
- If no in-scope snapshot exists, behavior degrades to the current
  event-based path.
- The 256KB tail-read bound and the 5s hook timeout budget still hold.

### 4. As a Quill maintainer, I want exactly one continuity store, so that there is no dead write path pretending to be durable memory.

Acceptance criteria (resolved: RETIRE — see Clarifications Q1/Q2):
- The three MCP tools (`quill_record_continuity_event`,
  `quill_create_compaction_snapshot`, `quill_get_compaction_snapshot`) are
  removed from `src-tauri/claude-integration/mcp/tools/context.py`.
- Every repo reference is scrubbed: `README.md:67`, the instruction bullet
  at `src-tauri/claude-integration/mcp/server.py:51-52`, and the three
  retired names in `CONTEXT_TOOLS` of both provider copies of
  `context-capture.cjs` (the two scripts stay byte-identical).
- Kept deliberately: `mcp.continuity` taxonomy entry + tests in
  `src-tauri/src/context_category.rs`, the `useContextSavingsStats`
  fallback, and existing SQLite tables/rows (no drop migration).
- `lat.md` sections (features.md Continuity Capture, backend.md schema,
  infrastructure.md deployment) match the shipped behavior and `lat check`
  passes.

## Constraints

- **Single shared script:** the hook lives at
  `src-tauri/claude-integration/scripts/context-capture.cjs` with a
  byte-identical copy at `src-tauri/codex-integration/scripts/` (deployed to
  `~/.config/quill/scripts/`). Both copies must stay identical; provider is
  inferred at runtime. Any change ships to both providers at once.
- **Zero-dependency Node:** the script is plain `.cjs` run by `node` with a
  5-second hook timeout and swallowed errors by design. No npm dependencies
  are available; reading SQLite from it would need a child process
  (`sqlite3` CLI is NOT installed on this machine) or another mechanism —
  a hard constraint on the "wire in" option of Q1.
- **Key logic locations:** `extractHints` (215–235), `buildEvent` (283–301),
  `buildSnapshot` (303–328), `scopedRecentRecords` (380–393),
  `buildDirective` (395–425), empty-gate at 407. MCP path:
  `src-tauri/claude-integration/mcp/tools/context.py:1952` (record event),
  `:2043` / `:2147` (snapshot create/get).
- **Read bounds:** 256KB JSONL tail, 7-day window, ≤3+3 hints at ≤180 chars,
  directive ~1.2KB today — keep payload in the same ballpark.
- **Preserve shipped fixes:** per-project scoping (git-root project key) and
  the all-empty gate must survive refactoring.
- **Docs:** `lat.md/` must be updated with any behavior change and
  `lat check` must pass (features.md 197–203 "Continuity Capture",
  backend.md:217 SQLite tables, infrastructure.md deployment gating).
- **Deployment:** users get the new script only when Quill redeploys managed
  assets; consider whether the deployed-copy update path (first-launch
  auto-deployment / version gating) needs a version bump to roll this out.

## Open Questions

All resolved — see **Clarifications** (Q1→retire, Q2→node:sqlite moot,
Q3→generic heuristic hardcoded, Q4→coherence-first with deterministic
fallbacks, Q5→snapshot-first covers last_prompt too, Q6→hash-gated redeploy
suffices, additive JSONL needs no migration). Original questions retained
below for the record.

1. **Q1 — Unify direction:** wire the SQLite MCP continuity store into the
   injection, or retire the dead MCP write path? Empirics favor retiring
   (13 writes ever, dead since May; passive capture won because it costs the
   model nothing), but retiring deletes the only model-curated,
   >30-day-durable store. Recommendation: retire the write tools, keep
   read-only snapshot retrieval if anything depends on it.
2. **Q2 — If wiring in:** how does a zero-dependency Node hook read SQLite
   within budget? (`sqlite3` CLI absent; bundling a reader is heavy; a
   Quill-side export-to-JSONL bridge is a third option.) Moot if Q1 =
   retire.
3. **Q3 — Triviality rule:** exact alias list + length threshold, or a
   lightweight information heuristic (e.g. must contain a verb-ish token /
   ≥2 words)? Where does the alias list live — hardcoded in the script, or
   deployed config so users can extend it?
4. **Q4 — Coherence fallback:** when the chosen last_prompt's session has no
   hints at all, is filling all 3+3 slots from other sessions acceptable, or
   should the block then prefer the newest session that HAS hints (making
   hints drive selection rather than the prompt)?
5. **Q5 — Snapshot-first scope:** should snapshot-first selection also change
   what `prompt_summaries` feeds `last_prompt` (snapshots keep ≤5), or is
   snapshot-first only for hints?
6. **Q6 — Rollout:** does the managed-asset deployment need an explicit
   version/flag bump for existing installs to pick up the new script, and is
   there any migration concern for existing JSONL records (schema is
   additive today)?

## Clarifications

The human delegated these decisions ("investigate anything you need and pick
the best options", 2026-07-23); answers below were picked after a targeted
code investigation and are binding for planning.

**Q1: Retire, retire-write-only, or wire-in the SQLite MCP continuity path?**
A: **Retire fully** (option A). All three tools go
(`quill_record_continuity_event`, `quill_create_compaction_snapshot`,
`quill_get_compaction_snapshot`). Grep confirms no external caller of the
read side. Wire-in, though mechanically possible via the `node:sqlite`
built-in, requires a schema migration (tables lack a cwd/project column),
prose re-parsing of pre-rendered snapshots, and WAL-lock latency risk — not
worth it for a store with 13 rows.

**Q2: Retirement blast radius?**
A: **Full repo scrub, non-destructive elsewhere.** Scrub exactly:
`README.md:67`, `src-tauri/claude-integration/mcp/server.py:51-52`
(instruction bullet), the three tool definitions in
`src-tauri/claude-integration/mcp/tools/context.py` (1952, 2043, 2147 + their
`source=` emitters), and the three retired names in `CONTEXT_TOOLS` of BOTH
`src-tauri/claude-integration/scripts/context-capture.cjs:25-27` and
`src-tauri/codex-integration/scripts/context-capture.cjs:25-27`
(`context-router.cjs` arrays contain none of them — untouched). Keep: the
`mcp.continuity` taxonomy entry in `src-tauri/src/context_category.rs` and
its tests (verified zero-code-change safe — the Telemetry tile falls back
cleanly and hook `capture.*` events keep it non-zero), and the existing
SQLite tables/rows (harmless historical data; no drop migration).
Follow-up outside this change: the user's personal `~/.claude/CLAUDE.md`
references the retired tools and should be updated by the user post-ship.

**Q3: Selection-algorithm precedence?**
A: **Coherence-first** (option A). Anchor = newest in-scope session that has
BOTH a non-trivial prompt signal and ≥1 hint (from its snapshot if present,
else its events). All directive fields come from that session. Fallbacks in
order: (1) no such session → newest session with a non-trivial prompt,
hints filled from other sessions' records (newest-first) into remaining
slots; (2) no non-trivial prompt anywhere in scope → omit `last_prompt`,
hints by recency; (3) all fields empty → existing empty-gate suppresses the
directive. Within the anchor session, snapshots are preferred over raw
events (Q5 resolved: snapshot-first applies to both `last_prompt` sourcing
via `prompt_summaries` and hints).

**Q4: Triviality rule and alias-list location?**
A: **Generic heuristic only, hardcoded** (option A). A prompt is trivial if,
after trimming, it is under 12 characters OR is a single whitespace-free
token. No alias list at all — personal shorthands like `ctc` are caught by
the heuristic, nothing user-specific ships in the byte-identical script, and
Codex parity is automatic. No deployed config surface this round.

**Q5: Coverage metric under the new filters?**
A: **Accept the coverage drop; make quality measurable** (option A). The
≥43% floor is dropped as an acceptance criterion. Instead the
`capture.guidance` telemetry event gains fields: `source`
(`snapshot`|`event`), `trivialSkipped` (count of trivial prompts passed
over), and `coherent` (whether hints came from the anchor session). Success
= no trivial `last_prompt` in emitted directives + telemetry shows the new
paths exercising.

**Q6: Concurrent-writer prune race?**
A: **Fix in this round** (option A). `pruneJsonlFile` gets temp-write +
`renameSync` (atomic replace, no torn reads) plus an advisory lockfile
(`openSync` with `wx`, pid+timestamp payload, ~30s stale-steal, bounded
retries, proceed-anyway fallback so capture never hard-fails) shared by the
prune and aggregate-file appends. Zero dependencies, well inside the 5s
budget.

**Q7: Test harness?**
A: **Yes** (option A — the gate delegation counts as the explicit request
the repo convention requires). A fixture harness feeds stdin JSON hook
events to the `.cjs` against a temp continuity dir and asserts the emitted
directive. Regression coverage: per-project scoping, empty-gate, triviality
filter, coherence anchor selection (including the reproduced 2026-05-18
mixed-hints shape), snapshot-first sourcing, and prune atomicity.

Constitution: none exists; the human was offered a stop and delegated —
pipeline proceeds without one.

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility,
scope, stakeholders) were run against this draft. Corrected facts
discovered during review, superseding the Constraints section where they
conflict:

- `node:sqlite` IS available as a zero-dependency built-in (deployed Node is
  v25; the API needs ≥22.5). Wire-in is mechanically possible — the "no
  SQLite reader" premise in Constraints/Q2 is wrong on this machine.
- However, the SQLite `continuity_events`/`compaction_snapshots` tables have
  **no cwd/project column** (cannot satisfy per-project scoping without a
  schema migration) and `compaction_snapshots.snapshot` is a pre-rendered
  prose blob, not structured hints — wire-in requires migration + prose
  parsing + WAL-lock latency risk against the 5s budget.
- `session_id` is already present on every JSONL event and snapshot, so
  thread-coherence (Story 2) and snapshot-first (Story 3) need no capture
  changes — they are pure `buildDirective` selection rewrites.
- Grep found **no external caller** of `get_compaction_snapshot` /
  `continuity_events` / `compaction_snapshots` outside their own
  definitions — the read side can be retired too if desired.
- Managed-asset redeployment is hash-gated
  (`src-tauri/src/integrations/manager.rs:460-482`), so any byte change
  auto-triggers redeploy on existing installs; Q6's version-bump concern is
  likely moot, but the Claude/Codex byte-identical invariant must be
  asserted.

### Critical Questions (answer before planning)

1. **Q1 direction: retire, retire-write-only, or wire-in?** Every dimension
   flagged this as the fork that gates Story 4 and sizes the whole feature.
   Wire-in is now known to be *possible* (`node:sqlite`) but is phase-2 in
   disguise: schema migration (no cwd column), prose re-parsing, WAL-lock
   risk, and a Node ≥22.5 floor. Retire is a deletion plus doc scrub; no
   external reader exists. — flagged by: all 6.
2. **If retiring, how far does the blast radius go?** Retirement orphans
   three live guidance surfaces (MCP server instructions
   `src-tauri/claude-integration/mcp/server.py:51`, the `CONTEXT_TOOLS`
   array printed into every directive, the user's global `~/.claude/CLAUDE.md`
   "Working context tools" section), flatlines the `mcp.continuity`
   Now-tab analytics category (`src-tauri/src/context_category.rs:37,59`,
   `src/hooks/useContextSavingsStats.ts`), and leaves 13 dead SQLite rows
   with no stated disposition (drop vs. leave). The spec's acceptance
   criteria name only the `context_tools` line + lat.md — incomplete.
   — flagged by: stakeholders, gaps, scope.
3. **Selection-algorithm precedence: what picks the anchor?** When the
   newest in-scope snapshot's session differs from the session of the
   newest non-trivial prompt, does snapshot-recency or prompt-coherence
   win? And when the anchor session has no hints (Q4): fill remaining
   slots from other sessions, or re-pick the newest session that HAS
   hints? Story 2's "deterministic" claim is undefined until both are
   answered; Q5 (does snapshot-first also source `last_prompt`?) folds
   into the same rule. — flagged by: ambiguity, requirements, feasibility.
4. **Triviality rule: exact definition, and where does the alias list
   live?** The example aliases (`ctc`, `fce`, `nccy`…) are one user's
   personal CLAUDE.md shorthand; hardcoding them ships personal config in
   a byte-identical script deployed to every install and both providers
   (Codex has different conventions). Options: generic rule only (length /
   ≥2-words heuristic), hardcoded minimal list, or deployed per-user
   config (adds surface). — flagged by: feasibility, stakeholders, gaps,
   scope, ambiguity, requirements.
5. **Coverage metric: the goal is in tension with the fix and the yardstick
   moves.** Triviality-filter + snapshot-first remove candidates and route
   more sessions into the empty-gate, so "≥43% coverage" may be
   unmeetable-by-construction; and the metric is measured via the same
   `context_savings_events` telemetry the change perturbs. Define: the
   acceptable coverage delta, a pre-change baseline capture, and whether
   new telemetry fields (source=snapshot|event, trivial-skipped,
   coherence outcome) are in scope. — flagged by: scope, stakeholders,
   requirements, ambiguity.
6. **Concurrent-writer prune race: fix now or defer explicitly?**
   `pruneJsonlFile` rewrites `events.jsonl`/`snapshots.jsonl` with
   `writeFileSync` while parallel sessions `appendFileSync` the same
   files — silent write loss is possible today and the refactor touches
   this code. In scope for this round, or an explicit non-goal?
   — flagged by: gaps.
7. **Testability: what harness makes the acceptance criteria verifiable?**
   The hook runs outside the app; Stories 1–3 demand deterministic
   selection (including the reproduced 2026-05-18 shape) but no fixture
   format or stdin-JSON harness exists, and the two must-not-regress fixes
   (per-project scoping, empty-gate) have no regression tests. Note: repo
   convention is tests only on explicit request — confirm tests are wanted
   here. — flagged by: gaps, requirements.

### Non-Blocking Observations

- Stage as two changes: ship triviality + snapshot-first + retire first
  (single-script selection changes + deletion), land thread-coherence
  second — validates the coverage metric before layering thread-scoping.
- Worktree quirk: `projectKey` treats a worktree's `.git` file as a project
  root, so `.worktrees/*` sessions scope separately from the main checkout.
- Empty/partial snapshot state: define fallback when the newest snapshot has
  empty hints so snapshot-first never emits an emptier block than the event
  path it replaced.
- Add explicit non-goals: no backfill/migration of existing SQLite
  continuity rows; no cross-provider/cross-repo continuity merge (likely
  day-after ask).
- Docs should note `last_prompt` becomes legitimately optional in the block.
- Label selection rules MUST vs SHOULD when answering Q3–Q5 so fallback
  ordering is normative.
- If wire-in is chosen after all: state the Node ≥22.5 floor and a
  degradation path for older runtimes.
