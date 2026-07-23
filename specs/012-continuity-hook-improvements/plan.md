# Plan: continuity-hook-improvements

Traces to `spec.md` Clarifications Q1–Q7 and Spec Review corrected facts. No
constitution.md exists — constitution gate skipped.

## Architecture Approach

Two orthogonal edits, no schema or capture-cadence changes:

1. **Selection rewrite** — replace `buildDirective` (395–425) and its inputs
   (`scopedRecentRecords` 380–393, plus new pure helpers) inside the single
   shared `context-capture.cjs`. `session_id` is already on every JSONL event
   and snapshot (Spec Review), so thread-coherence and snapshot-first are pure
   selection logic over already-captured records — `buildEvent` (283–301),
   `buildSnapshot` (303–328), and the JSONL record shapes are untouched.
2. **Retirement** — delete three MCP tools from `context.py` and scrub every
   directive/instruction/doc reference. Pure deletion + text scrub; the SQLite
   tables and rows stay as inert historical data (no drop migration).

**Alternatives considered and rejected:**

- *Wire the SQLite MCP store into the SessionStart injection (Q1 option
  wire-in).* Rejected. `node:sqlite` IS available (deployed Node v25.8.2 ≥
  22.5, contradicting the original Constraints premise), so it is mechanically
  possible — but `continuity_events`/`compaction_snapshots` have **no
  cwd/project column**, so per-project scoping needs a schema migration;
  `compaction_snapshots.snapshot` is a pre-rendered prose blob, not structured
  hints, so wire-in needs prose re-parsing; and a WAL-lock read against the 5s
  hook budget adds latency risk. Not worth it for a store with 13 rows, dead
  since 2026-05-19, with no external reader (grep-confirmed). → **retire**
  (Q1/Q2).
- *Deployed per-user trivial-alias config.* Rejected per Q4 — adds a config
  surface and would ship one user's personal shorthand in a byte-identical
  script deployed to both providers. A generic hardcoded heuristic catches
  `ctc`-style aliases with zero config and automatic Codex parity.
- *Defer the prune race.* Rejected per Q6 — the refactor already touches this
  code and silent append loss is live today; fix now with zero-dep
  lockfile + atomic rename.
- *Adopt `node:test`.* Rejected for repo consistency: the existing
  `context-router.test.cjs` is a bespoke standalone zero-dep runner
  (`node context-router.test.cjs`, exits 0/1). The new harness mirrors that
  convention rather than introducing a second test idiom (see Testing).

## Affected Components

Files touched:

| File | Change |
|------|--------|
| `src-tauri/claude-integration/scripts/context-capture.cjs` | Rewrite `buildDirective` + add selection helpers (triviality predicate, coherence anchor, snapshot-first sourcing); harden `pruneJsonlFile` (77–94) with lockfile + atomic rename; drop 3 retired names from `CONTEXT_TOOLS` (25–27); add `source`/`trivialSkipped`/`coherent` to the `capture.guidance` telemetry metadata (435–447) |
| `src-tauri/codex-integration/scripts/context-capture.cjs` | Byte-identical mirror of the above (verified identical today via `cmp`) |
| `src-tauri/claude-integration/mcp/tools/context.py` | Delete `quill_record_continuity_event` (1951–2007), `_resolve_source_refs` helper (2010–2039, used only by the snapshot creator), `quill_create_compaction_snapshot` (2042–2143), `quill_get_compaction_snapshot` (2146–2206 = EOF) |
| `src-tauri/claude-integration/mcp/server.py` | Remove instruction bullet at 51–52 (`quill_record_continuity_event / quill_create_compaction_snapshot: preserve …`) |
| `README.md` | Line 67: drop the three retired tool names from the Context MCP tools enumeration |
| `src-tauri/claude-integration/scripts/context-capture.test.cjs` | **New** standalone fixture harness (mirrors `context-router.test.cjs` location/style) |
| `lat.md/features.md` | Continuity Capture (197–203): the 203 narrative describes the retired `buildDirective` semantics + empty-gate history — **REWRITE it in place** to describe the new selection algorithm (triviality filter, coherence anchor, snapshot-first, optional `last_prompt`); do NOT append, or `lat check` passes while the prose still documents retired behavior. Context Savings Telemetry (205–211): new `capture.guidance` fields |
| `lat.md/backend.md` | Working Context Store (213–217): note the two continuity tables are now inert (no MCP writer) |
| `lat.md/infrastructure.md` | Deployed Assets (203–220): note hash-gated redeploy carries the new script; no version bump needed. **Two `### Deployed Assets` headings exist (203 and 226)** — target the 203 instance and keep any lat refs line-scoped or full-path so they disambiguate |

**Deliberately untouched** (Q2 keep-list; verified zero-code-change safe):

- `src-tauri/src/context_category.rs` — `mcp.continuity` taxonomy entry + tests
  stay; hook `capture.*` events keep the category non-zero.
- `src/hooks/useContextSavingsStats.ts` — Telemetry tile falls back cleanly.
- `context-router.cjs` (both copies) — its arrays contain none of the retired
  names.
- SQLite `continuity_events` / `compaction_snapshots` tables — CREATE
  (context.py 427/441), purge DELETE list (1935–1936), and stats counts
  (1890–1901) all stay; tables persist as inert historical data.
- `~/.claude/CLAUDE.md` (user's personal) — out of scope; user updates
  post-ship.

## Data Model

**No schema changes.** JSONL event and snapshot record shapes are unchanged;
`session_id` already present on all records enables coherence/snapshot-first
with no capture edit. Additive only:

- `capture.guidance` telemetry `metadata` gains `source` (`snapshot`|`event`),
  `trivialSkipped` (int count of trivial prompts passed over), `coherent`
  (bool — hints came from the anchor session). Purely additive JSONL/telemetry
  payload; no migration (Q5, Q6).
- New filesystem artifacts in `~/.config/quill/context/continuity/`: an advisory
  lockfile (`openSync` `wx`, pid+timestamp payload) and `.tmp` files for the
  atomic-rename prune. Transient; not part of any read path.

SQLite `continuity_events` / `compaction_snapshots` tables **become inert** —
still created and counted, never written (MCP writers deleted), never read (no
external reader). No backfill, no drop.

## API / Interface Changes

- **Three MCP tools removed:** `quill_record_continuity_event`,
  `quill_create_compaction_snapshot`, `quill_get_compaction_snapshot`.
  Breaking for any external caller — **none found** (grep-confirmed, Spec
  Review): no code outside the tool definitions references the read side.
- **`CONTEXT_TOOLS` directive line shrinks** from 6 to 3 tool names
  (`quill_search_context`, `quill_get_context_source`, `search_history`). The
  `context_tools:` line rendered into every `<quill_continuity>` block gets
  shorter; no format change.
- **`<quill_continuity>` block format unchanged** (tag, field names, 3-hint ×
  180-char caps). Docs must note `last_prompt` is now **legitimately optional**
  — it is omitted (not faked with a trivial prompt) when no non-trivial prompt
  exists in scope; the empty-gate at 407 still suppresses the whole block when
  `last_prompt`, tasks, and decisions are all empty.

**New selection algorithm** (MUST unless noted; Q3/Q4/Q5):

1. **Triviality predicate** (MUST): a prompt is trivial if, after `.trim()`,
   its length < 12 chars OR it is a single whitespace-free token (no alias
   list; Q4). Trivial prompts are never selected as `last_prompt`; they are
   still captured to JSONL (capture unchanged).
2. **Anchor = coherence-first** (MUST): the newest in-scope session having BOTH
   a non-trivial prompt signal AND ≥1 hint. `last_prompt` and hints both come
   from that session. **Snapshot-first within the anchor:** prefer the
   session's snapshot (`prompt_summaries` for `last_prompt`, `decisions`/`tasks`
   for hints) over its raw events; raw events fill only fields the snapshot
   leaves empty (guards against an empty-hint snapshot emitting a thinner block
   than the event path — Spec Review non-blocking observation).
3. **Fallback chain** (MUST, deterministic order):
   a. No coherent session → newest in-scope session with a non-trivial prompt;
      hints filled from other sessions' records newest-first into remaining
      slots (`coherent=false`).
   b. No non-trivial prompt anywhere in scope → omit `last_prompt`; hints by
      recency.
   c. All fields empty → existing empty-gate (407) suppresses the directive.
4. **2026-05-18 shape** (MUST): newest prompt in thread A, richer hints only in
   thread B → the block anchors on whichever thread satisfies the coherence
   rule; never an arbitrary A/B interleave.

Bounds preserved: 256KB tail read, 7-day window, ≤3 tasks + ≤3 decisions at
≤180 chars, ~1.2KB directive, 5s hook budget.

## Testing Strategy

New harness `src-tauri/claude-integration/scripts/context-capture.test.cjs`,
mirroring the existing `context-router.test.cjs` convention: standalone,
zero-dependency, hand-rolled `it()` runner, exits 0 on pass / 1 with
diagnostics. Invoked with `node context-capture.test.cjs` from its directory
(no test runner, no npm). Repo has no package.json for the scripts and no
`node:test` usage — this matches the sole existing precedent. (Note: the task
brief mentioned `node:test` is available zero-dep; it is, but repo convention
is the bespoke runner — chosen for consistency.)

**Pure-helper exports (unit surface):** additively export the pure selection
helpers (triviality predicate, coherence-anchor selection, snapshot-first
sourcing) from `context-capture.cjs` alongside the existing
`module.exports = { handleInput }` (482) — extend to
`module.exports = { handleInput, isTrivialPrompt, selectAnchor, sourceHints }`
(names indicative). This lets the harness unit-assert boundary cases directly
(length 11 vs 12, single-token detection, empty-hint-snapshot degrade-to-events)
instead of only black-box spawn + directive-string parsing. Keep the spawn tests
for the end-to-end paths.

**Harness design:** spawn `node context-capture.cjs` as a child with
`QUILL_*` / `HOME` env pointed at a per-test `mkdtempSync` temp continuity dir,
seed `events.jsonl` / `snapshots.jsonl` / `sessions/` fixtures, feed one
hook-event JSON object on stdin, capture stdout, parse the
`hookSpecificOutput.additionalContext` directive, and assert on its lines. Set
`QUILL_DEBUG=1` and assert stderr is clean so the by-design swallowed-error
path cannot silently mask a regression.

**Regression cases** (Q7):

- **Per-project scoping** — records from another git-root project never leak
  into a new session's directive (must-not-regress shipped fix). Fixture must
  account for the worktree quirk: `projectKey` treats a worktree's `.git` FILE
  as a project root, so `.worktrees/*` sessions scope separately from the main
  checkout — seed both and assert no cross-leak in either direction.
- **Empty-gate** — no `last_prompt`/tasks/decisions ⇒ no directive emitted
  (must-not-regress shipped fix).
- **Triviality filter** — a newest `ctc`-style prompt (< 12 chars / single
  token) is skipped; falls back to the newest non-trivial prompt; if none,
  `last_prompt` omitted but block still emits when hints exist.
- **Coherence anchor** — hints come from the anchor session; assert `coherent`
  telemetry field.
- **Reproduced 2026-05-18 mixed-hints fixture** — prompt thread A + hints
  thread B; assert the emitted block is single-thread, not interleaved.
- **Snapshot-first sourcing** — with an in-scope snapshot present, `source`
  telemetry = `snapshot` and hints come from the snapshot aggregate; degrade to
  events when no snapshot; empty-hint snapshot does not emit a thinner block
  than the event path.
- **Prune atomicity** — drive `pruneJsonlFile` while a concurrent
  `appendFileSync` runs against the same file (simulate via a second write
  between lock acquire and rename); assert no appended record is lost and no
  torn read.
- **Byte-identical copies** — assert `cmp`-equality of the two
  `context-capture.cjs` files (fail loudly on drift). Can live in the harness
  or a one-line check in the sequencing gate.

## Risks

- **Silent-failure design hides regressions.** The hook swallows errors by
  design, so a broken selection path emits nothing rather than failing.
  *Mitigate:* the fixture harness exercises every path; `QUILL_DEBUG=1` stderr
  assertions catch swallowed exceptions.
- **Coverage drop** from stricter triviality + snapshot-first filters. Accepted
  per Q5 (the ≥43% floor is dropped as an acceptance criterion); the new
  `source`/`trivialSkipped`/`coherent` telemetry fields make the new paths
  observable instead.
- **Lockfile staleness.** A crashed writer could leave a stale lock. *Mitigate:*
  ~30s stale-steal on the pid+timestamp payload, bounded retries, and a
  proceed-anyway fallback so capture never hard-fails (Q6).
- **Dual-copy drift** between the Claude and Codex script copies. *Mitigate:*
  byte-identical `cmp` test; sync the Codex copy as an explicit sequenced step
  after all `.cjs` edits land.
- **Managed-asset redeploy timing.** Redeploy is hash-gated
  (`src-tauri/src/integrations/manager.rs` `repair_provider` /
  `deployment_is_current`), so any byte change auto-triggers reinstall on
  existing installs — no version bump needed (Q6 concern moot per Spec Review).
  *Verify:* the deployment stamp changes after the edit.
- **Rollback** = redeploy the previous script version (revert the `.cjs` bytes;
  hash-gate redeploys the prior content). No schema/state to unwind since all
  changes are additive/inert.

## Sequencing

Ordered work items sized ~one bead each; blocking relationships explicit (this
is the bead DAG).

1. **Test harness scaffolding** — create `context-capture.test.cjs` (spawn +
   temp-dir + stdin-JSON + directive-parse skeleton) mirroring
   `context-router.test.cjs`. *Blocks all selection work (2, 3, 4).*
2. **Triviality predicate + telemetry fields + `CONTEXT_TOOLS` trim** — add the
   trim/len<12/single-token predicate, thread `source`/`trivialSkipped`/
   `coherent` into the `capture.guidance` metadata, and drop the 3 retired tool
   names from `CONTEXT_TOOLS` (25–27) so the trim is pinned to a concrete bead
   and cannot fall through a seam. Also export the pure helpers per Testing.
   *After 1.* Independent of 4, 5.
3. **Snapshot-first + coherence-first `buildDirective` rewrite** — replace
   `scopedRecentRecords`/`buildDirective` with the anchor + fallback chain and
   snapshot-first-within-session sourcing. *After 1 and 2.*
4. **Prune lockfile + atomic rename** — harden `pruneJsonlFile` (temp-write +
   `renameSync`, advisory `wx` lockfile shared with aggregate-file appends).
   *After 1; independent of 2, 3.*
5. **MCP retirement + scrub** — delete the three tools + `_resolve_source_refs`
   from `context.py`, scrub `server.py:51-52` and `README.md:67`. *Independent
   of 1–4.* (The `CONTEXT_TOOLS` trim lives in the `.cjs` and is pinned to item
   2 — not this item.) **Verify:** after deleting the three tool functions,
   grep the tree to confirm no `source="mcp.continuity"` / continuity `source=`
   emitter survives outside the deleted ranges.
6. **Sync Codex copy + byte-identical test** — copy the finished
   `claude-integration` `context-capture.cjs` over the `codex-integration`
   copy; wire the `cmp` assertion. **Depends on all `.cjs`-touching items
   {2, 3, 4}** — item 4 (prune hardening) also edits the script, so this cannot
   ride only the 1→2→3→6 chain. **Done-condition (redeploy):** after the script
   bytes change, confirm the managed-asset hash gate triggers — the deployment
   stamp's source-hash input changes and `deployment_is_current` flips false
   then true across a `repair_provider` run.
7. **lat.md updates + `lat check`** — update features.md / backend.md /
   infrastructure.md to match shipped behavior; run `lat check` until clean.
   **REWRITE `features.md:203`** (the Continuity Capture narrative of the retired
   `buildDirective` + empty-gate history) in place — do not append, else
   `lat check` stays green while the prose describes retired behavior. In
   `infrastructure.md`, edit the `### Deployed Assets` instance at 203 (a second
   same-named heading sits at 226); keep any lat refs line-scoped/full-path.
   **Confirm** (do not assume) `lat check` stays green re: `require-code-mention`
   test-spec sections — `context-router.test.cjs` carries no `@lat` comments, so
   the new harness likely needs none; only add spec sections + refs if the
   existing frontmatter actually requires them. *Last (after 1–6).*

Critical path: 1 → 2 → 3 → 6 → 7. Item 4 parallels 2–3 (all after 1) but also
gates 6 (6 depends on {2, 3, 4}). Item 5 is fully parallel (Python/docs only).
Item 7 gates on everything.

## Alignment fixes applied

No must-fix findings — both review passes (A: alignment, B: quality) found the
plan aligned and sound. The following should-fix items were folded in:

- **[A · should-fix]** `lat.md/features.md:203` (the Continuity Capture
  narrative of the retired `buildDirective` + empty-gate history) must be
  REWRITTEN in place, not appended to — otherwise `lat check` stays green while
  docs describe retired behavior. Pinned into Affected Components + item 7.
- **[B · should-fix]** Additively export the pure selection helpers (triviality
  predicate, coherence-anchor selection, snapshot-first sourcing) from
  `context-capture.cjs` alongside `module.exports = { handleInput }` (482) so
  the harness can unit-assert boundary cases (length 11 vs 12, single-token,
  empty-hint-snapshot degrade) directly, not only via spawn + directive parsing.
  Added to Testing Strategy + item 2.
- **[B · should-fix]** Pinned the `CONTEXT_TOOLS` array trim (3 retired names) to
  item 2 explicitly, removing the "rides with item 2 or 3" seam where the step
  could be dropped.
- **[A · should-fix]** Item 6 now explicitly depends on all `.cjs`-touching
  items {2, 3, 4}, not just the 1→2→3→6 chain — item 4 (prune hardening) also
  edits the script. Critical-path note updated.
- **[B · should-fix]** Added a concrete redeploy done-condition to item 6: after
  the script bytes change, confirm the managed-asset hash gate triggers (source-
  hash input changes / `deployment_is_current` flips false→true across repair).
- **[A · should-fix]** Noted `lat.md/infrastructure.md` has two `### Deployed
  Assets` headings (203 and 226) — edits target 203 and lat refs stay line-
  scoped/full-path. Item 7 must CONFIRM (not assume) `lat check` stays green re:
  `require-code-mention` (existing `context-router.test.cjs` carries no `@lat`
  comments, so the new harness likely needs none).
- **[A · should-fix]** Per-project scoping regression fixture must account for
  the worktree quirk: `projectKey` treats a worktree's `.git` FILE as a project
  root, so `.worktrees/*` sessions scope separately from the main checkout.
- **[B · should-fix]** Added a post-deletion grep verification to item 5:
  confirm no `source="mcp.continuity"` / continuity `source=` emitter survives
  outside the deleted ranges in `context.py`.
