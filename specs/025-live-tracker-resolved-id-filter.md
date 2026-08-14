# Spec: live-tracker-resolved-id-filter

## Problem Statement

Claude live tracking stores every `tool_result` ID seen anywhere in a live
session tree in `LiveSession.resolved`. Those IDs remain until the whole
session is evicted, tracking is reset, or its provider is disabled. Only the
open-agent calculation reads the set, but inserting an unrelated result also
marks the fold changed and emits `sessions-live-updated`.

The broad set is deliberate. A spawning result can fold before the agent's
`.meta.json`, and consumed transcript bytes are not read again when metadata
later appears. Results for depth-2 agents also land in the parent agent's
transcript. Remembering every result therefore preserves correct closure
without depending on file-event order.

Cold start and rewrite recovery reread active transcripts and rebuild the same
state. Idle eviction releases a quiet session after 15 minutes, but a session
that stays active can retain unrelated result IDs for its full lifetime. No
reproducible measurement yet shows that this costs enough memory or refresh
work to justify a more complex representation.

## Goals

- Create one P2 read-only measurement-and-decision task by refining
  `quill-tcq7` in place under a new epic; create no production code task now.
- Measure resolved-ID cardinality and a conservative policy accounting proxy
  on authorized local Claude transcripts through a reproducible
  aggregate-only protocol. Allocator truth remains unknown.
- Apply the approved materiality gate: at least 1 MiB of unmatched-ID policy
  proxy in one active session or at least 16 MiB across the live tracker,
  where 1 MiB is exactly 1,048,576 bytes.
- Retire `quill-tcq7` as an evidence-backed non-goal when both materiality
  thresholds miss; the user has explicitly approved that disposition.
- When either threshold passes, record the evidence and require a later
  refinement before any production implementation.
- Preserve result-before-metadata handling, depth-2 closure, cold rebuilds,
  nullable coverage, and the live tracker's single-owner model.
- Preserve the 300 ms Sessions read budget, established sweep bands, and a 10%
  p95 ceiling on any later candidate's added fold cost.
- End materialization with `quill-tcq7` at P2 and Ready P4 at zero.

## Non-Goals

- Rewriting `LiveTracker`, changing its public read surface, or moving live
  state into SQLite.
- Pruning agent records, Codex thread indexes, file offsets, or other tracker
  collections.
- Changing the 15-minute idle cutoff, 120-second recovery sweep, provider
  toggles, restart behavior, or remote-host null semantics.
- Adding UI, IPC, database schema, configuration, background telemetry, or
  off-device reporting.
- Adding production filtering, expiry, or notification-suppression code in the
  measurement-and-decision task.
- Discarding unmatched results immediately or relying on transcript event
  order that current evidence does not guarantee.
- Treating `IDLE_AFTER` as a safe metadata deadline without lifecycle proof.
- Adding automated test code during the measurement-only task. The user's test
  authorization applies if a later refinement changes production behavior.
- Attaching to, resizing, repositioning, or otherwise mutating the user's live
  Quill window during measurement or verification.
- Retaining transcript content, IDs, paths, prompts, or tool payloads in the
  measurement artifact.

## Backlog Inputs

`quill-tcq7` is the source P4. It records the deliberate `ponytail:` shortcut
in `src-tauri/src/live_tracker.rs`: every `tool_result` ID is retained because
a result can arrive before the spawning agent's metadata. Its upgrade condition
is measured memory worth reclaiming, not the existence of the broad set alone.

The source has no structural epic ancestor and no `discovered-from` path to an
epic. This run refines it in place from P4 to one P2 read-only
measurement-and-decision task under the new epic. Topic similarity to the
completed live-session-tracker work is not a parent relationship.

If the baseline misses both approved materiality thresholds, the user has
already approved closing `quill-tcq7` as an evidence-backed non-goal and
closing the epic with zero implementation tasks. If either threshold passes,
the task and epic close as a completed measurement decision without
authorizing production code; implementation requires a later refinement. An
inconclusive run leaves both open with the exact evidence gap.

## Target Epic

Create a new focused epic titled **Bound live-tracker resolved IDs**. Its only
task in this run is `quill-tcq7`, refined in place to P2 for read-only
measurement and the resulting decision. Do not create a conditional or
production implementation task.

When measurement misses both materiality thresholds, close the source and epic
under the approved non-goal disposition with zero implementation tasks. When
measurement passes either threshold, preserve the aggregate evidence and send
production work through a later evidence-backed refinement, then close this
decision epic. An inconclusive run closes neither task nor epic.

Do not attach the epic or source P4 to the completed live-session-tracker epic
without a real Beads hierarchy or provenance edge.

## User Stories

1. **Measure the live cost safely.** As a maintainer, I want real cardinality
   and memory evidence so a correctness-sensitive optimization is not built
   for a hypothetical problem.

   Acceptance criteria:

   - Measurement reads local Claude transcripts without mutation. The artifact
     records only allowlisted aggregate counts, policy-proxy components,
     timings, and non-identifying corpus metadata.
   - The artifact identifies capture time, build profile, hardware, exact
     command or harness, corpus file count and bytes, sessions included,
     exclusion rules, and the policy accounting formula. It stores no
     transcript content, IDs, paths, prompts, or tool payloads.
   - It reports distributions across sessions for all result IDs, known
     spawn-result IDs, and unmatched IDs, including median, p95, and maximum,
     plus aggregate counts, payload bytes, and policy-proxy bytes for all
     three dimensions.
   - It distinguishes cold rebuild cost from appended steady-state folding and
     does not infer live-state cost from unbounded historical audit rows.
   - It may count unmatched-result update decisions separately from visible
     agent changes, but notification suppression enters no production scope
     unless that independent evidence is material.
   - It performs no window attachment, file mutation, or off-device transfer
     and records gaps as unknown under constitution principle 1.

2. **Make one durable decision.** As a maintainer, I want the measurement to
   dispose the source P4 without creating conditional implementation work.

   Acceptance criteria:

   - The baseline is material when
     `(per_session_unmatched_policy_proxy >= 1 MiB OR
     aggregate_unmatched_policy_proxy >= 16 MiB)`.
   - If neither clause passes, close `quill-tcq7` under the user's approved
     evidence-backed non-goal disposition and close the epic with zero
     implementation tasks.
   - If either clause passes, preserve the aggregate evidence, close the task
     and epic as a completed decision, and require a later refinement before
     creating or changing production code.
   - If evidence is incomplete, record the exact gap and leave the task and
     epic open.
   - This run creates one new epic, refines `quill-tcq7` from P4 to P2 as its
     only task, and leaves Ready P4 at zero.

3. **Preserve closure truth.** As a user, I want any later implementation to
   close agents from their actual spawning results regardless of transcript or
   metadata arrival order.

   Acceptance criteria:

   - A result folded before `.meta.json` must still close the agent when
     metadata later identifies its `toolUseId`, without rereading consumed
     bytes.
   - A depth-2 result in the parent agent transcript must close the nested
     agent.
   - No finite pending-ID expiry is authorized until evidence proves a
     lifecycle boundary after which metadata cannot arrive. `IDLE_AFTER` alone
     is not that proof.
   - Known resolved spawn IDs plus unmatched pending results remain a candidate
     design only; this run neither selects nor implements it.
   - Startup and rewrite folds, partial-line handling, workflow journals, Codex
     turn boundaries, resets, and nullable semantics remain unchanged.

4. **Hold measurable budgets.** As a Sessions user, I want baseline and any
   later candidate compared through one reproducible protocol.

   Acceptance criteria:

   - Baseline measurement runs five cold sweeps and at least 20 read samples,
     reporting median, nearest-rank p95, and maximum.
   - Any later candidate captures a fresh fixed byte-boundary corpus and runs
     the current baseline and candidate against it on the same hardware,
     toolchain, and profile. The binaries differ by design, and neither reads
     a mutable corpus during comparison.
   - `get_session_breakdown` read maximum stays at or below 300 ms. Cold-sweep
     p95 stays at or below 80 ms and warm stat-only p95 at or below 21 ms;
     faster results pass. Candidate fold p95 overhead stays at or below 10%.
   - Every baseline-qualified memory dimension falls by at least 50%, aggregate
     savings reach at least 1 MiB, and every performance gate passes.
   - Timing samples and aggregate accounting are recorded, while unmeasured UI
     or allocator cost remains explicit under constitution principle 10.
   - Existing formatting, lint, build, test, and `lat check` gates pass under
     constitution principles 6 and 8.

## Constraints

- Local transcript evidence remains authoritative and gaps stay explicit under
  constitution principle 1.
- The existing Rust/Tauri `LiveTracker` remains the owning layer under
  constitution principle 2; no new dependency or storage layer is justified.
- Folding and sweeps remain bounded background work, and reads perform no
  transcript I/O, under constitution principle 3.
- Expected missing or late evidence must degrade without false positive live
  state under constitution principle 5.
- This run adds no production or test code. If a later refinement changes
  production behavior, the user authorizes the smallest owning-layer tests for
  result-before-metadata, depth-2 closure, the proven pending boundary,
  unrelated results, reset paths, and any notification rule that enters scope
  under constitution principle 7.
- Any behavior or test change updates the matching `lat.md` live-tracker and
  test-spec sections, followed by `lat check`, under principle 8.
- Performance and memory claims follow the approved numeric gate and protocol
  under principle 10.
- Measurement may read local Claude transcripts, but only the probe source and
  allowlisted aggregates may be versioned in this spec. No content, IDs,
  paths, prompts, tool payloads, hostname, exact CPU model, or exact RAM value
  is retained.
- Measurement mutates no transcript or live window and sends nothing
  off-device under principle 11.
- Notification suppression remains out of scope unless its independent
  measurement demonstrates material refresh work.
- Transcript rewrite insert-only semantics remain unchanged unless separately
  approved.
- Materialization creates exactly one epic and one P2 task by refining
  `quill-tcq7` in place. It creates no production implementation task.

## Open Questions

No product or scope questions remain open. Planning must create only the new
epic and one P2 read-only measurement-and-decision task by refining
`quill-tcq7` in place. Any production implementation depends on the approved
numeric evidence gate and a later refinement.

## Spec Review

All seven critical questions were resolved with option A. The alternatives
below remain as the historical review record, not active planning choices.

### Critical Questions (answer before planning)

1. What should this Molecule run materialize?

   - **A (recommended):** Create the new epic, refine `quill-tcq7` in place
     from P4 to one P2 measurement task under it, and defer production code to
     a later evidence-backed refinement.
   - **B:** Refine `quill-tcq7` into one conditional task that measures first
     and implements only when the gate passes.
   - **C:** With explicit human approval, create and close a minimal decision
     epic and retire `quill-tcq7` immediately as a non-goal.

   Option A gives Beads one unconditional deliverable. Option B leaves an
   implementation task whose required output is unknown before measurement.
   In every option the new epic owns the source disposition and Ready P4 must
   end at zero. flagged by: requirements, scope, feasibility, maintainability.

2. What local data may the measurement inspect and record?

   - **A (recommended):** Authorize read-only inspection of local Claude
     transcripts. Version only the probe source and allowlisted aggregates;
     do not retain transcript content, IDs, paths, prompts, tool payloads,
     hostname, exact CPU model, or exact RAM in artifacts.
   - **B:** Permit isolated synthetic fixtures only and accept that production
     cardinality remains unproven.
   - **C:** Authorize neither source, which forces immediate non-goal review.

   The protocol must name capture time, build profile, hardware, corpus file
   count and bytes, sessions included, exclusion rules, and the policy
   accounting formula. It must not mutate files, open the live Quill window,
   or transmit data off-device. flagged by: privacy, security, stakeholders,
   evidence quality.

3. What numeric gate and verification protocol define material impact?

   - **A (recommended):** Implement only if either one active session retains
     at least 1 MiB of unmatched-ID policy proxy or the measured live tracker
     retains at least 16 MiB in aggregate. Require at least 50% reduction in
     every baseline-qualified dimension and at least 1 MiB absolute aggregate
     reduction. Here 1 MiB is 1,048,576 bytes.
   - **B:** Supply different numeric thresholds before planning.
   - **C:** Treat any unmatched ID as sufficient reason to implement.

   A later candidate captures a fresh fixed byte-boundary corpus and runs the
   current baseline and candidate on the same hardware, toolchain, and profile.
   Report five cold sweeps and at least 20 reads. Read maximum must be at most
   300 ms, cold p95 at most 80 ms, warm p95 at most 21 ms, and fold p95 overhead
   at most 10%; faster-than-recorded sweeps pass. The binaries differ by design
   and neither reads a mutable corpus during the pair. flagged by: performance,
   ambiguity, acceptance evidence.

4. What is the source disposition when measured cost misses the gate?

   - **A (recommended):** Explicitly approve `quill-tcq7` as an evidence-backed
     non-goal, close it, and close the new epic with zero implementation tasks.
   - **B:** Refine it to an actionable P3 with a concrete new trigger and
     evidence source.
   - **C:** Leave it at P4 for possible future work.

   Option C is incompatible with this Molecule's required `Ready P4: 0` result.
   Option B is valid only if the trigger is real work, not a placeholder for
   repeating the same measurement. flagged by: backlog completeness, scope,
   delivery.

5. What correctness boundary permits unmatched pending IDs to expire?

   - **A (recommended):** Require evidence for a lifecycle boundary after
     which metadata cannot still arrive. Without that proof, do not apply a
     finite expiry and do not authorize this candidate implementation.
   - **B:** Approve `IDLE_AFTER` as the pending-ID lifetime and accept the
     resulting late-metadata behavior explicitly.
   - **C:** Plan a different representation that identifies spawn IDs before
     their metadata arrives, then retain only results for those IDs.

   Session or agent idleness does not by itself prove `.meta.json` can no
   longer appear. The plan may not assume `IDLE_AFTER` is safe merely because
   it already governs abandonment and eviction. flagged by: correctness,
   source-backed truth, edge cases, feasibility.

6. Is notification suppression part of this feature?

   - **A (recommended):** Keep it out of scope unless independent measurement
     shows material refresh work from unmatched result IDs.
   - **B:** Include it and emit only when the tracker projection visible to
     Sessions changes before versus after one locked fold.

   If option B is selected, planning must define the projected fields compared,
   cold-rebuild emission, batched changes, and metadata-late closure. Storage
   insertion alone is not a visible-state change. flagged by: scope,
   observability, performance, ambiguity.

7. If production behavior changes, are new automated tests authorized?

   - **A (recommended):** Authorize the smallest owning-layer tests for
     result-before-metadata, depth-2 closure, the approved pending boundary,
     unrelated results, reset paths, and any projected-state notification
     rule that enters scope.
   - **B:** Add no test code and restrict this run to measurement or non-goal
     disposition.

   Existing Feature 024 authorization does not automatically cover this new
   behavior. Each key test needs one matching `lat.md` test-spec reference.
   flagged by: constitution principle 7, verification, maintainability.

### Non-Blocking Observations

- Transcript rewrite handling currently folds replacement records into the
  session's insert-only closure evidence. Changing subtraction or provenance
  semantics needs separate approval; this feature should preserve it.
- Workflow journals and Codex turn-boundary state do not use Claude
  `tool_result` IDs and remain unchanged.
- Invalid, empty, or oversized result IDs matter only if the approved redesign
  creates a new trust boundary or memory accounting rule. Do not expand scope
  before then.
- No database migration, schema, IPC contract, frontend, configuration, or UI
  work follows from either measurement or the candidate in-memory change.
- A measurement-only or non-goal result changes no runtime behavior and needs
  no `lat.md` content edit, but `lat check` remains mandatory.

## Clarifications

1. **Q1: What should this Molecule run materialize?**

   **Answer A.** Create **Bound live-tracker resolved IDs**, refine
   `quill-tcq7` in place from P4 to its only child, one P2 read-only
   measurement-and-decision task, and create no production code task. Any
   implementation requires measured evidence and a later refinement.

2. **Q2: What local data may measurement inspect and record?**

   **Answer A.** Measurement may read local Claude transcripts without
   mutation. This spec may version only the probe source and strict-allowlist
   aggregates. It retains no transcript content, IDs, paths, prompts, tool
   payloads, hostname, exact CPU model, or exact RAM. Measurement never
   attaches to the live Quill window or transfers data off-device.

3. **Q3: What numeric gate and protocol define material impact?**

   **Answer A.** Baseline materiality requires per-session unmatched-ID policy
   proxy of at least 1 MiB or aggregate unmatched-ID policy proxy of at least
   16 MiB, where 1 MiB is 1,048,576 bytes. A later candidate must reduce every
   baseline-qualified memory dimension by at least 50% and save at least 1 MiB
   aggregate. It captures a fresh fixed byte-boundary corpus, then runs the
   current baseline and candidate on the same hardware, toolchain, and profile.
   Read maximum stays at or below 300 ms, cold p95 at or below 80 ms, warm p95
   at or below 21 ms, and fold p95 overhead at or below 10%. Faster sweeps pass.

4. **Q4: What happens when measured cost misses the gate?**

   **Answer A.** The user explicitly approves retiring `quill-tcq7` as an
   evidence-backed non-goal and closing the new epic with zero implementation
   tasks. The source does not remain P4.

5. **Q5: What correctness boundary permits pending IDs to expire?**

   **Answer A.** A finite expiry requires evidence for a lifecycle boundary
   after which metadata cannot arrive. Without that proof, no expiry-based
   candidate is authorized. `IDLE_AFTER` is not assumed safe merely because it
   governs abandonment and session eviction.

6. **Q6: Is notification suppression part of this feature?**

   **Answer A.** No, unless an independent measurement demonstrates material
   refresh work from unmatched result IDs. If that later evidence adds the
   behavior to scope, a new refinement must define emission from projected
   Sessions state rather than storage insertion alone.

7. **Q7: Are tests authorized if production behavior later changes?**

   **Answer A.** Yes. The user authorizes the smallest owning-layer tests for
   result-before-metadata, depth-2 closure, the proven pending boundary,
   unrelated results, reset paths, and any notification rule later admitted to
   scope. Each key test receives its required one-to-one `lat.md` test-spec
   link. This measurement-only task adds no test code.

## Architecture Approach

Use one evidence-first task. `quill-tcq7` will measure the current broad
`LiveSession.resolved` set, record a decision, and stop. It will not change
`LiveTracker`, tests, IPC, storage, frontend code, or notification behavior.

The task uses two existing boundaries:

- A temporary Python standard-library probe reads authorized local Claude
  transcripts and reproduces the resolved-ID accounting rules. The full probe
  source, literal command, and aggregate output are copied into this spec, then
  the temporary files are removed. Nothing is added to the repository.
- The existing isolated `live_tracker.rs` fixture path supplies cold-sweep,
  warm-sweep, appended-fold, and Sessions-read timings. Measurement-only
  prints may expose samples from the existing test during the run, but no test
  is added. The file must be restored byte-for-byte before completion.

The local probe fixes one capture timestamp and the production 15-minute idle
cutoff. First it identifies root sessions whose newest eligible activity in
the root or any descendant is inside that cutoff. It then scans each selected
session's full transcript tree through the byte size fixed at capture,
regardless of descendant mtime. This deliberately conservative policy proxy
models a continuously running tracker that kept earlier IDs while the session
remained active; a cold rebuild would usually retain less.

The scan reads only complete newline-terminated records, groups root and
nested `subagents/` transcripts by `claude_root_session_id`, and reads
`tool_result.tool_use_id` plus sibling `agent-*.meta.json` `toolUseId` values.
A shrink, replacement, malformed record, unreadable file, or unresolved
outside-root symlink is an evidence gap. Only a complete upper-proxy miss may
close the work as a non-goal; any gap yields an inconclusive run.
"Upper-proxy" means full observable population plus the deliberately padded
policy formula. It does not claim a mathematical bound on allocator bytes.

All clarification answers remain binding:

| Answer | Plan effect |
| --- | --- |
| 1A | Create one epic and refine `quill-tcq7` in place to its only P2 task. |
| 2A | Read local Claude transcripts only; version only the probe source and strict-allowlist aggregates. |
| 3A | Use the approved memory, reduction, and performance gates verbatim. |
| 4A | A valid upper-proxy miss closes the source and epic as an approved non-goal. |
| 5A | Do not propose expiry without a proven metadata lifecycle boundary. |
| 6A | Keep notification suppression out of scope unless later independent evidence earns a new refinement. |
| 7A | Add no tests now; later production work may add only the authorized owning-layer cases. |

The plan maps the constitution as follows:

| Principle | Application |
| --- | --- |
| 1. Local source-backed truth | Full-tree disk evidence forms the policy proxy; gaps and proxy limits are recorded. |
| 2. Established stack and boundaries | `LiveTracker` remains the sole owner; no new layer is added. |
| 3. Responsive execution | The probe runs outside Quill and never on setup, UI, or read paths. |
| 4. Recoverable mutation | No transcript, database, configuration, or production source is mutated. |
| 5. Typed failure boundaries | Allowlisted count-only failures make the run inconclusive rather than a miss. |
| 6. Zero-warning quality gates | Verified format, lint, build, test, diff, and `lat check` commands must pass. |
| 7. Authorized behavior testing | This task adds no test code; 7A governs any later behavior change. |
| 8. Architecture traceability | No runtime behavior changes, so no `lat.md` edit is needed; `lat check` still runs. |
| 9. Glass Cockpit discipline | No UI or design-system change exists. |
| 10. Measured performance | Fixed samples, percentile rules, budgets, and decision formulas produce the verdict. |
| 11. Explicit external transmission | The probe sends nothing off-device and retains no identifying source data. |
| 12. Gated delivery | Beads owns the task and disposition; commit and sync policy remains separately gated. |

## Affected Components

Only planning, measurement evidence, and Beads metadata change durably.

| Component | Planned treatment |
| --- | --- |
| `specs/025-live-tracker-resolved-id-filter.md` | Retain the exact probe, commands, allowlisted aggregates, timing samples, policy formula, gaps, and verdict. |
| `quill-tcq7` | Refine in place from P4 to P2, parent under the new epic, then close only after a valid decision. |
| New epic `Bound live-tracker resolved IDs` | Own the single task and follow the explicit miss, pass, or inconclusive branch. |
| Authorized local Claude transcript root | Read only through a caller-supplied environment variable; never print or persist its resolved path. |
| `src-tauri/src/live_tracker.rs` | Read as semantic authority. Existing-test timing prints, if needed, are removed and hash-verified. No test is added. |
| `specs/024-live-session-tracker/verification.md` | Reuse its 300 ms read budget and recorded cold/warm timings as comparison evidence; do not edit it. |
| `lat.md/` | No edit because this task changes no functionality, architecture, tests, or behavior. Run `lat check`. |

No Cargo dependency, Python package, binary, script, schema, API, UI file, or
committed benchmark harness is added.

## Data Model

The probe holds sensitive identifiers only in process-local sets and discards
them after aggregation. It emits no individual value.

For each included root session, transient state contains:

| Field | Definition |
| --- | --- |
| `all_results` | Distinct non-empty `tool_result.tool_use_id` values in the active transcript tree. |
| `spawn_ids` | Distinct non-empty `toolUseId` values from readable sibling agent metadata. |
| `known_spawn_results` | `all_results` intersected with `spawn_ids`. |
| `unmatched_results` | `all_results` minus `spawn_ids`. |
| `payload_bytes` | Sum of raw UTF-8 lengths, computed separately for all three result dimensions. |
| `policy_proxy_components` | Rounded payload, reserved slots, control bytes, allowance, and total for each dimension. |
| `gap_counts` | Malformed records, unreadable files or metadata, replacements, and incomplete trailing lines. |

Apply the same policy accounting formula independently to `all_results`,
`known_spawn_results`, and `unmatched_results`. For one dimension, let `n` be
its ID count, `length_i` each UTF-8 byte length, `s = 3 * pointer_bytes`, and
`b` the smallest power of two of at least 8 for which
`n <= floor(7b / 8)`. When `n = 0`, every component and the total are zero.
Otherwise record:

```text
payload_bytes = sum(length_i)
rounded_payload_bytes = sum(round_up(max(1, length_i), 16))
reserved_slot_bytes = b * s
control_bytes = b + 16
allocation_allowance_bytes = 32 * (n + 1)
policy_accounting_proxy_bytes = rounded_payload_bytes
                              + reserved_slot_bytes
                              + control_bytes
                              + allocation_allowance_bytes
```

Rounding and the fixed 32-byte-per-allocation allowance deliberately bias this
policy proxy upward for thresholding. They are not a formal allocator bound.
Rust allocator metadata, fragmentation, implementation-specific hash-table
layout, the inline `HashSet`, and unrelated tracker state remain unknown. The
artifact must use `policy_accounting_proxy_bytes`, never `heap_bytes`, for
this metric.

Aggregate output contains only:

- Capture time, git revision, OS family, architecture, logical CPU count, RAM
  bucket, pointer width, Python/Rust/Cargo versions, and Rust build profile.
  RAM buckets are `<8`, `8-15`, `16-31`, `32-63`, `64-127`, or `>=128` GiB.
  Hostname, exact CPU model, and exact RAM are forbidden.
- Candidate and included file counts and bytes, included session count,
  cutoff, exclusions, and gap counts. Paths and names are omitted.
- Per-session median, nearest-rank p95, and maximum for all results, known spawn
  results, and unmatched results. Each dimension reports count, raw payload,
  every proxy component, and total policy-proxy bytes.
- Tracker-wide component sums for all three dimensions, plus maximum
  per-session and aggregate unmatched policy-proxy bytes used by the gate.
- Cold, warm, appended-fold, and read sample counts with median, nearest-rank
  p95, and maximum timing. No raw record or identifier appears.

Nearest-rank p95 sorts `n` values and selects one-based rank `ceil(0.95n)`.
Median uses the midpoint average for an even sample count. Counts and byte
totals remain integers. One MiB is exactly 1,048,576 bytes.

## API / Interface Changes

There are no production API or interface changes. `LiveTracker` keeps its
single mutex, `HashSet<String>`, fold rules, toggles,
`session_ranking_keys()`, `overlay()`, and `sessions-live-updated` behavior.
SQLite, Tauri IPC, React types, settings, and transcript formats stay fixed.

The temporary probe has one maintainer-only command contract. The task must
replace the timestamp token with one literal UTC value, paste the full Python
source into this spec, and retain the literal command exactly as run:

```bash
python3 -I /tmp/quill-live-resolved-probe.py --self-test
python3 -I /tmp/quill-live-resolved-probe.py \
  --projects-dir "$QUILL_MEASURE_CLAUDE_ROOT" \
  --capture-time "<UTC RFC3339>" \
  --idle-seconds 900
```

`QUILL_MEASURE_CLAUDE_ROOT` is set outside the recorded command so the spec
never stores the resolved local path. Successful measurement output has this
exact top-level key allowlist and no other key:

```text
schema_version, capture, environment, population, dimensions, timings, gaps, decision
```

Errors exit 2 with empty standard output. Standard error is one JSON object
whose only top-level key is `errors`; its values are integer counts keyed by a
fixed error kind. It contains no free text, input value, path, ID, hostname, or
environment dump. The success privacy check rejects any unexpected key before
the output is copied into this spec. Every nested object also follows the fixed
schema declared in the pasted probe; the check rejects unknown keys
recursively.

`--self-test` creates only temporary synthetic trees and exits nonzero on any
mismatch. On a 32-bit/64-bit target it asserts these exact aggregate results:

| Synthetic case | `all_results` count/payload/proxy | `known_spawn_results` count/payload/proxy | `unmatched_results` count/payload/proxy | Other expectation |
| --- | --- | --- | --- | --- |
| Root result `root` | `1 / 4 / 200 or 296` | `0 / 0 / 0` | `1 / 4 / 200 or 296` | Root selected as active. |
| Depth-2 result `depth2` | `1 / 6 / 200 or 296` | `1 / 6 / 200 or 296` | `0 / 0 / 0` | Parent transcript resolves nested metadata. |
| Result `late` before metadata | `1 / 4 / 200 or 296` | `1 / 4 / 200 or 296` | `0 / 0 / 0` | Scan order does not lose late metadata. |
| Complete `root`, malformed line, partial `partial` | `1 / 4 / 200 or 296` | `0 / 0 / 0` | `1 / 4 / 200 or 296` | `malformed_records=1`, `incomplete_trailing_lines=1`. |

The first proxy value is 32-bit and the second is 64-bit. Across the four
successful trees, the self-test expects four included sessions, all-result
totals `count=4`, `payload=18`, `proxy=800 or 1184`; known-spawn totals
`count=2`, `payload=10`, `proxy=400 or 592`; unmatched totals `count=2`,
`payload=8`, `proxy=400 or 592`. It also checks count distributions
`all=1/1/1`, `known=0.5/1/1`, `unmatched=0.5/1/1` and payload distributions
`all=4/6/6`, `known=2/6/6`, `unmatched=2/4/4`, each as median/p95/maximum.
Policy-proxy distributions are `all=200/200/200 or 296/296/296` and
`known=unmatched=100/200/200 or 148/296/296` in the same 32-bit/64-bit order.

The remaining self-test cases expect exit 2, empty standard output, and exactly
`{"errors":{"missing_root":1}}`,
`{"errors":{"relative_root":1}}`, or
`{"errors":{"outside_root_symlink":1}}` on standard error. A passing
self-test prints exactly `{"cases":7,"failed":0}`. The probe opens inputs
read-only, never follows an outside-root symlink, and makes no network call.

The existing production-path timing command is also retained literally with
the measurement evidence:

```bash
cd src-tauri
cargo test live_tracker::tests::the_read_path_costs_a_map_lock_rather_than_a_scan -- --nocapture
```

No temporary Rust test replaces the Python probe, and no test code is added.
If existing-test timing prints are temporarily exposed, record the exact
command and restore the file to its recorded `git hash-object` value. No new
runtime flag, command, IPC payload, configuration key, or public method is
created.

## Testing Strategy

The P2 task performs one reproducible descriptive baseline and decision pass.
It does not claim candidate savings or authorize production code.

1. Fix one UTC capture time. Record toolchain, build profile, hardware, git
   revision, architecture, logical CPU count, RAM bucket, 900-second cutoff,
   corpus file/session counts and bytes, inclusion rules, exclusion rules, and
   every gap count. Omit paths, IDs, hostname, exact CPU model, and exact RAM.
   Select sessions active at capture, then scan every file in each selected
   tree through its fixed capture size regardless of descendant mtime.
2. Paste the complete temporary Python probe and literal commands into this
   spec. Run `--self-test` first and require its exact seven-case result. Run
   the probe against the authorized local Claude root only after that passes.
   Reject output whose top-level keys differ from the strict allowlist.
3. For all results, resolved known-spawn results, and unmatched results, report
   per-session median, nearest-rank p95, and maximum plus tracker-wide count,
   raw payload, every proxy component, and policy-proxy totals. Independently
   recompute component sums before accepting the artifact. Allocator truth
   remains unknown.
4. Reuse the isolated `LiveTracker` fixture, never the user's live Quill
   window. Capture five independent cold sweeps, five warm stat-only sweeps,
   at least 20 Sessions read samples, and at least 20 appended one-record fold
   samples. Report every sample count and median, p95, and maximum. Existing
   test timing prints may be exposed temporarily; do not add a test.
5. Apply performance gates as `read_max_ms <= 300`, `cold_p95_ms <= 80`, and
   `warm_p95_ms <= 21`. These are upper bounds, not bands; faster results pass.
   Preserve baseline appended-fold p95 for the later paired comparison.
6. Apply the baseline materiality predicate exactly:

   ```text
   max_per_session_unmatched_policy_proxy >= 1_048_576 bytes
   OR aggregate_unmatched_policy_proxy >= 16_777_216 bytes
   ```

   Only a complete full-tree upper-proxy miss takes the approved non-goal
   branch. A pass records every qualified dimension and requires a new
   refinement. Any missing, malformed, unstable, unreadable, or escaped input
   makes the run inconclusive and leaves the task and epic open with the exact
   count-only gap.
7. A later candidate captures a fresh corpus with immutable per-file byte
   boundaries. Build current baseline and candidate binaries from their own
   revisions with the same hardware, toolchain, and profile, then alternate
   both over that fixed corpus. Do not compare different mutable captures or
   claim the binaries are identical. For each baseline-qualified dimension
   `d`, and for fold timing, compute:

   ```text
   reduction_d = (baseline_d - candidate_d) / baseline_d
   fold_overhead = (candidate_fold_p95 - baseline_fold_p95)
                   / baseline_fold_p95

   every reduction_d >= 0.50
   AND baseline_aggregate_unmatched_proxy
       - candidate_aggregate_unmatched_proxy >= 1_048_576 bytes
   AND fold_overhead <= 0.10
   AND every performance gate passes
   ```

   If both maximum-per-session and aggregate unmatched dimensions qualify,
   both must fall by at least 50%. A negative fold overhead passes. A zero
   baseline fold p95 makes overhead inconclusive rather than dividing by zero.
8. Before any temporary timing print, run and record
   `git hash-object src-tauri/src/live_tracker.rs`. After measurement, restore
   the file, run the same command, and require the two literal digests to match.
   Delete the Python probe and require
   `test ! -e /tmp/quill-live-resolved-probe.py`. Then run these commands,
   verified against Cargo help, `package.json`, and `.github/workflows/ci.yml`:

   ```bash
   cargo fmt --manifest-path src-tauri/Cargo.toml --check
   cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
   cargo check --manifest-path src-tauri/Cargo.toml --all-targets
   cargo test --manifest-path src-tauri/Cargo.toml live_tracker::tests -- --nocapture
   npm test
   npm run lint
   npm run build
   git diff --check
   lat check
   ```

The task adds no automated test. If later production behavior changes, 7A
authorizes only the smallest owning-layer cases for result-before-metadata,
depth-2 closure, a proven pending boundary, unrelated results, reset paths,
and any separately admitted notification rule. Each key test then receives
one `lat.md` test-spec link.

## Measurement Evidence

The 2026-08-14 baseline is gap-free and misses both materiality thresholds.
It changes no runtime behavior and authorizes no production implementation.

### Fixed capture protocol

Capture time was `2026-08-14T05:00:54Z`; the active cutoff was 900 seconds,
at `2026-08-14T04:45:54Z`. The probe fixed every candidate transcript and
metadata byte size before reading. It selected root sessions from eligible
activity in the root or any descendant, then read every selected tree through
those byte boundaries regardless of descendant mtime.

Inputs were opened read-only. The walker resolved symlinks and rejected any
target outside the authorized root. Device, inode, size, and modification
checks detected replacement, shrink, and in-place instability. Only complete
newline-terminated JSONL records entered the result sets. No network API,
Quill process, browser protocol, or live window was used.

The root came only from a nonprinted `QUILL_MEASURE_CLAUDE_ROOT` value. Neither
the probe nor this artifact stores its resolved value. The build profile was
debug on Linux x86_64 with 64 logical CPUs, 64-bit pointers, and a `64-127`
GiB RAM bucket. Toolchain versions were Python 3.12.3, rustc 1.95.0, and Cargo
1.95.0. Git revision was
`458f218ca8822c826260f8adf44abd915016826f`.

### Literal commands

The environment value was set outside these recorded commands and never
printed. These are the commands run against the one capture:

```bash
python3 -I /tmp/quill-live-resolved-probe.py --self-test
python3 -I /tmp/quill-live-resolved-probe.py \
  --projects-dir "$QUILL_MEASURE_CLAUDE_ROOT" \
  --capture-time "2026-08-14T05:00:54Z" \
  --idle-seconds 900
```

The seven required cases passed exactly:

```json
{"cases":7,"failed":0}
```

Isolated production-path timings used the required existing test command:

```bash
cd src-tauri
cargo test live_tracker::tests::the_read_path_costs_a_map_lock_rather_than_a_scan -- --nocapture
```

### Probe source

This is the exact 648-line temporary probe, SHA-256
`b3a256495fb9c41a6db603ad8cf1a175a941d07cb8178fa1f9b04dac3e471514`.
It uses only the Python standard library.

```python
#!/usr/bin/env python3
import argparse
import datetime as dt
import json
import math
import os
from pathlib import Path
import platform
import statistics
import struct
import subprocess
import sys
import tempfile


TOP_KEYS = {
    "schema_version", "capture", "environment", "population", "dimensions",
    "timings", "gaps", "decision",
}
DIMENSIONS = ("all_results", "known_spawn_results", "unmatched_results")
COMPONENTS = (
    "count", "payload_bytes", "rounded_payload_bytes", "reserved_slot_bytes",
    "control_bytes", "allocation_allowance_bytes",
    "policy_accounting_proxy_bytes",
)
GAP_KEYS = (
    "malformed_records", "unreadable_files", "unstable_files", "replaced_files",
    "incomplete_trailing_lines",
)
ERROR_KEYS = {
    "invalid_arguments", "missing_root", "relative_root", "unreadable_root",
    "outside_root_symlink", "broken_symlink", "invalid_capture_time",
    "invalid_idle_seconds", "privacy_schema_failure",
}


class SafeParser(argparse.ArgumentParser):
    def error(self, _message):
        raise ValueError


def compact(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def error_result(kind, count=1):
    assert kind in ERROR_KEYS
    return 2, "", compact({"errors": {kind: count}})


def parse_utc(value):
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (AttributeError, ValueError):
        return None
    if parsed.tzinfo is None:
        return None
    return parsed.astimezone(dt.timezone.utc)


def utc_text(value):
    return value.astimezone(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def command_version(*command):
    try:
        result = subprocess.run(
            command, check=True, capture_output=True, text=True, timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return "unavailable"
    return result.stdout.strip().splitlines()[0]


def ram_bucket():
    try:
        gib = os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE") / 2**30
    except (OSError, ValueError):
        return "unknown"
    for low, high, label in (
        (0, 8, "<8"), (8, 16, "8-15"), (16, 32, "16-31"),
        (32, 64, "32-63"), (64, 128, "64-127"),
    ):
        if low <= gib < high:
            return label
    return ">=128"


def empty_gaps():
    return {key: 0 for key in GAP_KEYS}


def add_gaps(target, source):
    for key in GAP_KEYS:
        target[key] += source[key]


def within_root(path, root):
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def snapshot_files(root):
    files = []
    gaps = empty_gaps()
    errors = {"outside_root_symlink": 0, "broken_symlink": 0}
    seen_dirs = set()
    seen_files = set()

    def walk(directory):
        try:
            entries = list(os.scandir(directory))
        except OSError:
            gaps["unreadable_files"] += 1
            return
        try:
            directory_stat = os.stat(directory)
        except OSError:
            gaps["unreadable_files"] += 1
            return
        directory_key = (directory_stat.st_dev, directory_stat.st_ino)
        if directory_key in seen_dirs:
            return
        seen_dirs.add(directory_key)
        for entry in entries:
            path = Path(entry.path)
            try:
                if entry.is_symlink():
                    try:
                        resolved = path.resolve(strict=True)
                    except OSError:
                        errors["broken_symlink"] += 1
                        continue
                    if not within_root(resolved, root):
                        errors["outside_root_symlink"] += 1
                        continue
                    path = resolved
                if path.is_dir():
                    walk(path)
                    continue
                name = path.name
                if path.suffix != ".jsonl" and not (
                    name.startswith("agent-") and name.endswith(".meta.json")
                ):
                    continue
                stat = path.stat()
                file_key = (stat.st_dev, stat.st_ino)
                if file_key in seen_files:
                    continue
                seen_files.add(file_key)
                files.append({
                    "path": path,
                    "size": stat.st_size,
                    "dev": stat.st_dev,
                    "ino": stat.st_ino,
                    "mtime_ns": stat.st_mtime_ns,
                })
            except OSError:
                gaps["unreadable_files"] += 1

    walk(root)
    return files, gaps, {key: value for key, value in errors.items() if value}


def session_key(path, root):
    try:
        parts = path.relative_to(root).parts
    except ValueError:
        return None
    if "subagents" in parts:
        index = parts.index("subagents")
        return parts[:index] if index else None
    if path.suffix == ".jsonl":
        return (*parts[:-1], path.stem)
    return None


def read_fixed(item):
    gaps = empty_gaps()
    try:
        descriptor = os.open(item["path"], os.O_RDONLY | getattr(os, "O_CLOEXEC", 0))
    except OSError:
        gaps["unreadable_files"] += 1
        return None, gaps
    try:
        before = os.fstat(descriptor)
        if (before.st_dev, before.st_ino) != (item["dev"], item["ino"]):
            gaps["replaced_files"] += 1
            return None, gaps
        if before.st_size < item["size"]:
            gaps["unstable_files"] += 1
            return None, gaps
        chunks = []
        remaining = item["size"]
        while remaining:
            chunk = os.read(descriptor, min(1 << 20, remaining))
            if not chunk:
                gaps["unstable_files"] += 1
                return None, gaps
            chunks.append(chunk)
            remaining -= len(chunk)
        after = os.fstat(descriptor)
    except OSError:
        gaps["unreadable_files"] += 1
        return None, gaps
    finally:
        os.close(descriptor)
    try:
        current = item["path"].stat()
    except OSError:
        gaps["replaced_files"] += 1
        return None, gaps
    if (current.st_dev, current.st_ino) != (item["dev"], item["ino"]):
        gaps["replaced_files"] += 1
    if after.st_size < item["size"] or current.st_size < item["size"]:
        gaps["unstable_files"] += 1
    if (
        current.st_mtime_ns != item["mtime_ns"]
        and current.st_size == item["size"]
    ):
        gaps["unstable_files"] += 1
    return b"".join(chunks), gaps


def records(raw, gaps):
    if raw is None:
        return []
    complete = len(raw)
    if raw and not raw.endswith(b"\n"):
        gaps["incomplete_trailing_lines"] += 1
        complete = raw.rfind(b"\n") + 1
    parsed = []
    for line in raw[:complete].splitlines():
        if not line:
            continue
        try:
            value = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            gaps["malformed_records"] += 1
            continue
        if not isinstance(value, dict):
            gaps["malformed_records"] += 1
            continue
        parsed.append(value)
    return parsed


def scan_group(items, capture, cutoff):
    all_results = set()
    spawn_ids = set()
    newest_activity = None
    gaps = empty_gaps()
    for item in items:
        raw, file_gaps = read_fixed(item)
        add_gaps(gaps, file_gaps)
        if raw is None:
            continue
        if item["path"].name.endswith(".meta.json"):
            try:
                meta = json.loads(raw)
            except (UnicodeDecodeError, json.JSONDecodeError):
                gaps["malformed_records"] += 1
                continue
            if not isinstance(meta, dict):
                gaps["malformed_records"] += 1
                continue
            tool_use_id = meta.get("toolUseId")
            if isinstance(tool_use_id, str) and tool_use_id:
                spawn_ids.add(tool_use_id)
            continue
        for record in records(raw, gaps):
            if record.get("type") != "attachment":
                timestamp = parse_utc(record.get("timestamp"))
                if timestamp is not None and timestamp <= capture:
                    newest_activity = max(newest_activity, timestamp) if newest_activity else timestamp
            message = record.get("message")
            content = message.get("content") if isinstance(message, dict) else None
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    continue
                tool_use_id = block.get("tool_use_id")
                if isinstance(tool_use_id, str) and tool_use_id:
                    all_results.add(tool_use_id)
    active = newest_activity is not None and newest_activity >= cutoff
    return {
        "active": active,
        "all_results": all_results,
        "known_spawn_results": all_results & spawn_ids,
        "unmatched_results": all_results - spawn_ids,
        "gaps": gaps,
    }


def proxy(values, pointer_bytes):
    lengths = [len(value.encode("utf-8")) for value in values]
    count = len(lengths)
    if not count:
        return {key: 0 for key in COMPONENTS}
    buckets = 8
    while count > 7 * buckets // 8:
        buckets *= 2
    rounded = sum(((max(1, length) + 15) // 16) * 16 for length in lengths)
    components = {
        "count": count,
        "payload_bytes": sum(lengths),
        "rounded_payload_bytes": rounded,
        "reserved_slot_bytes": buckets * 3 * pointer_bytes,
        "control_bytes": buckets + 16,
        "allocation_allowance_bytes": 32 * (count + 1),
    }
    components["policy_accounting_proxy_bytes"] = sum(
        components[key] for key in COMPONENTS[2:-1]
    )
    return components


def normalized_number(value):
    return int(value) if int(value) == value else value


def distribution(values):
    if not values:
        return {"median": 0, "p95": 0, "maximum": 0}
    ordered = sorted(values)
    return {
        "median": normalized_number(statistics.median(ordered)),
        "p95": ordered[math.ceil(0.95 * len(ordered)) - 1],
        "maximum": ordered[-1],
    }


def dimension_summary(session_metrics, name):
    metrics = [session[name] for session in session_metrics]
    return {
        "per_session": {
            key: distribution([metric[key] for metric in metrics])
            for key in COMPONENTS
        },
        "tracker_total": {
            key: sum(metric[key] for metric in metrics) for key in COMPONENTS
        },
    }


def timing_placeholder():
    return {"sample_count": 0, "median_ms": None, "p95_ms": None, "maximum_ms": None}


def validate_success(result, session_metrics):
    if set(result) != TOP_KEYS:
        return False
    if set(result["capture"]) != {"utc", "cutoff_utc", "idle_seconds", "fixed_byte_boundaries"}:
        return False
    if set(result["environment"]) != {
        "git_revision", "os_family", "architecture", "logical_cpu_count", "ram_bucket_gib",
        "pointer_width_bits", "python_version", "rust_version", "cargo_version", "rust_build_profile",
    }:
        return False
    if set(result["population"]) != {
        "candidate_file_count", "candidate_file_bytes", "candidate_session_count",
        "included_file_count", "included_file_bytes", "included_session_count",
        "excluded_inactive_session_count",
    }:
        return False
    if set(result["dimensions"]) != set(DIMENSIONS):
        return False
    for name in DIMENSIONS:
        summary = result["dimensions"][name]
        if set(summary) != {"per_session", "tracker_total"}:
            return False
        if set(summary["per_session"]) != set(COMPONENTS):
            return False
        if set(summary["tracker_total"]) != set(COMPONENTS):
            return False
        if any(set(stat) != {"median", "p95", "maximum"} for stat in summary["per_session"].values()):
            return False
        recomputed = dimension_summary(session_metrics, name)
        if recomputed != summary:
            return False
    if set(result["timings"]) != {"cold_sweep_ms", "warm_sweep_ms", "appended_fold_ms", "sessions_read_ms"}:
        return False
    if any(
        set(timing) != {"sample_count", "median_ms", "p95_ms", "maximum_ms"}
        for timing in result["timings"].values()
    ):
        return False
    if set(result["gaps"]) != set(GAP_KEYS):
        return False
    if set(result["decision"]) != {
        "max_per_session_unmatched_policy_proxy_bytes",
        "aggregate_unmatched_policy_proxy_bytes", "per_session_threshold_bytes",
        "aggregate_threshold_bytes", "memory_gate_passed", "disposition",
    }:
        return False
    allowed_strings = {
        result["capture"]["utc"], result["capture"]["cutoff_utc"],
        result["environment"]["git_revision"], result["environment"]["os_family"],
        result["environment"]["architecture"], result["environment"]["ram_bucket_gib"],
        result["environment"]["python_version"], result["environment"]["rust_version"],
        result["environment"]["cargo_version"], "debug", "miss", "pass", "inconclusive",
    }
    def strings(value):
        if isinstance(value, str):
            return [value]
        if isinstance(value, dict):
            return [item for child in value.values() for item in strings(child)]
        if isinstance(value, list):
            return [item for child in value for item in strings(child)]
        return []
    return all(value in allowed_strings for value in strings(result))


def measure(root, capture, idle_seconds):
    cutoff = capture - dt.timedelta(seconds=idle_seconds)
    files, walk_gaps, errors = snapshot_files(root)
    if errors:
        return None, errors
    groups = {}
    for item in files:
        key = session_key(item["path"], root)
        if key is not None:
            groups.setdefault(key, []).append(item)
    recent_groups = {
        key: items for key, items in groups.items()
        if any(item["mtime_ns"] / 1e9 >= cutoff.timestamp() for item in items if item["path"].suffix == ".jsonl")
    }
    selected = []
    selected_items = []
    gaps = walk_gaps
    for items in recent_groups.values():
        scanned = scan_group(items, capture, cutoff)
        if scanned["active"] or any(scanned["gaps"].values()):
            add_gaps(gaps, scanned["gaps"])
            selected.append(scanned)
            selected_items.extend(items)
    pointer_bytes = struct.calcsize("P")
    session_metrics = [
        {name: proxy(session[name], pointer_bytes) for name in DIMENSIONS}
        for session in selected
    ]
    dimensions = {
        name: dimension_summary(session_metrics, name) for name in DIMENSIONS
    }
    unmatched = [session["unmatched_results"] for session in session_metrics]
    maximum_unmatched = max(
        (metric["policy_accounting_proxy_bytes"] for metric in unmatched), default=0,
    )
    aggregate_unmatched = dimensions["unmatched_results"]["tracker_total"]["policy_accounting_proxy_bytes"]
    gap_total = sum(gaps.values())
    passed = maximum_unmatched >= 1_048_576 or aggregate_unmatched >= 16_777_216
    disposition = "inconclusive" if gap_total else ("pass" if passed else "miss")
    result = {
        "schema_version": 1,
        "capture": {
            "utc": utc_text(capture),
            "cutoff_utc": utc_text(cutoff),
            "idle_seconds": idle_seconds,
            "fixed_byte_boundaries": True,
        },
        "environment": {
            "git_revision": command_version("git", "rev-parse", "HEAD"),
            "os_family": platform.system(),
            "architecture": platform.machine(),
            "logical_cpu_count": os.cpu_count() or 0,
            "ram_bucket_gib": ram_bucket(),
            "pointer_width_bits": pointer_bytes * 8,
            "python_version": platform.python_version(),
            "rust_version": command_version("rustc", "--version"),
            "cargo_version": command_version("cargo", "--version"),
            "rust_build_profile": "debug",
        },
        "population": {
            "candidate_file_count": len(files),
            "candidate_file_bytes": sum(item["size"] for item in files),
            "candidate_session_count": len(recent_groups),
            "included_file_count": len(selected_items),
            "included_file_bytes": sum(item["size"] for item in selected_items),
            "included_session_count": len(selected),
            "excluded_inactive_session_count": len(recent_groups) - len(selected),
        },
        "dimensions": dimensions,
        "timings": {
            "cold_sweep_ms": timing_placeholder(),
            "warm_sweep_ms": timing_placeholder(),
            "appended_fold_ms": timing_placeholder(),
            "sessions_read_ms": timing_placeholder(),
        },
        "gaps": gaps,
        "decision": {
            "max_per_session_unmatched_policy_proxy_bytes": maximum_unmatched,
            "aggregate_unmatched_policy_proxy_bytes": aggregate_unmatched,
            "per_session_threshold_bytes": 1_048_576,
            "aggregate_threshold_bytes": 16_777_216,
            "memory_gate_passed": passed,
            "disposition": disposition,
        },
    }
    if not validate_success(result, session_metrics):
        return None, {"privacy_schema_failure": 1}
    return result, None


def execute(projects_dir, capture_time, idle_seconds):
    if projects_dir is None:
        return error_result("missing_root")
    candidate = Path(projects_dir)
    if not candidate.is_absolute():
        return error_result("relative_root")
    try:
        root = candidate.resolve(strict=True)
    except OSError:
        return error_result("missing_root")
    if not root.is_dir():
        return error_result("unreadable_root")
    capture = parse_utc(capture_time)
    if capture is None:
        return error_result("invalid_capture_time")
    if idle_seconds <= 0:
        return error_result("invalid_idle_seconds")
    result, errors = measure(root, capture, idle_seconds)
    if errors:
        kind, count = sorted(errors.items())[0]
        return error_result(kind, count)
    return 0, compact(result), ""


def result_record(timestamp, tool_use_id):
    return compact({
        "type": "user", "timestamp": timestamp,
        "message": {"content": [{"type": "tool_result", "tool_use_id": tool_use_id}]},
    })


def run_self_test():
    failed = 0
    capture = "2026-08-14T00:00:00Z"
    timestamp = "2026-08-13T23:59:00Z"
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary).resolve()
        project = root / "project"
        project.mkdir()

        (project / "s1.jsonl").write_text(result_record(timestamp, "root") + "\n")

        (project / "s2.jsonl").write_text(compact({"type": "user", "timestamp": timestamp}) + "\n")
        parent = project / "s2" / "subagents"
        child = parent / "agent-parent" / "subagents"
        child.mkdir(parents=True)
        (parent / "agent-parent.jsonl").write_text(result_record(timestamp, "depth2") + "\n")
        (child / "agent-child.jsonl").write_text(compact({"type": "assistant", "timestamp": timestamp}) + "\n")
        (child / "agent-child.meta.json").write_text(compact({"toolUseId": "depth2"}))

        (project / "s3.jsonl").write_text(result_record(timestamp, "late") + "\n")
        late = project / "s3" / "subagents"
        late.mkdir(parents=True)
        (late / "agent-late.jsonl").write_text(compact({"type": "assistant", "timestamp": timestamp}) + "\n")
        (late / "agent-late.meta.json").write_text(compact({"toolUseId": "late"}))

        (project / "s4.jsonl").write_bytes(
            (result_record(timestamp, "root") + "\n{malformed}\n" + result_record(timestamp, "partial")).encode()
        )
        capture_epoch = parse_utc(capture).timestamp()
        for path in root.rglob("*"):
            if path.is_file():
                os.utime(path, (capture_epoch, capture_epoch))

        code, stdout, stderr = execute(str(root), capture, 900)
        if code or stderr:
            failed += 1
        else:
            output = json.loads(stdout)
            pointer = struct.calcsize("P")
            one = 200 if pointer == 4 else 296
            expected = {
                "all_results": (4, 18, 4 * one, (1, 1, 1), (4, 6, 6), (one, one, one)),
                "known_spawn_results": (2, 10, 2 * one, (0.5, 1, 1), (2, 6, 6), (one / 2, one, one)),
                "unmatched_results": (2, 8, 2 * one, (0.5, 1, 1), (2, 4, 4), (one / 2, one, one)),
            }
            for name, values in expected.items():
                dimension = output["dimensions"][name]
                total = dimension["tracker_total"]
                ordered = lambda metric: tuple(
                    dimension["per_session"][metric][key]
                    for key in ("median", "p95", "maximum")
                )
                observed = (
                    total["count"], total["payload_bytes"], total["policy_accounting_proxy_bytes"],
                    ordered("count"), ordered("payload_bytes"),
                    ordered("policy_accounting_proxy_bytes"),
                )
                if observed != values:
                    failed += 1
            if output["population"]["included_session_count"] != 4:
                failed += 1
            if output["gaps"]["malformed_records"] != 1 or output["gaps"]["incomplete_trailing_lines"] != 1:
                failed += 1

        missing = root / "absent"
        cases = [
            (execute(str(missing), capture, 900), (2, "", '{"errors":{"missing_root":1}}')),
            (execute("relative", capture, 900), (2, "", '{"errors":{"relative_root":1}}')),
        ]
        outside = Path(tempfile.mkdtemp())
        try:
            (root / "escape").symlink_to(outside, target_is_directory=True)
            cases.append((
                execute(str(root), capture, 900),
                (2, "", '{"errors":{"outside_root_symlink":1}}'),
            ))
            for observed, expected in cases:
                if observed != expected:
                    failed += 1
        finally:
            os.rmdir(outside)
    print(compact({"cases": 7, "failed": failed}))
    return int(failed != 0)


def main(argv=None):
    parser = SafeParser(add_help=True)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--projects-dir")
    parser.add_argument("--capture-time")
    parser.add_argument("--idle-seconds", type=int, default=900)
    try:
        arguments = parser.parse_args(argv)
    except (ValueError, SystemExit):
        code, stdout, stderr = error_result("invalid_arguments")
    else:
        if arguments.self_test:
            return run_self_test()
        code, stdout, stderr = execute(
            arguments.projects_dir, arguments.capture_time, arguments.idle_seconds,
        )
    if stdout:
        print(stdout)
    if stderr:
        print(stderr, file=sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
```

### Strict-allowlist aggregate output

The output below is the probe's complete standard output. Its only top-level
keys are `schema_version`, `capture`, `environment`, `population`,
`dimensions`, `timings`, `gaps`, and `decision`. The zero timing placeholders
are filled by the separate isolated Rust fixture evidence below.

```json
{"capture":{"cutoff_utc":"2026-08-14T04:45:54Z","fixed_byte_boundaries":true,"idle_seconds":900,"utc":"2026-08-14T05:00:54Z"},"decision":{"aggregate_threshold_bytes":16777216,"aggregate_unmatched_policy_proxy_bytes":59968,"disposition":"miss","max_per_session_unmatched_policy_proxy_bytes":57328,"memory_gate_passed":false,"per_session_threshold_bytes":1048576},"dimensions":{"all_results":{"per_session":{"allocation_allowance_bytes":{"maximum":15904,"median":8416,"p95":15904},"control_bytes":{"maximum":1040,"median":544,"p95":1040},"count":{"maximum":496,"median":262,"p95":496},"payload_bytes":{"maximum":14880,"median":7860,"p95":14880},"policy_accounting_proxy_bytes":{"maximum":57392,"median":30016,"p95":57392},"reserved_slot_bytes":{"maximum":24576,"median":12672,"p95":24576},"rounded_payload_bytes":{"maximum":15872,"median":8384,"p95":15872}},"tracker_total":{"allocation_allowance_bytes":16832,"control_bytes":1088,"count":524,"payload_bytes":15720,"policy_accounting_proxy_bytes":60032,"reserved_slot_bytes":25344,"rounded_payload_bytes":16768}},"known_spawn_results":{"per_session":{"allocation_allowance_bytes":{"maximum":64,"median":32,"p95":64},"control_bytes":{"maximum":24,"median":12,"p95":24},"count":{"maximum":1,"median":0.5,"p95":1},"payload_bytes":{"maximum":30,"median":15,"p95":30},"policy_accounting_proxy_bytes":{"maximum":312,"median":156,"p95":312},"reserved_slot_bytes":{"maximum":192,"median":96,"p95":192},"rounded_payload_bytes":{"maximum":32,"median":16,"p95":32}},"tracker_total":{"allocation_allowance_bytes":64,"control_bytes":24,"count":1,"payload_bytes":30,"policy_accounting_proxy_bytes":312,"reserved_slot_bytes":192,"rounded_payload_bytes":32}},"unmatched_results":{"per_session":{"allocation_allowance_bytes":{"maximum":15872,"median":8400,"p95":15872},"control_bytes":{"maximum":1040,"median":544,"p95":1040},"count":{"maximum":495,"median":261.5,"p95":495},"payload_bytes":{"maximum":14850,"median":7845,"p95":14850},"policy_accounting_proxy_bytes":{"maximum":57328,"median":29984,"p95":57328},"reserved_slot_bytes":{"maximum":24576,"median":12672,"p95":24576},"rounded_payload_bytes":{"maximum":15840,"median":8368,"p95":15840}},"tracker_total":{"allocation_allowance_bytes":16800,"control_bytes":1088,"count":523,"payload_bytes":15690,"policy_accounting_proxy_bytes":59968,"reserved_slot_bytes":25344,"rounded_payload_bytes":16736}}},"environment":{"architecture":"x86_64","cargo_version":"cargo 1.95.0 (f2d3ce0bd 2026-03-21)","git_revision":"458f218ca8822c826260f8adf44abd915016826f","logical_cpu_count":64,"os_family":"Linux","pointer_width_bits":64,"python_version":"3.12.3","ram_bucket_gib":"64-127","rust_build_profile":"debug","rust_version":"rustc 1.95.0 (59807616e 2026-04-14)"},"gaps":{"incomplete_trailing_lines":0,"malformed_records":0,"replaced_files":0,"unreadable_files":0,"unstable_files":0},"population":{"candidate_file_bytes":2356232909,"candidate_file_count":6228,"candidate_session_count":4,"excluded_inactive_session_count":2,"included_file_bytes":5940088,"included_file_count":4,"included_session_count":2},"schema_version":1,"timings":{"appended_fold_ms":{"maximum_ms":null,"median_ms":null,"p95_ms":null,"sample_count":0},"cold_sweep_ms":{"maximum_ms":null,"median_ms":null,"p95_ms":null,"sample_count":0},"sessions_read_ms":{"maximum_ms":null,"median_ms":null,"p95_ms":null,"sample_count":0},"warm_sweep_ms":{"maximum_ms":null,"median_ms":null,"p95_ms":null,"sample_count":0}}}
```

The probe's strict recursive key check ran before output. Its independent
component recomputation matched every emitted distribution and tracker total.
Manual arithmetic also matched the three tracker-wide proxies:

```text
all:       16,768 + 25,344 + 1,088 + 16,832 = 60,032 bytes
known:         32 +    192 +    24 +     64 =    312 bytes
unmatched: 16,736 + 25,344 + 1,088 + 16,800 = 59,968 bytes
```

The privacy audit found no unexpected key or string value and no content, ID,
path, prompt, payload, per-file row, hostname, exact CPU model, exact RAM, raw
environment, or transcript hash. Every gap count was zero: malformed records,
unreadable files, unstable files, replacements, and incomplete trailing lines.

### Isolated timing samples

The existing fixture created 400 synthetic folded sessions. Five fresh
trackers supplied independent cold and warm sweeps. The last tracker supplied
20 appended one-record folds and 20 Sessions read samples. Values are
milliseconds, as printed by the temporary timing statements before their
removal.

```text
cold = [51.707733, 44.047132, 40.488298, 40.678463, 42.848277]
warm = [18.909773, 18.265895, 17.807614, 17.426772, 17.782615]
appended = [0.111444, 0.035658, 0.033554, 0.034697, 0.033455,
            0.032723, 0.032262, 0.035117, 0.032513, 0.032994,
            0.032673, 0.032622, 0.032362, 0.032553, 0.036891,
            0.032693, 0.032132, 0.032743, 0.031952, 0.031972]
read = [11.141484, 11.206450, 11.021124, 11.072582, 11.050690,
        11.067763, 11.002427, 11.011966, 11.037364, 11.110035,
        10.994823, 11.093372, 11.336119, 11.413548, 12.364256,
        12.060322, 13.131942, 12.442587, 12.212735, 13.255340]
```

Nearest-rank p95 uses rank `ceil(0.95n)` and median uses the midpoint average
for even counts.

| Dimension | Samples | Median ms | p95 ms | Maximum ms | Gate | Result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Cold sweep | 5 | 42.848277 | 51.707733 | 51.707733 | p95 <= 80 | pass |
| Warm stat-only sweep | 5 | 17.807614 | 18.909773 | 18.909773 | p95 <= 21 | pass |
| Appended one-record fold | 20 | 0.032708 | 0.036891 | 0.111444 | baseline only | recorded |
| Sessions read | 20 | 11.1257595 | 13.131942 | 13.255340 | max <= 300 | pass |

No timing is a UI or allocator measurement. The appended p95 is the baseline
for a later paired candidate; this task has no candidate overhead claim.

### Restoration, policy, and decision

`git hash-object src-tauri/src/live_tracker.rs` returned
`e8b16395205c4eec9765f692ff16ca1b6d0131ee` before temporary timing prints and
the same digest after their removal. No source or test diff remains.

For each nonempty dimension with count `n`, pointer size `p`, payload lengths
`length_i`, and the smallest power-of-two bucket count `b >= 8` satisfying
`n <= floor(7b / 8)`, the policy was applied exactly:

```text
rounded_payload_bytes = sum(round_up(max(1, length_i), 16))
reserved_slot_bytes = b * (3 * p)
control_bytes = b + 16
allocation_allowance_bytes = 32 * (n + 1)
policy_accounting_proxy_bytes = rounded_payload_bytes
                              + reserved_slot_bytes
                              + control_bytes
                              + allocation_allowance_bytes
```

All components are zero when `n = 0`. This deliberately padded policy proxy
is neither allocator truth nor a heap or RSS bound. Allocator metadata,
fragmentation, concrete Rust `HashSet` layout, its inline value, and unrelated
tracker state remain unmeasured.

The complete, gap-free baseline has a maximum per-session unmatched policy
proxy of 57,328 bytes and an aggregate unmatched policy proxy of 59,968 bytes.
Both miss their exact 1,048,576-byte and 16,777,216-byte thresholds. Cold,
warm, and read gates pass. The measurement disposition is therefore **miss**:
retire `quill-tcq7` as the approved evidence-backed non-goal, create no
production task, and require fresh refinement before any later implementation.

## Risks

- **Disk reconstruction differs from a live heap.** Full-tree scanning is a
  conservative continuously-running-tracker policy proxy, not the running
  Quill allocator. Record this limit and do not claim an allocator bound or
  RSS measurement.
- **Concurrent appends can blur the capture.** Read only the initial byte
  extent, ignore incomplete trailing lines, and report shrink or replacement
  as a gap. Never convert a gap to zero.
- **Python can drift from Rust semantics.** Keep the probe small, mirror
  `tool_result_ids`, `read_agent_meta`, `claude_root_session_id`, and
  `IDLE_AFTER` directly, and require the seven exact `--self-test` cases.
- **Policy accounting differs from allocator truth.** Preserve every proxy
  component and its exclusions. If a future decision needs allocator truth,
  require a separate refinement instead of relabeling this estimate.
- **Private data can leak through evidence.** Review keys and values before
  pasting output. Reject IDs, paths, transcript text, prompts, payloads,
  transcript hashes, environment dumps, or per-file rows.
- **Performance results can be mislabeled.** Fixture timings measure tracker
  code; the Python scan measures corpus cardinality. Keep them separate and
  do not relabel backend timing as UI or allocator cost.
- **A tempting filter can break closure.** No result filtering, pending expiry,
  or notification suppression enters this task. `IDLE_AFTER` is not metadata
  lifecycle proof.
- **A failed measurement can strand the disposition.** Fail closed, retain the
  P2 task and epic, record the count-only gap, and rerun. Close either only
  after the evidence record supports a pass or miss branch.

## Sequencing

The dependency graph contains one implement-ready measurement task and no
task-to-task dependency:

```text
Bound live-tracker resolved IDs
└── Measure live-tracker resolved-ID cost and decide disposition
```

`Measure live-tracker resolved-ID cost and decide disposition` is
`quill-tcq7`, refined in place to P2. It is ready immediately after the epic
exists. Its execution order is fixed: freeze protocol, run aggregate probe,
run isolated timings, audit privacy and gaps, apply the gate, record the
verdict, run quality gates, then update Beads disposition.

No production task follows automatically. Outcomes are exhaustive:

| Measurement outcome | Beads disposition | Next work |
| --- | --- | --- |
| Complete upper-proxy miss | Close `quill-tcq7` as the approved evidence-backed non-goal; close the epic. | None. |
| Complete upper-proxy pass | Close `quill-tcq7` and the epic as a completed measurement decision. | A new human-reviewed refinement is required before production work. |
| Inconclusive | Leave `quill-tcq7` and the epic open; record the exact count-only evidence gap. | Repair or rerun the same P2 task. |

## Backlog Refinement

Create `Bound live-tracker resolved IDs` as a new focused epic. Reparent
`quill-tcq7` under it, change its priority from P4 to P2, and replace its
shortcut wording with the measurement-and-decision scope in this plan. Do not
create a replacement issue, conditional implementation issue, test issue, or
notification issue.

The refined task acceptance criteria are:

- Read authorized local Claude transcripts without mutation and retain only
  the exact reproducible probe, strict-allowlist aggregates, non-identifying
  protocol metadata, gaps, timing samples, policy formula, and decision in
  this spec.
- Demonstrate five cold sweeps, five warm sweeps, at least 20 read samples,
  and at least 20 appended-fold samples without touching the live Quill
  window.
- Select active root sessions, scan their full fixed-byte transcript trees,
  and apply the approved policy-proxy predicate with no evidence gap.
- On a miss, close `quill-tcq7` as an evidence-backed non-goal and close the
  epic with zero implementation tasks.
- On a pass, close the measurement task after recording evidence, keep the
  epic's decision record, close the epic as a completed decision, and require
  a new refinement before production work.
- On an inconclusive run, leave the task and epic open with the exact
  count-only gap; do not classify it as a miss.
- Require the seven-case `--self-test`, matching before/after
  `git hash-object` values for `live_tracker.rs`, an absent temporary probe,
  no source/test diff, no sensitive evidence, and no open or ready P4 in this
  feature closure.
- Finish with relevant existing tests, formatting checks, `git diff --check`,
  and `lat check` passing.

Immediately before materialization and completion, recompute the epic's
hierarchy-plus-`discovered-from` closure and include direct source
`quill-tcq7`. Required final report: epic ID, one task, ready P0-P3 count,
blocked count, source disposition, and `Ready P4: 0`.

## Alignment fixes applied

The two plan reviews are resolved in this revision. No open alignment item
changes the one-task, measurement-only scope.

| Review item | Resolution |
| --- | --- |
| Population | Select active root sessions, then scan each full fixed-byte tree regardless of descendant mtime; only a gap-free upper-proxy miss may close. |
| Memory naming | Replace heap claims with a conservative policy accounting proxy, zero every component for zero IDs, and state allocator truth is unknown. |
| Comparison corpus | Make the current baseline descriptive; later work captures a fresh fixed corpus and pairs separate current/candidate binaries on matching hardware, toolchain, and profile. |
| Numeric gates | Define 1 MiB as 1,048,576 bytes, use upper bounds for read/cold/warm, define exact fold overhead, and require 50% reduction in every qualified dimension plus 1 MiB aggregate savings. |
| Probe validation | Add seven exact `--self-test` cases, a success-key allowlist, and count-only failure objects. |
| Privacy | Limit versioned evidence to allowlisted aggregates; omit hostname, exact CPU model, exact RAM, paths, IDs, and content. |
| Restoration and gates | Compare `git hash-object`, require the temporary path absent, and list only commands verified from Cargo help, package scripts, and CI. |
| Outcomes | Make miss, pass, and inconclusive branches explicit for both task and epic. |
| Dimension evidence | Require count, raw payload, proxy components, and proxy totals for all, known-spawn, and unmatched results. |

Should-fixes are also clear: the probe remains Python standard library; no
temporary Rust probe or new test is introduced; the task title has no numbered
prefix; the graph has one ready P2 task and no task dependency; faster timing
results pass; no committed helper, dependency, or production change is planned.
