# Contract — Landlock Sandbox & Actionable Diagnostics

Internal contract for feature 007. No external API/IPC surface changes; the only user-facing surface change is the diagnostic emitted to `log::error!` and to the per-run log column when filesystem confinement is unavailable.

## C-A — Confinement chain (Linux)

- **C-A1**: `detect_sandbox_kind` on Linux probes mechanisms in this order — Landlock (cheap in-process ruleset-create probe) → bwrap (PATH probe) → None. The first success wins. The result is cached process-wide.
- **C-A2**: `SandboxKind::as_str` writes only members of the closed vocabulary `{"landlock", "bwrap", "sandbox-exec", "job-object", "none"}`. The retired `"process-only"` write is removed; reads of historical `"process-only"` / `"unshare"` rows continue to decode tolerantly (C-D).
- **C-A3**: `sandbox_tag_is_fs_confined` returns `true` for `{"landlock", "bwrap", "sandbox-exec"}` and `false` for `{"job-object", "none"}` and any unknown tag. The string classifier is the single source of truth.
- **C-A4**: A confinement tag is recorded for 100% of analysis runs, success and failure paths, every platform (feature 005 SC-013 preserved).
- **C-A5**: macOS (`sandbox-exec`) and Windows (`JobObject`) confinement paths are byte-for-byte unchanged from feature 006.

## C-B — Landlock application

- **C-B1**: Landlock policy is `ABI::V3` declared with `CompatLevel::BestEffort` so the access-rights set degrades cleanly on older Landlock-capable kernels (≥5.13). If the kernel doesn't support Landlock at all, the detection probe fails and the chain advances to bwrap.
- **C-B2**: The `RulesetCreated` value is constructed pre-fork in the parent (allocations + `PathFd` opens allowed). The `restrict_self` call happens in the forked child via the `pre_exec` hook, after `prctl(PR_SET_NO_NEW_PRIVS, 1, …)`.
- **C-B3**: The default Landlock policy grants RO `path_beneath` rights to `{/usr, /bin, /sbin, /lib, /lib32, /lib64, /etc, /opt, /nix}` (silently skipping any path that does not exist) plus the resolved `claude_install_root`; and RW `path_beneath` rights to *exactly* the per-call `TempDir`. Anything not listed is deny-by-default.
- **C-B4**: No Landlock network rules are added; outbound network is fully preserved (matches the existing "the CLI makes the model API call itself" requirement). The ABI v4 outbound TCP allowlist is out of scope (feature-008 candidate).
- **C-B5**: If `build_ruleset` returns `Err` for any reason, the call falls through to the bwrap fallback (or `None` if bwrap is unavailable). Never fail-closed.

## C-C — Bwrap fallback (kept; no behavior change inside the wrapper)

- **C-C1**: The existing bwrap arm in `apply_sandbox` is preserved unchanged in its argument construction (RO-binds, RW-bind, namespaces, network preserved). Only the *position* in the chain changes (from primary to first fallback after Landlock).
- **C-C2**: A bwrap invocation that fails at spawn time has its stderr classified by `classify_bwrap_failure(&str) -> BwrapBrokenCause` and the result latched in a process-wide `OnceLock<BwrapBrokenCause>`. Subsequent calls in this process skip bwrap entirely and treat the host as `SandboxKind::None`.

## C-D — Decode forward-compatibility (deploy safety)

- **C-D1**: Any persisted `sandbox` string — including the retired `"process-only"`, the pre-feature-006 `"unshare"`, and any unknown future tag — MUST decode without error.
- **C-D2**: The raw persisted tag MUST be preserved verbatim in the decoded `RunInferenceConfinement.sandbox` field for audit fidelity.
- **C-D3**: The `fs_confined` classification reflects what the recorded tag *historically meant* — `"bwrap"` and `"sandbox-exec"` are FS-confined (because those historical runs actually were); `"process-only"`, `"unshare"`, `"job-object"`, `"none"`, and any unknown tag are not. Feature 006-A's `sandbox_tag_is_fs_confined` is extended only to add `"landlock"`→true; the conservative-default behavior for unknown tags is unchanged.

## C-E — Actionable diagnostic

- **C-E1**: The classifier `classify_bwrap_failure(&str) -> BwrapBrokenCause` returns `AppArmorRestrictUserns` iff the stderr substring-matches *any* of the known signatures:
  - `setting up uid map: Permission denied`
  - `loopback: Failed RTM_NEWADDR: Operation not permitted`
  - Otherwise returns `Other`.
- **C-E2**: When the Linux confinement chain falls through to `None`, the system emits **exactly one** diagnostic per Quill process (guarded by `OnceLock<()>`), to two surfaces simultaneously:
  - `log::error!` (visible in the `tauri dev` terminal).
  - The per-call log channel that lands in `learning_runs.logs` (visible in run-history detail).
- **C-E3**: The diagnostic message uses **one of two templates** keyed on the cause:
  - **Generic (FR-014)** — when both mechanisms unavailable at detection (Landlock unsupported AND bwrap not on PATH), or when bwrap failed at invocation with `BwrapBrokenCause::Other`. Stable substring (testable): `Filesystem confinement is unavailable on this host.`
  - **AppArmor-specific (FR-015)** — when bwrap failed at invocation with `BwrapBrokenCause::AppArmorRestrictUserns`. Stable substring (testable): `AppArmor's \`restrict_unprivileged_userns\` policy is blocking bubblewrap`. The full template includes the concrete remediation (install the `bwrap-userns-restrict` AppArmor profile, or upgrade to a Linux ≥5.13 kernel for Landlock to take over, or — explicitly marked not-recommended — `sysctl kernel.apparmor_restrict_unprivileged_userns=0`).
- **C-E4**: Once a `BwrapBrokenCause` has been latched on this host, subsequent calls **must not** re-attempt the bwrap spawn — they go straight to the unwrapped command and record `sandbox: "none"`.

## Verification

All four contract surfaces (C-A..C-E) are verified by deterministic unit tests on the FR-021 CI-gated learning surface — the existing `TempDir`/`#[serial]` harness for storage, the `cc_client` `#[cfg(test)]` `InferenceDoubleGuard` for inference, and pure-function unit tests for the classifier/diagnostic-latch logic — plus the integrated 0-warning baseline and a manual quickstart V-acceptance on this Ubuntu 24.04 host (where the AppArmor-restricted-bwrap scenario is naturally reproduced today).
