# Feature Specification: Landlock Inference Sandbox

**Feature Branch**: `007-landlock-inference-sandbox`
**Created**: 2026-05-19
**Status**: Draft
**Input**: Promote the in-kernel Landlock LSM to the **primary** Linux inference sandbox; **keep `bwrap` as a fallback** (when Landlock is unsupported and `bwrap` can actually spawn); remove the just-shipped (feature 006) `ProcessOnly` tier (theatrical — broken on the same userns-restricted hosts as bwrap, with no FS-confinement value either way). Linux confinement becomes a three-tier chain: `Landlock` → `Bwrap` → `None`, with feature 006-A's honest disclosure surfacing whichever applied. When the chain falls through to `None`, the operator gets an actionable diagnostic explaining how to restore confinement — and a *different*, more specific diagnostic when AppArmor's `restrict_unprivileged_userns` is the detected cause of `bwrap` failing. macOS (`sandbox-exec`) and Windows (`JobObject`) are untouched.

## Overview

A real learning run on Ubuntu 24.04 just failed end-to-end (run id 49, 2026-05-20 00:17:13) at the spawn step on every stream with stderr `bwrap: setting up uid map: Permission denied`. Investigation showed the failure was *not* in our code (`cc_client::apply_sandbox`'s Bwrap arm is byte-for-byte unchanged from main); it was the **host** — Ubuntu 23.10+ ships `kernel.apparmor_restrict_unprivileged_userns=1` by default and `/usr/bin/bwrap` has no AppArmor profile in stock 24.04. The kernel refuses the unprivileged user-namespace creation `bwrap` needs. The fallback tier (`ProcessOnly`, just shipped in feature 006) fails identically on the same host because its `unshare(2)` wrapper requires the exact same `CLONE_NEWUSER` capability AppArmor blocks. The current fallback chain is broken end-to-end on default Ubuntu 24.04+, which is a large fraction of our audience.

Ecosystem signals say the answer in 2026 is **Linux's Landlock LSM** — an in-process, kernel-enforced, no-user-namespaces-required FS-deny primitive that bypasses the AppArmor restriction class by construction. Codex CLI 0.117.0 has already moved this way. Anthropic's own `anthropic-experimental/sandbox-runtime` still uses bubblewrap and has open Issue #74 about this exact problem, with no upstream fix. Promoting Landlock to the primary slot makes the Linux sandbox work out of the box on a current default Ubuntu host; **keeping bwrap as a fallback** preserves FS confinement on hosts where bwrap still works (kernel < 5.13, or Ubuntu with the AppArmor profile installed, or other distros without the restriction). When **both** fail, the operator gets an actionable error pointing at the specific cause (AppArmor-blocked vs. neither-installed), not a silent degradation. The retired `ProcessOnly` tier added vocabulary without adding a reliably-applicable mechanism (it relied on the same userns capability AppArmor blocks), so it goes.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Learning runs complete on a default modern Linux host (Priority: P1)

As the operator running Quill on a stock modern Linux host (Ubuntu 22.04+ / 24.04+, current Fedora, etc.), I need learning runs to complete with real filesystem confinement applied, without me having to sudo-install AppArmor profiles, flip sysctl knobs, or otherwise touch system policy. The privacy guarantee of the loop (untrusted captured content can't read my home/credentials/project trees) must hold by default, not as something I unlock manually.

**Why this priority**: The defect is total: today on default Ubuntu 24.04 every learning run fails to spawn with no actionable signal in the UI. Restoring a working FS-confined loop on the most common Linux dev host is the headline value; everything else in this feature follows from it.

**Independent Test**: On a stock Ubuntu 24.04 host with `kernel.apparmor_restrict_unprivileged_userns=1` (default) and no special profiles installed, trigger a learning run. It completes, produces findings, and the per-run record shows the confinement state as "filesystem-confined" with no manual remediation involved.

**Acceptance Scenarios**:

1. **Given** a default modern Linux host with the OS hardening defaults that currently break the loop, **When** a learning run is triggered, **Then** the run completes; every stream's inference call spawned successfully; the recorded per-call confinement state on those calls is "filesystem-confined" and the run-history UI does not show the remediation marker.
2. **Given** that confined run completed, **When** the spawned analysis process attempted to read out of its per-call workspace (a deliberate negative test path), **Then** the kernel denied the access — the FS-confinement guarantee from feature 005 SC-013 / FR-005 is materially preserved, not just labeled.
3. **Given** a Linux kernel old enough that the new primitive is unsupported, **When** a learning run is triggered, **Then** the run still completes (never fail-closed) and the per-run state is recorded as not-filesystem-confined with feature 006-A's existing UI disclosure.

---

### User Story 2 - When both mechanisms fail, operator gets the right error (Priority: P2)

As the operator on a host where **both** Linux confinement mechanisms (the primary and the fallback) cannot be applied, I still need the learning loop to run, and the system must tell me — in language I can act on — *why* there is no confinement and *what specifically I can do* to restore it. A generic "no confinement" message is not enough when the cause is a known, fixable OS policy. The diagnostic must distinguish "neither mechanism is installed/supported" from "AppArmor's restrict-unprivileged-userns policy is blocking the fallback," because the remediations are different.

**Why this priority**: "Never fail-closed" is the feature 005 R-7 invariant; this story is what preserves it through the primitive swap. The disclosure machinery already exists from feature 006-A; this story is what makes it *actionable* — turning a silent degradation into something an operator can fix in five minutes, not a mystery they have to dig through SQLite to understand (the very situation the bwrap incident put us in).

**Independent Test**: On a host where the primary mechanism is unavailable AND the fallback is unavailable or invocation-blocked, a learning run completes (no fail-closed), the per-run confinement state is recorded as not-filesystem-confined, the run-history UI renders the existing not-FS-confined marker (feature 006-A), AND a clear actionable diagnostic message appears in the run logs and the application's standard error stream. The diagnostic text differentiates between the two distinct causes.

**Acceptance Scenarios**:

1. **Given** neither Linux mechanism is available (primary unsupported by the kernel AND the fallback binary is not installed), **When** a learning run executes, **Then** the run completes successfully (no fail-closed), the recorded state does NOT claim filesystem confinement, and the operator sees a "to get filesystem confinement, install either a kernel that supports the primary mechanism, OR install the fallback binary" diagnostic in the run log and stderr.
2. **Given** the primary mechanism is unsupported AND the fallback binary is installed but its invocation is blocked by AppArmor's unprivileged-user-namespace restriction (the precise scenario that motivated this feature), **When** a learning run executes, **Then** the diagnostic is specifically about the AppArmor restriction and includes the exact remediation (e.g. installing the AppArmor profile that grants `userns,` to the fallback binary, or upgrading the kernel to one that supports the primary mechanism).
3. **Given** the diagnostic has been emitted once per process, **When** further calls in the same process attempt confinement, **Then** the system does not re-spam the diagnostic and does not waste time retrying a known-broken mechanism — subsequent calls skip directly to unconfined-and-disclosed.
4. **Given** any run on any platform, on success or failure, **When** the run record is written, **Then** a confinement tag is recorded (SC-013 100%-recorded invariant preserved).

---

### User Story 3 - Historical run records keep displaying correctly (Priority: P3)

As an operator or auditor reviewing past runs, the per-call confinement state recorded by earlier versions of Quill (feature 005 wrote `bwrap`/`sandbox-exec`/`job-object`/`none`; feature 006 also wrote `process-only`; even pre-feature-006 records may carry `unshare`) must continue to decode and display correctly forever. The change of write vocabulary must not orphan existing history.

**Why this priority**: Quietly losing fidelity on existing rows would erode trust in the audit trail and is preventable.

**Independent Test**: Seed run records whose `inference_metadata` carries each of `bwrap`, `sandbox-exec`, `process-only`, `unshare`, `job-object`, `none`, and a deliberately-unknown future tag. All decode without error, the raw tag is preserved verbatim, and the per-call "filesystem-confined" classification matches what the tag historically meant.

**Acceptance Scenarios**:

1. **Given** an existing run record with `sandbox: "bwrap"` (written by feature 005), **When** it is decoded for run history, **Then** the run is classified as filesystem-confined (because that historical run actually was), and the raw tag is preserved verbatim for audit.
2. **Given** existing records with `sandbox: "process-only"` or `sandbox: "unshare"`, **When** decoded, **Then** the calls are classified as not-filesystem-confined and rendered with the existing disclosure marker (no new behavior needed — feature 006-A's classifier already covers this conservatively).
3. **Given** a future unknown tag appearing in a record, **When** decoded, **Then** no decode error occurs, the raw tag is preserved verbatim, and the conservative classification is not-filesystem-confined.

---

### Edge Cases

- Kernel does not support the new primitive at all → run completes unconfined; tag recorded honestly; UI shows disclosure.
- The new primitive's setup errors mid-flight (e.g. a path-resolution failure for a system directory) → never fail-closed; the spawned process either runs with the partial confinement that did succeed (recorded as such) or runs unconfined and is tagged honestly. The integrated test surface covers this without requiring a real kernel error.
- Mixed historical decode: a single run's `inference_metadata` carrying calls with different older tags → each call's classification is independent; the run-level rollup applies the existing AND-fold.
- macOS host: behavior unchanged from feature 006 (`sandbox-exec` keeps working; the recorded tag stays `sandbox-exec`).
- Windows host: behavior unchanged (`job-object` keeps being recorded).
- A run that fails before the analysis process is spawned: a confinement tag is still recorded (SC-013) reflecting what would have been applied on this host.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A learning run on a default modern Linux host (no manual OS-level configuration) MUST complete with real filesystem confinement applied, restoring the privacy guarantee feature 005 FR-005 specified.
- **FR-002**: The system MUST NOT require any per-user manual OS configuration (sudo, AppArmor profile install, sysctl flip, package install beyond the standard Quill install) to obtain filesystem confinement on a default modern Linux host.
- **FR-003**: When filesystem confinement cannot be applied (older kernel, primitive errored at setup, or any other reason), the run MUST still complete (never fail-closed) and the lack of confinement MUST be honestly recorded and surfaced in run history (preserves feature 005 R-7 / feature 006-A behavior).
- **FR-004**: A confinement tag MUST be recorded for 100% of analysis runs on every platform, on both success and failure paths (preserves feature 005 SC-013).
- **FR-005**: The recorded confinement vocabulary MUST distinguish "filesystem-confined" from "not filesystem-confined", so the recorded state never implies stronger protection than was actually applied (preserves feature 006-A FR-001).
- **FR-006**: macOS confinement behavior MUST stay unchanged from feature 006 (`sandbox-exec`).
- **FR-007**: Windows confinement behavior MUST stay unchanged from feature 006 (`job-object`).
- **FR-008**: Existing recorded `sandbox` tags written by prior versions (including but not limited to `bwrap`, `process-only`, `unshare`, `sandbox-exec`, `job-object`, `none`) MUST continue to decode without error, preserve the raw tag verbatim for audit, and classify with the correct historical filesystem-confinement meaning (`bwrap`/`sandbox-exec` historically were FS-confined; the others were not).
- **FR-009**: An unknown future tag in a decoded record MUST classify conservatively as not-filesystem-confined and MUST NOT cause a decode error.
- **FR-010**: The change MUST NOT introduce a database schema migration (the persisted `sandbox` field is an additive string already; only its write vocabulary changes).
- **FR-011**: The run-history UI disclosure (marker + remediation hint for not-filesystem-confined runs) MUST keep working unchanged when the new mechanism is in play (no UI behavior regression from feature 006-A).
- **FR-012**: New behavior MUST be covered by deterministic automated tests on the CI-gated learning-logic surface using the existing temp-database / serialized-execution / offline scripted-inference test harness; no test may require a live external analysis process, network, or actually-applied kernel-level confinement on the test process itself.
- **FR-013**: The project knowledge base (`lat.md/`) MUST be updated to reflect the new primary mechanism, the kept fallback, and the removal of the retired `ProcessOnly` tier; link validation MUST pass.
- **FR-014**: When neither Linux confinement mechanism can be applied (primary unsupported AND fallback unavailable/blocked), the system MUST emit a clear, actionable diagnostic explaining that filesystem confinement is unavailable and naming the two remediations (kernel support for the primary mechanism, OR install/repair the fallback binary). The diagnostic MUST be visible to the operator in (a) the per-run log surfaced in run history detail and (b) the application's standard error stream.
- **FR-015**: When the specific cause of the fallback's failure is AppArmor's restriction on unprivileged user namespaces (detectable from the fallback's error output), the diagnostic emitted under FR-014 MUST be *specifically* about that cause and MUST include the concrete remediation (the AppArmor profile path/contents needed to grant `userns,` to the fallback binary), not the generic FR-014 message.
- **FR-016**: The diagnostic emitted under FR-014/FR-015 MUST be one-shot per application process — emitted once on first detection, suppressed on subsequent calls — to avoid log spam, AND once a Linux fallback mechanism has been observed to be invocation-blocked on this host, subsequent confinement attempts within this process MUST skip directly to unconfined-and-disclosed rather than re-spawning the known-broken mechanism (preserves the latency profile of the learning run).

### Key Entities *(include if feature involves data)*

- **Run Confinement Record**: Unchanged shape from feature 006 (`{ sandbox: string, fs_confined: bool }` per call; an `all_fs_confined` AND-fold per run). The write vocabulary for the `sandbox` string contracts (no more `bwrap` or `process-only` from new code); the decode vocabulary stays open and forward-compatible.
- **Inference Sandbox Mechanism**: The closed set of OS-level confinement mechanisms the system records. On Linux, contracts from feature 006's set to a smaller set with one new entry replacing the two retired ones. macOS and Windows entries unchanged.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On a default modern Linux host that today fails 100% of learning runs at spawn due to OS hardening, after this feature 100% of learning runs (across the deterministic test matrix that reproduces this host config) complete with filesystem confinement applied.
- **SC-002**: Across the deterministic test matrix, 0% of analysis runs are recorded or displayed with a state that implies filesystem confinement when it was not applied (preserves feature 006-A SC-001).
- **SC-003**: 100% of analysis runs (success and failure paths, every platform) carry a recorded confinement tag (feature 005 SC-013 preserved and still passes its existing test).
- **SC-004**: Learning is never disabled for lack of OS confinement — 0 fail-closed events in the test matrix; the loop completes on a host where the new primitive is unavailable.
- **SC-005**: For 100% of historical run records carrying any prior-version `sandbox` tag (including `bwrap`, `process-only`, `unshare`, and any unknown future tag), the decode path returns without error, preserves the raw tag verbatim, and classifies the call correctly per the historical meaning of that tag.
- **SC-006**: No new database migration is introduced; the existing schema version is unchanged; the existing schema-version test still holds.
- **SC-007**: The full integrated baseline still passes with zero new warnings: format check, a forced clean lint with warnings denied, the library test suite, the frontend build, and the knowledge-base link check.
- **SC-008**: The default Linux install footprint of the application does not grow by any new operator-installable tool, OS profile, system-policy package, or sudo'able artifact (the change is in-app only).
- **SC-009**: Across the deterministic test matrix that simulates both Linux Linux failure modes (primary unsupported + fallback unavailable; primary unsupported + fallback present-but-AppArmor-blocked), 100% of test runs produce the *correct* actionable diagnostic in the run log and stderr — generic FR-014 message for the first scenario, AppArmor-specific FR-015 message for the second. 0% emit the wrong diagnostic, and 0% silently degrade without any diagnostic at all.

## Assumptions

- **Architectural decision is settled.** The technical decision is: Landlock becomes the **primary** Linux mechanism; `bwrap` is **kept as a fallback** (still useful on kernels/distros where it works); `ProcessOnly` is **retired** (theatrical — broken on the same userns-restricted hosts as bwrap, with no FS-confinement value either way). The chain is Landlock → Bwrap → None with an actionable diagnostic at the falls-through-to-None step. The spec records this as a constraint, not an open question.
- **Platform scope.** Implementation and verification target the Linux development host. The macOS configuration path cannot be compiled here and is explicitly out of scope; Windows is out of scope. Both stay byte-for-byte unchanged.
- **One new vetted dependency** (the in-kernel-LSM Rust binding by the kernel feature's author) was approved during scoping. No other dependency additions are in scope.
- **Never fail-closed** (feature 005 R-7 / feature 006 FR-004) remains the binding invariant; this feature preserves it through the primitive swap.
- **No database migration.** The persisted `sandbox` field is an additive string with tolerant decode (feature 005 / 006); only the write vocabulary changes.
- **Disclosure UI from feature 006-A is reused as-is.** No new operator-visible UI surface is built; the existing marker + remediation hint + per-call detail render unchanged and automatically surface the new mechanism's outcomes.
- **Test harness reused.** New tests use the existing temp-database + serialized-execution harness and the existing offline scripted-inference double (with its RAII teardown guard) for the inference path. No live analysis process or network in tests, and no test may apply kernel-level confinement to the test process itself.
- **Deferred for future features (not this one)**:
  - A network allowlist (deny outbound except the model-API endpoint) using the primitive's network-restriction capability. Logical feature-008 candidate.
  - A separately-recorded process-namespace tier (PID/IPC/UTS) without the user-namespace dependency. Logical future feature; today this feature consolidates to two tiers.
  - Frontend test infrastructure (still the same deferred decision from feature 006 T012).
- **Combined-or-separate.** This is a single feature; design options, recommendation, ABI handling details, test strategy, lat.md sync points, and the dependency-ordered task list belong in `/speckit-plan` and `/speckit-tasks` (same flow as feature 006).
- **Implementation is approved before any production code lands.** The plan and task list are presented for approval; no code lands until the user gives the go-ahead, parallel subagents drive disjoint-file tracks, the integrated 0-warning baseline is the authoritative gate, and one squashed conventional commit lands on the feature branch.
