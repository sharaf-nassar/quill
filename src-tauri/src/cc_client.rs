//! Claude Code subprocess invocation surface.
//!
//! Replaces the direct Anthropic API path that previously lived in
//! `ai_client.rs`. Every inference call (learning streams + synthesis,
//! memory optimizer, prose compression) goes through [`invoke_typed`] or
//! [`invoke_text`], which spawn the `claude` CLI in headless mode with
//! `-p --output-format json` and the isolation flags documented in
//! `specs/003-cc-inference-migration/research.md` (R-5, R-6, R-14).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Per-type JSON Schema cache. `schemars::schema_for!(T)` plus
/// serialization is pure and deterministic for a given `T`, so it is
/// computed once per concrete type and reused. Keyed by
/// `std::any::type_name::<T>()`, which is a stable `&'static str` for
/// the program's lifetime.
static SCHEMA_CACHE: LazyLock<Mutex<HashMap<&'static str, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Hard per-call timeout. Acts as a hang detector around a single
/// one-shot Claude Code invocation. Comfortably accommodates the
/// largest realistic Sonnet response; no app-side retry — if a call
/// exceeds this, the run fails.
///
/// See FR-009 and the clarification recorded in spec 003.
const INVOCATION_TIMEOUT: Duration = Duration::from_secs(300);

/// Logical phase tag attached to each invocation's metadata so the
/// per-call entries inside a run's `inference_metadata` array can be
/// attributed to a specific call site.
///
/// `StreamC` is the active Quill-native session-insights path
/// (`learning::analyze_sessions_stream`): it derives signal from Quill's
/// own local session index and extracts via this client like Stream A/B,
/// so `stream_c` entries now appear in the metadata array. (Feature 004
/// replaced the former `claude /insights --print` subprocess.)
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    StreamA,
    StreamB,
    StreamC,
    Synthesis,
    MemoryOptimizer,
    ProseCompression,
}

impl Phase {
    /// Stable string tag persisted on the metadata record.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::StreamA => "stream_a",
            Phase::StreamB => "stream_b",
            Phase::StreamC => "stream_c",
            Phase::Synthesis => "synthesis",
            Phase::MemoryOptimizer => "memory_optimizer",
            Phase::ProseCompression => "prose_compression",
        }
    }
}

/// Model selection alias understood by `claude --model`.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // `Sonnet`/`Haiku` retained for easy revert; the pipeline is single-model Sonnet46 (extraction + synthesis)
pub enum Model {
    Haiku,
    Sonnet,
    /// Sonnet 4.6 pinned by full model name (not the rolling `sonnet`
    /// alias). Used for all extraction streams and the memory optimizer.
    Sonnet46,
}

impl Model {
    fn alias(self) -> &'static str {
        match self {
            Model::Haiku => "haiku",
            Model::Sonnet => "sonnet",
            Model::Sonnet46 => "claude-sonnet-4-6",
        }
    }
}

// ---------------------------------------------------------------------------
// OS-level confinement for the spawned `claude` CLI (H-5 / FR-005 / SC-013).
//
// The headless `claude` agent processes untrusted captured content (session
// transcripts, git data). Flag isolation (`--allowedTools`/`--add-dir`/env
// scrub) is in-process to the agent and can be subverted; only an OS boundary
// is a real control. R-7.6 (committed): wrap the child with the best-available
// host mechanism, RW-carving out EXACTLY the per-call temp dir, NETWORK
// PRESERVED (the CLI makes the model API call itself), graceful degradation —
// NEVER fail closed. The applied mechanism is recorded on
// `InferenceCallMetadata.sandbox` so SC-013 is verifiable on every platform.
// ---------------------------------------------------------------------------

/// Confinement mechanism actually applied to a single `claude` spawn. The
/// stable lowercase string (see [`SandboxKind::as_str`]) is what gets
/// persisted on `InferenceCallMetadata.sandbox` and asserted by SC-013. The
/// closed write vocabulary is `{landlock, bwrap, sandbox-exec, job-object,
/// none}` (feature 007: Landlock is the primary Linux mechanism in front of
/// the bwrap fallback; the feature-006-A `ProcessOnly` tier is retired).
// Per-platform variants are each constructed only under their own
// `#[cfg(target_os=...)]` arm in `detect_sandbox_kind`/`apply_sandbox`, so on
// any single target the others read as dead. They are all live across the
// supported platforms and all reachable via `as_str` (the persisted/serialized
// form), mirroring the `#[allow(dead_code)]` the `Model` enum already uses for
// platform/revert-retained variants.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxKind {
    /// Linux: in-process Landlock LSM ruleset (kernel ≥5.13). Deny-by-default
    /// out-of-workspace filesystem with RO `path_beneath` rules for the
    /// system tree + resolved `claude_install_root`, and a single RW
    /// `path_beneath` for EXACTLY the per-call temp dir. Network preserved.
    /// Applied via a forked-child `pre_exec` hook
    /// (`prctl(PR_SET_NO_NEW_PRIVS)` + `ruleset.restrict_self()`) — no helper
    /// binary, no user namespace, no AppArmor entanglement. Feature 007
    /// (R-A..R-D / C-A1 / C-B1..C-B3).
    Landlock,
    /// Linux: `bwrap` (bubblewrap) — FS/IPC/PID namespace, deny-by-default
    /// filesystem with a single RW bind of the per-call temp dir, network
    /// preserved. Kept as the first fallback after Landlock for hosts where
    /// Landlock is unsupported but bwrap is unblocked.
    Bwrap,
    /// macOS: `sandbox-exec` with a deny-by-default SBPL profile (exec of the
    /// resolved binary + system reads, file r/w only under the temp dir,
    /// `network-outbound` allowed).
    SandboxExec,
    /// Windows: process is constrained by the existing `kill_on_drop` Job
    /// Object association (partial; documented best-effort).
    JobObject,
    /// No usable OS confinement on this host — the process runs with the
    /// existing flag isolation only. Recorded/disclosed; NEVER fails closed.
    None,
}

impl SandboxKind {
    /// Stable string tag persisted on `InferenceCallMetadata.sandbox`. One of
    /// the closed write vocabulary `{landlock, bwrap, sandbox-exec,
    /// job-object, none}` (feature 007 C-A2). The tag is honest about the
    /// actual boundary; the decode classifier
    /// [`sandbox_tag_is_fs_confined`] is the single source of truth for
    /// whether a recorded tag denotes real out-of-workspace FS confinement.
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxKind::Landlock => "landlock",
            SandboxKind::Bwrap => "bwrap",
            SandboxKind::SandboxExec => "sandbox-exec",
            SandboxKind::JobObject => "job-object",
            SandboxKind::None => "none",
        }
    }
}

/// Whether a recorded `sandbox` tag denotes real out-of-workspace
/// filesystem confinement. Single source of truth for the FS-confinement
/// classification, keyed on the stable [`SandboxKind::as_str`] tag (the
/// value is always transported and persisted as that string — the JSON
/// `inference_metadata`, the decode path, the UI). Only `landlock`
/// (in-process LSM, deny-by-default `path_beneath`), `bwrap`
/// (deny-by-default binds), and `sandbox-exec` (deny-by-default SBPL)
/// actually deny out-of-workspace filesystem R/W; `job-object` (process-kill
/// only), `none`, the retired feature-006-A `process-only` tag, the legacy
/// pre-feature-006 `unshare` tag, and any unknown future tag are NOT
/// filesystem-confined. Drives the run-history confinement disclosure.
/// Feature 006 Follow-up A (R-A / C-A2) extended by feature 007 C-A3.
pub(crate) fn sandbox_tag_is_fs_confined(tag: &str) -> bool {
    matches!(tag, "landlock" | "bwrap" | "sandbox-exec")
}

/// Resolve a helper binary (`bwrap`, `sandbox-exec`) on the inherited `PATH`
/// plus the conventional absolute locations. Mirrors the
/// `std::env::split_paths` resolution config.rs already uses; no new crate.
/// `sandbox-exec` lives at a fixed system path on macOS and may not be on a
/// minimal `PATH`, so the absolute fallbacks matter.
#[cfg(unix)]
fn resolve_on_path(binary: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(binary);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for base in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let candidate = Path::new(base).join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Landlock policy + ruleset construction (feature 007 R-A..R-C, C-B1..C-B3).
//
// Linux-only. The policy is a pure data value (no syscalls in the
// constructor); `build_ruleset` opens `PathFd`s and creates the kernel
// `RulesetCreated` value pre-fork in the parent. The actual
// `restrict_self` call lives in the forked child via a `pre_exec` hook
// installed in `apply_sandbox`'s Landlock arm.
// ---------------------------------------------------------------------------

/// Default read-only path set for the Landlock policy. Mirrors the bwrap
/// arm's `--ro-bind-try` set, **plus** the host's `/proc`, `/sys`, and `/dev`
/// — Landlock has no namespace (unlike bwrap's `--proc`/`--dev`/`--tmpfs`
/// which mount fresh pseudo-filesystems), so it can only grant or deny
/// access to the real host pseudo-fs. The launcher's Bun runtime SIGILLs
/// on `readlink(/proc/self/exe)` / `open(/dev/urandom)` if these are denied,
/// so they must be in the allow list for the subprocess to even start.
/// Trade-off vs bwrap: `/proc/N/*` reveals other PIDs' cmdlines/environments
/// to the confined subprocess (bwrap's private procfs hides them). Absent
/// optional paths (`/lib32`, `/nix`, `/opt` are commonly missing) are
/// silently skipped at ruleset build time. Plus the resolved
/// `claude_install_root` is appended dynamically per call (R-C / C-B3), and
/// the user's `~/.claude.json` + `~/.claude/` paths are added in
/// [`LandlockPolicy::default_for_call`] so claude's launcher can read its
/// config + cached credentials (without those, claude 2.1.152's Bun runtime
/// silently `process.exit(0)`s with empty stdout/stderr on EACCES). `$HOME`
/// (outside the two claude paths) and `~/.config` / project paths remain
/// denied.
#[cfg(target_os = "linux")]
const LANDLOCK_DEFAULT_RO_PATHS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/etc",
    "/opt",
    "/nix",
    "/proc",
    "/sys",
    "/dev",
    // R-H: required for DNS resolution from a Tokio-runtime parent.
    // `/etc/resolv.conf` is a symlink to `/run/systemd/resolve/stub-resolv.conf`
    // on systemd-resolved hosts; without RO access here, the child's glibc
    // resolver fails to follow the symlink and falls back to default name
    // servers (`127.0.0.1:53`), where nothing is listening — yielding the
    // 180-second `ConnectionRefused` retry storm we observed in run #53.
    // `path_beneath_rules` silently skips absent entries, so hosts without
    // systemd-resolved are unaffected. `/run/dbus` is added for the
    // libnss_resolve.so.2 D-Bus fallback path.
    "/run/systemd/resolve",
    "/run/dbus",
];

/// Value object carrying the Landlock policy for one call. Pure data: no
/// filesystem syscalls happen in the constructor — paths are stored as
/// `PathBuf`s and only opened (`PathFd::new`) by [`build_ruleset`].
/// Feature 007 R-C / C-B3.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
struct LandlockPolicy {
    /// Read-only `path_beneath` targets — the system tree + the resolved
    /// `claude_install_root`. Absent entries are silently skipped.
    ro_paths: Vec<PathBuf>,
    /// Read-write `path_beneath` targets — exactly the per-call temp dir.
    /// Anything not listed in `ro_paths`/`rw_paths` is deny-by-default.
    rw_paths: Vec<PathBuf>,
    /// Landlock ABI to declare. `ABI::V3` with `CompatLevel::BestEffort`
    /// means available access rights degrade cleanly on kernels older than
    /// the ABI introduced — preserves capability on modern kernels (5.13+).
    abi: landlock::ABI,
}

#[cfg(target_os = "linux")]
impl LandlockPolicy {
    /// Construct the default per-call policy: the system RO tree + the
    /// resolved `claude_install_root` + the user's claude config paths
    /// (`~/.claude.json`, `~/.claude/`) for RO, the per-call temp dir for RW,
    /// `ABI::V3`. No syscalls.
    ///
    /// The `~/.claude.json` / `~/.claude/` RO entries deviate from spec 007's
    /// original `no $HOME / no ~/.claude` design. Reason: claude 2.1.152's
    /// launcher reads one of those paths during startup; on EACCES (vs. ENOENT)
    /// the Bun runtime silently `process.exit(0)` with empty stdout and stderr
    /// — there is no actionable error to surface. Granting RO `path_beneath`
    /// on the two paths lets claude read its config + cached OAuth credentials
    /// without giving the subprocess write access (so the launcher cannot
    /// modify session history, hooks, plugins, or the credentials file).
    /// `path_beneath_rules` silently skips absent entries, so the optional
    /// `home_dir()` lookup is best-effort. See
    /// `specs/007-landlock-inference-sandbox/research.md` R-G for the
    /// bisection evidence and `lat.md/backend.md#Claude Code Inference Client`
    /// for the updated RO set documentation.
    fn default_for_call(rw_dir: &Path, claude_path: &Path) -> Self {
        let mut ro_paths: Vec<PathBuf> = LANDLOCK_DEFAULT_RO_PATHS
            .iter()
            .map(|p| PathBuf::from(*p))
            .collect();
        if let Some(root) = claude_install_root(claude_path) {
            ro_paths.push(root);
        }
        if let Some(home) = dirs::home_dir() {
            ro_paths.push(home.join(".claude.json"));
            ro_paths.push(home.join(".claude"));
        }
        // RW: the per-call temp dir (the spec's "single RW bind"), plus
        // `/dev/null` so anything the launcher redirects to it (Node's
        // `child_process` spawns, devnull file descriptors) does not fail
        // with EACCES under Landlock. `path_beneath` on the device node
        // itself grants exactly /dev/null — nothing wider under /dev.
        Self {
            ro_paths,
            rw_paths: vec![rw_dir.to_path_buf(), PathBuf::from("/dev/null")],
            abi: landlock::ABI::V3,
        }
    }
}

/// Pure construction of a `RulesetCreated` from a [`LandlockPolicy`]. Opens
/// `PathFd`s for each present RO/RW path (silently skipping absent optional
/// paths via the crate's `path_beneath_rules` helper, which filters out
/// paths that fail to open), then `add_rule`s each. Does **NOT** call
/// `restrict_self` — that is consumed by the per-call `pre_exec` hook in
/// `apply_sandbox`'s Landlock arm (the closure runs in the forked child).
/// Feature 007 C-B2.
#[cfg(target_os = "linux")]
fn build_ruleset(
    policy: &LandlockPolicy,
) -> Result<landlock::RulesetCreated, landlock::RulesetError> {
    use landlock::{
        ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
        path_beneath_rules,
    };
    let abi: ABI = policy.abi;
    let ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        // path_beneath_rules silently skips paths that fail to open
        // (the crate-documented behavior we rely on for absent
        // optional system dirs like /lib32, /nix, /opt).
        .add_rules(path_beneath_rules(
            &policy.ro_paths,
            AccessFs::from_read(abi),
        ))?
        .add_rules(path_beneath_rules(
            &policy.rw_paths,
            AccessFs::from_all(abi),
        ))?;
    Ok(ruleset)
}

// ---------------------------------------------------------------------------
// Bwrap-failure classifier + one-shot diagnostic emitter (feature 007 R-E,
// R-F, C-E1..C-E4).
//
// When the Linux confinement chain falls through to None (Landlock
// unsupported AND bwrap unavailable or invocation-blocked by AppArmor), we
// emit exactly one actionable diagnostic per Quill process. The classifier
// is pure (substring match on known bwrap-stderr signatures). The emitter
// latches via `OnceLock<()>` so multi-stream runs don't spam.
// ---------------------------------------------------------------------------

/// Cause classification for a failed bwrap invocation. Pure-function output
/// of [`classify_bwrap_failure`]. Feature 007 C-E1.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BwrapBrokenCause {
    /// Stderr matched one of the AppArmor / userns-restriction signatures —
    /// e.g. Ubuntu 24.04's default
    /// `kernel.apparmor_restrict_unprivileged_userns=1` policy.
    AppArmorRestrictUserns,
    /// Any other stderr signature. Diagnostic falls back to the generic
    /// FR-014 template with the captured stderr appended.
    Other,
}

/// Classify a bwrap-spawn stderr string. Pure function — no `#[serial]`
/// needed in unit tests. Substring matches the two known
/// userns-restriction signatures (the dev-host one and the Codex-blog
/// `loopback` variant) → [`BwrapBrokenCause::AppArmorRestrictUserns`];
/// anything else → [`BwrapBrokenCause::Other`]. Feature 007 C-E1.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn classify_bwrap_failure(stderr: &str) -> BwrapBrokenCause {
    if stderr.contains("setting up uid map: Permission denied")
        || stderr.contains("loopback: Failed RTM_NEWADDR: Operation not permitted")
    {
        BwrapBrokenCause::AppArmorRestrictUserns
    } else {
        BwrapBrokenCause::Other
    }
}

/// Process-wide latch: once a bwrap invocation fails on this host, every
/// subsequent call within this Quill process skips the bwrap wrapping and
/// treats the host as `SandboxKind::None`. First writer wins (via
/// `OnceLock::set` — the already-set Err is ignored). Feature 007 C-C2 /
/// C-E4.
#[cfg(target_os = "linux")]
static BWRAP_BROKEN_ON_THIS_HOST: OnceLock<BwrapBrokenCause> = OnceLock::new();

/// Process-wide one-shot guard for the no-confinement diagnostic. Once set,
/// the emitter no-ops on subsequent calls so multi-stream runs do not spam
/// the same diagnostic three times. Feature 007 C-E2.
#[cfg(target_os = "linux")]
static NO_CONFINEMENT_DIAGNOSTIC_EMITTED: OnceLock<()> = OnceLock::new();

/// AppArmor profile body for the FR-015 remediation hint. 5 lines, lifted
/// from `specs/007-landlock-inference-sandbox/contracts/landlock-sandbox.md`
/// C-E3. Embedded verbatim in the diagnostic message so the operator can
/// drop it into `/etc/apparmor.d/bwrap`.
#[cfg(target_os = "linux")]
const APPARMOR_BWRAP_PROFILE_BODY: &str = "abi <abi/4.0>,\n\
     include <tunables/global>\n\
     profile bwrap /usr/bin/bwrap flags=(unconfined) {\n\
         userns,\n\
     }";

/// Format the no-confinement diagnostic message body (FR-014 generic or
/// FR-015 AppArmor-specific) per cause. Returned as a `String` so callers
/// can both `log::error!` it and unit-test the content. Stable substrings
/// from contract C-E3 are present unchanged in each template:
/// - Generic: `Filesystem confinement is unavailable on this host.`
/// - AppArmor: `` AppArmor's `restrict_unprivileged_userns` policy is blocking bubblewrap ``.
#[cfg(target_os = "linux")]
fn format_no_confinement_diagnostic(
    cause: Option<BwrapBrokenCause>,
    captured_stderr: Option<&str>,
) -> String {
    match cause {
        Some(BwrapBrokenCause::AppArmorRestrictUserns) => {
            let signature_hint = captured_stderr
                .map(|s| {
                    // Quote a short bounded slice of the captured stderr
                    // in the message; bounded so a giant stderr never
                    // bloats the log column.
                    format!(" (detected from bwrap stderr \"{}\")", truncate(s, 256))
                })
                .unwrap_or_default();
            format!(
                "Filesystem confinement is unavailable on this host because \
                 AppArmor's `restrict_unprivileged_userns` policy is blocking \
                 bubblewrap's user-namespace creation{signature_hint}. Quill \
                 will continue running learning analyses unwrapped (each \
                 per-call inference processes untrusted captured content with \
                 full read access to your home directory, ~/.claude \
                 credentials, and project trees). To restore confinement, \
                 either:\n\
                 1. (Recommended) Install an AppArmor profile that grants \
                 `userns,` to /usr/bin/bwrap. Ubuntu 24.04.02+ ships this as \
                 the `bwrap-userns-restrict` package; on stock 24.04, create \
                 /etc/apparmor.d/bwrap with the profile body below and run \
                 `sudo apparmor_parser -r /etc/apparmor.d/bwrap`:\n\n\
                 {APPARMOR_BWRAP_PROFILE_BODY}\n\n\
                 2. Install a Linux kernel that supports Landlock LSM (any \
                 5.13+; Ubuntu 22.04+ already does — if Landlock isn't being \
                 detected on your 5.13+ kernel, that's a separate issue \
                 worth filing).\n\
                 3. (Not recommended; weakens system hardening) \
                 `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0` \
                 and persist in /etc/sysctl.d/."
            )
        }
        None | Some(BwrapBrokenCause::Other) => {
            let stderr_suffix = captured_stderr
                .map(|s| format!(" Captured stderr: \"{}\".", truncate(s, 256)))
                .unwrap_or_default();
            format!(
                "Filesystem confinement is unavailable on this host. Quill \
                 will continue running learning analyses unwrapped (each \
                 per-call inference processes untrusted captured content \
                 with full read access to your home directory, ~/.claude \
                 credentials, and project trees). To restore filesystem \
                 confinement, install either a Linux kernel >= 5.13 with \
                 Landlock LSM enabled (the in-process primitive Quill \
                 prefers), or the `bubblewrap` package (the subprocess \
                 fallback Quill keeps for older kernels and non-Landlock \
                 distros). See run-history detail for the per-call recorded \
                 state.{stderr_suffix}"
            )
        }
    }
}

/// One-shot per Quill process: emit the no-confinement diagnostic to
/// `log::error!`. Returns the formatted message if this call was the first
/// to trip the latch, `None` if the latch was already set (so unit tests
/// can prove the one-shot semantics). The function is best-effort by
/// design — the per-call log channel is not currently plumbed through
/// `cc_client` (the public `invoke_typed`/`invoke_text` surface carries no
/// channel), so persisted-log emission is left as a future plumbing
/// extension; the stderr surface is sufficient for the tauri-dev terminal
/// signal documented in C-E2. Feature 007 R-F / C-E2.
#[cfg(target_os = "linux")]
fn emit_no_confinement_diagnostic(
    cause: Option<BwrapBrokenCause>,
    captured_stderr: Option<&str>,
) -> Option<String> {
    if NO_CONFINEMENT_DIAGNOSTIC_EMITTED.set(()).is_err() {
        // Latch already tripped — silent no-op on subsequent calls.
        return None;
    }
    let message = format_no_confinement_diagnostic(cause, captured_stderr);
    log::error!("{message}");
    Some(message)
}

/// Process-cached detection result. The probe is idempotent (Landlock
/// kernel support and the bwrap PATH presence don't change over the lifetime
/// of a single Quill process), and the Landlock probe creates+discards a
/// kernel ruleset fd — cheap but not free. Cache it process-wide on first
/// call. Feature 007 C-A1.
static DETECTED_SANDBOX_KIND: OnceLock<SandboxKind> = OnceLock::new();

/// Decide which confinement mechanism this host can apply, independent of any
/// single call's temp dir. Pure host probe — used both by the spawn path and
/// by [`failed_metadata`] so the recorded `sandbox` state is correct even when
/// a call fails before (or without) a successful spawn (SC-013: 100% of runs,
/// every platform). Cached process-wide on first call.
fn detect_sandbox_kind() -> SandboxKind {
    *DETECTED_SANDBOX_KIND.get_or_init(detect_sandbox_kind_uncached)
}

/// Uncached host probe — called once via `OnceLock::get_or_init`. Linux
/// chain (feature 007 C-A1): Landlock → Bwrap → None. macOS / Windows /
/// other targets unchanged from feature 006.
fn detect_sandbox_kind_uncached() -> SandboxKind {
    #[cfg(target_os = "linux")]
    {
        // Landlock probe: build a minimal ruleset and discard. If the
        // kernel supports Landlock at all (≥5.13), this succeeds; older
        // kernels and Landlock-disabled hosts return Err and the chain
        // advances to the bwrap fallback.
        if probe_landlock_available() {
            return SandboxKind::Landlock;
        }
        if resolve_on_path("bwrap").is_some() {
            return SandboxKind::Bwrap;
        }
        // No mechanism available — emit the generic FR-014 diagnostic
        // exactly once for this process. The classifier sees no captured
        // bwrap stderr because the chain never reached the bwrap arm.
        let _ = emit_no_confinement_diagnostic(None, None);
        SandboxKind::None
    }
    #[cfg(target_os = "macos")]
    {
        if resolve_on_path("sandbox-exec").is_some() {
            return SandboxKind::SandboxExec;
        }
        SandboxKind::None
    }
    #[cfg(target_os = "windows")]
    {
        // `kill_on_drop` already associates the child with a Job Object that
        // is terminated on drop — a partial, best-effort constraint. A
        // restricted primary token would be stronger but is not portable
        // across all Windows configs; documented best-effort (FR-005).
        SandboxKind::JobObject
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        SandboxKind::None
    }
}

/// Cheap Landlock-availability probe: try to build a minimal ruleset with
/// the lowest-common-denominator ABI (V1) and discard. `Ok` means the
/// running kernel exposes Landlock and we can use it. `Err` means
/// kernel < 5.13, the LSM is disabled, or seccomp blocked the syscall —
/// the chain advances to bwrap. Pure probe; no `restrict_self` call.
/// Feature 007 C-A1 / C-B1.
#[cfg(target_os = "linux")]
fn probe_landlock_available() -> bool {
    use landlock::{ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr};
    Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI::V1))
        .and_then(|r| r.create())
        .is_ok()
}

/// Build a deny-by-default macOS SBPL profile string. Default-deny, then
/// re-allow exactly: process exec/fork, the SCOPED set of read paths the
/// node+`claude` launcher needs to resolve its own runtime, the per-call
/// temp dir for read+write, and outbound network (the `claude` CLI makes the
/// model API call itself — egress MUST stay open). `temp_dir` is the
/// per-call `TempDir` path; nothing broader is writable.
///
/// Threat model — the spawned `claude` processes UNTRUSTED captured session
/// content, so a prompt-injected agent must NOT be able to read host secrets.
/// A blanket `(allow file-read*)` would let it read `~/.claude/config` (the
/// Anthropic API key), `~/.ssh`, the Quill SQLite DB, and arbitrary project
/// trees — directly contradicting FR-005 ("cannot read or modify data
/// outside a disposable workspace"). So reads are confined to: (a) the
/// immutable system + runtime prefixes node needs to exec and dlopen its own
/// interpreter/libs (`/usr`, `/System`, `/Library`, `/private/var/select`,
/// `/opt`, `/bin`, `/sbin`, `/dev` — system, NOT `$HOME`), and (b) the
/// resolved `claude`/node install tree when it lives outside those (npm
/// prefix, `~/.claude/local`, a bun/volta dir). Notably ABSENT: `$HOME`,
/// `~/.claude`, `~/.config`, `~/.ssh`, and any project/repo path. Writes
/// were already correctly confined to the per-call temp subpath; this change
/// only narrows reads — no production behavior change beyond the SBPL string.
#[cfg(target_os = "macos")]
fn macos_sbpl_profile(temp_dir: &Path, claude_path: &Path) -> String {
    // SBPL string literals are double-quoted; escape embedded quotes/backslashes
    // in any resolved path so an unusual path cannot break the profile.
    fn sbpl_escape(p: &Path) -> String {
        p.display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }
    let escaped_rw = sbpl_escape(temp_dir);
    // The resolved claude/node install tree, allowed READ-ONLY so the
    // launcher can find its interpreter + sibling deps when it lives outside
    // the system prefixes below. Mirrors the bwrap path's `claude_install_root`
    // (same walk-up logic); omitted entirely if it can't be resolved.
    let claude_root_rule = match claude_install_root(claude_path) {
        Some(root) => format!(
            "         (allow file-read*\n  (subpath \"{}\"))\n",
            sbpl_escape(&root)
        ),
        None => String::new(),
    };
    format!(
        "(version 1)\n\
         (deny default)\n\
         (allow process-exec)\n\
         (allow process-fork)\n\
         (allow sysctl-read)\n\
         (allow mach-lookup)\n\
         (allow signal (target self))\n\
         (allow file-read*\n\
  (subpath \"/usr\")\n\
  (subpath \"/System\")\n\
  (subpath \"/Library\")\n\
  (subpath \"/private/var/select\")\n\
  (subpath \"/opt\")\n\
  (subpath \"/bin\")\n\
  (subpath \"/sbin\")\n\
  (literal \"/dev/null\")\n\
  (subpath \"/dev\"))\n\
         {claude_root_rule}\
         (allow file-read* file-write*\n  (subpath \"{escaped_rw}\"))\n\
         (allow network-outbound)\n\
         (allow network-bind (local ip))\n"
    )
}

/// Replay the parent command's *explicit* environment deltas onto the
/// wrapper command. `build_command` only ever calls `env_remove` (the R-6
/// scrub) and never `env`-sets, so this re-applies exactly the scrub onto
/// the new outer process while the rest of the inherited environment (PATH,
/// HOME for node's module resolution, etc.) flows through normally — the
/// wrapper binaries (`bwrap`/`sandbox-exec`) all pass the parent env to
/// their child by default, so the scrub must be re-asserted here. The
/// Landlock arm reuses the inner `Command` directly (no outer wrapper
/// process), so it does not call `replay_env_scrub`.
fn replay_env_scrub(src: &std::process::Command, dst: &mut Command) {
    for (k, v) in src.get_envs() {
        match v {
            Some(v) => {
                dst.env(k, v);
            }
            None => {
                dst.env_remove(k);
            }
        }
    }
}

/// Re-wire the inherited stdio/kill-on-drop the spawn loop relies on. The
/// outer wrapper process is the one tokio waits on, so it (not the inner
/// `claude`) must own the piped stdin/stdout/stderr and the kill-on-drop.
fn wire_child_io(cmd: &mut Command) {
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
}

/// Wrap `inner` (already fully flag-isolated and configured to run the real
/// `claude`) with the best host confinement mechanism, RW-carving out
/// EXACTLY `rw_dir` (the per-call `TempDir`, or `None` for the free-form
/// text path which never writes an artifact). Returns the possibly-rewrapped
/// command and the kind actually applied. NETWORK IS PRESERVED on every
/// branch (no net namespace / `network-outbound` allowed) because the CLI
/// makes the model API call itself. If no mechanism is available the
/// original flag-isolated command is returned untouched — NEVER fail closed.
#[cfg_attr(
    any(
        target_os = "windows",
        not(any(target_os = "linux", target_os = "macos"))
    ),
    allow(unused_variables)
)]
fn apply_sandbox(
    inner: Command,
    claude_path: &Path,
    rw_dir: Option<&Path>,
) -> (Command, SandboxKind) {
    // Effective mechanism for THIS call: start from the host probe, then
    // demote to None on Linux if the bwrap-broken-on-this-host latch is set
    // (feature 007 C-C2 / C-E4). Subsequent calls in the same Quill process
    // never re-attempt the known-broken bwrap.
    #[allow(unused_mut)]
    let mut detected = detect_sandbox_kind();
    #[cfg(target_os = "linux")]
    if matches!(detected, SandboxKind::Bwrap) && BWRAP_BROKEN_ON_THIS_HOST.get().is_some() {
        detected = SandboxKind::None;
    }
    match detected {
        #[cfg(target_os = "linux")]
        SandboxKind::Landlock => {
            // Landlock requires an RW carve-out (the per-call temp dir).
            // The free-form text path passes `None` and never writes an
            // artifact; without an RW carve-out a Landlock ruleset would
            // make every write deny-by-default, which the launcher needs
            // for its own scratch (node module loader fd dance, etc.).
            // Fall through to the bwrap arm for that case.
            let Some(rw) = rw_dir else {
                return apply_sandbox_bwrap_fallback(inner, claude_path);
            };
            let policy = LandlockPolicy::default_for_call(rw, claude_path);
            let ruleset = match build_ruleset(&policy) {
                Ok(r) => r,
                Err(e) => {
                    // Best-effort fall-through to the bwrap fallback. Never
                    // fail-closed (feature 007 C-B5).
                    log::warn!(
                        "cc_client: Landlock ruleset construction failed ({e}); \
                         falling back to bwrap arm"
                    );
                    return apply_sandbox_bwrap_fallback(inner, claude_path);
                }
            };
            // Wrap the `RulesetCreated` in a `Mutex<Option<_>>` so the
            // FnMut closure can `take` it on the first (and only) call
            // inside the forked child. `pre_exec` requires `FnMut + Send +
            // Sync + 'static`; `restrict_self` consumes the ruleset by
            // value, so `take()` is the canonical FnOnce-inside-FnMut
            // pattern. The Mutex is contention-free in practice — the
            // pre-exec closure runs exactly once per spawned child.
            let ruleset_slot = std::sync::Arc::new(std::sync::Mutex::new(Some(ruleset)));
            let mut cmd = inner;
            {
                use std::os::unix::process::CommandExt as _;
                let std_cmd = cmd.as_std_mut();
                let slot = std::sync::Arc::clone(&ruleset_slot);
                // SAFETY: the closure runs in the forked child between
                // `fork` and the launch of `claude` and performs only
                // async-signal-safe syscalls: `prctl(PR_SET_NO_NEW_PRIVS)`
                // and `landlock_restrict_self` (the latter via the
                // crate's `restrict_self`). No allocator, no global
                // locks, no file I/O. The `Mutex` lock cannot deadlock
                // because the parent never holds it past the spawn call.
                unsafe {
                    std_cmd.pre_exec(move || {
                        // 1. PR_SET_NO_NEW_PRIVS — refuse to gain
                        //    privileges via setuid/file caps after this
                        //    point. The crate's `restrict_self` does this
                        //    too, but we set it explicitly for
                        //    defense-in-depth (the contract is two
                        //    syscalls in order: prctl, then restrict).
                        let rc = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1u64, 0u64, 0u64, 0u64);
                        if rc != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        // 2. landlock_restrict_self — consume the
                        //    RulesetCreated and apply the FS ruleset to
                        //    this child and everything it launches.
                        match slot.lock() {
                            Ok(mut guard) => {
                                if let Some(ruleset) = guard.take()
                                    && let Err(e) = ruleset.restrict_self()
                                {
                                    return Err(std::io::Error::other(e.to_string()));
                                }
                            }
                            Err(_) => {
                                return Err(std::io::Error::other(
                                    "landlock ruleset mutex poisoned",
                                ));
                            }
                        }
                        Ok(())
                    });
                }
            }
            (cmd, SandboxKind::Landlock)
        }
        #[cfg(target_os = "linux")]
        SandboxKind::Bwrap => apply_sandbox_bwrap(inner, claude_path, rw_dir),
        #[cfg(target_os = "macos")]
        SandboxKind::SandboxExec => {
            let Some(sb) = resolve_on_path("sandbox-exec") else {
                return (inner, SandboxKind::None);
            };
            // sandbox-exec needs a concrete writable subpath. The free-form
            // text path has no per-call temp dir; fall back to confining
            // writes to the system temp root (it still never writes an
            // artifact, and reads stay broadly allowed). Profile is passed
            // inline via `-p` so no profile file is written anywhere.
            let rw = rw_dir
                .map(Path::to_path_buf)
                .unwrap_or_else(std::env::temp_dir);
            let profile = macos_sbpl_profile(&rw, claude_path);
            let std_inner = inner.into_std();
            let mut se = Command::new(&sb);
            se.arg("-p").arg(&profile).arg("--");
            se.arg(std_inner.get_program());
            for a in std_inner.get_args() {
                se.arg(a);
            }
            replay_env_scrub(&std_inner, &mut se);
            if let Some(cwd) = std_inner.get_current_dir() {
                se.current_dir(cwd);
            }
            wire_child_io(&mut se);
            (se, SandboxKind::SandboxExec)
        }
        // Windows Job Object is established by the existing `kill_on_drop`
        // association in `build_command`; no command rewrite. `None` leaves
        // the flag-isolated command intact. Either way: NEVER fail closed.
        other => (inner, other),
    }
}

/// Apply the Linux bwrap fallback wrapping (deny-by-default FS, RW carve
/// out exactly `rw_dir`, network preserved). Extracted into a helper so the
/// Landlock arm of [`apply_sandbox`] can fall through to bwrap when its
/// per-call ruleset construction fails or when there is no RW carve-out
/// (the free-form text path). Behavior matches the feature-005 bwrap arm
/// byte-for-byte — only the surrounding scaffolding moved.
#[cfg(target_os = "linux")]
fn apply_sandbox_bwrap(
    inner: Command,
    claude_path: &Path,
    rw_dir: Option<&Path>,
) -> (Command, SandboxKind) {
    let Some(bwrap) = resolve_on_path("bwrap") else {
        // Raced away between detect and apply — degrade, never fail.
        return (inner, SandboxKind::None);
    };
    let std_inner = inner.into_std();
    let mut bw = Command::new(&bwrap);
    // Deny-by-default FS: a fresh proc/dev, the system tree and the
    // resolved claude/node install dir bound READ-ONLY, NO bind of
    // $HOME / ~/.claude / ~/.config / project trees, a private
    // `/tmp` tmpfs. Namespaces isolate IPC/PID/UTS/cgroup.
    // `--unshare-net` is DELIBERATELY OMITTED — the CLI makes the
    // model API call itself, so egress MUST stay open.
    bw.arg("--die-with-parent")
        .arg("--unshare-pid")
        .arg("--unshare-ipc")
        .arg("--unshare-uts")
        .arg("--unshare-cgroup-try")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--tmpfs")
        .arg("/tmp");
    // Read-only system binds. `/nix` covers Nix-store installs of
    // node/claude; each `--ro-bind-try` is a no-op if absent.
    for ro in [
        "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/opt", "/nix",
    ] {
        bw.arg("--ro-bind-try").arg(ro).arg(ro);
    }
    // The resolved claude binary's own tree (npm shim + its node)
    // bound READ-ONLY so the launcher can find its runtime even when
    // it lives outside the system dirs above (e.g. ~/.claude/local,
    // a bun/volta prefix). No write-back to the install.
    if let Some(claude_root) = claude_install_root(claude_path) {
        let p = claude_root.display().to_string();
        bw.arg("--ro-bind-try").arg(&p).arg(&p);
    }
    // The ONLY writable path: EXACTLY the per-call temp dir (used
    // for the typed-output `out.json`). Nothing broader.
    if let Some(dir) = rw_dir {
        let p = dir.display().to_string();
        bw.arg("--bind").arg(&p).arg(&p);
    }
    // bwrap inherits and passes the parent env through by default
    // (no `--clearenv`), so node's PATH/HOME module resolution still
    // works; re-assert only the R-6 scrub via `replay_env_scrub`.
    if let Some(cwd) = std_inner.get_current_dir() {
        bw.arg("--chdir").arg(cwd);
    }
    bw.arg("--");
    bw.arg(std_inner.get_program());
    for a in std_inner.get_args() {
        bw.arg(a);
    }
    replay_env_scrub(&std_inner, &mut bw);
    wire_child_io(&mut bw);
    (bw, SandboxKind::Bwrap)
}

/// Internal fall-through used by the Landlock arm when its per-call ruleset
/// cannot be applied (build failed, no RW carve-out). Reaches for the bwrap
/// arm if bwrap is on PATH; otherwise returns the flag-isolated command
/// untouched and `SandboxKind::None` — NEVER fails closed. Feature 007 C-B5.
#[cfg(target_os = "linux")]
fn apply_sandbox_bwrap_fallback(inner: Command, claude_path: &Path) -> (Command, SandboxKind) {
    if BWRAP_BROKEN_ON_THIS_HOST.get().is_some() || resolve_on_path("bwrap").is_none() {
        return (inner, SandboxKind::None);
    }
    apply_sandbox_bwrap(inner, claude_path, None)
}

/// Best-effort root of the resolved `claude` install so it (and its bundled
/// node) can be bound/allowed read-only — inside `bwrap` on Linux and inside
/// the macOS SBPL profile — even when it lives outside the system dirs
/// (npm-global, `~/.claude/local`, a bun/volta prefix). Walk up from the
/// resolved binary: a `node_modules`/`.bin`-style shim layout means the
/// package root is a couple levels up; otherwise bind the parent `bin`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn claude_install_root(claude_path: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(claude_path).unwrap_or_else(|_| claude_path.to_path_buf());
    // `.../node_modules/.bin/claude` → bind `.../node_modules` (covers the
    // launcher AND its sibling runtime deps). Otherwise bind the parent dir
    // (e.g. `~/.claude/local`, `/usr/local/bin`'s sibling libexec is already
    // covered by the system ro-binds).
    let mut cur = real.as_path();
    while let Some(parent) = cur.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some("node_modules") {
            return Some(parent.to_path_buf());
        }
        cur = parent;
    }
    real.parent().map(Path::to_path_buf)
}

/// Inputs for one Claude Code invocation. Mirrors the legacy
/// `ai_client::analyze_typed` / `ai_client::complete_text` argument
/// list with the addition of a `phase` tag.
pub struct InvokeArgs {
    pub phase: Phase,
    pub prompt: String,
    pub preamble: String,
    pub model: Model,
    /// Output budget upper bound. Carried into the metadata record so
    /// future analysis can correlate budget vs. actual usage; the
    /// `claude` CLI does not expose a direct max-tokens knob in
    /// headless mode but this remains informative.
    pub max_tokens: u64,
}

/// Successful invocation result: the deserialized `T` (for
/// [`invoke_typed`]) or `String` (for [`invoke_text`]) bundled with
/// the per-call metadata to be persisted on the parent run record.
pub struct InvokeOutcome<T> {
    pub value: T,
    pub metadata: InferenceCallMetadata,
}

/// Per-Claude-Code-invocation structured metadata persisted as one
/// element of the JSON array stored in `learning_runs.inference_metadata`
/// or `optimization_runs.inference_metadata`. See `data-model.md`
/// § "Inference Call Metadata" for the field-by-field contract.
#[derive(Clone, Debug, Default, Serialize)]
pub struct InferenceCallMetadata {
    pub phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub max_tokens_requested: u64,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub ttft_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub total_cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub permission_denials: Vec<Value>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<&'static str>,
    /// OS-level confinement actually applied to the spawned `claude` for
    /// this call: one of the closed write vocabulary
    /// `{landlock, bwrap, sandbox-exec, job-object, none}` (feature 007
    /// C-A2 / H-5 / FR-005 / SC-013). The tag distinguishes real
    /// **filesystem** confinement (`landlock`, `bwrap`, `sandbox-exec` —
    /// deny-by-default out-of-workspace R/W) from process/flag-only
    /// isolation with **no** filesystem confinement (`job-object`, `none`);
    /// see [`sandbox_tag_is_fs_confined`]. Decode is forward-compatible with
    /// retired tags (`process-only` from feature 006-A, pre-feature-006
    /// `unshare`) which classify as not-FS-confined. Always set for every
    /// record this build produces (success AND failure paths) so SC-013 —
    /// confinement state recorded for 100% of analysis runs on every
    /// platform — is verifiable. `None` only on legacy/pre-feature-005
    /// decoded records (no migration; tolerant decode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
}

/// Categorized inference failure. See `research.md` R-7 for the full
/// signal-to-variant mapping and the contracts file for user-facing
/// message intent.
#[derive(Debug)]
#[non_exhaustive]
pub enum InferenceError {
    /// `claude` not on PATH (spawn returned `NotFound`).
    ClaudeCodeMissing,
    /// `claude` is present but rejected one of our required flags;
    /// captured `--version` if the probe succeeded.
    ClaudeCodeTooOld { detected_version: Option<String> },
    /// Claude Code reports the user is not signed in.
    NotSignedIn,
    /// Claude Code returned a rate-limit or overload error.
    RateLimited { message: String },
    /// The model produced output that does not satisfy the requested
    /// schema, or `T` deserialization from `envelope.result` failed.
    SchemaValidationFailed { details: String },
    /// `tokio::time::timeout` fired — the subprocess was killed.
    TimedOut { after: Duration },
    /// Other `Command::spawn` / I/O failures.
    Spawn(String),
    /// Stdout did not parse as the documented `--output-format json`
    /// envelope shape.
    BadEnvelope { details: String },
}

impl InferenceError {
    /// Stable string tag persisted on the failed metadata record.
    pub fn kind(&self) -> &'static str {
        match self {
            InferenceError::ClaudeCodeMissing => "claude_code_missing",
            InferenceError::ClaudeCodeTooOld { .. } => "claude_code_too_old",
            InferenceError::NotSignedIn => "not_signed_in",
            InferenceError::RateLimited { .. } => "rate_limited",
            InferenceError::SchemaValidationFailed { .. } => "schema_validation_failed",
            InferenceError::TimedOut { .. } => "timed_out",
            InferenceError::Spawn(_) => "spawn",
            InferenceError::BadEnvelope { .. } => "bad_envelope",
        }
    }
}

impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InferenceError::ClaudeCodeMissing => write!(
                f,
                "Claude Code (`claude` CLI) is not installed or not on PATH. \
                 Install from https://claude.com/claude-code/install and restart Quill."
            ),
            InferenceError::ClaudeCodeTooOld { detected_version } => match detected_version {
                Some(v) => write!(
                    f,
                    "Claude Code v{v} is too old for this feature. \
                     Run `claude update` (or reinstall) to get a current version."
                ),
                None => write!(
                    f,
                    "The installed Claude Code does not support a flag required by Quill. \
                     Run `claude update` (or reinstall) to get a current version."
                ),
            },
            InferenceError::NotSignedIn => write!(
                f,
                "Claude Code is not signed in. Run `claude /login` in a terminal."
            ),
            InferenceError::RateLimited { message } => write!(
                f,
                "Claude Code reported a rate limit: {message}. \
                 Wait a few minutes and try again."
            ),
            InferenceError::SchemaValidationFailed { details } => write!(
                f,
                "Claude Code returned a response that did not match the expected schema: {details}"
            ),
            InferenceError::TimedOut { after } => write!(
                f,
                "Claude Code invocation exceeded the {}s hang-detector timeout and was killed.",
                after.as_secs()
            ),
            InferenceError::Spawn(message) => {
                write!(f, "Failed to spawn `claude` subprocess: {message}")
            }
            InferenceError::BadEnvelope { details } => write!(
                f,
                "Claude Code returned output that could not be parsed as the expected JSON envelope: {details}"
            ),
        }
    }
}

impl std::error::Error for InferenceError {}

/// Build a metadata record for a failed call so callers can append it
/// to the run's `inference_metadata` array without contorting the
/// `Result` shape. Records the host-level confinement that WOULD have been
/// applied (`detect_sandbox_kind`): the failure may occur before — or
/// entirely without — a successful spawn (e.g. `ClaudeCodeMissing`,
/// `TimedOut`), but SC-013 still requires the confinement state on 100% of
/// runs. The decision is a pure host probe, identical to what the spawn
/// path would have selected, so callers in learning.rs / memory_optimizer.rs
/// need no sandbox awareness.
pub fn failed_metadata(
    phase: Phase,
    max_tokens_requested: u64,
    err: &InferenceError,
) -> InferenceCallMetadata {
    InferenceCallMetadata {
        phase: phase.as_str(),
        max_tokens_requested,
        success: false,
        failure_kind: Some(err.kind()),
        sandbox: Some(detect_sandbox_kind().as_str().to_string()),
        ..InferenceCallMetadata::default()
    }
}

// ---------------------------------------------------------------------------
// Envelope shape returned by `claude -p --output-format json`. Forward-compat:
// unknown fields are tolerated, optional numerics default to zero, model id is
// the highest-cost entry in the `modelUsage` map (agent runs report several:
// the requested model plus a cheap tool-loop model). `result` is the model's
// prose reply and `structured_output` (when present) the schema-validated
// payload — but typed callers no longer rely on either: `invoke_typed` reads
// the JSON artifact the agent writes to a sandboxed temp file.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Envelope {
    #[serde(rename = "type")]
    type_: String,
    subtype: String,
    is_error: bool,
    api_error_status: Option<String>,
    duration_ms: u64,
    duration_api_ms: u64,
    ttft_ms: u64,
    result: String,
    stop_reason: Option<String>,
    total_cost_usd: f64,
    usage: EnvelopeUsage,
    #[serde(rename = "modelUsage")]
    model_usage: std::collections::BTreeMap<String, Value>,
    permission_denials: Vec<Value>,
    structured_output: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EnvelopeUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    service_tier: Option<String>,
}

fn parse_envelope(stdout: &str) -> Result<Envelope, InferenceError> {
    serde_json::from_str::<Envelope>(stdout).map_err(|e| InferenceError::BadEnvelope {
        details: format!("{e} (first 256 chars: {})", truncate(stdout, 256)),
    })
}

fn metadata_from_envelope(
    phase: Phase,
    max_tokens_requested: u64,
    env: &Envelope,
    sandbox: SandboxKind,
) -> InferenceCallMetadata {
    // Agent-mode runs report MULTIPLE models in `modelUsage`: the
    // requested `--model` does the main generation while a cheap model
    // handles tool-loop overhead. Attribute the run to the model that
    // did the most work by cost — NOT the alphabetically-first BTreeMap
    // key, which would always pick "claude-haiku-…" over
    // "claude-sonnet-…" and misreport the model. Empty map → None.
    let model = env
        .model_usage
        .iter()
        .max_by(|a, b| {
            let cost = |v: &Value| v.get("costUSD").and_then(Value::as_f64).unwrap_or(0.0);
            cost(a.1).total_cmp(&cost(b.1))
        })
        .map(|(k, _)| k.clone());
    InferenceCallMetadata {
        phase: phase.as_str(),
        model,
        max_tokens_requested,
        duration_ms: env.duration_ms,
        duration_api_ms: env.duration_api_ms,
        ttft_ms: env.ttft_ms,
        input_tokens: env.usage.input_tokens,
        output_tokens: env.usage.output_tokens,
        cache_creation_input_tokens: env.usage.cache_creation_input_tokens,
        cache_read_input_tokens: env.usage.cache_read_input_tokens,
        total_cost_usd: env.total_cost_usd,
        service_tier: env.usage.service_tier.clone(),
        stop_reason: env.stop_reason.clone(),
        permission_denials: env.permission_denials.clone(),
        // Invariant: `metadata_from_envelope` is only called from
        // `invoke_typed` / `invoke_text` on the `Ok(envelope)` path,
        // which `invoke_raw` only returns when `is_error == false`.
        // Failed calls produce metadata via `failed_metadata` instead.
        success: true,
        failure_kind: None,
        // The confinement `invoke_raw` actually applied to THIS spawn
        // (not a re-probe): records the real boundary the agent ran
        // under for SC-013 audit.
        sandbox: Some(sandbox.as_str().to_string()),
    }
}

// ---------------------------------------------------------------------------
// Command construction and error classification.
// ---------------------------------------------------------------------------

/// Build the fully isolated child command and return it alongside the OS
/// confinement that was applied. The flag isolation below is kept verbatim
/// (defense in depth); the OS sandbox is layered on top as the last step so
/// it inherits the already-curated args/cwd/scrubbed-env. The RW carve-out
/// is EXACTLY `artifact_dir` (the per-call `TempDir`) and nothing broader —
/// the typed-output `out.json` contract still works; the free-form text path
/// passes `None` and writes no artifact.
fn build_command(
    args: &InvokeArgs,
    artifact_dir: Option<&Path>,
    claude_path: &Path,
) -> (Command, SandboxKind) {
    let mut cmd = Command::new(claude_path);

    // Headless one-shot mode with the documented JSON envelope.
    cmd.arg("-p").arg("--output-format").arg("json");
    cmd.arg("--model").arg(args.model.alias());
    cmd.arg("--append-system-prompt").arg(&args.preamble);

    cmd.arg("--disable-slash-commands");
    cmd.arg("--no-session-persistence");
    cmd.arg("--setting-sources").arg("");
    cmd.arg("--exclude-dynamic-system-prompt-sections");

    match artifact_dir {
        // Typed path: the agent delivers the result by writing a JSON
        // artifact. Grant ONLY Write, sandboxed to `dir`. Scoped,
        // bounded reversal of spec-003 R-5 total tool isolation. No
        // `--json-schema` (the CLI does not enforce it; the schema is
        // embedded in the prompt by invoke_typed instead).
        Some(dir) => {
            cmd.arg("--allowedTools").arg("Write");
            cmd.arg("--disallowedTools")
                .arg("Bash Edit Read WebFetch WebSearch Glob Grep");
            cmd.arg("--permission-mode").arg("acceptEdits");
            cmd.arg("--add-dir").arg(dir);
            cmd.current_dir(dir);
            // Route the launcher's transient writes (Node compile cache,
            // libc tmpfile()) into the already-allowed per-call dir so the
            // Landlock arm does not need a writable `/tmp`. Honored by Node
            // (`os.tmpdir()` → `$TMPDIR`) and by Bun. No-op under bwrap
            // (which mounts a fresh writable `/tmp` tmpfs anyway) and the
            // `None` sandbox (where `/tmp` was always writable).
            cmd.env("TMPDIR", dir);
            cmd.env("NODE_COMPILE_CACHE", dir);
        }
        // Free-form path (invoke_text): unchanged total isolation (R-5).
        None => {
            cmd.arg("--tools").arg("");
            if let Some(state_dir) = state_dir() {
                cmd.current_dir(state_dir);
            }
        }
    }

    // I/O wiring — prompt body delivered on stdin (R-2).
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    // Environment scrub — R-6.
    let scrub_keys: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.to_str().map(str::to_owned))
        .filter(|k| {
            k.starts_with("CLAUDE_CODE_") || k.starts_with("ANTHROPIC_") || k == "NODE_OPTIONS"
        })
        .collect();
    for key in scrub_keys {
        cmd.env_remove(OsStr::new(&key));
    }

    // OS-level confinement (H-5 / FR-005 / SC-013) wraps the fully-formed
    // command LAST so it captures the curated args, cwd and scrubbed env.
    // RW carve-out = exactly the per-call `TempDir` (`artifact_dir`); the
    // free-form path has no artifact dir so writes are denied beyond a
    // private tmpfs. Network is preserved on every branch (the CLI makes
    // the model API call itself). Graceful degradation: if no mechanism is
    // available the flag-isolated command is returned untouched and the
    // call still runs — NEVER fail closed (do not disable inference).
    apply_sandbox(cmd, claude_path, artifact_dir)
}

fn state_dir() -> Option<PathBuf> {
    // App-controlled CWD (R-14). Prefer the platform's per-user data
    // dir; fall back to the home directory if data_local_dir is
    // unavailable. If neither is available we omit `current_dir` and
    // the subprocess inherits ours.
    dirs::data_local_dir().or_else(dirs::home_dir)
}

fn truncate(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

fn classify_error(
    exit_status: std::process::ExitStatus,
    stderr: &str,
    envelope: Option<&Envelope>,
) -> InferenceError {
    // Version mismatch — `claude` rejected one of our required flags.
    // `detected_version` is filled in asynchronously by the caller
    // (`invoke_raw`) so this sync classifier never blocks on a
    // `claude --version` subprocess.
    let stderr_lc = stderr.to_lowercase();
    if !exit_status.success()
        && (stderr_lc.contains("unknown option")
            || stderr_lc.contains("unrecognized option")
            || stderr_lc.contains("no such option")
            || stderr_lc.contains("error: unknown argument"))
    {
        return InferenceError::ClaudeCodeTooOld {
            detected_version: None,
        };
    }

    // Auth / login failure from stderr is only meaningful when the process
    // itself failed. A successful exit means the run produced an envelope,
    // and any auth-related substrings in that envelope (or in stdout that
    // leaked to stderr) belong to the model's reply, not to Claude Code's
    // own auth state.
    if !exit_status.success()
        && (stderr_lc.contains("claude /login")
            || stderr_lc.contains("not authenticated")
            || stderr_lc.contains("please log in")
            || stderr_lc.contains("not signed in"))
    {
        return InferenceError::NotSignedIn;
    }
    if let Some(env) = envelope {
        let result_lc = env.result.to_lowercase();
        if env.is_error
            && (result_lc.contains("claude /login")
                || result_lc.contains("not authenticated")
                || result_lc.contains("not signed in"))
        {
            return InferenceError::NotSignedIn;
        }
    }

    // Rate-limit / overload signaled via envelope.
    if let Some(env) = envelope
        && env.is_error
    {
        let is_rate_limit_status = env
            .api_error_status
            .as_deref()
            .map(|s| s.starts_with("429") || s.eq_ignore_ascii_case("rate_limit_error"))
            .unwrap_or(false);
        let result_lc = env.result.to_lowercase();
        let is_rate_limit_text = result_lc.contains("rate limit")
            || result_lc.contains("overloaded")
            || result_lc.contains("quota");
        if is_rate_limit_status || is_rate_limit_text {
            // Truncate consistent with every other error variant — the
            // Anthropic rate-limit response body can in principle be
            // arbitrarily large and ends up stored in the run record's
            // error column and rendered in the run history UI.
            let raw = if env.result.is_empty() {
                env.api_error_status.clone().unwrap_or_default()
            } else {
                env.result.clone()
            };
            return InferenceError::RateLimited {
                message: truncate(&raw, 512).to_string(),
            };
        }
        // Other envelope-reported error — fall through to BadEnvelope.
        return InferenceError::BadEnvelope {
            details: format!(
                "envelope reported is_error=true ({}, status={:?}): {}",
                env.subtype,
                env.api_error_status,
                truncate(&env.result, 256)
            ),
        };
    }

    if !exit_status.success() {
        return InferenceError::Spawn(format!(
            "claude exited with {} (stderr first 256 chars: {})",
            exit_status,
            truncate(stderr, 256)
        ));
    }

    InferenceError::BadEnvelope {
        details: "successful exit but no parseable envelope".to_string(),
    }
}

/// Async `claude --version` probe. Only runs on the version-mismatch
/// failure path to enrich `ClaudeCodeTooOld`. Uses the async
/// `tokio::process::Command` so it never blocks a runtime worker even
/// if several streams hit the version-mismatch path concurrently.
async fn probe_claude_version(claude_path: &Path) -> Option<String> {
    let output = Command::new(claude_path)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Return the JSON Schema string for `T`, computing it once per
/// concrete type and caching the result. `schema_for!` plus
/// serialization is deterministic, so the cached string is reused for
/// every subsequent `invoke_typed::<T>` call.
fn cached_schema<T>() -> Result<String, InferenceError>
where
    T: schemars::JsonSchema + 'static,
{
    let key = std::any::type_name::<T>();
    {
        let cache = SCHEMA_CACHE.lock().expect("schema cache mutex poisoned");
        if let Some(schema) = cache.get(key) {
            return Ok(schema.clone());
        }
    }
    let schema = serde_json::to_string(&schemars::schema_for!(T)).map_err(|e| {
        InferenceError::SchemaValidationFailed {
            details: format!("schema generation failed: {e}"),
        }
    })?;
    SCHEMA_CACHE
        .lock()
        .expect("schema cache mutex poisoned")
        .insert(key, schema.clone());
    Ok(schema)
}

// ---------------------------------------------------------------------------
// Test-only inference seam (T011 / R-4 Decision 5).
//
// A `#[cfg(test)]`-gated injectable double that lets the learning-logic and
// evaluation-harness suites exercise synthesis/eval code paths fully offline,
// with NO live `claude` process. The slot is consulted at the very top of
// `invoke_typed` / `invoke_text` (before the schema cache, the per-call
// `TempDir`, and `invoke_raw`'s spawn) and returns a scripted result.
//
// This entire block — and every reference to it — is compiled out of
// non-`test` builds. The production code path below is byte-for-byte
// unchanged: when the `#[cfg(test)]` consult helper does not exist, the
// function bodies fall straight through to the real spawn. The public
// free-fn signatures of `invoke_typed`/`invoke_text` are NOT touched, so
// the real callers (`learning.rs`, `memory_optimizer.rs`) need no changes.
//
// Generic `T` handling: scripted typed responses are stored as a
// `serde_json::Value` and deserialized via `serde_json::from_value::<T>`,
// which fits the existing `T: Deserialize` bound exactly (no new bound, no
// `T: Serialize` requirement). A scripted error short-circuits with that
// `InferenceError`. Responses are queued FIFO so a test can script a
// multi-call sequence (e.g. WITH/WITHOUT + judge in the eval harness).
// ---------------------------------------------------------------------------

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum ScriptedResponse {
    /// Deserialized into the caller's `T` (typed) — invalid for `invoke_text`.
    TypedJson(Value),
    /// Returned verbatim as the string body for `invoke_text` — invalid for
    /// `invoke_typed`.
    Text(String),
    /// Propagated as-is from `invoke_typed`/`invoke_text`.
    Err(InferenceError),
}

#[cfg(test)]
static INFERENCE_DOUBLE: LazyLock<Mutex<Option<std::collections::VecDeque<ScriptedResponse>>>> =
    LazyLock::new(|| Mutex::new(None));

/// TEST-ONLY: install a queue of scripted responses. Each subsequent
/// `invoke_typed`/`invoke_text` call dequeues one (FIFO) instead of
/// spawning `claude`. Replaces any previously installed script. Use with
/// `#[serial]` since the slot is process-global.
#[cfg(test)]
pub(crate) fn set_inference_double(responses: Vec<ScriptedResponse>) {
    *INFERENCE_DOUBLE
        .lock()
        .expect("inference double mutex poisoned") = Some(responses.into_iter().collect());
}

/// TEST-ONLY: remove the scripted double; subsequent calls take the real
/// `claude` spawn path again. Call in test teardown to avoid cross-test
/// bleed.
#[cfg(test)]
pub(crate) fn clear_inference_double() {
    *INFERENCE_DOUBLE
        .lock()
        .expect("inference double mutex poisoned") = None;
}

/// TEST-ONLY: RAII teardown for the process-global inference double. Holding
/// this guard guarantees the scripted queue is cleared when it drops —
/// including on a panic or an early `?`/`return` — so a forgotten manual
/// `clear_inference_double()` can no longer leave a stale queue for the next
/// `#[serial]` test to silently consume (which would be a false pass and
/// erode the FR-021 CI gate). Prefer `set_inference_double_scoped` over the
/// bare `set_inference_double` for exactly this reason.
#[cfg(test)]
pub(crate) struct InferenceDoubleGuard;

#[cfg(test)]
impl Drop for InferenceDoubleGuard {
    fn drop(&mut self) {
        clear_inference_double();
    }
}

/// TEST-ONLY (recommended path): install a queue of scripted responses and
/// return an [`InferenceDoubleGuard`] that auto-clears it on drop. Bind it to
/// a named local (`let _guard = set_inference_double_scoped(...);`) for the
/// duration of the test; the double is torn down deterministically even if
/// the test panics or returns early. Still requires `#[serial]` since the
/// slot is process-global. Replaces any previously installed script.
#[cfg(test)]
#[must_use = "drop of the returned guard is what clears the inference double; bind it to a local"]
pub(crate) fn set_inference_double_scoped(
    responses: Vec<ScriptedResponse>,
) -> InferenceDoubleGuard {
    set_inference_double(responses);
    InferenceDoubleGuard
}

/// Pop the next scripted response if a double is installed. `None` (no
/// double, or queue drained) means "fall through to the real path".
#[cfg(test)]
fn next_scripted_response() -> Option<ScriptedResponse> {
    let mut guard = INFERENCE_DOUBLE
        .lock()
        .expect("inference double mutex poisoned");
    guard
        .as_mut()
        .and_then(std::collections::VecDeque::pop_front)
}

/// Synthetic metadata for a doubled call: no envelope exists, so report a
/// successful zero-cost call tagged with the requesting phase. Mirrors the
/// shape `failed_metadata` / `metadata_from_envelope` produce — including the
/// recorded host-level `sandbox` kind — so callers that persist
/// `inference_metadata` behave identically offline and the SC-013 invariant
/// (confinement state present on every record) holds even on the test seam.
#[cfg(test)]
fn doubled_metadata(phase: Phase, max_tokens_requested: u64) -> InferenceCallMetadata {
    InferenceCallMetadata {
        phase: phase.as_str(),
        max_tokens_requested,
        success: true,
        sandbox: Some(detect_sandbox_kind().as_str().to_string()),
        ..InferenceCallMetadata::default()
    }
}

// ---------------------------------------------------------------------------
// Public surface.
// ---------------------------------------------------------------------------

/// One-shot Claude Code invocation with JSON-Schema-validated output.
/// Drop-in replacement for the prior `ai_client::analyze_typed::<T>`.
pub async fn invoke_typed<T>(args: InvokeArgs) -> Result<InvokeOutcome<T>, InferenceError>
where
    T: for<'de> Deserialize<'de> + schemars::JsonSchema + Send + Sync + 'static,
{
    // TEST-ONLY seam: compiled out of non-test builds. Placed before any
    // production work (schema cache, TempDir, spawn) so the doubled path
    // never touches the real subprocess and the production path is
    // byte-for-byte unchanged when no double is installed.
    #[cfg(test)]
    if let Some(scripted) = next_scripted_response() {
        return match scripted {
            ScriptedResponse::TypedJson(value) => {
                let value: T = serde_json::from_value(value).map_err(|e| {
                    InferenceError::SchemaValidationFailed {
                        details: format!("scripted double value did not match target type: {e}"),
                    }
                })?;
                Ok(InvokeOutcome {
                    value,
                    metadata: doubled_metadata(args.phase, args.max_tokens),
                })
            }
            ScriptedResponse::Err(err) => Err(err),
            ScriptedResponse::Text(_) => Err(InferenceError::SchemaValidationFailed {
                details: "scripted Text response used with invoke_typed (expected TypedJson)"
                    .to_string(),
            }),
        };
    }

    let schema = cached_schema::<T>()?;

    // Per-call sandbox. `TempDir::drop` deletes the directory and its
    // contents unconditionally — every `?` early return, the timeout
    // path, and panics included. This IS the design's drop-guard.
    let dir = tempfile::Builder::new()
        .prefix("quill-cc-")
        .tempdir()
        .map_err(|e| InferenceError::Spawn(format!("temp dir create failed: {e}")))?;
    let out_path = dir.path().join("out.json");

    // The schema is the binding contract (the CLI does not enforce
    // `--json-schema`). Delivery is a Write tool action, not prose.
    let mut args = args;
    args.prompt = format!(
        "{prompt}\n\n## Output contract\n\
         Produce a single JSON value that strictly conforms to this JSON Schema:\n\
         {schema}\n\n\
         Every required field MUST be present with the correct type. No extra \
         fields, no markdown, no prose. Use the Write tool to write ONLY that \
         JSON to the absolute path {out}. Then re-read that file and confirm it \
         parses and satisfies the schema before finishing. Do not print the \
         JSON in your reply.",
        prompt = args.prompt,
        out = out_path.display(),
    );

    let (envelope, sandbox) = invoke_raw(&args, Some(dir.path())).await?;

    // std::fs (tokio "fs" feature is not enabled); out.json is a tiny
    // local file so the brief blocking read is acceptable. The agent
    // wrote it through the bwrap `--bind` of `dir` (a real bind mount of
    // this host dir, not a tmpfs), so it is present on the host fs here.
    let raw =
        std::fs::read_to_string(&out_path).map_err(|e| InferenceError::SchemaValidationFailed {
            details: format!(
                "agent did not produce {out} ({e}); stop_reason={sr:?}, \
                 result preview: {rp}",
                out = out_path.display(),
                sr = envelope.stop_reason,
                rp = truncate(&envelope.result, 256),
            ),
        })?;
    let value: T =
        serde_json::from_str(&raw).map_err(|e| InferenceError::SchemaValidationFailed {
            details: format!(
                "artifact did not match target type: {e} (first 256 chars: {})",
                truncate(&raw, 256)
            ),
        })?;
    let metadata = metadata_from_envelope(args.phase, args.max_tokens, &envelope, sandbox);
    Ok(InvokeOutcome { value, metadata })
}

/// One-shot Claude Code invocation that returns the model's reply
/// verbatim as a string. Drop-in replacement for the prior
/// `ai_client::complete_text`.
pub async fn invoke_text(args: InvokeArgs) -> Result<InvokeOutcome<String>, InferenceError> {
    // TEST-ONLY seam: see `invoke_typed`. Compiled out of non-test builds.
    #[cfg(test)]
    if let Some(scripted) = next_scripted_response() {
        return match scripted {
            ScriptedResponse::Text(body) => Ok(InvokeOutcome {
                value: body,
                metadata: doubled_metadata(args.phase, args.max_tokens),
            }),
            ScriptedResponse::Err(err) => Err(err),
            ScriptedResponse::TypedJson(_) => Err(InferenceError::SchemaValidationFailed {
                details: "scripted TypedJson response used with invoke_text (expected Text)"
                    .to_string(),
            }),
        };
    }

    let (envelope, sandbox) = invoke_raw(&args, None).await?;
    let metadata = metadata_from_envelope(args.phase, args.max_tokens, &envelope, sandbox);
    Ok(InvokeOutcome {
        value: envelope.result,
        metadata,
    })
}

/// Enrich a freshly classified error: if it is `ClaudeCodeTooOld`
/// without a detected version, run the async version probe to fill it
/// in. Keeps the synchronous `classify_error` free of blocking calls.
async fn enrich_error(err: InferenceError, claude_path: &Path) -> InferenceError {
    match err {
        InferenceError::ClaudeCodeTooOld {
            detected_version: None,
        } => InferenceError::ClaudeCodeTooOld {
            detected_version: probe_claude_version(claude_path).await,
        },
        other => other,
    }
}

/// Spawn `claude` and return the parsed envelope together with the OS
/// confinement that was actually applied to the child. The sandbox kind is
/// surfaced on the success path so `metadata_from_envelope` records the real
/// boundary; the error paths route through `failed_metadata`, which re-probes
/// the (deterministic, host-level) kind, so SC-013 holds on every exit.
async fn invoke_raw(
    args: &InvokeArgs,
    artifact_dir: Option<&Path>,
) -> Result<(Envelope, SandboxKind), InferenceError> {
    // Resolve the `claude` binary via the project's cached,
    // login-shell-aware resolver (R-12). This picks up Anthropic's
    // `claude migrate-installer` target and auto-refreshes when the
    // user triggers a PATH rescan from the integrations menu.
    let claude_path = match crate::config::resolve_command_path("claude") {
        Some(path) => path,
        None => return Err(InferenceError::ClaudeCodeMissing),
    };

    let (mut cmd, sandbox) = build_command(args, artifact_dir, &claude_path);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(InferenceError::ClaudeCodeMissing);
        }
        Err(e) => return Err(InferenceError::Spawn(e.to_string())),
    };

    // Write the prompt to stdin and collect output concurrently in the
    // same future, not via a detached `tokio::spawn`. For large prompts
    // (Stream A can reach 50-150 KB), the OS pipe buffer can fill before
    // the child fully drains stdin; if the child is also producing
    // stdout, both sides can stall. By joining the stdin-write with
    // `wait_with_output` (which drains stdout and stderr in the
    // background), the two halves cannot deadlock. Write errors are
    // logged but do not preempt the child's output — the most
    // informative failure is typically in the envelope.
    let stdin = child.stdin.take();
    let prompt = args.prompt.clone();
    let stdin_writer = async move {
        if let Some(mut stdin) = stdin {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await?;
        }
        Ok::<(), std::io::Error>(())
    };

    let work = async move {
        let (write_result, output) = tokio::join!(stdin_writer, child.wait_with_output());
        (write_result, output)
    };

    let (write_result, output) = match tokio::time::timeout(INVOCATION_TIMEOUT, work).await {
        Ok((write_result, Ok(output))) => (write_result, output),
        Ok((_, Err(e))) => return Err(InferenceError::Spawn(e.to_string())),
        Err(_) => {
            // The whole future was dropped, which kills the child via
            // kill_on_drop and aborts the stdin writer.
            return Err(InferenceError::TimedOut {
                after: INVOCATION_TIMEOUT,
            });
        }
    };

    if let Err(e) = write_result {
        // Broken-pipe is expected when the child exits early (e.g.
        // schema error before stdin is fully read). Log at debug; the
        // child's actual failure surfaces below via the envelope.
        log::debug!("cc_client: stdin write returned {e}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Try to parse the envelope first because the classify_error logic
    // wants to inspect it.
    let envelope_result = parse_envelope(&stdout);

    if !output.status.success() {
        // Feature 007 C-C2 / C-E4: if the active mechanism was Bwrap and
        // it failed to spawn (no parseable envelope on stdout means the
        // outer bwrap wrapper, not `claude`, produced the failure), latch
        // the host as bwrap-broken and emit the one-shot diagnostic.
        // Subsequent calls in this Quill process will skip the bwrap
        // wrapping entirely. The captured stderr drives the
        // AppArmor-vs-other classification.
        #[cfg(target_os = "linux")]
        if matches!(sandbox, SandboxKind::Bwrap) && envelope_result.is_err() {
            let cause = classify_bwrap_failure(&stderr);
            // First writer wins; ignore the already-set Err.
            let _ = BWRAP_BROKEN_ON_THIS_HOST.set(cause);
            let _ = emit_no_confinement_diagnostic(Some(cause), Some(&stderr));
        }
        let err = classify_error(output.status, &stderr, envelope_result.as_ref().ok());
        return Err(enrich_error(err, &claude_path).await);
    }

    let envelope = match envelope_result {
        Ok(env) => env,
        Err(InferenceError::BadEnvelope { details }) => {
            // Success exit with unparseable stdout: surface the stderr so
            // silent-exit failures (e.g. sandbox-denied paths the launcher
            // swallowed) are diagnosable from the run history error column.
            return Err(InferenceError::BadEnvelope {
                details: format!(
                    "{details} | exit={} stderr first 1024 chars: {}",
                    output.status,
                    truncate(&stderr, 1024)
                ),
            });
        }
        Err(e) => return Err(e),
    };

    if envelope.is_error {
        let err = classify_error(output.status, &stderr, Some(&envelope));
        return Err(enrich_error(err, &claude_path).await);
    }
    if envelope.type_ != "result" {
        return Err(InferenceError::BadEnvelope {
            details: format!(
                "expected envelope type=result, got {} subtype={}",
                envelope.type_, envelope.subtype
            ),
        });
    }

    Ok((envelope, sandbox))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn exit_ok() -> ExitStatus {
        ExitStatus::from_raw(0)
    }

    fn exit_fail() -> ExitStatus {
        // Unix wait status: exit code N is encoded as N << 8.
        ExitStatus::from_raw(1 << 8)
    }

    #[test]
    fn parse_envelope_full_extracts_fields() {
        let json = r#"{"type":"result","subtype":"success","is_error":false,
            "api_error_status":null,"duration_ms":1777,"duration_api_ms":1636,
            "ttft_ms":1738,"result":"PONG","stop_reason":"end_turn",
            "total_cost_usd":0.008,"usage":{"input_tokens":10,"output_tokens":44,
            "cache_creation_input_tokens":6252,"cache_read_input_tokens":0,
            "service_tier":"standard"},
            "modelUsage":{"claude-haiku-4-5-20251001":{"costUSD":0.008}},
            "permission_denials":[]}"#;
        let env = parse_envelope(json).expect("valid envelope");
        assert_eq!(env.result, "PONG");
        assert!(!env.is_error);
        let meta = metadata_from_envelope(Phase::StreamA, 4096, &env, SandboxKind::Bwrap);
        assert_eq!(meta.input_tokens, 10);
        assert_eq!(meta.output_tokens, 44);
        assert_eq!(meta.cache_creation_input_tokens, 6252);
        assert_eq!(meta.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert!(meta.success);
        assert_eq!(meta.phase, "stream_a");
        // The applied confinement is threaded onto the success metadata.
        assert_eq!(meta.sandbox.as_deref(), Some("bwrap"));
    }

    #[test]
    fn parse_envelope_minimal_applies_defaults() {
        let env = parse_envelope(r#"{"type":"result","result":"hi"}"#)
            .expect("minimal envelope still parses via serde defaults");
        assert_eq!(env.result, "hi");
        assert_eq!(env.duration_ms, 0);
        assert_eq!(env.usage.input_tokens, 0);
        assert!(env.permission_denials.is_empty());
    }

    #[test]
    fn parse_envelope_rejects_garbage() {
        let err = parse_envelope("not json at all").unwrap_err();
        assert!(matches!(err, InferenceError::BadEnvelope { .. }));
    }

    #[test]
    fn metadata_model_is_none_when_model_usage_empty() {
        let env = parse_envelope(r#"{"type":"result","result":"x","modelUsage":{},"usage":{}}"#)
            .expect("valid");
        let meta = metadata_from_envelope(Phase::Synthesis, 8192, &env, SandboxKind::None);
        assert_eq!(meta.model, None);
        assert_eq!(meta.phase, "synthesis");
        assert_eq!(meta.max_tokens_requested, 8192);
        // Even the degraded ("none") confinement is recorded, never absent.
        assert_eq!(meta.sandbox.as_deref(), Some("none"));
    }

    // --- R-7.7 / L-2: multi-model cost-tiebreak attribution ----------------

    #[test]
    fn metadata_model_is_highest_cost_entry_not_first_key() {
        // Agent-mode envelope: `modelUsage` carries TWO models. The
        // alphabetically/BTreeMap-iteration-first key is the cheap
        // tool-loop helper (haiku); the costly primary that did the
        // generation (sonnet) sorts later. Regression for 46a13bc: the
        // run MUST be attributed to the highest-`costUSD` entry, never
        // the first BTreeMap key (which would always misreport haiku).
        let json = r#"{"type":"result","subtype":"success","is_error":false,
            "duration_ms":4200,"result":"ok","stop_reason":"end_turn",
            "total_cost_usd":0.051,"usage":{"input_tokens":120,
            "output_tokens":800},
            "modelUsage":{
              "claude-haiku-4-5-20251001":{"costUSD":0.001,
                "inputTokens":40,"outputTokens":20},
              "claude-sonnet-4-6-20251101":{"costUSD":0.05,
                "inputTokens":80,"outputTokens":780}},
            "permission_denials":[]}"#;
        let env = parse_envelope(json).expect("valid multi-model envelope");
        // BTreeMap iteration order is haiku-first; the fix must still
        // pick sonnet (the expensive primary), proving cost — not key
        // order — drives attribution.
        assert_eq!(
            env.model_usage.keys().next().map(String::as_str),
            Some("claude-haiku-4-5-20251001"),
            "precondition: cheap helper is the first BTreeMap key"
        );
        let meta = metadata_from_envelope(Phase::Synthesis, 8192, &env, SandboxKind::Bwrap);
        assert_eq!(
            meta.model.as_deref(),
            Some("claude-sonnet-4-6-20251101"),
            "run must be attributed to the highest-cost (primary) model"
        );
        assert!(meta.success);
        assert_eq!(meta.phase, "synthesis");
    }

    #[test]
    fn metadata_model_tiebreak_is_deterministic_on_equal_or_absent_cost() {
        // Companion edge case: when `costUSD` ties (or is absent →
        // treated as 0.0) the result must still be deterministic and
        // `Some`. `model_usage` is a BTreeMap, so `iter()` yields keys
        // in ascending order; `max_by` returns the LAST of equally
        // maximal elements. The OBSERVED deterministic outcome is
        // therefore the lexicographically-greatest key — asserted here
        // as actual behavior (the 46a13bc intent is silent on ties;
        // there is no "highest cost" to honor). `aaa-model` sorts first
        // and is expected to LOSE the tie to `zzz-model`.

        // Both entries carry equal explicit costUSD.
        let equal = r#"{"type":"result","result":"x",
            "modelUsage":{
              "aaa-model-1":{"costUSD":0.02},
              "zzz-model-2":{"costUSD":0.02}},
            "usage":{}}"#;
        let env = parse_envelope(equal).expect("valid equal-cost envelope");
        let meta = metadata_from_envelope(Phase::StreamB, 4096, &env, SandboxKind::None);
        assert_eq!(
            meta.model.as_deref(),
            Some("zzz-model-2"),
            "equal-cost tie deterministically resolves to the last \
             BTreeMap key (lexicographically greatest)"
        );

        // Neither entry has costUSD → both default to 0.0 → same tie,
        // same deterministic resolution; result is still Some.
        let absent = r#"{"type":"result","result":"x",
            "modelUsage":{
              "aaa-model-1":{"inputTokens":10},
              "zzz-model-2":{"inputTokens":10}},
            "usage":{}}"#;
        let env = parse_envelope(absent).expect("valid absent-cost envelope");
        let meta = metadata_from_envelope(Phase::StreamB, 4096, &env, SandboxKind::None);
        assert_eq!(
            meta.model.as_deref(),
            Some("zzz-model-2"),
            "absent costUSD defaults to 0.0; tie resolves identically \
             and deterministically (never None when the map is non-empty)"
        );
    }

    #[test]
    fn classify_error_detects_version_mismatch() {
        let err = classify_error(exit_fail(), "error: unknown option '--json-schema'", None);
        assert!(matches!(
            err,
            InferenceError::ClaudeCodeTooOld {
                detected_version: None
            }
        ));
    }

    #[test]
    fn classify_error_auth_only_when_process_failed() {
        // Regression guard: a *successful* exit whose stderr happens to
        // contain an auth-looking string must NOT be classified as
        // NotSignedIn (the envelope path owns success classification).
        let err = classify_error(exit_ok(), "note: run claude /login someday", None);
        assert!(!matches!(err, InferenceError::NotSignedIn));

        // But a failed exit with the same stderr IS NotSignedIn.
        let err = classify_error(exit_fail(), "Error: not authenticated", None);
        assert!(matches!(err, InferenceError::NotSignedIn));
    }

    #[test]
    fn classify_error_rate_limit_via_status() {
        let env = parse_envelope(
            r#"{"type":"result","subtype":"error","is_error":true,
               "api_error_status":"429","result":"slow down"}"#,
        )
        .expect("valid");
        let err = classify_error(exit_ok(), "", Some(&env));
        match err {
            InferenceError::RateLimited { message } => assert_eq!(message, "slow down"),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn classify_error_rate_limit_message_is_truncated() {
        let big = "rate limit ".to_string() + &"x".repeat(5000);
        let env = Envelope {
            type_: "result".into(),
            subtype: "error".into(),
            is_error: true,
            result: big,
            ..Envelope::default()
        };
        let err = classify_error(exit_ok(), "", Some(&env));
        match err {
            InferenceError::RateLimited { message } => {
                assert!(message.len() <= 512, "got {} bytes", message.len());
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    // --- T011: test-only inference double seam ----------------------------

    use crate::models::StreamFindings;
    use serial_test::serial;

    fn probe_args(phase: Phase) -> InvokeArgs {
        InvokeArgs {
            phase,
            prompt: "unused — the double short-circuits before spawn".into(),
            preamble: String::new(),
            model: Model::Sonnet46,
            max_tokens: 4096,
        }
    }

    #[tokio::test]
    #[serial]
    async fn inference_double_returns_scripted_typed_value_without_spawn() {
        // A StreamFindings-shaped JSON payload — the same type Stream A/B/C
        // synthesis pulls through invoke_typed. With the double installed
        // there is no `claude` on the test PATH guarantee needed: the seam
        // returns before invoke_raw, so no process is ever spawned.
        let scripted = serde_json::json!({
            "patterns": [{
                "name": "prefer-absolute-paths",
                "domain": "tooling",
                "description": "Use absolute paths in agent file ops.",
                "evidence": "obs:42; obs:57",
                "confidence": 0.81,
                "is_anti_pattern": false
            }],
            "verdicts": [{ "name": "old-rule", "verdict": "confirm", "strength": 0.7 }]
        });
        // Scoped guard: the double is torn down on drop even if an
        // assertion below panics, so a later `#[serial]` test can never
        // silently consume a stale scripted queue (FR-021 CI gate).
        let _double = set_inference_double_scoped(vec![ScriptedResponse::TypedJson(scripted)]);

        // `InvokeOutcome<T>` is intentionally not `Debug` (production
        // type), so destructure rather than `.expect()`.
        let outcome = match invoke_typed::<StreamFindings>(probe_args(Phase::StreamA)).await {
            Ok(o) => o,
            Err(e) => panic!("expected scripted Ok value, got {e:?}"),
        };
        assert_eq!(outcome.value.patterns.len(), 1);
        assert_eq!(outcome.value.patterns[0].name, "prefer-absolute-paths");
        assert_eq!(outcome.value.verdicts.len(), 1);
        assert_eq!(outcome.value.verdicts[0].verdict, "confirm");
        // Synthetic metadata is success-tagged with the requesting phase.
        assert!(outcome.metadata.success);
        assert_eq!(outcome.metadata.phase, "stream_a");
        assert_eq!(outcome.metadata.max_tokens_requested, 4096);

        // Queue drained → next call would fall through to the real path,
        // so re-arm a single error response and confirm it propagates
        // verbatim (no process spawn, deterministic). Re-arming just
        // replaces the queue contents; `_double` still owns teardown, so
        // no trailing manual `clear_inference_double()` is needed — the
        // guard restores the real path on scope exit (incl. panic).
        set_inference_double(vec![ScriptedResponse::Err(InferenceError::RateLimited {
            message: "scripted overload".into(),
        })]);
        match invoke_typed::<StreamFindings>(probe_args(Phase::Synthesis)).await {
            Err(InferenceError::RateLimited { message }) => {
                assert_eq!(message, "scripted overload")
            }
            Err(other) => panic!("expected scripted RateLimited, got {other:?}"),
            Ok(_) => panic!("expected scripted Err, got Ok"),
        }
    }

    #[tokio::test]
    #[serial]
    async fn inference_double_text_and_clear_restores_real_path() {
        // This test's SUBJECT is the explicit `clear_inference_double()`
        // below (it asserts the slot empties), so the manual clear stays.
        // The guard is still bound for panic-safety: if an assertion before
        // the clear panics, teardown still runs. The guard's final clear is
        // idempotent, so the redundant second clear is harmless.
        let _double =
            set_inference_double_scoped(vec![ScriptedResponse::Text("scripted reply".into())]);
        let outcome = match invoke_text(probe_args(Phase::ProseCompression)).await {
            Ok(o) => o,
            Err(e) => panic!("expected scripted text Ok, got {e:?}"),
        };
        assert_eq!(outcome.value, "scripted reply");
        assert!(outcome.metadata.success);
        assert_eq!(outcome.metadata.phase, "prose_compression");

        // After clearing, the slot is empty so `next_scripted_response`
        // returns None and the production code path is taken again. We do
        // NOT invoke past the seam here (that would spawn a real `claude`);
        // asserting the slot is empty proves the seam is disengaged.
        clear_inference_double();
        assert!(
            next_scripted_response().is_none(),
            "cleared double must not yield a scripted response"
        );
    }

    // --- T022: OS sandbox confinement state recorded (H-5 / FR-005 / SC-013)

    /// Every metadata record this build produces — including via the offline
    /// double and the `failed_metadata` error path — MUST carry a non-empty
    /// `sandbox` string drawn from the closed vocabulary, and on the current
    /// platform it must be one of that platform's expected mechanisms. This
    /// makes SC-013 ("confinement state recorded for 100% of analysis runs on
    /// every platform") verifiable without a live `claude` spawn.
    #[tokio::test]
    #[serial]
    async fn sandbox_metadata_is_recorded_for_every_call() {
        // The complete closed set serialized by `SandboxKind::as_str`
        // (feature 007 C-A2). `sandbox` must always be one of these.
        const ALL: &[&str] = &["landlock", "bwrap", "sandbox-exec", "job-object", "none"];
        // Mechanisms reachable on the platform this test binary runs on.
        // Linux chain (feature 007 C-A1): Landlock → Bwrap → None; macOS
        // sandbox-exec→none; Windows is always job-object; other targets
        // always none.
        let platform_expected: &[&str] = if cfg!(target_os = "linux") {
            &["landlock", "bwrap", "none"]
        } else if cfg!(target_os = "macos") {
            &["sandbox-exec", "none"]
        } else if cfg!(target_os = "windows") {
            &["job-object"]
        } else {
            &["none"]
        };

        // `as_str` round-trips every variant to a member of the closed set,
        // and the host probe never yields a value outside the platform set.
        for k in [
            SandboxKind::Landlock,
            SandboxKind::Bwrap,
            SandboxKind::SandboxExec,
            SandboxKind::JobObject,
            SandboxKind::None,
        ] {
            assert!(ALL.contains(&k.as_str()), "{k:?} not in closed set");
        }
        let detected = detect_sandbox_kind().as_str();
        assert!(
            platform_expected.contains(&detected),
            "host probe yielded `{detected}`, not one of {platform_expected:?}"
        );

        // Success path (doubled — no real spawn): metadata.sandbox populated.
        // Scoped guard auto-clears on drop even if an assertion below panics.
        let _double = set_inference_double_scoped(vec![ScriptedResponse::Text("ok".into())]);
        let outcome = match invoke_text(probe_args(Phase::StreamA)).await {
            Ok(o) => o,
            Err(e) => panic!("expected scripted Ok, got {e:?}"),
        };
        let sb = outcome
            .metadata
            .sandbox
            .as_deref()
            .expect("sandbox must be recorded on the success metadata");
        assert!(!sb.is_empty(), "sandbox string must be non-empty");
        assert!(ALL.contains(&sb), "`{sb}` not in the closed sandbox set");
        assert!(
            platform_expected.contains(&sb),
            "`{sb}` not a valid mechanism for this platform {platform_expected:?}"
        );

        // Failure path: `failed_metadata` must ALSO record the confinement
        // state (SC-013 = 100% of runs, including failed ones). It re-probes
        // the deterministic host-level kind, so it agrees with the detection.
        let fm = failed_metadata(Phase::Synthesis, 4096, &InferenceError::ClaudeCodeMissing);
        let fsb = fm
            .sandbox
            .as_deref()
            .expect("sandbox must be recorded on failed metadata too");
        assert!(!fsb.is_empty(), "failed-path sandbox must be non-empty");
        assert_eq!(
            fsb, detected,
            "failed_metadata sandbox must match the host probe"
        );

        // No trailing manual clear: `_double` restores the real path on
        // scope exit (and on panic), which is the whole point of the guard.
    }

    /// Feature 006 Follow-up A (R-A / C-A1 / C-A2) extended by feature 007
    /// C-A2 / C-A3. Pure, host-independent mapping check: for EVERY
    /// `SandboxKind` variant, `as_str()` is a member of the closed
    /// vocabulary AND `sandbox_tag_is_fs_confined(as_str())` matches the
    /// FS/non-FS classification table (`Landlock`/`Bwrap`/`SandboxExec` ⇒
    /// true; `JobObject`/`None` ⇒ false). No spawn, no host probe, no
    /// `#[serial]` — the closed set + classification are total over the
    /// enum so a future variant or a wrong classifier arm fails.
    #[test]
    fn sandbox_kind_as_str_and_fs_confinement_mapping_is_total() {
        const CLOSED_SET: &[&str] = &["landlock", "bwrap", "sandbox-exec", "job-object", "none"];
        // (variant, expected as_str, expected fs_confined)
        let cases = [
            (SandboxKind::Landlock, "landlock", true),
            (SandboxKind::Bwrap, "bwrap", true),
            (SandboxKind::SandboxExec, "sandbox-exec", true),
            (SandboxKind::JobObject, "job-object", false),
            (SandboxKind::None, "none", false),
        ];
        for (kind, expect_tag, expect_fs) in cases {
            assert_eq!(
                kind.as_str(),
                expect_tag,
                "{kind:?} must serialize to its honest tag"
            );
            assert!(
                CLOSED_SET.contains(&kind.as_str()),
                "{kind:?} tag `{}` escaped the closed vocabulary",
                kind.as_str()
            );
            assert_eq!(
                sandbox_tag_is_fs_confined(kind.as_str()),
                expect_fs,
                "{kind:?} filesystem-confinement classification is wrong \
                 (only landlock/bwrap/sandbox-exec deny out-of-workspace FS R/W)"
            );
        }
    }

    // --- T012, T013, T020-T022: feature 007 Landlock + diagnostic tests ----

    /// Feature 007 T012 (C-B1 / C-B3). On a Landlock-supported Linux host,
    /// the default per-call policy + the ruleset builder produce an
    /// `Ok(RulesetCreated)` without ever calling `restrict_self`. On hosts
    /// without Landlock support the test early-returns Ok (we cannot exercise
    /// the build path; the production code falls through to bwrap).
    #[cfg(target_os = "linux")]
    #[test]
    fn build_ruleset_succeeds_with_default_policy_on_landlock_host() {
        if !probe_landlock_available() {
            // Host doesn't support Landlock — production code falls through
            // to bwrap. This test cannot exercise the build path here; that
            // is by design (we never call restrict_self in any test).
            return;
        }
        let rw = tempfile::tempdir().expect("tempdir");
        // Any system binary path the host actually has resolves
        // `claude_install_root` to something sensible (or None — both are
        // tolerated by build_ruleset).
        let claude_path = std::path::PathBuf::from("/bin/true");
        let policy = LandlockPolicy::default_for_call(rw.path(), &claude_path);
        let result = build_ruleset(&policy);
        assert!(
            result.is_ok(),
            "build_ruleset must succeed on a Landlock-supported host (err: {:?})",
            result.err()
        );
    }

    /// Feature 007 T013 (R-C, C-B3 silent-skip behavior). When the RO set
    /// includes a path that does not exist, the crate's `path_beneath_rules`
    /// helper silently skips it (documented behavior); the ruleset build
    /// still succeeds. Verifies the "absent optional paths" guarantee
    /// without coupling to internal error mapping.
    #[cfg(target_os = "linux")]
    #[test]
    fn build_ruleset_skips_absent_optional_paths_without_error() {
        if !probe_landlock_available() {
            return;
        }
        let rw = tempfile::tempdir().expect("tempdir");
        let policy = LandlockPolicy {
            ro_paths: vec![
                std::path::PathBuf::from("/usr"),
                // Deliberately non-existent — must be silently skipped.
                std::path::PathBuf::from("/nix-does-not-exist-feature-007"),
            ],
            rw_paths: vec![rw.path().to_path_buf()],
            abi: landlock::ABI::V3,
        };
        let result = build_ruleset(&policy);
        assert!(
            result.is_ok(),
            "missing optional RO paths must be skipped silently, not errored \
             (err: {:?})",
            result.err()
        );
    }

    /// Feature 007 T020 (C-E1). Pure classifier: both known bwrap
    /// userns-restriction signatures (the dev-host one and the Codex-blog
    /// loopback variant) classify as AppArmor.
    #[test]
    fn classify_bwrap_failure_detects_apparmor_userns_signature() {
        // The dev-host signature (run-49 evidence). Embedded in a realistic
        // bwrap stderr block to verify substring matching, not full-line.
        let stderr_a = "bwrap: setting up uid map: Permission denied";
        assert_eq!(
            classify_bwrap_failure(stderr_a),
            BwrapBrokenCause::AppArmorRestrictUserns,
            "dev-host uid-map signature must classify as AppArmor"
        );
        // The Codex-blog signature.
        let stderr_b = "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted";
        assert_eq!(
            classify_bwrap_failure(stderr_b),
            BwrapBrokenCause::AppArmorRestrictUserns,
            "Codex-blog loopback signature must classify as AppArmor"
        );
        // Signature embedded in a multi-line stderr — still detected.
        let stderr_c = "some preamble\nbwrap: setting up uid map: Permission denied\ntrailer";
        assert_eq!(
            classify_bwrap_failure(stderr_c),
            BwrapBrokenCause::AppArmorRestrictUserns,
            "substring match must succeed across line boundaries"
        );
    }

    /// Feature 007 T021 (C-E1 negative case). Unrelated stderr classifies
    /// as `Other` (the generic FR-014 template will be used).
    #[test]
    fn classify_bwrap_failure_returns_other_for_unrelated_stderr() {
        let stderr = "bwrap: cannot find `claude` on PATH";
        assert_eq!(
            classify_bwrap_failure(stderr),
            BwrapBrokenCause::Other,
            "unrelated bwrap error must classify as Other (generic template)"
        );
        assert_eq!(
            classify_bwrap_failure(""),
            BwrapBrokenCause::Other,
            "empty stderr classifies as Other"
        );
        assert_eq!(
            classify_bwrap_failure("some random rust panic message"),
            BwrapBrokenCause::Other,
            "non-bwrap stderr classifies as Other"
        );
    }

    /// Feature 007 T022 (C-E2). The `OnceLock` latch makes the diagnostic
    /// emitter one-shot per Quill process; the second call returns `None`.
    /// The formatted message includes the contract-mandated stable
    /// substrings (C-E3) keyed on cause.
    ///
    /// Stable-substring testing uses `format_no_confinement_diagnostic`
    /// directly so it can be called twice (the latch lives inside the
    /// public `emit_no_confinement_diagnostic`); the emitter is then called
    /// twice on this serial test to prove the OnceLock semantics.
    #[cfg(target_os = "linux")]
    #[test]
    #[serial]
    fn emit_no_confinement_diagnostic_is_one_shot_per_process() {
        // Stable substrings from contract C-E3.
        let generic =
            format_no_confinement_diagnostic(Some(BwrapBrokenCause::Other), Some("captured"));
        assert!(
            generic.contains("Filesystem confinement is unavailable on this host."),
            "generic template must contain the FR-014 stable substring; got: {generic}"
        );
        let apparmor = format_no_confinement_diagnostic(
            Some(BwrapBrokenCause::AppArmorRestrictUserns),
            Some("setting up uid map: Permission denied"),
        );
        assert!(
            apparmor.contains(
                "AppArmor's `restrict_unprivileged_userns` policy is blocking bubblewrap"
            ),
            "AppArmor template must contain the FR-015 stable substring; got: {apparmor}"
        );
        let none_cause = format_no_confinement_diagnostic(None, None);
        assert!(
            none_cause.contains("Filesystem confinement is unavailable on this host."),
            "cause=None must use the generic template; got: {none_cause}"
        );

        // One-shot latch. We cannot reset `OnceLock` in a non-`#[cfg(test)]`
        // surface, so this test depends on prior tests in the same process
        // not having emitted. Order across `#[serial]` tests is determined
        // by the harness; this test is the only emitter caller. If a future
        // test changes that assumption it MUST also gate emit-callers
        // behind the latch already-set state.
        let first = emit_no_confinement_diagnostic(
            Some(BwrapBrokenCause::AppArmorRestrictUserns),
            Some("setting up uid map: Permission denied"),
        );
        let second = emit_no_confinement_diagnostic(
            Some(BwrapBrokenCause::AppArmorRestrictUserns),
            Some("setting up uid map: Permission denied"),
        );
        // Exactly one of these is Some; the other is None. Because we
        // can't deterministically guarantee `first` was the actual first
        // emit in this process (other tests could in principle race),
        // assert the one-shot invariant directly: first.is_some() XOR
        // second.is_some() is false (since latch can only flip once;
        // either both are None — already-latched — or first=Some,
        // second=None).
        match (first.is_some(), second.is_some()) {
            (true, false) => {
                // Latch flipped on this call's first invocation; second
                // returned None. Expected happy path. Verify the message
                // we got carried the AppArmor substring.
                let msg = first.expect("Some by match arm");
                assert!(
                    msg.contains(
                        "AppArmor's `restrict_unprivileged_userns` policy is blocking bubblewrap"
                    ),
                    "first emission must carry the AppArmor substring"
                );
            }
            (false, false) => {
                // Latch was already set by a prior call in this process —
                // also acceptable evidence of one-shot semantics. Verify
                // OnceLock state explicitly.
                assert!(
                    NO_CONFINEMENT_DIAGNOSTIC_EMITTED.get().is_some(),
                    "latch must be set when emitter returned None"
                );
            }
            (_, true) => panic!(
                "OnceLock invariant violated: second emit_no_confinement_diagnostic \
                 call returned Some; latch must be one-shot"
            ),
        }
    }
}
