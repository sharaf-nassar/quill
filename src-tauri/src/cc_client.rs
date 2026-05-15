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
use std::sync::{LazyLock, Mutex};
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
/// `StreamC` is reserved for a future migration of the insights
/// extractor (currently it runs `claude /insights --print` directly
/// rather than going through `cc_client`, so it never appears in the
/// metadata array today). The variant is retained so the contract
/// surface in `contracts/cc-client.md` stays accurate.
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
pub enum Model {
    Haiku,
    Sonnet,
}

impl Model {
    fn alias(self) -> &'static str {
        match self {
            Model::Haiku => "haiku",
            Model::Sonnet => "sonnet",
        }
    }
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
/// `Result` shape.
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
        ..InferenceCallMetadata::default()
    }
}

// ---------------------------------------------------------------------------
// Envelope shape returned by `claude -p --output-format json`. Forward-compat:
// unknown fields are tolerated, optional numerics default to zero, model id is
// extracted from the `modelUsage` map's first key.
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
) -> InferenceCallMetadata {
    // The map's first key is the concrete model id Claude Code resolved
    // the alias to (e.g. "claude-haiku-4-5-20251001"). BTreeMap iter
    // order is stable; a missing entry means we don't know.
    let model = env.model_usage.keys().next().cloned();
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
    }
}

// ---------------------------------------------------------------------------
// Command construction and error classification.
// ---------------------------------------------------------------------------

fn build_command(args: &InvokeArgs, json_schema: Option<&str>, claude_path: &Path) -> Command {
    let mut cmd = Command::new(claude_path);

    // Headless one-shot mode with the documented JSON envelope.
    cmd.arg("-p").arg("--output-format").arg("json");
    cmd.arg("--model").arg(args.model.alias());
    cmd.arg("--append-system-prompt").arg(&args.preamble);

    // Isolation — see research.md R-5.
    cmd.arg("--tools").arg("");
    cmd.arg("--disable-slash-commands");
    cmd.arg("--no-session-persistence");
    cmd.arg("--setting-sources").arg("");
    cmd.arg("--exclude-dynamic-system-prompt-sections");

    if let Some(schema) = json_schema {
        cmd.arg("--json-schema").arg(schema);
    }

    // I/O wiring — prompt body delivered on stdin (R-2).
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    // CWD isolation — R-14.
    if let Some(state_dir) = state_dir() {
        cmd.current_dir(state_dir);
    }

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

    cmd
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
// Public surface.
// ---------------------------------------------------------------------------

/// One-shot Claude Code invocation with JSON-Schema-validated output.
/// Drop-in replacement for the prior `ai_client::analyze_typed::<T>`.
pub async fn invoke_typed<T>(args: InvokeArgs) -> Result<InvokeOutcome<T>, InferenceError>
where
    T: for<'de> Deserialize<'de> + schemars::JsonSchema + Send + Sync + 'static,
{
    let schema = cached_schema::<T>()?;
    let envelope = invoke_raw(&args, Some(&schema)).await?;
    let value: T = serde_json::from_str(&envelope.result).map_err(|e| {
        InferenceError::SchemaValidationFailed {
            details: format!(
                "could not deserialize result into target type: {e} (first 256 chars of result: {})",
                truncate(&envelope.result, 256)
            ),
        }
    })?;
    let metadata = metadata_from_envelope(args.phase, args.max_tokens, &envelope);
    Ok(InvokeOutcome { value, metadata })
}

/// One-shot Claude Code invocation that returns the model's reply
/// verbatim as a string. Drop-in replacement for the prior
/// `ai_client::complete_text`.
pub async fn invoke_text(args: InvokeArgs) -> Result<InvokeOutcome<String>, InferenceError> {
    let envelope = invoke_raw(&args, None).await?;
    let metadata = metadata_from_envelope(args.phase, args.max_tokens, &envelope);
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

async fn invoke_raw(
    args: &InvokeArgs,
    json_schema: Option<&str>,
) -> Result<Envelope, InferenceError> {
    // Resolve the `claude` binary via the project's cached,
    // login-shell-aware resolver (R-12). This picks up Anthropic's
    // `claude migrate-installer` target and auto-refreshes when the
    // user triggers a PATH rescan from the integrations menu.
    let claude_path = match crate::config::resolve_command_path("claude") {
        Some(path) => path,
        None => return Err(InferenceError::ClaudeCodeMissing),
    };

    let mut cmd = build_command(args, json_schema, &claude_path);
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
        let err = classify_error(output.status, &stderr, envelope_result.as_ref().ok());
        return Err(enrich_error(err, &claude_path).await);
    }

    let envelope = match envelope_result {
        Ok(env) => env,
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

    Ok(envelope)
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
        let meta = metadata_from_envelope(Phase::StreamA, 4096, &env);
        assert_eq!(meta.input_tokens, 10);
        assert_eq!(meta.output_tokens, 44);
        assert_eq!(meta.cache_creation_input_tokens, 6252);
        assert_eq!(meta.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert!(meta.success);
        assert_eq!(meta.phase, "stream_a");
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
        let meta = metadata_from_envelope(Phase::Synthesis, 8192, &env);
        assert_eq!(meta.model, None);
        assert_eq!(meta.phase, "synthesis");
        assert_eq!(meta.max_tokens_requested, 8192);
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
}
