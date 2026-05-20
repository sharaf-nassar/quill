# Contract — Honest Confinement Disclosure & Version/Evidence Atomicity

Two internal contracts for feature 006. No external API/IPC surface changes; the only user-facing surface is the run-history confinement disclosure.

## C-A — Honest confinement vocabulary & disclosure (Follow-up A)

- **C-A1**: `SandboxKind::as_str()` is a closed vocabulary. The Linux `bwrap`-absent path MUST map to a tag whose documented meaning is "process/namespace isolation only, NO filesystem confinement" — it MUST NOT map to a tag documented as filesystem confinement.
- **C-A2**: The single FS-confinement classifier `cc_client::sandbox_tag_is_fs_confined(tag)` (keyed on the stable `SandboxKind::as_str` tag — the value is always transported/persisted as that string) returns `true` only for tags that actually deny out-of-workspace filesystem read/write (`bwrap`, `sandbox-exec`); `false` otherwise (`process-only`, `job-object`, `none`, and any unknown future tag). It is the sole source of truth (no duplicate typed method).
- **C-A3**: A confinement tag is recorded on **100%** of analysis runs, success and failure paths, every platform (feature-005 SC-013 preserved). Failure path keeps re-probing the deterministic host kind.
- **C-A4**: Never fail-closed — absence of FS confinement never disables or skips learning.
- **C-A5**: The run-history UI MUST visually distinguish a not-FS-confined run from an FS-confined run and present a remediation hint. FS-confined run rendering is unchanged from feature 005.
- **C-A6**: macOS/Windows behavior unchanged and out of scope.

## C-B — Version/evidence atomicity (Follow-up B)

- **C-B1**: After any re-derivation of a pending (`awaiting_review`) rule, `learned_rules.current_version` MUST resolve to a `rule_version` whose `rule_evidence_citations` snapshot exists, OR `current_version` MUST NOT have advanced for that pending change. The advance and the new-version snapshot are atomic with respect to any reader of review eligibility.
- **C-B2**: If the evidence (re)write fails during re-derivation, `current_version` MUST NOT advance; the rule remains review-eligible on its prior good snapshot (no permanently un-reviewable rule).
- **C-B3**: Re-derivation with unchanged content MUST NOT advance the version or change eligibility.
- **C-B4**: Feature-005 governance is preserved unchanged: lifecycle states/transitions, suppression-stickiness, tombstone gating, single-writer approval, "α/β + content merge always happens" (a citation failure does not roll back the merge), the single indexed point-read eligibility model, and `current_version` as the sole pending-change marker (no new column).

## Verification

Both contracts are verified by deterministic unit tests on the FR-021 CI-gated learning surface (existing `TempDir`/`#[serial]`/`init_storage_in` harness; `cc_client` `InferenceDoubleGuard` offline double) plus a `RunHistory` render assertion and the integrated 0-warning baseline. See [quickstart.md](./quickstart.md).
