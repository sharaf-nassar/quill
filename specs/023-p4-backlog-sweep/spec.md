# Spec: p4-backlog-sweep

First-pass draft dispositioning three P4 backlog findings from a prior audit:
two edge-case bugs in the live observed-subagent registry (`src-tauri/src/server.rs`)
and one ingestion gap for Codex parentage metadata (`src-tauri/src/transcript_identity.rs`).
Quick-depth sweep — breadth over polish, uncertainty surfaced explicitly.

## Problem Statement

Quill's live analytics show a per-session observed subagent count and a model-group
breakdown ("chips") sourced from lifecycle hook events. Two narrow races make that
display briefly wrong: (1) the count and the model groups can describe *different*
generations of the open-agent set when a Stop+Start pair lands between the two lock
acquisitions in `merge()` and `enrich_model_groups()`; (2) a wall-clock step-back
(NTP correction) between a SubagentStart and its SubagentStop causes the stop to be
silently dropped, leaving the agent counted as open for up to the 15-minute
`OBSERVED_ACTIVE_TTL`. Both violate the "local source-backed truth" principle for
users watching live counts, even though each self-heals eventually.

Separately, Codex sessions have no agent identity at all: `agent_id` is structurally
`None` for the Codex ingestion path, so per-agent analytics (model attribution,
retention rows keyed on `agent_id`) are Claude-only. Codex `session_meta` now
mainstream-ships `thread_source` (user|subagent), `agent_nickname`, `agent_path`,
`multi_agent_version`, and `fork_context` — evidence Quill currently ignores — which
could close that gap for users reviewing per-agent stats across both providers.

## Goals

- Displayed observed subagent count and model groups always describe the same
  open-agent set snapshot (quill-qqt).
- A SubagentStop with a timestamp slightly earlier than its recorded Start still
  transitions the agent out of the open set, or invalidates the root so no wrong
  count is displayed (quill-hzx) — never a silent drop that overcounts.
- Codex subagent rollouts resolve to real agent identity
  (`agent_id == chain_id == own thread id`) ingested from the nested
  `source.subagent.thread_spawn` object, with `agent_nickname` as a separate
  display label, backfilled for existing data via the established
  migration + reingest-flag pattern (quill-kdx — decided, see Clarifications).
- Each of the three P4 beads is refined in place (priority, acceptance criteria)
  or superseded by a child task under a new epic — none left as bare findings.

## Non-Goals

- No redesign of the observed-root registry (epochs, TTL, invalidation model stay).
- No general clock-skew handling beyond the single Start→Stop inversion case; no
  attempt to make hook timestamps monotonic.
- No full Codex multi-agent feature (fork trees, `agent_path` hierarchies —
  `agent_path` is workdir-like and v2-only; cwd is already ingested).
  `fork_context` does not exist in session_meta (the prior audit's 47 hits
  were JSON-schema tool definitions, a false positive). quill-kdx scope is
  spawn-object ingestion → `agent_id`/`parent_chain_id`/`is_sidechain` +
  nickname label; the rest is recorded, not built.
- No frontend changes beyond what already renders `agent_id`-derived data.
- No automated test additions without explicit authorization (per sweep constraint;
  the live-count test suite is authorization-gated per
  `lat.md/live-subagent-count-tests.md`).

## Backlog Inputs

### quill-qqt (bug) — enrich_model_groups size-only guard race

- **Current intent:** `merge()` captures `row.observed_subagent_count = root.count()`
  under one lock acquisition (server.rs:532, count at :553). `enrich_model_groups()`
  reacquires the lock (server.rs:630) and guards only on
  `agents.len() == expected as usize` (server.rs:657 — drifted from the recorded
  :591). A Stop+Start pair between the two acquisitions keeps size constant while
  swapping membership: count describes the old set, model chips the new one.
  Mislabeled chip for one request; self-heals next poll.
- **Missing decisions:** whether to compare captured agent-id membership instead of
  size, or restructure so merge+enrich share one lock acquisition (enrich currently
  releases the lock deliberately so the `resolve` DB callback runs unlocked —
  merging acquisitions changes that).
- **Expected refinement:** refine in place to P3 with the structural-fuse
  design and acceptance criteria (see Clarifications Q1): merge + enrichment
  become one lock acquisition; the size guard and `enrich_model_groups` as a
  separate public method are deleted.

### quill-hzx (bug) — clock step-back leaves observed agent open

- **Current intent:** in `observe_agent` (server.rs:221), an existing agent is only
  updated when `at > current.at || (at == current.at && !open && current.open)`
  (server.rs:254 — drifted from the recorded :249). A SubagentStop with
  `at < current.at` returns `false` at :266, leaving the agent open (overcount)
  until `OBSERVED_ACTIVE_TTL` (15 min, server.rs:50) staleness invalidation or a
  new epoch. Requires wall-clock inversion between the two hook fires. Note also
  the epoch guard at :236 (`at < *epoch`) drops stops from before the root epoch —
  same family, currently intentional.
- **Missing decisions:** tolerance policy — accept any backdated Stop for a known
  agent id (close it regardless of delta), accept only a bounded negative delta,
  or invalidate the root (registry's existing "when unsure, show nothing" idiom).
- **Expected refinement:** refine in place to P3 with the root-invalidation
  guard design and acceptance criteria (see Clarifications Q2); the epoch
  guard at :236 stays unchanged for unknown agents.

### quill-kdx (task) — ingest codex parentage metadata now mainstream

- **Current intent:** `codex_metadata` reads only `parent_thread_id` /
  `forked_from_id` (transcript_identity.rs:260-261, verified at recorded lines) and
  `resolve_codex_native_identity` hardcodes `agent_id: None`
  (transcript_identity.rs:326, verified). Codex `session_meta` now carries
  `parent_thread_id` (4406/5410 local rollouts), `forked_from_id` (2571),
  `thread_source` (user|subagent), `agent_nickname`, `agent_path`,
  `multi_agent_version`, `fork_context` (47). `thread_source=subagent` +
  `agent_nickname` could populate real Codex agent identity. No existing
  `thread_source`/`agent_nickname` handling anywhere in src-tauri.
- **Missing decisions:** evaluation-only vs. full ingestion; what value fills
  `agent_id` (nickname? thread id? nickname is human-chosen, likely non-unique);
  whether the parallel `model_usage.rs` Codex identity path must change in
  lockstep; whether restatement-tolerance in `resolve_codex_native_identity`
  must also validate the new fields for consistency.
- **Expected refinement:** decision made during clarification research (see
  Clarifications Q3/Q4) — no evaluation task needed. Supersede with
  implementation task(s) at P2: the census showed 1,424 subagent rollouts
  currently mis-rooted (parent only in the nested spawn object Quill doesn't
  read), so this is a live correctness defect in chain topology, not just an
  enhancement.

## Target Epic

No existing epic fits: all three sources are unparented P4 beads with no
discovered-from provenance. This run creates a new epic (working title:
"live-count edge cases + codex agent identity") and parents the refined/split
tasks under it.

## User Stories

### Story 1 — consistent live chips

As a Quill user watching live session analytics, I want the subagent count and
the model chips on a session row to describe the same set of running agents, so
that I never see a chip labeled with a model that doesn't match the count shown
beside it.

Acceptance Criteria:
- Count and model groups returned for a row are derived from a single snapshot
  of the root's open-agent set (single lock acquisition, or membership-compared
  snapshot — per resolved Open Question).
- A Stop+Start pair arriving between summary computation and enrichment yields
  either the old consistent pair or the new consistent pair, never a mix.
- Existing behavior preserved: enrichment still skips rows whose set changed
  (or now uses the fresh consistent set), and the `resolve` evidence callback
  still runs without holding the registry lock.

### Story 2 — no phantom open agents after clock steps

As a Quill user watching live session analytics, I want a subagent that has
stopped to leave the count even if my machine's clock stepped backward between
its start and stop hooks, so that the live count doesn't overstate running
agents for up to 15 minutes.

Acceptance Criteria:
- A SubagentStop for a known agent id with timestamp earlier than the recorded
  Start results in the agent no longer counted open (closed or root invalidated
  — per resolved Open Question), not a silent drop.
- Ordinary out-of-order duplicates (stale re-delivered Starts) still cannot
  reopen a closed agent.
- The epoch guard (stops predating the root epoch) behavior is explicitly
  decided, not accidentally changed.

### Story 3 — Codex per-agent stats

As a Quill user reviewing per-agent stats, I want Codex subagent sessions to
carry an agent identity like Claude sessions do, so that per-agent analytics
(model attribution, retention breakdowns) are not Claude-only.

Acceptance Criteria (decided — see Clarifications Q3/Q4):
- Every rollout whose session_meta carries spawn metadata (nested
  `source.subagent.thread_spawn` or top-level `thread_source: "subagent"`)
  resolves with `agent_id == chain_id == source_session_id`,
  `is_sidechain == true`, and `parent_chain_id` from top-level
  `parent_thread_id` / nested `thread_spawn.parent_thread_id` /
  `forked_from_id` in that precedence. This includes the legacy era
  (nickname + object `source`, no top-level fields) — 1,281 rollouts.
- Rollouts with `thread_source: "user"` or no spawn metadata keep
  `agent_id == None`, exactly as today.
- `agent_nickname` is ingested as a separate display label, never as
  identity (255 distinct nicknames across 4,430 subagent rollouts — heavily
  reused).
- After the backfill migration + one full sweep, no Codex sidechain event
  row has NULL `agent_id`, no duplicate event rows exist, and pre-migration
  `retention_daily_aggregates` rows (agent_id='') are preserved — pruned-day
  history is the sole surviving record and per-agent split there is
  forward-only by necessity.

## Constraints

- **Local source-backed truth:** analytics never invent data — when live evidence
  is inconsistent, show nothing (the registry's existing null/invalidate idiom,
  see `lat.md/frontend#…#Observed Subagent Counts`: null and zero reserve no
  element and make no numeric claim). Fixes must prefer null over guesses.
- **Established Rust/Tauri boundaries:** changes stay inside the existing
  server.rs registry and transcript_identity.rs resolution; no new crates or
  cross-layer restructuring.
- **Responsive execution:** `enrich_model_groups` deliberately drops the lock
  before the `resolve` DB callback; a fix must not hold the registry mutex
  across DB work.
- **Typed failure boundaries:** identity conflicts keep returning
  `IdentityError` variants; no stringly-typed error expansion.
- **Zero-warning gates** apply to any code change.
- **lat.md traceability:** affected sections (`live-subagent-count-tests.md`,
  `frontend#Observed Subagent Counts`, `backend#…#Codex Identity Restatement
  And Cycles`) must be updated with any behavior change; test specs there are
  authorization-gated.
- All three P4s carry file:line evidence from a prior audit (two lines drifted;
  corrected above). User chose one quick-depth sweep. No automated test
  additions without explicit authorization.

## Open Questions

1. **quill-qqt fix shape:** membership snapshot (capture open agent-id set in
   `merge`, compare set equality in `enrich_model_groups`) vs. computing model
   groups from the enrich-time snapshot and overwriting both count and groups
   there vs. merging the two acquisitions. Merging acquisitions conflicts with
   the "no lock across `resolve`" constraint; membership comparison requires
   threading the captured set from `merge` to `enrich_model_groups`, which are
   currently separate public calls — where does that snapshot live?
2. **quill-hzx tolerance:** is "any backdated Stop for a known agent id closes
   it" safe, or does it let a stale re-delivered Stop close a legitimately
   restarted agent? The existing tie-break at :254 already prefers Stop at equal
   timestamps; a bounded negative delta (e.g. ≤ a few seconds) needs a chosen
   constant with no principled value. Alternative: `invalidate` the root
   (registry idiom, shows null instead of a wrong number) — is null for the
   whole root acceptable collateral for one skewed stop?
3. **quill-hzx epoch guard:** should a Stop predating the root epoch (:236)
   also invalidate rather than drop? Currently intentional-looking; out of
   recorded scope but same failure family.
4. **quill-kdx scope:** evaluation-only bead vs. straight-to-implementation.
   The evidence counts (4406/5410 parent_thread_id) came from a prior local
   audit — should the decision re-verify against current rollouts first?
5. **quill-kdx agent_id value:** `agent_nickname` is human-readable but likely
   non-unique; the child thread id is unique but is already the `chain_id`.
   Claude's `agent_id` is distinct from its session id (server.rs:488 rejects
   agent_id == session_id) — what Codex value satisfies the same invariants?
6. **quill-kdx blast radius:** `agent_id` flows into transcript_analytics
   identity comparison (:683), model_usage native-source consistency checks
   (:874, :1188), and retention_engine SQLite rows keyed on
   `(provider, source_key, session_id, day, agent_id, file_path)`. The column
   already exists (no schema migration), but newly non-None agent_id for
   already-indexed Codex sessions changes row identity — does that force a
   reindex, and is that churn acceptable?
7. **quill-kdx live-count interplay:** Codex live counts currently get model_id
   from hooks (server.rs:492); would ingested `agent_nickname` also feed the
   observed `agent_type` slot Claude uses, or stay ingestion-only?

## Spec Review

Quick-depth self-review pass covering requirements, gaps, ambiguity,
feasibility, scope, and stakeholders. Cross-dimension hits promoted to
critical.

### Critical Questions (answer before planning)

1. **quill-qqt fix shape** — snapshot the open agent-id set in `merge()` and
   compare membership in `enrich_model_groups()`, recompute both count and
   groups from the enrich-time set, or merge the two lock acquisitions? Two
   engineers would build different things, and the "no registry lock across
   the `resolve` DB callback" constraint (Constitution 3, responsive
   execution) rules out the naive merge. The snapshot needs a home across two
   separate public calls. Flagged by: ambiguity, requirements, feasibility.
2. **quill-hzx policy + scope** — on a backdated SubagentStop: close the agent
   unconditionally, close only within a bounded delta (no principled
   constant), or invalidate the root (the registry's "show nothing over a
   wrong number" idiom, Constitution 1)? And does the same-family epoch guard
   at server.rs:236 join scope or stay explicitly untouched? Also fixes the
   undecided P3-vs-P4 refinement for this bead. Flagged by: ambiguity, scope,
   requirements.
3. **quill-kdx disposition** — evaluation-only bead or straight to
   implementation? If implementing: what value fills `agent_id`?
   `agent_nickname` is likely non-unique, the child thread id is already
   `chain_id`, and the Claude-side invariant rejects `agent_id == session_id`
   (server.rs:488) — no candidate obviously satisfies uniqueness, stability
   across restatements, and that invariant. Evidence counts (4406/5410) are
   from a prior audit and may need re-verification first. Flagged by: scope,
   feasibility.
4. **quill-kdx reindex churn** — newly non-None `agent_id` changes
   retention_engine row identity for already-indexed Codex sessions (column
   exists, no migration, but rows are keyed on it). Is a reindex forced, and
   is that churn acceptable for large local corpora? Discovered-mid-build
   this doubles the effort. Flagged by: feasibility, gaps, stakeholders.

### Non-Blocking Observations

- Live-count interplay (Open Question 7) can be recorded as out of scope for
  this sweep; revisit only if the quill-kdx decision lands on implementation.
- No observability when a backdated Stop is handled — a debug log line at the
  drop/close site would make the next clock-skew report diagnosable.
- Story 2's "stale re-delivered Starts cannot reopen a closed agent" is
  asserted but currently only guaranteed by the same tie-break being modified
  — worth an explicit check during planning.
- Any test additions for the live-count suite remain authorization-gated per
  `lat.md/live-subagent-count-tests.md`; the clarify gate should capture
  whether the user authorizes tests for these fixes.

## Clarifications

The human delegated the critical questions to deep research ("investigate
and research the correct answers; no half measures; best long-term design").
Three parallel research passes answered them with code-verified evidence.

**Q1: quill-qqt fix shape?**
A: **Structural fuse** — `merge()` absorbs enrichment into one lock
acquisition. `merge()` and `enrich_model_groups()` have exactly one
production caller, back-to-back in the same closure (lib.rs:3509-3518); the
two-call split IS the defect. Under a single guard: existing merge logic,
then the model-group snapshot over the final truncated rows; guard drops;
the `resolve` DB callback runs after unlock on the snapshot (the
no-lock-across-DB constraint is preserved — enrichment already applies
evidence to its lock-time snapshot without re-locking, server.rs:679-700).
The `agents.len() == expected` guard (:657) and `enrich_model_groups` as a
separate public method are deleted — mismatch becomes impossible by
construction, compile-enforced. `merge` gains a
`resolve: impl FnOnce(&[ObservedAgentModelKey]) -> Result<HashMap<...>, String>`
parameter and returns `Result`. Rejected: membership snapshot and
enrich-time recompute both keep the two-generation architecture (the
latter also mixes generations across rows and can rewrite an
observed-only row's count against the null/zero "reserve no element"
semantics). Diff ≈ −40/+50 in server.rs, small caller/test churn.

**Q2: quill-hzx backdated-Stop policy?**
A: **Invalidate the root** on the precise contradiction: a Stop for a
known, currently-open agent with `ts` earlier than that agent's recorded
Start. Timestamps are stamped client-side by the hook scripts
(`claude-integration/scripts/observe.cjs:117`,
`codex-integration/scripts/hook-observe.cjs:36,61`), so receive-order
cannot dissolve the case, and transport is fire-and-forget without retry —
a backdated Stop is indistinguishable between clock step-back and a stale
cross-life Stop from a SendMessage-restarted agent id. The registry's
documented, tested answer to ordering ambiguity is null
(lat.md/backend.md:1183; `malformed_ordering_and_root_ties_fail_closed`).
One guard at the top of the `Active` arm (before the epoch check) covers
both drop sites, because a retained open agent always has
`current.at > epoch` (server.rs:167); includes a `log::debug!` line for
diagnosability. Epoch guard at :236 stays unchanged for unknown agents
(legitimately stale pre-epoch leftovers must stay dropped). Recovery is the
existing path: next SessionStart re-establishes coverage. Rejected:
unconditional close (stale cross-life Stop would close a running agent —
confident undercount), bounded delta (no principled constant, both
false-accept and false-miss survive), sequence numbers (cross-component
wire-contract change to solve what invalidation already covers). Priority:
P3.

**Q3: quill-kdx — ingest Codex agent identity, and with what value?**
A: **Yes — ingest from the nested `source.subagent.thread_spawn` object**,
which is the only representation present in both schema eras (legacy
1,281 rollouts: nickname + `agent_role` + object source, no top-level
fields; modern 3,149: duplicated top-level, nested and top-level parent
agree 3,006/3,006 where both present). This is a correctness fix, not just
an enhancement: 1,424 subagent rollouts currently index as parentless
non-sidechain roots. `agent_id` = **the subagent's own thread id**
(`agent_id == chain_id == source_session_id`): Claude's `agent_id` is
semantically an instance id and Quill already enforces
`chain_id == agent_id` for Claude sidechains
(transcript_analytics.rs:1265-1273, server.rs:1385-1386), so this yields
one cross-provider invariant: sidechain ⇒ `agent_id == chain_id`. Census:
thread id unique 3,149/3,149, zero `id == parent_thread_id` collisions
(so the hook-side `agent_id != session_id` invariant at server.rs:484-488
holds). `agent_nickname` becomes a separate display label (255 distinct
across 4,430 — reused constantly; using it as identity would merge
unrelated threads). `agent_path` skipped (workdir-like, v2-only; cwd
already ingested). `fork_context` does not exist in session_meta (prior
audit's 47 = JSON-schema false positive). Restatement tolerance extends
first-child-wins to spawn metadata; conflicting restated spawn identity →
`ConflictingNativeIdentity`. The resolver is a single point feeding all
three consumers (sessions.rs:3774, transcript_analytics.rs:1322,
model_usage.rs:1400); identity-equality checks already include `agent_id`,
so the cache-invalidation migration must ship in the same release. Live
path needs no change. Priority: P2 (mis-rooted data today).

**Q4: reindex policy for existing Codex rows?**
A: **Backfill via the established migration + reingest-flag pattern**
(precedents: migrations 20/21/26/27 — set pending flag, clear
`file_mtimes`, next sweep reprocesses). Forward-only concretely fails:
`session_events`' identity index coalesces NULL agent_id, so re-extracted
files would insert duplicate rows beside old NULL rows (storage.rs:6689);
model_usage compares stored vs freshly-parsed identity including agent_id
→ `ObservationIdentityMismatch` churn as files grow; and the 1,424
mis-rooted rollouts would stay mis-rooted forever. The migration deletes
re-derivable codex-scoped rows (`session_events`, `tool_actions`,
`response_times`, `skill_usages`, model observation sources/rollups,
transcript_analytics sources) but NOT `hook_invocations` (live-only,
non-derivable) and NOT pre-migration `retention_daily_aggregates`
(agent_id='') rows — those are the sole surviving record of already-pruned
detail; per-agent split for pruned days is forward-only by necessity,
which is the maximum recoverable. Cost: one boot-time sweep (~5,435 Codex
files, same profile as prior reingest migrations).

**Test authorization:** the acceptance criteria above include unit /
regression tests in the authorization-gated live-count suite and identity
test tables. Explicit user authorization for these test additions is
carried forward to the analyze gate for confirmation.
