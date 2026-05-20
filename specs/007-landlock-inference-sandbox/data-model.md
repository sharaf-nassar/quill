# Phase 1 Data Model — Landlock Inference Sandbox

The feature introduces **no SQLite migration** and **no new persisted column**. This document records the in-memory / vocabulary shape changes and the deploy-safety reconciliation for existing rows.

## Schema impact

**None.** No `CREATE TABLE`/`ALTER TABLE`/migration is added.

- `learning_runs.inference_metadata` / `optimization_runs.inference_metadata` remain the same JSON TEXT columns from migration 25.
- `InferenceCallMetadata.sandbox` is unchanged in shape (`Option<String>`).
- The closed **write** vocabulary contracts on Linux (drops `process-only`; adds `landlock`); the **decode** classifier `cc_client::sandbox_tag_is_fs_confined` adds one positive case and remains tolerant for everything else.

## Entity / serialized-shape deltas

- **`SandboxKind`** (`src-tauri/src/cc_client.rs`): closed enum changes — **remove** `ProcessOnly` (introduced in feature 006-A), **add** `Landlock`, **keep** `Bwrap`/`SandboxExec`/`JobObject`/`None`. The closed `as_str()` vocabulary becomes `{"landlock", "bwrap", "sandbox-exec", "job-object", "none"}`.
- **`cc_client::sandbox_tag_is_fs_confined(tag)`**: positive set becomes `{"landlock", "bwrap", "sandbox-exec"}`; everything else (including legacy `"process-only"`, `"unshare"`, any future unknown tag) classifies as not-FS-confined. Forward-compatible.
- **`LandlockPolicy`** (new, `cc_client.rs` private): value object carrying the RO path set, the RW path (per-call temp dir), and the ABI choice (`ABI::V3` + `BestEffort`). Pure data; testable in isolation.
- **`BwrapBrokenCause`** (new, `cc_client.rs` private enum): `AppArmorRestrictUserns | Other`. Output of `classify_bwrap_failure(stderr)`.
- **`InferenceCallMetadata.sandbox`** (`src-tauri/src/cc_client.rs`): same field; doc updated to enumerate the new write vocabulary; tolerant of legacy values via the decode classifier.

## Deploy-safety reconciliation (existing rows)

Existing production `learning_runs.inference_metadata` JSON rows may carry any of: `"bwrap"`, `"sandbox-exec"`, `"job-object"`, `"none"` (feature 005); `"process-only"` (feature 006-A); legacy `"unshare"` (pre-feature-006). After feature 007:

| Recorded `sandbox` tag | Decode result | Notes |
|---|---|---|
| `"landlock"` (new) | `fs_confined = true` | New write |
| `"bwrap"` (legacy, feature 005+; new writes still possible when Landlock is unsupported and bwrap is the active fallback) | `fs_confined = true` | Historical run actually was FS-confined; new bwrap-fallback writes also valid |
| `"sandbox-exec"` (macOS) | `fs_confined = true` | Unchanged |
| `"job-object"` (Windows) | `fs_confined = false` | Unchanged |
| `"none"` (any) | `fs_confined = false` | Unchanged |
| `"process-only"` (legacy, feature 006-A only) | `fs_confined = false` | Retired tier; historical rows decode honestly |
| `"unshare"` (legacy, pre-feature-006) | `fs_confined = false` | Retired tag; historical rows decode honestly |
| any unknown future tag | `fs_confined = false` | Conservative default; never errors |

## Run-history UI shape

Unchanged from feature 006-A: per-call `confinement?: { sandbox: string; fs_confined: boolean }` and per-run `all_fs_confined?: boolean` rollup. The new mechanism flows through automatically.

## Diagnostic content (cross-reference, not stored)

The actionable diagnostic emitted under FR-014/FR-015 is **not persisted as a structured field**; it appears in `learning_runs.logs` (the existing log text column) and in `log::error!`. Stable substrings checkable by tests:

- Generic (FR-014) includes the substring: `Filesystem confinement is unavailable on this host.`
- AppArmor-specific (FR-015) includes the substring: `AppArmor's \`restrict_unprivileged_userns\` policy is blocking bubblewrap`.

(Exact wording lives in `contracts/landlock-sandbox.md`.)

## Reconciliation hooks into prior features

Append dated lines (mirroring feature 006's reconciliation style; do not rewrite history):

- `specs/005-learning-system-hardening/research.md` R-7 / R-7.6: feature 007 introduces Landlock LSM as the primary Linux mechanism in front of bwrap (kept as fallback). The "best-available OS-level confinement" hierarchy from R-7 is preserved; only the top tier changes.
- `specs/006-learning-hardening-followups/research.md` R-A: feature 007 retires `SandboxKind::ProcessOnly` (introduced here in 006-A) — broken on the same userns-restricted hosts as bwrap; theatrical. The honest-tag classifier `sandbox_tag_is_fs_confined` and the RunHistory disclosure UI from 006-A are **built upon and kept**, not undone.
