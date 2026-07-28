# Implementation Plan: Retention Second Corpus

## Architecture Approach

Feature 017 delivers study tooling and a reproducible protocol, not a second
corpus result. No independent corpus is available or approved, so no real
database is opened, no budget is classified, and no production retention
constant or preset changes in this feature.

The implementation adds a separate internal maintainer binary,
`src-tauri/src/bin/retention_corpus.rs`, backed by
`src-tauri/src/retention_study.rs`. The binary is a repo-local measurement
surface invoked through Cargo. It is not registered as a Tauri command, exposed
through IPC, installed as a supported user CLI, or connected to the UI.

`retention_study.rs` owns the study protocol and is the only public Rust module
needed by the binary crate. Its exported items are explicitly unstable,
maintainer-only interfaces. It delegates destructive scratch work to Quill's
current retention implementation rather than copying SQL out of
`retention_engine.rs`. This keeps the future replay aligned with the three
current retention targets, daily aggregate writes, optional JSONL archive,
watermark behavior, delete preflight, chunking, and compaction.

The current `retention_spike` binary remains unchanged. It is coupled to the
frozen Feature 014 fixture and reimplements the original two-table measurement
shape. Extending it would mix synthetic budget calibration with private-corpus
study behavior and would omit current-engine behavior such as
`model_usage_observations`, `retention_daily_aggregates`, and archive-on replay.

The workflow has five isolation boundaries:

1. An approval record names one explicit, already-local `usage.db`. There is no
   default path, home-directory scan, acquisition, upload, or transfer.
2. Quill is stopped before source inventory. The source is opened with
   `SQLITE_OPEN_READ_ONLY`, inventoried as-is, and never migrated, checkpointed,
   vacuumed, or written. Before backup, the profiler fails closed unless the
   file is SQLite, its schema is recognizable as Quill, and its schema version
   is no newer than this build supports. Non-SQLite, unrecognized/non-Quill,
   and too-new databases never reach backup or migration.
3. Each replay begins with a new SQLite online-backup destination. The backup
   copies database pages into a private scratch database and incorporates a
   consistent WAL snapshot without using an ordinary filesystem copy.
4. Only the scratch database enters Quill's existing migration and retention
   paths. Archive-off and archive-on never reuse one another's copies.
5. Exact manifests and scratch artifacts remain private and are removed by
   default. A separate renderer produces the minimized report eligible for
   privacy review and eventual commit.

`rusqlite 0.31` provides `backup::Backup::new(&Connection, &mut Connection)`,
`step`, `progress`, and `run_to_completion`, but the module is behind the
crate's `backup` feature. The current dependency enables only `bundled` and
`hooks`; implementation must add `backup` to the existing feature list. The
runner uses a bounded `Backup::step` loop rather than
`run_to_completion`, allowing progress, cancellation, and typed handling of
`Busy` and `Locked`.

The scratch migration entry point must take an explicit database path. Refactor
`Storage::init` so production path resolution and production startup side
effects remain unchanged, while a study-only constructor applies the same
schema creation, migrations, and startup indexes to a named scratch database.
The study constructor skips unrelated startup cleanup, transcript discovery,
rules archival, and other home-directory behavior. It adds no migration and
cannot target the source connection.

Long phases share an `Arc<AtomicBool>` cancellation token. A private cancellation
marker is watched by a small standard-library thread and flips the token.
Backup checks between page batches; inventory, migration, `dbstat`, scan, and
VACUUM use SQLite progress interruption; archive checks between rows; delete
checks between committed chunks. Cancellation may leave committed mutations in
a scratch copy, but never in the source, and cleanup removes that copy by
default.

Before every write, the runner resolves canonical paths and filesystem
identities for the source, its `-wal` and `-shm` sidecars, scratch database and
sidecars, archive directory, private manifest, cancellation marker, and report
output. Existing objects are compared by filesystem identity, not spelling.
New objects use exclusively created files inside a private scratch directory,
then are rechecked. Any source/output alias, symlink escape, identity ambiguity,
or collision aborts before mutation. Private files and directories use
owner-only permissions on Unix and an equivalent owner-only ACL on Windows;
an unsupported or unverifiable permission model fails closed.

Source immutability is a measured invariant, not an assumption. Immediately
before the first source open and after all source reads finish, the private
manifest records SHA-256, byte size, filesystem identity, and presence for
`usage.db` and each existing `-wal`/`-shm` sidecar. A changed digest, size,
identity, presence bit, or sidecar set yields `source_changed` and invalidates
the run. The already-present `sha2` dependency supplies hashing.

Disk checks are phase-specific and use checked arithmetic:

- Backup reserves the source page count multiplied by page size plus scratch
  sidecars and safety headroom.
- Archive preflight computes a conservative JSON encoding bound from typed
  SQLite values before streaming rows.
- Delete uses the current engine's WAL and TEMP preflight and periodic recheck.
- Compaction uses the current two-times-whole-file preflight.

The future matrix uses one fixed UTC anchor and strict `timestamp < cutoff`
semantics. The manifest records the concurrent-load policy. Each controlled
warm run requires Quill stopped, no concurrent workload, a newly created page
backup, and the same fixed read-only priming query set before timing.
Archive-off and archive-on each receive exactly three such results. One
additional best-effort cold observation per mode uses a fresh, unprimed copy;
it is separately labeled, never substituted for a controlled run, and never
included in classification.

Evidence keeps three baselines distinct: the 2026-07-24 real source inventory,
the later `VACUUM INTO` index-drop copy, and the frozen synthetic retention
timing fixture. They are never blended into one corpus or denominator.
Non-conforming timestamps are counted and reported but never normalized into
retention-eligible rows. Calendar summaries use UTC month buckets. Published
durations round to the nearest whole millisecond, byte counts to whole bytes,
rates to two decimal places, and percentages to one decimal place.

### Constitution check

All twelve principles were checked against this tooling-only plan:

| Principle | Result | Plan treatment |
| --- | --- | --- |
| 1. Local source-backed truth | Pass | No corpus means no measurements or classifications. Future fields distinguish observed, missing, not applicable, and suppressed values. |
| 2. Established stack and boundaries | Pass with implementation tension | A Rust binary and Rust domain module stay inside the established backend. The explicit-path `Storage` refactor must preserve production initialization byte-for-byte. |
| 3. Responsive execution | Pass | The utility is outside Tauri and all I/O is bounded, observable, and cancellable. No setup or UI thread is involved. |
| 4. Recoverable mutation | Pass | Source access is read-only. Migrations, archive, delete, aggregate, and VACUUM operate only on fresh verified scratch copies. |
| 5. Typed failure boundaries | Pass | Expected approval, path, identity, schema, disk, cancellation, and measurement failures have stable codes; unexpected errors retain private context and are scrubbed before publication. |
| 6. Zero-warning quality gates | Pass | Existing format, lint, typecheck, build, test, and `lat check` gates are required for implementation. |
| 7. Authorized behavior testing | Pass | This plan adds no automated test code. New tests remain conditional on explicit later authorization. |
| 8. Architecture traceability | Pass | Implementation updates `lat.md/backend.md` and must pass `lat check`. |
| 9. Glass Cockpit discipline | Pass as not applicable | No UI, Tauri, IPC, or product-facing surface is added. |
| 10. Measured performance | Pass with intentional open gate | The protocol fixes repetitions, fields, and classification rules, but no budget conclusion is allowed until an approved corpus is measured. |
| 11. Explicit external transmission | Pass | All work is local. Exact evidence stays private; a scrubbed aggregate report requires review before entering Git or another destination. |
| 12. Gated delivery | Pass | Corpus access, report publication, product changes, Beads state, commit, sync, and push retain their separate gates. |

## Affected Components

Implementation work item 1 may change only the following code and documentation
surfaces:

- `src-tauri/Cargo.toml`
  - Add `backup` to the existing `rusqlite` features.
  - Add no new database, argument-parser, telemetry, or networking dependency.
- `src-tauri/src/bin/retention_corpus.rs`
  - Thin internal maintainer entry point and argument validation.
  - Require explicit paths and an approval record; provide no production
    defaults.
  - Expose internal `synthetic-smoke` and `dbstat` subcommands. The former
    keeps the existing retention fixture alive and emits a non-sensitive
    pass/fail checklist; the latter supplies capability only, with real
    measurement and the retain/reject decision owned by work item 4.
- `src-tauri/src/retention_study.rs`
  - Approval parsing, source inventory, filesystem identity checks, SQLite
    backup, disk preflight, matrix orchestration, cancellation, private manifest
    writes, report scrubbing, evidence classification, synthetic smoke, and
    `dbstat` measurement helpers.
- `src-tauri/src/lib.rs`
  - Export the maintainer-only study module for the separate binary crate.
  - Expose only the minimum crate-internal retention orchestration needed by the
    study module; do not register a command.
- `src-tauri/src/storage.rs`
  - Factor production `Storage::init` into shared explicit-path migration
    plumbing plus production-only startup effects.
  - Add a study-scratch constructor that cannot resolve or mutate the production
    database.
- `src-tauri/src/retention_engine.rs`
  - Add optional cancellation plumbing where the current scan, archive, delete,
    and compaction paths lack a bounded interruption point.
  - Keep the normal product controls and results unchanged when no study token
    is supplied.
- `specs/017-retention-second-corpus/protocol.md`
  - Canonical maintainer runbook: approval, stopping Quill, inventory, backup,
    replay, cleanup, privacy review, and failure handling.
- `specs/017-retention-second-corpus/private-manifest.schema.json`
  - Versioned exact local evidence contract.
- `specs/017-retention-second-corpus/evidence-report-template.md`
  - Canonical scrubbed committed artifact template. Other documentation links
    here rather than copying mutable results. It cites the owning sources
    `src-tauri/src/retention_engine.rs`,
    `src-tauri/src/bin/retention_spike.rs`,
    `specs/014-retention-pruning/retention-timing-spike.md`, and
    `specs/014-retention-pruning/index-drop-measurement.md`, plus
    `lat.md/backend.md` sections `Retention timing spike`,
    `Retention pruning`, `Retention delete engine`, and
    `Retention aggregates`.
- `lat.md/backend.md`
  - Document the internal study boundary, page-preserving backup, scratch-only
    replay, and private/public evidence split after implementation.

No frontend file, Tauri command list, TypeScript contract, persisted setting,
production policy, or database schema migration changes. No source database,
real-corpus report, archive, or exact manifest is committed.

## Data Model

The utility does not add a table. All study state is file-backed and private by
default.

### Approval record

Before any source open, the private manifest must contain:

- non-sensitive corpus label;
- exact local source path;
- owner or custodian;
- authorized operator and reviewers;
- `approved_by`, `approved_at`, approval expiry, revocation state, and the
  allowed-use scope;
- allowed uses, including whether scratch replay and aggregate publication are
  authorized;
- cleanup deadline;
- prior prune, rollup, and archive state, each observed or explicitly unknown;
- independence rationale proving this is not the 2026-07-24 corpus or a
  derivative;
- fixed UTC anchor and requested retention window;
- explicit acknowledgement that Quill has been stopped.

Missing approval fields produce `approval_incomplete`; they are not inferred
from filesystem access.

### Private manifest

`private-manifest.schema.json` is the exact local record. Its root contains:

- `schema_version`, `run_id`, tool commit, lifecycle status, and timestamps;
- approval record and cleanup disposition;
- scrubbed environment facts plus private local paths;
- original source inventory taken before any scratch migration, including
  schema recognition/version, concurrent-load policy, and pre/post SHA-256,
  byte size, filesystem identity, and presence for `usage.db` and all existing
  WAL/SHM sidecars;
- backup identity, page counts, duration, and integrity result;
- scratch schema before and after migration;
- one matrix record per archive mode and cache state;
- phase timings, progress, cancellation, disk preflights, and typed failures;
- whole-table/object totals plus unsuppressed private category/month cells;
- Feature 014 comparable measurements;
- separately labeled current-engine measurements;
- cleanup results and any explicitly retained artifact, custodian, reason, and
  expiry.

Each optional datum uses a typed envelope:

```text
status = observed | missing | not_applicable | suppressed
value  = typed value, present only when observed
reason = stable code, required otherwise
```

Zero remains a valid observed value and is never used to represent missing
evidence.

### Decision-subject registry

Every classification subject has one owner, evidence type, and fixed current
reference. Budgets are ceilings or policy inputs; observations are immutable
measurements. Reports must never relabel an observation as a budget.

| Decision subject | Type | Current reference and derivation |
| --- | --- | --- |
| 90-day recommendation | Policy recommendation | No numeric product tradeoff threshold exists; always `insufficient evidence` in this feature |
| Chunk size | Budget/input | 25,000 rows; largest swept size with pooled p95 hold below 1,000 ms |
| Per-chunk wall | Budget/ceiling | 1,603 ms; 534.3 ms measured p95 × 3 |
| WAL bytes per row | Budget/rate | 788.7 B; worst full 25,000-row chunk divided by actual rows |
| TEMP bytes per doomed row | Budget/rate | 11.05 B; measured doomed-rowid b-tree bytes divided by doomed rows |
| WAL preflight term | Derived budget | `ceil(788.7 × min(25,000, doomed_rows))` bytes |
| TEMP preflight term | Derived budget | `ceil(11.05 × doomed_rows)` bytes; retained in evidence even when `temp_store=MEMORY` |
| Delete preflight total | Derived budget | `2 × (WAL term + TEMP term)` with checked arithmetic; ×2 is concurrent disk-consumption safety, not timing headroom |
| Free-space recheck interval | Budget/input | 3 committed chunks; about one second at the measured 417.7 ms mean hold |
| Counting wall | Budget/ceiling | 2,616 ms; 871.7 ms measured × 3 |
| Stale-preview tolerance | Budget/ceiling | 2,616 ms; one counting budget |
| Total delete wall | Budget/ceiling | 40,598 ms; 13,532.6 ms measured × 3 |

For every mode/metric pair, classification requires exactly three observed,
comparable controlled-warm results. If any member is missing, suppressed,
failed, or incomparable, the result is `insufficient evidence`; extras are
diagnostic and cannot replace a member. `confirm` requires all three at or
below the current ceiling. `revise` requires at least two of three to exceed it
by more than 10%. Every other complete result is `insufficient evidence`.

The report publishes the three raw observations, median, and maximum. It
removes no outlier and performs no machine-, row-, or byte-normalization unless
the registry defines that metric as a per-row rate; those rates divide by
actual conforming rows only. Non-conforming timestamps remain separate counts.
Cold results and cross-baseline comparisons are never normalization controls.
For a revision candidate, timing ceilings retain the documented ×3 margin over
the new observed maximum; WAL/TEMP rates use the worst comparable full-chunk
or b-tree rate and retain the separate ×2 delete-preflight multiplier. Chunk
size and recheck proposals must repeat their documented threshold/derivation
rather than inventing a margin.

Feature 014 comparable fields cover the original source inventory,
`tool_actions`, `session_events`, doomed rows, counting time, chunk size,
per-chunk wall, WAL bytes per row, TEMP bytes per doomed row, free-space
rechecks, total delete wall, and before/after database bytes.

Current-engine fields separately cover:

- `model_usage_observations`;
- `retention_daily_aggregates` before and after replay;
- aggregate rows and counters created by each target's delete chunks;
- archive manifest, row totals, bytes, and wall time;
- delete and compaction results from current Quill code;
- schema and SQLite versions;
- prior watermark, retention settings, and audit record;
- object-level `dbstat` results when work item 4 eventually runs.

The split prevents current additions from silently changing Feature 014's
denominators while still measuring the engine users run now.

### Scrubbed evidence report

`evidence-report-template.md` is the only commit-eligible report shape. It may
contain exact whole-table/object totals, SQL definitions, phase timings,
median/maximum summaries, classification outcomes, and scrubbed environment
facts.

Category and calendar-month cells with counts below 10 render as
`suppressed (<10)`; they are not rounded, merged, or emitted exactly.
Whole-table totals remain exact. The renderer removes absolute paths, source
and run identifiers, hostnames, projects, sessions, agents, payloads,
transcript content, archive rows, and raw error strings. Expected errors become
stable code plus safe summary.

The exact manifest is never transformed in place. Rendering reads private
input and writes a new exclusively created report, then runs a denylist scan.
Classification requires successful manifest-schema validation. Publication to
Git or Beads requires explicit human privacy signoff after schema validation
and denylist scanning. External transfer is otherwise prohibited, occurs only
after the same human privacy signoff, and requires a separate opt-in naming
destination, exact fields, scrub procedure, and purpose; corpus approval or
Git/Beads approval does not imply transfer consent.

## API / Interface Changes

The internal invocation is intentionally repo-local and unsupported:

```text
cargo run --release --bin retention_corpus -- \
  profile --approval <private-json> --source <usage.db> \
  --workspace <private-dir> --cancel-marker <private-path>

cargo run --release --bin retention_corpus -- \
  replay --manifest <private-json> --workspace <private-dir> \
  --cancel-marker <private-path>

cargo run --release --bin retention_corpus -- \
  render-report --manifest <private-json> --output <review.md>

cargo run --release --bin retention_corpus -- \
  synthetic-smoke --workspace <private-dir>

cargo run --release --bin retention_corpus -- \
  dbstat --manifest <private-json> --scratch <usage.db> \
  --cancel-marker <private-path>
```

Argument names are maintainer protocol, not a supported end-user contract.
Every path is mandatory. `replay` refuses unless `profile` completed against
the same canonical source identity and the approval permits replay.
`synthetic-smoke` is the sole no-corpus route: it generates the existing
retention fixture, runs the protocol smokes, and emits only a named pass/fail
checklist. `dbstat` accepts only an identity-verified scratch database and
emits private manifest fields; it never opens the source or publishes a report.

The Rust study surface should expose typed requests and results rather than
stringly dispatch:

- `ProfileRequest` and `profile_source`;
- `ReplayRequest` and `run_replay_matrix`;
- `ReportRequest` and `render_scrubbed_report`;
- `SyntheticSmokeRequest` and `run_synthetic_smoke`;
- `DbstatRequest` and `measure_dbstat`;
- `StudyCancellation`;
- `StudyError` with approval, source, schema, identity, disk, cancellation,
  privacy, and measurement variants;
- serde manifest/report structures versioned independently from the production
  retention audit schema.

No Tauri command, event, window, IPC type, persisted app preference, timer, or
automatic background task is added.

Alternatives rejected:

- **`VACUUM INTO`** compacts the destination and changes the page shape being
  studied. It remains appropriate for Feature 014's isolated index-footprint
  question, not this replay.
- **Ordinary filesystem copy** can tear a live WAL database and cannot bind a
  consistent source snapshot. It is rejected even though the protocol requires
  Quill to stop.
- **Mutating the source to migrate it** destroys the original schema and shape
  evidence and violates the recoverable-mutation principle.
- **Routing scratch through demo environment variables** is process-global and
  risks unrelated startup side effects and home-directory access. Use an
  explicit study constructor.
- **Extending `retention_spike` in place** conflates frozen synthetic budget
  calibration with private current-schema study work and would preserve its
  stale two-table SQL.
- **A product CLI, Tauri command, IPC endpoint, or UI** expands consent,
  support, privacy, and responsiveness obligations without evidence of user
  value.
- **A new database table or migration** would persist study machinery in every
  user database even though the study is maintainer-only.

## Testing Strategy

No automated test code is authorized by this feature. Implementation uses
existing gates plus manual and synthetic smoke validation.

Required existing gates:

- `cargo fmt --check`;
- `cargo clippy --all-targets --all-features -- -D warnings`;
- existing Rust tests;
- frontend lint, typecheck, and build commands when the repository's quality
  runner includes them;
- repository pre-commit runner;
- `lat check`.

Manual smoke validation uses only a newly generated synthetic database from
the existing retention fixture. The documented `synthetic-smoke` subcommand
runs these checks and emits a non-sensitive pass/fail checklist:

1. Prove a missing approval record and omitted explicit path fail before a
   source open.
2. Prove non-SQLite, unrecognized/non-Quill, and too-new schemas fail before
   backup or migration; inventory a recognizable older schema before migrating
   only its scratch copy.
3. Profile the fixture read-only. Record private pre/post SHA-256, byte size,
   filesystem identity, and presence for `usage.db` and existing WAL/SHM
   sidecars; verify no content or sidecar-set delta.
4. Back up through `rusqlite::backup`, verify page size/page count and
   `PRAGMA integrity_check`, then migrate only the scratch copy.
5. Run one reduced archive-off and one reduced archive-on replay from separate
   fresh backups.
6. Trigger cancellation during backup, one SQL phase, and `dbstat`; verify the
   source is unchanged and scratch cleanup occurs.
7. Attempt path, sidecar, archive, manifest, and report collisions; verify each
   refuses before write.
8. Verify owner-only private directory/file permissions on Unix and Windows;
   unsupported, widened, or unverifiable permissions fail closed.
9. Mutate copied content and add/remove a copied sidecar between digest passes;
   verify each yields `source_changed`.
10. Render from schema-valid synthetic private data containing paths,
    identifiers, payload-like strings, low-cardinality cells, and raw errors;
    verify the denylist is clean and cells below 10 are suppressed. Reject an
    invalid manifest and reject publication without explicit privacy signoff.
11. Verify default cleanup removes scratch database, WAL/SHM, archive, logs,
    cancellation marker, and private manifest after the report is produced.
12. Run a reduced `dbstat` walk and verify exact byte reconciliation plus its
    pass/fail checklist without treating synthetic timing as corpus evidence.

These smokes validate mechanics only. Synthetic timing is not second-corpus
evidence and cannot classify a budget.

If the user later authorizes automated tests, highest-value candidates are
filesystem identity/collision cases, manifest schema round-trips, scrubber
denylist and suppression behavior, cancellation at each SQLite phase, and a
fixture-backed source-immutability test. They are not part of the four current
work items.

## Risks

- **Explicit-path migration refactor.** `Storage::init` currently combines path
  resolution, schema creation, 35 migrations, startup indexes, and unrelated
  cleanup. Factoring it can regress production startup if side effects move or
  reorder. Keep the production wrapper and order unchanged, review a focused
  diff, and run all existing migration tests.
- **Backup API is not currently compiled.** The local `rusqlite 0.31` source
  confirms the API, but Cargo enables only `bundled` and `hooks`. Work item 1
  must enable `backup`; code cannot be written as if it already exists.
- **Cross-platform filesystem identity.** Canonical path equality does not catch
  hard links, case folding, or every reparse-point alias. Implementation must
  verify the available Unix and Windows metadata APIs before choosing helpers
  and fail closed when identity cannot be established.
- **Quill-stop attestation is operational.** A portable read-only process cannot
  prove that every possible writer is stopped without taking a source write
  lock, which is forbidden. Record human acknowledgement, open read-only, and
  treat any observed concurrent change as `source_busy` or
  `source_changed`.
- **Cancellation crosses existing private APIs.** Current scan heartbeat always
  continues, archive streams without a cancellation control, and VACUUM has no
  exposed token. Adding optional interruption must not change product behavior
  when absent. A cancelled scratch database is disposable, but its outcome must
  still be typed.
- **Archive upper-bound accuracy.** JSON escaping can exceed SQLite payload
  bytes. Use per-type worst-case checked arithmetic and abort if a safe bound
  cannot be computed; never continue on an optimistic estimate.
- **Best-effort cold cache.** Portable code cannot guarantee eviction from the
  OS page cache. Record the attempted method and comparability; mark unavailable
  cold evidence as missing instead of relabeling a warm run.
- **Legacy and future schemas.** Source inventory may inspect an older
  recognizable Quill schema, but only the scratch migration path may upgrade
  it. A schema newer than the current maximum is refused, not approximated.
- **Private evidence leakage.** Exact manifests, errors, and archives can
  fingerprint a user. Private permissions, default cleanup, separate rendering,
  denylist scanning, and human review are all required; none alone is
  sufficient.
- **One second corpus remains one corpus.** Even after the blocked replay runs,
  findings generalize only to the measured shapes. The report must state that
  limit and cannot replace the deterministic fixture.
- **No corpus is available now.** This is an expected gate, not a failed study.
  Work items 2-4 remain blocked and Feature 017 makes no evidence claim.
- **Rollback and recovery.** Revert or remove the internal binary, study
  module, optional `rusqlite` feature/plumbing, and maintainer documents. There
  is no database or schema rollback because production databases are unchanged.
  The source stays untouched, scratch is disposable, and every retained
  private artifact records custodian, path, reason, expiry, and named cleanup
  action.

## Sequencing

1. Implement the P2 internal profiler/protocol utility, manifest schema, report
   template, explicit-path scratch migration, backup feature, cancellation, and
   privacy boundaries. Include `synthetic-smoke` and internal `dbstat`
   capability. Validate only with existing gates and documented synthetic
   manual smokes.
2. Materialization creates a real Beads human gate blocking item 2:
   `bd gate create --type=human --blocks <item2> --reason="Approved independent corpus required"`.
   Resolve it only after validating `approved_by`, `approved_at`, expiry and
   revocation state, allowed-use scope, corpus independence, custodian, cleanup
   deadline, and concurrent-load policy. Items 3 and 4 stay transitively
   blocked. Do not start corpus-dependent work before resolution.
3. For the approved corpus, stop Quill; inventory the source read-only as-is;
   reject non-SQLite/unrecognized/too-new schemas; then run archive-off and
   archive-on. Each mode gets exactly three controlled warm runs, each from a
   new page backup with no concurrent load and the fixed read-only priming
   query set, plus one separately labeled best-effort cold run from a fresh
   unprimed copy. Clean private artifacts by default.
4. Analyze only the three controlled runs per mode. Report median and maximum.
   Require every member to be observed, comparable, and schema-valid. Any
   missing, suppressed, failed, or incomparable member makes that mode/metric
   `insufficient evidence`. Mark a ceiling `confirm` only when all three are
   within it; mark `revise` only when at least two exceed it by more than 10%;
   otherwise mark `insufficient evidence`. Apply the registry's no-outlier,
   normalization, rounding, and safety-margin rules. The 90-day preset remains
   `insufficient evidence` until a separate product tradeoff threshold exists.
5. After timing-sensitive replay, run the P3 `dbstat` study against a separate
   identical snapshot so its page walk cannot warm replay caches. Before
   measurement, fix three controlled warm repetitions and their same read-only
   priming query set. Reconcile with zero arithmetic tolerance:
   `main database bytes + WAL bytes + SHM bytes = object b-tree bytes + freelist bytes + unattributed/non-btree bytes + WAL bytes + SHM bytes`.
   Record each category explicitly.

   Retain the offline subcommand/report only if all three warm walks finish
   within the current 2,616 ms counting budget, cancellation is observed within
   1,000 ms, incremental RSS is at most 64 MiB, execution remains outside
   setup/UI/quiesce paths, and the reconciliation equation balances exactly;
   otherwise reject/remove the capability. A product follow-up is allowed only
   when all retention conditions pass, per-object accounting also explains at
   least 90% of measured before/after database-byte delta, and at least one
   object occupying at least 5% of the file has a user-actionable storage
   lever. Any failed or missing predicate deterministically means no product
   task.
6. Any production constant, policy, preset, supported interface, or product UI
   proposal becomes separate, approved follow-up work. Measurement does not
   authorize implementation.

## Backlog Refinement

Materialization creates exactly four implementation-ready P0-P3 work items:

### 1. P2 — Build internal retention corpus profiler and protocol utility

**State:** ready now.

**Dependencies:** none.

**Acceptance:**

- Add the separate internal binary and shared study module without changing
  `retention_spike`, product UI, Tauri, IPC, or retention defaults.
- Add documented `synthetic-smoke` and internal `dbstat` subcommands. The
  synthetic command keeps the existing retention fixture exercised and emits
  only a non-sensitive named pass/fail checklist; item 4 owns real `dbstat`
  execution and disposition.
- Require explicit approval/source/workspace paths; perform no discovery or
  transfer.
- Fail closed before backup/migration for non-SQLite, unrecognized/non-Quill,
  and too-new schemas. Preserve original pre-migration inventory and migrate
  only scratch.
- Enable and use `rusqlite`'s verified `backup` feature through cancellable page
  steps; do not use `VACUUM INTO` or ordinary source copy.
- Inventory source read-only as-is; migrate and mutate only identity-verified
  scratch copies.
- Implement phase disk preflights, fail-closed cross-platform private
  permissions, cleanup, cancellation, typed errors, manifest schema, protocol,
  scrubbed report template, and private source/sidecar pre/post SHA-256, size,
  filesystem identity, and presence checks.
- Cover current retention targets, daily aggregates, archive-off/on, and
  Feature 014 comparable fields without recording real-corpus values.
- Keep the three named baselines separate, count but never normalize
  non-conforming timestamps, use UTC month buckets and fixed report rounding,
  and implement the complete typed decision-subject registry.
- Pass existing quality gates, documented `synthetic-smoke` checks, and
  `lat check`; add no automated test code.

### 2. P2 — Replay retention against an approved independent corpus

**State:** blocked.

**Dependencies:** item 1 and the explicit Beads human approved-corpus gate
created by the command in Sequencing.

**Acceptance:**

- Resolve the gate only after validating path, custodian, operators/reviewers,
  `approved_by`, `approved_at`, expiry/revocation state, allowed-use scope,
  cleanup deadline, independence rationale, concurrent-load policy, and prior
  prune/rollup/archive state.
- Stop Quill, inventory source read-only as-is, and prove source identity and
  content remain unchanged through the study using pre/post SHA-256, byte size,
  filesystem identity, and presence for `usage.db` and every existing WAL/SHM
  sidecar. Any content or sidecar-set delta is `source_changed`.
- Run archive-off and archive-on from fresh backups, with three controlled warm
  runs per mode: Quill stopped, no concurrent load, new backup per run, and the
  fixed read-only priming query set before timing. Run one separately labeled
  best-effort cold observation per mode from a fresh unprimed copy.
- Record exact private manifests, preflight and cancellation outcomes, current
  constants, fixed anchor/cutoffs, comparable fields, and current-engine fields.
- Clean scratch artifacts by default and retain any diagnostic only with path,
  reason, custodian, and expiry.
- Commit no database, WAL/SHM, archive, private manifest, path, identifier,
  payload, raw error, or classification.

### 3. P2 — Analyze evidence and recommend retention budget dispositions

**State:** blocked.

**Dependencies:** item 2.

**Acceptance:**

- Produce the scrubbed report from the private manifest with exact whole-table
  totals/timings, suppression of category/month cells below 10, and a clean
  privacy denylist.
- Validate the manifest schema before classification. Cite the owning source
  files and `lat.md/backend.md` sections named in Affected Components.
- Report median and maximum for each three-run controlled population; label cold
  observations separately.
- Apply `confirm`, `revise`, and `insufficient evidence` exactly as defined in
  the decision-subject registry. Missing, suppressed, failed, or incomparable
  members force `insufficient evidence`; do not drop outliers or substitute
  cold/additional results.
- Keep the 90-day preset `insufficient evidence` until a separately approved
  product tradeoff threshold exists.
- Require explicit human privacy signoff before Git or Beads publication.
  Prohibit external transfer without separate opt-in naming destination,
  fields, scrub, and purpose.
- Recommend follow-up work where justified; do not modify production constants,
  presets, or policy in this item.

### 4. P3 — Measure and decide offline `dbstat` footprint usefulness

**State:** blocked.

**Dependencies:** items 1 and 2. The direct item-1 edge is intentionally
retained despite transitive redundancy: it records capability ownership and
`quill-xsd` provenance independently from item 2's corpus execution.

**Acceptance:**

- Run `dbstat` only against a separate identical scratch snapshot after
  timing-sensitive replay.
- Use exactly three controlled warm repetitions with the fixed cache-priming
  query set. Measure full-page-walk wall time, cancellation latency,
  incremental RSS, placement, object/page totals, before/after object deltas,
  and WAL/SHM separately.
- Reconcile with zero arithmetic tolerance using the Sequencing equation and
  explicit object b-tree, freelist, unattributed/non-btree, WAL, and SHM
  categories.
- Retain the offline capability only if all three walks are at most 2,616 ms,
  cancellation is at most 1,000 ms, incremental RSS is at most 64 MiB,
  execution stays outside setup/UI/quiesce, and arithmetic reconciliation is
  exact. Otherwise reject/remove it.
- Add no UI or supported product surface. Propose a product task only if all
  retain gates pass, per-object accounting explains at least 90% of measured
  before/after database-byte delta, and an object at least 5% of file size has
  a user-actionable storage lever. Otherwise record deterministic
  retain/reject reasons and create no product task.

Dependency graph:

```text
item 1 (P2, ready)
├── item 2 (P2, approved-corpus gate)
│   ├── item 3 (P2, analysis)
│   └── item 4 (P3, dbstat decision)
└── item 4 (P3, also requires item 2)
```

`quill-buu` is split into items 1-3. `quill-xsd` is split into item 4. Create
all replacements, their acceptance criteria, dependencies, and provenance
links before superseding either source. Superseding records replacement
coverage; it does not claim replay or validation completed.

On the primary route, items 1-3 each carry `discovered-from quill-buu`; item 4
carries `discovered-from quill-xsd`; all four are structural children of
`quill-nm2`. On the fallback route, each task records `quill-nm2` target
metadata while preserving the same source-specific `discovered-from` edge.

No replacement has priority P4. After materialization, the source closure must
contain zero open P4 and zero ready P4. Blocked P2/P3 work remains visibly
blocked.

## Target Epic

Target existing closed epic: `quill-nm2`, **retention-pruning**.

The four replacements remain follow-up validation and deferred footprint work
for Feature 014, so the closed epic is the correct structural and historical
parent. Do not reopen it solely to add children.

If Beads rejects new children under a closed epic, create the four P2/P3 tasks
at the nearest permitted root level, preserve `discovered-from` provenance to
their source P4s and `quill-nm2`, and record `quill-nm2` as the target epic in
each task. Do not invent an unrelated epic. Supersede `quill-buu` and
`quill-xsd` only after this alternative provenance and all replacement
dependencies are verified.

## Alignment fixes applied

- **A MUST:** Completed typed decision registry, three-result classification,
  summary, normalization/outlier, rounding, and safety-margin rules.
- **A MUST:** Added fail-closed source/schema checks, scratch-only migration,
  repeatable warm/cold protocol, explicit human gate, and exact provenance.
- **A SHOULD:** Separated three baselines, UTC buckets, source/lat ownership
  citations, external-transfer consent, and rollback/recovery.
- **B MUST:** Added synthetic smoke, source/sidecar digest proof, permission
  checks, manifest validation, and human privacy signoff.
- **B MUST:** Fixed `dbstat` repetitions, reconciliation, performance/resource
  gates, and deterministic offline/product dispositions.
- **B SHOULD:** Explained the intentional redundant dependency and named
  cleanup ownership for retained private artifacts.
