# Spec: retention-second-corpus

## Problem Statement

Quill's retention defaults and operational budgets are grounded in one real
developer database and one synthetic fixture. The evidence is reproducible,
but it does not show whether the conclusions hold for a materially different
real workload.

Feature 014 measured one database on 2026-07-24. Its initial inventory was
7,544,053,760 bytes with 396,241 `tool_actions` rows and 1,598,902
`session_events` rows. That corpus covered about 3.5 months, contained no live
rows (`source_key IS NULL`), and could not represent a longer-lived,
Codex-heavy, remote-ingest, or differently indexed installation.

Feature 014 therefore made its correctness gates independent of that machine:
the frozen synthetic fixture supplies exact row-count assertions. Performance
constants still came from a single machine and synthetic shape:

| Published measurement | Feature 014 value |
| --- | ---: |
| Chunk size | 25,000 rows |
| Per-chunk wall target | 1,603 ms |
| WAL bytes per deleted row | 788.7 B |
| TEMP bytes per doomed row | 11.05 B |
| Counting-phase budget | 2,616 ms |
| Total delete-run wall budget | 40,598 ms |
| Measured doomed rows | 701,400 |
| Measured delete-run wall time | 13,533 ms |

The current product has moved beyond Feature 014's original two-table shape.
Retention now preserves daily aggregates, can archive rows before deletion,
and includes `model_usage_observations`. A second-corpus study must distinguish
the original comparable measurements from these current-schema additions. It
must not validate a stale design while claiming to validate current behavior.

No independent corpus is currently available or approved. Feature 017 is
therefore a tooling-and-planning feature, not a completed validation study. It
delivers an internal repo-local profiler/replay utility, a reproducible
protocol, a private-manifest schema, a scrubbed report template, and
dependency-wired follow-up work.

Real-corpus profiling and replay remain blocked until a human explicitly
approves a qualifying local corpus and its allowed uses. This feature records
no second-corpus measurements and cannot confirm or revise a budget,
recommendation, production constant, or retention preset.

## Clarifications

These human-approved answers are authoritative for planning and
materialization. The later Spec Review remains historical review evidence and
does not override them.

### Q1: Corpus availability

**Answer: B — no independent corpus exists.**

Feature 017 changes to tooling and planning only. Real-corpus profiling and
replay are blocked follow-up work requiring explicit later approval of the
corpus, custodian, authorized uses, retention period, and cleanup.

### Q2: Feature boundary

**Answer: A — internal utility, protocol, evidence template, and follow-up
beads only.**

The MVP has no production constants or preset changes, supported CLI,
Tauri/IPC/UI surface, transfer workflow, automatic policy changes, or product
behavior changes.

### Q3: Snapshot and schema protocol

**Answer: A — stop Quill, inventory as-is, back up pages, and migrate scratch
only.**

When a corpus is later approved, Quill must stop before inventory. The protocol
uses SQLite's page-preserving backup API, not `VACUUM INTO`, for
shape-sensitive replay. Canonical-path, filesystem-identity, sidecar, archive,
and output collision checks must prove source and outputs are distinct.

### Q4: Replay matrix

**Answer: A — archive-off and archive-on from fresh backups.**

Each mode uses three controlled warm runs and one best-effort cold run. The
future runner must preflight disk, support cancellation, create private
artifacts, and clean scratch data by default.

### Q5: Evidence classification

**Answer: A — report median and maximum under an objective rule.**

Future evidence may **confirm** a ceiling only when all comparable controlled
runs remain within it. It may recommend **revise** when at least two of three
controlled runs exceed it by more than 10%. All other outcomes are
**insufficient evidence**. The 90-day preset remains **insufficient evidence**
until a separate product tradeoff threshold exists. This tooling-only feature
classifies nothing without a corpus.

### Q6: Privacy contract

**Answer: A — exact private manifest, minimized committed report.**

The local run manifest may retain exact measurements. A committed artifact may
contain exact whole-table/object totals and timings, but category/month cells
below 10 are suppressed. Paths, identifiers, payloads, and raw errors are
removed, and publication requires privacy review.

### Q7: Backlog disposition

**Answer: A — replace both P4 sources with dependency-wired P2/P3 work.**

Materialization creates a P2 profiler/protocol task, a P2 future corpus replay
task blocked on approved-corpus evidence, a P2 analysis/recommendation task
depending on replay, and a P3 `dbstat` measurement/decision task depending on
the profiler and replay. `quill-buu` and `quill-xsd` are superseded only after
that replacement coverage exists. Product UI requires a later, separately
approved task and is created only if the measurement gate passes.

## Goals

This feature prepares a safe and reproducible future study without claiming
that the unavailable second corpus was measured.

- Deliver an internal repo-local profiler/replay utility with no default
  database path, filesystem discovery, or supported product interface.
- Define the stop-Quill, read-only inventory, page-preserving backup,
  scratch-only migration, replay, cleanup, and cancellation protocol.
- Define stable private-manifest and scrubbed-report schemas for comparable
  Feature 014 fields and separately labeled current-schema fields.
- Encode the future archive-off/archive-on replay matrix and objective budget
  classification rules without executing them against an unapproved corpus.
- Keep exact private evidence local and make any future committed report
  aggregate, minimized, suppression-aware, and privacy-reviewed.
- Replace both source P4s with bounded, prioritized, dependency-wired work,
  leaving corpus-dependent work visibly blocked.
- Preserve the `dbstat` uncertainty as a future measurement and decision task,
  without pre-authorizing a product surface.

## Non-Goals

This phase creates study tooling and plans. It does not produce study results.

- No claim that a second corpus was found, approved, profiled, replayed, or
  validated.
- No second-corpus measurements, evidence classifications, budget
  confirmations, budget revisions, or retention-preset recommendations.
- No automatic discovery, upload, copy, or external transmission of a user's
  `usage.db`.
- No acquisition or transfer workflow. A later human must approve an
  already-local path and state its custodian and permitted uses.
- No source mutation. Future destructive replay, archive generation, schema
  migration, and compaction run only on verified scratch copies.
- No raw or row-level evidence in Git, Beads, CI output, issue comments, or
  committed reports. This includes `source_key`, session ids, agent ids,
  project paths, hostnames, prompts, tool payloads, model outputs, and archive
  contents.
- No change to frozen-fixture correctness acceptance. A real corpus remains
  observational and cannot replace deterministic exact-count coverage.
- No claim that a single slower run proves the implementation is defective.
  Corpus shape, storage hardware, cache state, SQLite version, and concurrent
  load must be separated before changing a budget.
- No automatic change to retention policy, archive preference, or persisted
  user settings.
- No production constant, default, recommended-preset, supported CLI,
  Tauri/IPC, UI, or automatic policy change.
- No redesign of retention aggregation, model-observation pruning,
  pre-prune archival, compaction, or source ownership.
- No product-facing per-table footprint UI in this feature. `dbstat` is used
  only in later approved measurement work; a product surface requires a
  separate task after its measurement gate passes.
- No new automated test code without explicit user authorization. Existing
  quality gates remain required for any implementation changes.

## Backlog Inputs

This spec refines both remaining P4 retention issues in the existing
hierarchy-plus-provenance closure.

### Selected source: quill-buu

`quill-buu`, **Validate retention sizing against a second corpus**, is the
primary source. Its desired validation cannot run because no independent
corpus is available.

Materialization replaces its unbounded P4 request with:

- a P2 internal profiler/protocol task that is executable without private
  corpus access;
- a P2 future corpus replay task blocked until a human records an approved
  corpus and the consent details required by S2; and
- a P2 evidence analysis/recommendation task depending on the replay task.

`quill-buu` is superseded only after these replacement tasks exist with
acceptance criteria, dependencies, and explicit blocked state. Retirement does
not imply that validation happened.

### Related source: quill-xsd

`quill-xsd`, **Report per-table footprint with dbstat**, is not silently
discarded. Its unknown cost and usefulness become a P3
`dbstat` measurement/decision task depending on both the profiler and future
corpus replay tasks.

The intended disposition is:

- keep the task blocked on the profiler and approved-corpus replay;
- measure local, offline object totals, full-page-walk cost, cancellation, and
  usefulness when the dependencies are satisfied;
- create a product-facing follow-up only through a later, separately approved
  gate with a measured execution budget and owned user surface; and
- supersede `quill-xsd` only after the P3 replacement exists.

Materialization must leave no open P4 in the source closure. It may supersede
the sources after replacement coverage exists even though corpus-dependent
replacements remain blocked.

## Target Epic

The target is existing closed epic `quill-nm2`, **retention-pruning**.

The new work remains under that epic because it validates the assumptions,
budgets, and deferred reporting question created by Feature 014. Reopening the
epic is not required; new P0-P3 children can record follow-up work under a
closed parent if Beads permits it. If Beads forbids that structure, the plan
must record the smallest provenance-preserving alternative rather than invent
an unrelated epic.

## User Stories

These stories distinguish deliverables available now from corpus-dependent
work that remains blocked.

### S1: Deliver internal study tooling without claiming a study

As a maintainer, I want repo-local profiling and replay tooling plus a written
protocol, so that a later approved corpus can be studied consistently.

**Acceptance criteria:**

- The utility is internal and repo-local, with documented maintainer
  invocation but no supported user CLI, Tauri/IPC command, or UI.
- It accepts an explicit path only. It has no default database, home-directory
  scan, automatic acquisition, or transfer behavior.
- Its profiler fields, replay fields, decision subjects, typed missing values,
  and report schema are stable and documented.
- The protocol covers approval, source inventory, page-preserving backup,
  scratch-only migration, replay, cleanup, privacy review, and failure
  handling.
- Without an approved corpus, delivery contains no fabricated example
  measurements or second-corpus classifications.
- Existing repository quality gates apply. New automated test code remains
  outside scope without separate authorization.

### S2: Gate later work on explicit corpus approval

As a corpus custodian, I want access and use recorded before any inspection, so
that the tool cannot turn availability into implied consent.

**Acceptance criteria:**

- The future replay task remains blocked until a human names one already-local
  `usage.db`, its owner or custodian, authorized uses and reviewers, cleanup
  deadline, and whether it was previously pruned or rolled up.
- Approval records a non-sensitive label and why the corpus is independent of
  the 2026-07-24 corpus. A copy or derivative of that corpus does not qualify.
- Quill is stopped before inventory. The source is inventoried as-is before
  scratch migration or replay.
- The tool opens the source read-only and refuses unrecognized SQLite or Quill
  schemas. It performs no checkpoint, migration, write pragma, retention
  command, or VACUUM against the source.
- If no corpus is approved, the replay, analysis, and `dbstat` decisions stay
  blocked. This is the current expected state, not validation failure or
  completed evidence.

### S3: Preserve source shape and isolate every output

As a maintainer, I want shape-sensitive replay on recoverable scratch copies,
so that future evidence is meaningful and cannot mutate or alias the source.

**Acceptance criteria:**

- Shape-sensitive copies use SQLite's page-preserving backup API. They do not
  use `VACUUM INTO`, which would compact the evidence shape, and never use an
  ordinary copy of a live WAL database.
- Canonical paths and filesystem identity prove the source, scratch database,
  source and scratch sidecars, archive directory, local manifest, and report
  output cannot collide or alias before any write begins.
- Only scratch copies are migrated to current schema. Source inventory records
  the original schema and shape.
- Disk is preflighted before backup, archive, delete, and compaction. Failure
  stops before the relevant mutation.
- Scratch databases, archives, manifests, and logs use private permissions and
  are removed by default after success, failure, or cancellation.
- Any explicitly retained diagnostic artifact has a local path, reason,
  custodian, and expiry; it is never confused with the source.

### S4: Run a bounded future replay matrix

As a performance reviewer, I want comparable repeated runs, so that later
classifications separate controlled evidence from cache noise.

**Acceptance criteria:**

- Archive-off and archive-on modes each start from a fresh page-preserving
  backup.
- Each mode runs three controlled warm measurements. One best-effort cold run
  is labeled separately and never substituted for a controlled repetition.
- The run records environment, SQLite version, schema, corpus shape, current
  constants, fixed UTC anchor, cutoffs, cache state, and concurrent-load
  policy.
- It captures comparable inventory plus current-engine counting, chunk, WAL,
  TEMP, archive, delete, compaction, before/after size, and cancellation
  measurements.
- Progress is observable and all long phases support cancellation. The source
  remains unchanged on success, failure, or cancellation.
- The runner cleans scratch artifacts by default and emits typed,
  path-scrubbable failures for expected source, schema, identity, disk,
  cancellation, and measurement conditions.

### S5: Classify only approved future evidence

As a product maintainer, I want an objective report contract, so that a later
study cannot turn one noisy observation into a policy change.

**Acceptance criteria:**

- Future reports record median and maximum for the three controlled runs and
  label the best-effort cold observation separately.
- A ceiling is **confirm** only when all comparable controlled runs remain
  within it.
- A ceiling is **revise** only when at least two of three controlled runs
  exceed it by more than 10%.
- Every other outcome is **insufficient evidence**. Missing, incomparable,
  already-pruned, or internally conflicting evidence is never coerced into a
  decision.
- The 90-day preset remains **insufficient evidence** until a separate product
  tradeoff threshold defines acceptable disk savings and capability loss.
- This phase emits a template with no classification values because no corpus
  exists. Production changes require separately reviewed implementation work.

### S6: Publish reviewable evidence without publishing private data

As a reviewer, I want a concise, source-backed report and reproducible method,
so that I can verify the recommendation without receiving another user's
database.

**Acceptance criteria:**

- The exact local run manifest may contain source paths and unsuppressed
  aggregate measurements, but it stays private, uses restrictive permissions,
  and follows the recorded cleanup deadline.
- A future committed artifact may contain exact whole-table/object totals and
  timings, SQL definitions, scrubbed environment facts, classifications, and
  limitations.
- Category or month cells with counts below 10 are suppressed rather than
  rounded, merged, or exposed. Suppression does not alter whole-table totals.
- A denylist review confirms no database file, WAL/SHM file, retention archive,
  raw row, transcript text, payload, identifier, hostname, or absolute user
  path is staged. Raw errors are replaced with typed, scrubbed summaries.
- All measurement work is local by default. Any external transfer requires a
  separate explicit opt-in naming destination, fields, scrub, and purpose;
  absent that approval, transfer does not occur.
- Privacy review is required before any evidence enters Git, Beads, CI,
  support channels, or another external destination.
- The report identifies source files and `lat.md` sections that own retention
  architecture, and any implementation change updates those docs before
  completion.
- Applicable format, lint, build, existing-test, and `lat check` gates pass
  before implementation delivery. New tests are added only after explicit
  authorization.
- Beads records source-P4 dispositions, dependencies, blockers, and final
  evidence; commits, sync, and push remain gated by explicit authority.

### S7: Materialize bounded replacement work

As a maintainer, I want the two P4 sources retired only after actionable
coverage exists, so that the absent corpus remains visible without preserving
an unbounded backlog.

**Acceptance criteria:**

- Materialization creates the P2 profiler/protocol task, P2 corpus replay task,
  P2 analysis/recommendation task, and P3 `dbstat` measurement/decision task
  described in Backlog Inputs.
- The replay task has an explicit approved-corpus blocker. Analysis depends on
  replay. The `dbstat` task depends on both profiler and replay.
- `quill-buu` and `quill-xsd` are superseded only after all replacement tasks
  exist and trace back to their source requirements.
- The closure contains no open or ready P4 after materialization. Blocked P2/P3
  work is reported as blocked, not completed.
- No product UI task is created now. A later product task requires separate
  approval after `dbstat` meets an explicit usefulness, latency, placement,
  and cancellation gate.

## Constraints

These constraints apply the repository constitution to current tooling and
later corpus-dependent execution.

- **Principle 1 — Local source-backed truth.** Record observed values and
  explicit gaps. With no corpus, record no measurements or classifications.
  Do not infer row contents, deleted history, or representativeness from file
  size alone.
- **Principle 3 — Responsive execution.** Database copies, `dbstat` walks,
  full counts, archive replay, deletion, and VACUUM run off Tauri setup and UI
  threads. Product integration, if later approved, must be bounded and must
  not lengthen the retention quiesce lease without measured justification.
- **Principle 4 — Recoverable mutation.** Open the source read-only; mutate
  only a verified SQLite-consistent scratch copy. Preflight disk, preserve
  last-known-good evidence, and fail before destructive work when identity or
  headroom is uncertain.
- **Principle 5 — Typed failure boundaries.** Expected invalid-path, schema,
  insufficient-disk, source-busy, and measurement failures are distinguished.
  Unexpected errors retain operation and path context.
- **Principle 7 — Authorized behavior testing.** This spec does not authorize
  new automated tests. Measurement utilities and existing tests may be run;
  new test code requires explicit user approval.
- **Principle 8 — Architecture traceability.** Any behavior, architecture, or
  test change updates `lat.md`; completion requires `lat check`.
- **Principle 10 — Measured performance.** Budget conclusions require
  reproducible measurements with environment, corpus shape, repetitions, and
  current constants recorded. Tooling completion is not measurement evidence.
- **Principle 11 — Explicit external transmission.** The database and raw
  output remain local. Aggregate publication is scrubbed and reviewed; any
  external destination requires separate opt-in.
- **Principle 12 — Gated delivery.** Clarification, analysis, quality, Beads,
  commit, sync, and push gates remain in force. A measurement result does not
  self-authorize a product change.
- The original 2026-07-24 inventory, later index-drop copy, and synthetic
  timing fixture used different file sizes and purposes. The comparison must
  label them separately rather than select whichever baseline is favorable.
- Future source access requires an explicit local path, owner or custodian,
  authorized uses and reviewers, cleanup deadline, and prior
  prune/rollup/archive state. The utility never discovers a corpus.
- SQLite `dbstat` is available in the bundled build, but it walks database
  pages and excludes or categorizes some bytes differently from filesystem
  size. Reports must explain reconciliation rather than promise exact equality.
- Quill must stop before future source inventory. Shape-sensitive replay uses
  SQLite's page-preserving backup API, not an ordinary file copy or
  `VACUUM INTO`.
- Canonical-path and filesystem-identity checks include database sidecars,
  archives, manifests, and report outputs, not only source and scratch database
  names.
- Timestamps remain source data. Non-conforming values are counted but never
  normalized in the source for measurement convenience.
- Current retention includes more than Feature 014's original two target
  tables. The study must preserve an apples-to-apples subset and a separate
  current-engine view.
- Real-corpus evidence must not become a required CI fixture or enter the
  repository.
- The controlled classification population is three warm runs per mode. The
  cold run is best effort and separately labeled.
- Future committed evidence permits exact whole-table/object totals and
  timings, suppresses category/month cells below 10, strips paths,
  identifiers, payloads, and raw errors, and requires privacy review.
- The 90-day preset has no approved product tradeoff threshold and therefore
  cannot be confirmed or revised by this feature.

## Open Questions

Clarification resolved current scope, protocols, privacy, replay, evidence, and
backlog questions. These deferred inputs do not block tooling materialization:

1. Which independent local corpus, custodian, authorized uses, cleanup
   deadline, and prior-retention state will a human approve for the blocked
   replay task?
2. What product tradeoff threshold would let future evidence evaluate the
   90-day preset? Until separately approved, its disposition remains
   **insufficient evidence**.
3. What numeric usefulness, latency, execution-placement, and cancellation
   gate must `dbstat` pass before a separately approved product-facing task can
   be proposed?
4. Which exact repository path and serialization format should be canonical
   for a future scrubbed evidence report? Planning may choose one location;
   other documentation must link to it instead of duplicating mutable results.

## Spec Review

This section preserves the pre-clarification review as historical evidence.
Its questions motivated the authoritative answers above and are no longer
active blockers where Clarifications resolves them.

Six independent passes reviewed requirements, gaps, ambiguity, feasibility,
scope, and stakeholders against the current repository and all twelve
constitution principles. Cross-dimension findings are preserved below.

### Critical Questions (answer before planning)

1. **What exact corpus is admissible, and what consent governs it?** Name the
   already-local database, its owner or custodian, authorized operators and
   reviewers, and the approval scope for read-only inspection, scratch replay,
   aggregate publication, retention, and cleanup. Define a material
   independence rule that excludes copies or derivatives of the first corpus,
   and decide whether an already-pruned or rolled-up database can qualify. A
   blocked "no corpus available" report is not completed validation unless the
   human explicitly changes this feature's outcome. — flagged by:
   requirements, gaps, ambiguity, feasibility, scope, stakeholders

2. **Where does Feature 017 stop, and what artifact does it ship?** Choose
   whether the MVP is an internal repo-local profiler/replay utility plus
   evidence and follow-up beads, or whether it also changes production
   constants or the recommended preset. Decide whether "acquisition" means
   approving an existing local path only. A supported end-user CLI, Tauri
   command, UI, transfer workflow, and automatic policy change are separate
   product scope unless explicitly selected. — flagged by: requirements, gaps,
   ambiguity, scope, stakeholders

3. **What source, snapshot, and schema sequence preserves the evidence being
   measured?** Decide whether Quill must stop or one read transaction may bind
   a live source; which schema versions qualify; whether inventory runs before
   migration; and whether only the scratch copy is migrated for current-engine
   replay. Shape-sensitive runs need a page-preserving SQLite backup rather
   than `VACUUM INTO`, which compacts the copy. Define canonical-path plus
   filesystem-identity checks so source, scratch, WAL/SHM, archive, and output
   paths cannot alias. — flagged by: requirements, gaps, ambiguity,
   feasibility, scope

4. **What bounded replay matrix and scratch lifecycle are required?** Fix
   archive-off versus archive-on runs, fresh-copy resets, repetition count,
   warm/cold-cache definition and fallback, concurrent-load policy, supported
   scale, disk-headroom formula, progress/cancellation limits, and cleanup
   behavior after success, failure, or cancellation. Include private
   permissions and explicit expiry for any retained scratch database or
   archive. — flagged by: requirements, gaps, ambiguity, feasibility, scope,
   stakeholders

5. **What objective rule classifies each recommendation and budget?** Enumerate
   the decision subjects: 90-day recommendation, chunk size, per-chunk wall
   target, WAL and TEMP bytes per row, WAL/TEMP preflight terms, free-space
   recheck interval, counting budget, stale-preview tolerance, and total delete
   wall budget. Define the statistic, outlier handling, machine normalization,
   exceedance margin, safety-margin derivation, and evidence needed for
   **confirm**, **revise**, or **insufficient evidence**. Keep observations such
   as doomed-row count distinct from budgets. — flagged by: requirements,
   gaps, ambiguity, feasibility, scope

6. **What is the private evidence contract versus the committed report
   contract?** Define exact fields allowed in a local run manifest; fields
   allowed in Git and Beads; permitted category labels; low-cardinality
   suppression or rounding; path and error redaction; privacy sign-off; and how
   a reviewer verifies results without receiving the source database. Resolve
   the current conflict between exact aggregate acceptance criteria and
   potentially fingerprinting output. — flagged by: requirements, gaps,
   ambiguity, feasibility, stakeholders

7. **What concrete P0-P3 replacements close both backlog sources?** Name task
   boundaries, priorities, dependencies, and terminal evidence for
   `quill-buu`, then close it as superseded only after that coverage exists.
   For `quill-xsd`, define the maintainer or user decision that per-object
   footprint serves and the numeric latency/resource/cancellation and
   usefulness gate. Its P4 must end either in a bounded measurement task plus a
   separately approved product follow-up, or in an explicitly measured
   retirement; Feature 017 does not silently leave it open. — flagged by:
   requirements, gaps, ambiguity, scope, stakeholders

### Non-Blocking Observations

- Use typed `null` or `not_applicable` with a reason for missing tables,
  undefined timestamps, zero-denominator percentages, and fields that do not
  apply to `model_usage_observations`; do not coerce them to zero.
- Pin cutoff semantics to one UTC anchor, strict `timestamp < cutoff`,
  documented monthly buckets, and a fixed numeric-rounding convention.
- Run `dbstat` after timing-sensitive work or against a separate identical
  snapshot so its full-page walk does not warm the replay cache.
- Keep local path-rich failures out of committed artifacts. Scrub before
  copying diagnostics into Git, Beads, CI, or support channels.
- Pick one canonical evidence file during planning; other docs should summarize
  and link to it rather than duplicate mutable measurements.
- A product-facing per-table UI, supported user profiler, persistent telemetry,
  broad legacy-schema support, third-corpus study, and population-wide claim
  remain follow-up work unless the clarification gate explicitly adds them.
- Support and release work should never request a raw user database. Any later
  behavior change needs release, rollback, and affected-user analysis separate
  from this observational study.
