# Analysis: retention-second-corpus

The aligned spec and plan fully cover the tooling-only feature and preserve
future corpus work as explicitly blocked, source-backed follow-up work.

## Coverage Table

| Requirement | Status | Plan coverage |
| --- | --- | --- |
| S1 — Deliver internal study tooling without claiming a study | full | `Architecture Approach` defines the repo-local binary/module boundary and forbids corpus conclusions; `Affected Components` names the implementation and document surfaces; `API / Interface Changes` keeps the interface maintainer-only. |
| S2 — Gate later work on explicit corpus approval | full | `Architecture Approach` isolation boundaries 1-2 require explicit approval, stopped Quill, read-only source access, and fail-closed schema checks; `Data Model > Approval record` defines required consent fields; `Sequencing` step 2 creates the human gate. |
| S3 — Preserve source shape and isolate every output | full | `Architecture Approach` isolation boundaries 2-5 and the following backup, identity, permission, hashing, and disk-preflight paragraphs require page-preserving backup, scratch-only migration, alias refusal, source immutability proof, and cleanup. |
| S4 — Run a bounded future replay matrix | full | `Architecture Approach` fixes cancellation, phase preflights, UTC cutoff semantics, three warm runs plus one separately labeled cold run per archive mode; `Sequencing` step 3 defines fresh-copy execution and cleanup. |
| S5 — Classify only approved future evidence | full | `Data Model > Decision-subject registry` defines subjects, comparability, statistics, thresholds, normalization, rounding, and safety margins; `Sequencing` step 4 applies those rules and keeps the 90-day preset insufficient. |
| S6 — Publish reviewable evidence without publishing private data | full | `Data Model > Private manifest` separates exact local evidence; `Data Model > Scrubbed evidence report` defines suppression, denylist scanning, signoff, and transfer consent; `Testing Strategy` and `Affected Components` cover quality and architecture-document gates. |
| S7 — Materialize bounded replacement work | full | `Backlog Refinement` defines exactly four P2/P3 replacements, dependencies, provenance, blocked states, and zero-P4 invariant; `Target Epic` fixes the structural route and fallback. |
| Q1 — No independent corpus exists | full | `Architecture Approach` states this is tooling and planning only; `Sequencing` step 2 and `Backlog Refinement` item 2 preserve corpus approval as a future human gate. |
| Q2 — Stop at internal utility, protocol, template, and follow-up beads | full | `Architecture Approach`, `Affected Components`, and `API / Interface Changes` exclude constants, presets, supported CLI, Tauri/IPC, UI, transfer workflow, and product behavior changes. |
| Q3 — Stop Quill, inventory as-is, page-back up, migrate scratch only | full | `Architecture Approach` isolation boundaries 2-4, backup-feature paragraph, and explicit-path `Storage` paragraph define the sequence and prevent source migration. |
| Q4 — Archive-off/on, three warm plus one best-effort cold, bounded lifecycle | full | `Architecture Approach` cancellation, disk, and matrix paragraphs plus `Sequencing` step 3 cover fresh backups, repetitions, cache labels, preflight, cancellation, and default cleanup. |
| Q5 — Median/maximum and objective classification | full | `Data Model > Decision-subject registry` defines the complete three-result rules; `Sequencing` step 4 applies them without classification when evidence is absent. |
| Q6 — Exact private manifest and minimized committed report | full | `Data Model > Private manifest` and `Data Model > Scrubbed evidence report` distinguish private and publishable fields, suppress cells below 10, strip sensitive data, and require privacy review. |
| Q7 — Replace both P4 sources with dependency-wired P2/P3 work | full | `Backlog Refinement` maps `quill-buu` to items 1-3 and `quill-xsd` to item 4, then requires replacement and provenance verification before supersession. |
| Key non-goals and scope boundary | full | `Architecture Approach`, `Affected Components`, and `API / Interface Changes` keep real measurements, production constants, policy, supported product surfaces, acquisition, schema changes, and source mutation out of scope. |
| Consent, privacy, and external-transmission boundary | full | `Data Model > Approval record` requires explicit allowed use; `Data Model > Scrubbed evidence report` requires local handling, minimization, denylist review, privacy signoff, and separate destination-specific transfer consent. |
| Source, backup, identity, and cleanup protocol | full | `Architecture Approach` isolation boundaries and the identity, permission, hashing, disk, and cancellation paragraphs define fail-closed operation across source, sidecars, scratch, archive, manifest, marker, and report. |
| Evidence comparability and classification contract | full | `Data Model > Decision-subject registry` separates Feature 014 and current-engine fields, fixes three controlled results, preserves cold runs as diagnostics, and defines confirm/revise/insufficient outcomes. |
| Work item 1 — P2 profiler and protocol utility | full | `Backlog Refinement > 1. P2 — Build internal retention corpus profiler and protocol utility` is ready, dependency-free, and contains the tooling, protocol, privacy, smoke, and quality acceptance criteria. |
| Work item 2 — P2 approved-corpus replay | full | `Backlog Refinement > 2. P2 — Replay retention against an approved independent corpus` depends on item 1 and the explicit human gate, with source-proof, matrix, private-artifact, and cleanup acceptance criteria. |
| Work item 3 — P2 evidence analysis | full | `Backlog Refinement > 3. P2 — Analyze evidence and recommend retention budget dispositions` depends on item 2 and owns scrubbed publication, classification, privacy signoff, and follow-up recommendations. |
| Work item 4 — P3 `dbstat` decision | full | `Backlog Refinement > 4. P3 — Measure and decide offline dbstat footprint usefulness` depends on items 1 and 2 and has explicit reconciliation, performance, resource, cancellation, retain/reject, and product-follow-up gates. |

All spec stories, authoritative clarifications, critical boundaries, and the
four planned work items have full plan coverage. No requirement is partial or
uncovered.

## Backlog Disposition

| Source P4 | Replacement coverage | Disposition | Ready to resolve? |
| --- | --- | --- | --- |
| `quill-buu` | Items 1-3 | Split and supersede after replacements, dependencies, acceptance criteria, and provenance exist. Supersession records coverage, not completed corpus validation. | Yes, only after replacement creation and verification. |
| `quill-xsd` | Item 4 | Split and supersede after the replacement, dependencies, acceptance criteria, and provenance exist. Supersession records coverage, not a completed `dbstat` decision. | Yes, only after replacement creation and verification. |

The source P4s are not completed by supersession. Item 1 becomes ready;
corpus-dependent items 2-4 remain blocked until the approved-corpus human gate
is resolved.

## Target Epic

Target epic is resolved as existing closed epic `quill-nm2`,
**retention-pruning**. The primary route creates all four structural children
under it without reopening it.

If Beads rejects children under a closed parent, create the four tasks at the
nearest permitted root, retain each source-specific `discovered-from` edge,
record `quill-nm2` target metadata and provenance on every task, and verify
that route before superseding either source. There is no target-epic or
backlog ambiguity.

## Remaining Risks

- **No corpus by design.** Feature 017 cannot validate, confirm, or revise any
  budget. This is an explicit execution gate, not a planning defect.
- **`Storage::init` factoring.** Separating explicit-path schema work from
  production startup can reorder migrations, indexes, or cleanup. Preserve the
  production wrapper and ordering and review the focused diff.
- **Cancellation plumbing.** Backup, SQLite scans, archive streaming, chunked
  deletion, and VACUUM need different interruption points. Optional study
  cancellation must leave normal product behavior unchanged.
- **Page backup, identity, and permissions across platforms.** `rusqlite`
  backup support must be enabled and Unix/Windows identity and owner-only
  permission behavior verified. Ambiguous aliases or unverifiable permissions
  must fail closed.
- **Source hashes and sidecars.** Hash, size, identity, presence, and WAL/SHM
  set checks can detect change but cannot independently prove every writer is
  stopped. Human stop attestation plus `source_changed`/`source_busy` handling
  remains necessary.
- **Archive headroom.** Escaped JSON may exceed SQLite payload bytes.
  Conservative per-type checked bounds are required before archive work.
- **Cold-cache limits.** Portable code cannot guarantee OS cache eviction.
  Cold attempts stay best effort, separately labeled, and non-classifying.
- **Privacy leakage.** Exact manifests, errors, archives, paths, and small
  cells can fingerprint a user. Private permissions, cleanup, separate
  rendering, denylist scanning, suppression, and human review are cumulative
  controls.
- **Human-reviewable but unvalidated `dbstat` thresholds.** The 2,616 ms,
  1,000 ms, 64 MiB, 90%, and 5% gates are explicit and deterministic but have
  not been validated on an independent corpus. Item 4 must report each
  predicate rather than imply broader product value.
- **No automated-test authorization.** Implementation relies on existing
  gates and documented synthetic manual smoke checks. This leaves higher
  regression risk around identity, permissions, scrubbing, and cancellation;
  adding test code still requires separate user authorization.

## Unresolved Questions

| Kind | Input | Effect |
| --- | --- | --- |
| Deferred corpus input | Which independent already-local corpus, custodian, authorized uses/reviewers, approval scope and expiry, cleanup deadline, independence rationale, concurrent-load policy, and prior prune/rollup/archive state will be approved? | Blocks items 2-4 only. It does not block item 1 or Beads materialization. |
| Deferred product input | What product tradeoff threshold defines acceptable disk savings and capability loss for the 90-day preset? | Keeps that decision subject `insufficient evidence`; it does not block tooling, replay of other subjects, or Beads materialization. |
| Planning blocker | None. | Scope, protocol, privacy contract, classification, four work items, dependencies, source dispositions, and target epic are resolved. |

## Constitution Check

| Principle | Status | Assessment |
| --- | --- | --- |
| 1. Local source-backed truth | Pass | No corpus produces no measurements or conclusions; future missing and incomparable evidence remains explicit. |
| 2. Established stack and boundaries | Pass with tension | Rust backend tooling fits the established stack, but factoring `Storage::init` crosses a sensitive production boundary and must preserve startup behavior exactly. |
| 3. Responsive execution | Pass | Heavy work is outside Tauri/UI paths, bounded, observable, and cancellable. |
| 4. Recoverable mutation | Pass | Source access is read-only; all migration, archive, deletion, aggregate, and compaction mutation occurs on disposable verified scratch copies. |
| 5. Typed failure boundaries | Pass | Expected approval, source, schema, identity, disk, cancellation, privacy, and measurement failures receive stable codes; private context is scrubbed for publication. |
| 6. Zero-warning quality gates | Pass | Existing format, lint, typecheck, build, test, pre-commit, and `lat check` gates remain required. |
| 7. Authorized behavior testing | Pass | The plan intentionally adds no automated test code. It uses existing tests and synthetic manual smoke validation until separate authorization exists. |
| 8. Architecture traceability | Pass | Implementation must update `lat.md/backend.md`, link owning architecture, and pass `lat check`. |
| 9. Glass Cockpit discipline | Pass | No UI or product-facing surface changes; the principle is not activated. |
| 10. Measured performance | Pass with tension | Budgets, repetitions, comparability, and decision rules are explicit, but corpus measurement is deliberately deferred; no conclusion is authorized before reproducible evidence exists. |
| 11. Explicit external transmission | Pass | Work remains local; committed aggregates require privacy review, and any external transfer has a separate destination-, field-, scrub-, and purpose-specific opt-in gate. |
| 12. Gated delivery | Pass | Corpus access, privacy publication, product changes, Beads state, implementation, test code, commits, sync, and push retain independent approval gates. |

No constitution violation exists.

## Recommendation

**GO.** The aligned plan fully covers the spec, authoritative clarifications,
backlog sources, four replacement items, dependencies, target epic, privacy
boundary, and current no-corpus state. Remaining concerns are implementation
or deferred-evidence risks already owned by explicit gates and acceptance
criteria.

Approval of this analysis authorizes Beads replacement creation and source
disposition only. It does not authorize implementation, corpus access,
automated test code, commits, sync, or push.
