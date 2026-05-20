# Internal Contract: `cc_client` module

This is the internal contract for the new `src-tauri/src/cc_client.rs` module. It is not a public API and not exposed via Tauri IPC — it is a backend-only module called by `learning.rs` and `memory_optimizer.rs`.

## Public surface

```rust
/// One-shot, structured-output call. Direct replacement for the old
/// `ai_client::analyze_typed::<T>`.
pub async fn invoke_typed<T>(args: InvokeArgs) -> Result<InvokeOutcome<T>, InferenceError>
where
    T: DeserializeOwned + JsonSchema + Send + Sync + 'static;

/// One-shot, free-form text call. Direct replacement for the old
/// `ai_client::complete_text`.
pub async fn invoke_text(args: InvokeArgs) -> Result<InvokeOutcome<String>, InferenceError>;

/// Inputs for one invocation. Mirrors the legacy `analyze_typed` /
/// `complete_text` argument list.
pub struct InvokeArgs {
    pub phase: Phase,             // for metadata tagging — `stream_a`, `synthesis`, etc.
    pub prompt: String,           // user prompt; passed to claude via stdin
    pub preamble: String,         // system preamble; passed via --append-system-prompt
    pub model: Model,             // Haiku | Sonnet
    pub max_tokens: u64,          // maps to claude's max-tokens accounting
}

pub enum Model {
    Haiku,   // resolves to `--model haiku`
    Sonnet,  // resolves to `--model sonnet`
}

pub enum Phase {
    StreamA,
    StreamB,
    StreamC,
    Synthesis,
    MemoryOptimizer,
    ProseCompression,
}

/// Successful result: the deserialized `T` (for invoke_typed) or `String`
/// (for invoke_text), bundled with the per-call metadata to be persisted.
pub struct InvokeOutcome<T> {
    pub value: T,
    pub metadata: InferenceCallMetadata,
}

/// Persisted per-call metadata. Layout matches the
/// `Inference Call Metadata` entity in `data-model.md`.
pub struct InferenceCallMetadata {
    pub phase: Phase,
    pub model: String,                          // resolved concrete model id
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub ttft_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub total_cost_usd: f64,
    pub service_tier: Option<String>,
    pub stop_reason: Option<String>,
    pub permission_denials: Vec<serde_json::Value>,
    pub success: bool,
    pub failure_kind: Option<&'static str>,
}

/// Categorized failure. See research.md R-7 for the full mapping.
#[non_exhaustive]
pub enum InferenceError {
    ClaudeCodeMissing,
    ClaudeCodeTooOld { detected_version: Option<String> },
    NotSignedIn,
    RateLimited { message: String },
    SchemaValidationFailed { details: String },
    TimedOut { after: Duration },
    Spawn(std::io::Error),
    BadEnvelope { details: String },
}

impl std::fmt::Display for InferenceError { ... }   // user-facing message per variant
impl std::error::Error for InferenceError { ... }
```

## Semantics

### Invocation

For every call, `cc_client` constructs and runs:

```text
$CLAUDE_PATH
  -p
  --output-format json
  --model <alias>
  --append-system-prompt <preamble>
  --tools ""
  --disable-slash-commands
  --no-session-persistence
  --setting-sources ""
  --exclude-dynamic-system-prompt-sections
  [--json-schema <schema-json>]      # only for invoke_typed
```

with:

- `prompt` piped on stdin (closes stdin after write — no closing positional `[prompt]` argv)
- `cwd` set to the app's state directory (R-14)
- Environment scrubbed of `CLAUDE_CODE_*`, `ANTHROPIC_*`, `NODE_OPTIONS` (R-6)
- `kill_on_drop(true)`
- Wrapped in `tokio::time::timeout(Duration::from_secs(300), ...)` (FR-009)

### Output parsing

On a `tokio::time::timeout` success path, stdout is parsed as the envelope shape declared in `research.md` R-9. Typed and free-form calls read different envelope fields:

- For `invoke_typed`: the JSON Schema is embedded in the prompt (not `--json-schema`, which the CLI does not enforce). The agent is granted a `Write`-only tool sandboxed to a per-call temp dir and writes the result to `out.json`; `T` is obtained via `serde_json::from_str::<T>` of that file. Missing/unreadable/un-deserializable file → `InferenceError::SchemaValidationFailed`. No app-side retry.
- For `invoke_text`: `envelope.result` is returned verbatim as `String` (no schema call, so `structured_output` is absent).

### Error mapping (from research.md R-7)

| Source signal | Mapped to |
|---|---|
| `Command::spawn` errors with `ErrorKind::NotFound` | `ClaudeCodeMissing` |
| `tokio::time::timeout` fires | `TimedOut` |
| Process exits non-zero AND stderr matches "unknown option" / "unrecognized flag" / "no such option" patterns | `ClaudeCodeTooOld` (with one-shot `claude --version` probe to populate `detected_version`) |
| Envelope's `api_error_status` indicates 429 / overload, or `result` matches CC's rate-limit error string | `RateLimited` |
| Envelope or stderr matches "Run: claude /login" / "not authenticated" | `NotSignedIn` |
| Envelope's `is_error: true` with `subtype: "error"` and no more specific match | falls back to `RateLimited`/`Spawn` based on `api_error_status`; otherwise `BadEnvelope` |
| Stdout fails JSON envelope parse | `BadEnvelope` |
| The agent-written `out.json` artifact is missing/unreadable, or fails to deserialize into `T` | `SchemaValidationFailed` |
| Other `std::io::Error` from `Command::spawn` | `Spawn(_)` |

### Success contract

`Ok(InvokeOutcome { value, metadata })` MUST be returned only when:

- The subprocess exited zero AND
- The stdout envelope parsed AND `is_error: false` AND
- (for `invoke_typed`) `T` was obtained by deserializing the JSON artifact the agent wrote to the sandboxed `out.json`.

In all other cases, an `Err(InferenceError::...)` is returned with no `InvokeOutcome`. Failed calls still produce a `InferenceCallMetadata` record via a separate `cc_client::failed_metadata(phase, err)` helper so callers can append it to the run's metadata blob without contorting the `Result` shape. (Convention: callers `.inspect_err(|e| metadata.push(failed_metadata(phase, e)))` next to the call.)

## Idempotence and side-effects

Each invocation:

- Spawns one child process and waits for it.
- Does **not** mutate global state (no static caches except the one-time `which("claude")` result; no on-disk writes; no setting reads).
- Does **not** prompt for permissions (`--tools ""` removes the trigger surface).
- Does **not** write to the user's CC session store (`--no-session-persistence`).

## Concurrency contract

`invoke_typed` / `invoke_text` are `Send + 'static` Futures. The pipeline can call them under `tokio::join!`, `tokio::spawn`, or `FuturesUnordered` without further synchronization. The migration preserves the existing `learning.rs` shape:

```rust
let (a, b, c) = tokio::join!(
    cc_client::invoke_typed::<StreamFindings>(stream_a_args),
    cc_client::invoke_typed::<StreamFindings>(stream_b_args),
    cc_client::invoke_typed::<StreamCInsights>(stream_c_args),
);
// then sequentially:
let synth = cc_client::invoke_typed::<AnalysisOutput>(synthesis_args).await?;
```

This satisfies FR-014.

## Testing surface

The module is unit-testable by injecting a mock `Spawner` trait (returning canned stdout/stderr/exit-code per test). The contract tests should cover, at minimum, one happy-path case and one case per `InferenceError` variant.

## Out of scope for this module

- BYOK / `ANTHROPIC_API_KEY` opt-in (deferred to a future feature)
- UI surfacing of `InferenceCallMetadata` (deferred per FR-016)
- Streaming / multi-turn input (one-shot only)
- Cost limiting via `--max-budget-usd` (informational only; not enforced)

## R-5 deviation (artifact-file typed inference, 2026-05-17)

spec-003 R-5 specified total tool isolation (`--tools ""`). Typed inference now requires a narrow, bounded exception: `invoke_typed` grants `--allowedTools "Write"` (everything else `--disallowedTools`-denied), `--permission-mode acceptEdits`, and `--add-dir`/CWD confined to a unique per-call `tempfile::TempDir` destroyed unconditionally on drop. Rationale: the supported-CLI premise is non-negotiable but `--json-schema` is unenforced; a minimal sandboxed `Write` grant is the smallest capability that makes the supported path sound. R-6 env-scrub, `--no-session-persistence`, `--setting-sources ""`, `--exclude-dynamic-system-prompt-sections`, `kill_on_drop`, and the 300 s timeout are retained. `invoke_text` keeps full R-5 isolation.
