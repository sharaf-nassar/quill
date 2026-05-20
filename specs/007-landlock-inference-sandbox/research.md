# Phase 0 Research — Landlock Inference Sandbox

Design-decision record for feature 007. No `NEEDS CLARIFICATION` carried from the spec (architectural call was settled during scoping).

## R-A — Landlock ABI target

**Decision**: declare `ABI::V3` (read + write + truncate access rights) with `CompatLevel::BestEffort`. The crate auto-downgrades available access rights based on what the running kernel supports.

**Rationale**: Linux 5.13+ has at least ABI v1 (read/write). Ubuntu 22.04 LTS (5.15) is at v1; Ubuntu 24.04 (6.8) is at v5; ABI v6 is current (Mar 2026). Pinning latest excludes LTS users; pinning lowest wastes capability on modern kernels. Best-effort matches feature 006-A's "degrade and record honestly" philosophy at the ABI layer.

**Alternatives**: pin v1 (rejected — wastes truncate-deny on every modern kernel); pin v6 (rejected — fail-closed on 22.04 LTS and any kernel below 6.7).

## R-B — How to apply Landlock before invoking `claude`

**Decision**: build the `RulesetCreated` value pre-fork in the parent (allocations + `PathFd` opens allowed). On the `tokio::process::Command`'s underlying `std::process::Command`, install the `unsafe` pre-spawn hook closure via `CommandExt::pre_exec`. The closure runs in the forked child between `fork` and the launch of `claude`. Inside the closure: `prctl(PR_SET_NO_NEW_PRIVS, 1, ...)` then `ruleset.restrict_self()`. Both syscalls; both async-signal-safe.

**Rationale**: canonical Unix pattern; matches the rust-landlock `sandboxer` example; in-process (no helper binary); the ruleset binds the child + everything it forks via launch-inheritance under the no-new-privs flag. The pre-fork build avoids the multi-threaded-parent-post-fork allocation hazard.

**Alternatives**: restrict the parent (hard no — would break Quill itself); add a `quill-sandbox-launcher` helper binary (rejected — new shipped binary, packaging complexity, loses the in-process advantage).

## R-C — Ruleset construction & path policy

**Decision**: a `LandlockPolicy` value-object encapsulates the RO set (`/usr`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/etc`, `/opt`, `/nix` if present, plus the resolved `claude_install_root`), the RW path (per-call `TempDir` only), and the ABI choice. A private `build_ruleset(policy)` returning `Result<RulesetCreated, _>` constructs it via the crate's `path_beneath_rules` helper. Absent optional paths are silently skipped (not an error).

**Rationale**: mirrors bwrap's effective FS rules — same RO/RW intent, expressed as Landlock rules instead of bind mounts. Skip-if-absent matches the bwrap arm's `--ro-bind-try` behavior. The value object isolates the *what* (policy) from the *how* (kernel ruleset), making the construction half unit-testable without ever calling `restrict_self`.

**Out of scope**: ABI v4 network rules (api.anthropic.com:443 allowlist) — feature-008 candidate.

## R-D — Three-tier fallback chain

**Decision**: Linux chain is `Landlock → Bwrap → None`. Detection-time probe in `detect_sandbox_kind`: try Landlock (build a tiny ruleset; discard); if supported → `Landlock`. Else probe `bwrap` on PATH; if present → `Bwrap`. Else → `None`. The chain is cached process-wide (one probe per process). When bwrap is the active mechanism and its invocation fails at spawn time, parse the stderr (R-E), set a process-wide latch (`OnceLock<BwrapBrokenCause>`), and emit the diagnostic (R-F). Subsequent calls in the same process see the latch and use `None` directly — no retry of the known-broken bwrap.

**Rationale**: bwrap still works on a meaningful population of hosts (Debian sid, Fedora without policy restrictions, Ubuntu hosts with the `bwrap-userns-restrict` profile installed). Keeping it as fallback preserves FS confinement for that audience; the latch prevents re-spawning a known-broken mechanism (the run-49 failure mode had streams A+B+C all paying the bwrap spawn cost on the same broken host).

**Alternatives**: two-tier Landlock → None (rejected — gives up FS confinement on hosts where bwrap works); three-tier with retry on bwrap failure (rejected — silently spawning the same known-broken bwrap on every subsequent call wastes time and produces noise without changing the outcome).

## R-E — Diagnostic classifier (test-friendly, no kernel involved)

**Decision**: a pure private fn `classify_bwrap_failure(stderr: &str) -> BwrapBrokenCause` where the enum is `AppArmorRestrictUserns | Other`. Substring match on two known signatures:

- `"setting up uid map: Permission denied"` (the dev-host signature, run-49 evidence)
- `"loopback: Failed RTM_NEWADDR: Operation not permitted"` (the Codex-blog signature; tolerated even though our current bwrap args don't `--unshare-net`, because bwrap may emit it under variants)

→ both classify as `AppArmorRestrictUserns`. Anything else → `Other`.

**Rationale**: pure function, no `#[serial]`, unit-tested with captured real strings. The classifier is the only piece that needs to be "right" about kernel/AppArmor signatures; everything else just routes on the classification.

**Alternatives**: kernel-state probe (`sysctl kernel.apparmor_restrict_unprivileged_userns`) — rejected as the primary signal because stderr matching is what the failure path *actually* gave us, doesn't need a sysctl read, and is deterministic-testable.

## R-F — Diagnostic emission policy

**Decision**: two surfaces (`log::error!` for the tauri-dev terminal; the per-call log channel that ends up in `learning_runs.logs` for run-history detail), one-shot per Quill process via `OnceLock<()>`. Two messages keyed on cause: generic FR-014 template when both mechanisms are unavailable at detection, AppArmor-specific FR-015 template when bwrap is invocation-blocked by AppArmor. Message bodies live in `contracts/landlock-sandbox.md` so unit tests can string-match a stable substring.

**Rationale**: actionable — the operator sees what to install and how, not a mystery degradation. One-shot — multi-stream runs don't spam (stream A trips the latch, streams B+C are silent on the diagnostic). Stable substring — tests don't break on prose tweaks.

## Cross-cutting decisions

- **Verification surface**: FR-021 CI-gated learning surface. Existing test harness (TempDir/`#[serial]` + the `cc_client` `InferenceDoubleGuard` offline double) extended; no live kernel-confinement calls in any test.
- **Parallelization**: NONE this feature. The work is concentrated in `cc_client.rs` with shared latch state; a single subagent drives, and the integrated baseline is authoritative.
- **No fail-closed**: every error path falls through to `SandboxKind::None` + unwrapped spawn + honest disclosure + diagnostic.
- **No DB migration**: the persisted `sandbox` `Option<String>` is forward-compatible; existing rows decode forever via `sandbox_tag_is_fs_confined` (a `"landlock"`→true case is added; everything else stays).
- **No new crate beyond `landlock` 0.4.4**: explicitly approved during scoping.
