# Phase 0 Research: Migrate LLM Inference to Claude Code Integration

This document resolves the open implementation questions raised by the plan's Technical Context section. Each entry follows the format: **Decision**, **Rationale**, **Alternatives considered**.

## R-1: Subprocess invocation library

**Decision**: Use `tokio::process::Command` directly. No additional crate.

**Rationale**: The backend already depends on Tokio for all async work (see `lat.md/backend.md#Concurrency`). `tokio::process` is in-tree, integrates cleanly with the existing `tokio::join!` pattern in `learning.rs`, supports async stdin/stdout/stderr, kill-on-drop, and bounded timeouts via `tokio::time::timeout`. Adding a third-party process-management crate (e.g. `tokio-process-stream`, `which`) would add surface for marginal convenience.

**Alternatives considered**:
- `std::process::Command` synchronous + `tokio::task::spawn_blocking`: rejected — blocks a tokio worker per call, mismatches existing async pattern, makes the planned `tokio::join!` over three streams harder to reason about.
- A dedicated wrapper crate (`duct`, `command-group`): rejected — features we don't need; pulls in transitive deps.

## R-2: Prompt delivery: stdin vs argv

**Decision**: Pass the **prompt body via stdin** (no closing `[prompt]` positional argument). Pass system preamble via `--append-system-prompt` argv flag.

**Rationale**: Prompts in this codebase can be 50-150 KB for Stream A (up to 100 compressed observations) and Stream B (git history dump). Passing that via argv risks platform-specific `ARG_MAX` limits — Linux defaults around 128 KB total argv+env, macOS around 256 KB, Windows much tighter. Stdin has no such cap, is the documented `claude -p` ingestion path for piped use ("Print response and exit (useful for pipes)" — see `claude --help`), and is faster than building a giant arg string. The system preamble is short and stable per call type, so argv is fine for it.

**Alternatives considered**:
- All-argv: rejected — `ARG_MAX` risk on the larger Stream A/Synthesis prompts.
- `--input-format stream-json` over stdin: rejected — overkill; this is for streaming multi-turn input. Our calls are one-shot.
- Writing the prompt to a temp file and piping: rejected — extra syscalls, temp file lifecycle.

## R-3: Structured output enforcement

**Decision**: For every call site that currently uses `ai_client::analyze_typed<T>`, pass `--json-schema <schema>` where `<schema>` is the JSON Schema produced by `schemars::schema_for!(T)` (or the existing `JsonSchema` derive on `T`). Parse the model's reply (found in the `result` field of the JSON envelope) by deserializing into `T`.

**Rationale**: The `--json-schema` flag is documented in `claude --help` ("JSON Schema for structured output validation"). It moves schema enforcement into Claude Code instead of having the app validate after the fact. Quill's existing `T: DeserializeOwned + JsonSchema + Send + Sync + 'static` constraint (`ai_client.rs:71`) already supplies everything needed to generate the schema.

**Alternatives considered**:
- Ask for JSON in the prompt and validate locally: rejected — that's what we do today implicitly; CC's `--json-schema` is the direct replacement and is enforced server-side.
- Tool-use shape (have CC call a "submit" tool with structured input): rejected — overkill for one-shot output; would require keeping tools enabled.

## R-4: Schema string source of truth

**Decision**: Generate the schema string at call time with `serde_json::to_string(&schemars::schema_for!(T))`. Cache per-type if hot-path measurement shows it matters (deferred).

**Rationale**: `schemars::schema_for!(T)` is the existing project pattern. Inlining schema generation keeps the contract single-sourced from the Rust type definition — `StreamFindings`, `AnalysisOutput`, and the memory-optimizer types stay the canonical shape. No risk of the schema drifting from the type.

**Alternatives considered**:
- Precomputing static schema strings as `const`s: rejected — `schema_for!` is not const-evaluable, and lazy-static-style caching adds boilerplate not justified by the call counts (<6 per analyze).

## R-5: Subprocess isolation flags

**Decision**: Every invocation uses this exact flag set:

| Flag | Reason |
|---|---|
| `-p` | Headless one-shot mode |
| `--output-format json` | Single JSON envelope on stdout |
| `--model <alias>` | Either `haiku` or `sonnet` |
| `--json-schema <schema>` | When the call expects structured output (replaces `analyze_typed`); omitted for `complete_text`-equivalent calls |
| `--append-system-prompt <preamble>` | Replaces the `preamble` argument of `analyze_typed`/`complete_text` |
| `--tools ""` | No tool execution; pure inference |
| `--disable-slash-commands` | No skill/slash resolution |
| `--no-session-persistence` | Don't pollute the user's CC session history |
| `--setting-sources ""` | Don't load user/project/local settings — bypasses the user's hooks, plugins, agents, CLAUDE.md auto-discovery |
| `--exclude-dynamic-system-prompt-sections` | Stable, cacheable system prompt across invocations (cwd/env/git status not injected) |

**Rationale**: This is the minimum set that satisfies FR-006 (isolated from user's interactive config) while still using the OAuth subscription quota for inference (not `--bare`, which would require BYO `ANTHROPIC_API_KEY`). The live-validation test in the spec used a strict subset of this list and produced a clean response with no `permission_denials` and no side effects on session persistence.

**Alternatives considered**:
- `--bare`: rejected — requires `ANTHROPIC_API_KEY` env var or `apiKeyHelper`; would break unless we add a BYOK path, which is explicitly out of scope.
- Default flags only (no isolation): rejected — would let the user's UserPromptSubmit hooks rewrite our prompt, their PreToolUse hooks block our (nominally disabled) tools, their CLAUDE.md leak into our system prompt. Reproducibility tanks.

## R-6: Environment variables to scrub

**Decision**: Spawn each subprocess with an environment **inherited** from the parent **with these keys removed**:

- `CLAUDE_CODE_*` (any) — clear any inherited Claude Code state from the user's shell
- `ANTHROPIC_*` (any) — prevent BYOK env vars from accidentally redirecting our subprocess
- `NODE_OPTIONS` — Claude Code is Node-backed; user-set NODE_OPTIONS can change behavior
- `PATH` is **preserved** so the inner `claude` lookup of its dependencies works

**Rationale**: We want the subprocess to use the user's Claude credentials (so we get the subscription path) but nothing else that would alter behavior. Scrubbing these keys eliminates the largest contamination sources without breaking the spawn.

**Alternatives considered**:
- Pristine empty env: rejected — `claude` needs HOME, PATH, USER, etc. to find its credentials and Node runtime.
- Don't scrub: rejected — risks reproducibility regressions.

## R-7: Error categorization

**Decision**: The `cc_client::InferenceError` enum has exactly these variants, mapped from the listed signals:

| Variant | Trigger |
|---|---|
| `ClaudeCodeMissing` | `Command::spawn` fails with `ENOENT` (or the OS equivalent) |
| `ClaudeCodeTooOld` | The process exits with a non-zero code AND stderr contains a "unknown option" / "unrecognized flag" error for one of our required flags; OR a startup `claude --version` probe at first use reports a version below the floor (R-11) |
| `NotSignedIn` | Process stderr or the JSON envelope contains the documented "Run: claude /login" / unauthenticated error pattern |
| `RateLimited` | The JSON envelope's `api_error_status` indicates a 429 / overload condition, or `result` carries Claude Code's standard rate-limit error string |
| `SchemaValidationFailed` | The JSON envelope reports a `--json-schema` violation, OR the envelope is well-formed but `serde_json::from_str::<T>(&envelope.result)` fails |
| `TimedOut` | `tokio::time::timeout` fires |
| `Spawn(other)` | Anything else from `Command::spawn` |
| `BadEnvelope` | Stdout did not parse as the documented `--output-format json` envelope shape |

Each variant produces a single human-readable message that the existing `learning_runs.error` and memory optimizer error channels can store and display unchanged.

**Rationale**: These variants exactly cover the failure modes named in the spec's `Inference Failure` entity. They are testable in isolation (mock the subprocess) and they preserve the user-visible categorization without introducing new UI work.

**Alternatives considered**:
- Single opaque `String` error like today: rejected — FR-010 explicitly requires identifying which condition is the cause.
- Mirror Claude Code's full error taxonomy: rejected — overfits to internal CC details; the eight variants above are sufficient for the spec's requirements and stable against minor CC changes.

## R-8: Cancellation behavior

**Decision**: Use `Command::kill_on_drop(true)`. Wrap each invocation in `tokio::time::timeout(Duration::from_secs(300), child.wait_with_output())`. On timeout, drop the child to send SIGKILL (or the OS equivalent); return `InferenceError::TimedOut`.

**Rationale**: SIGTERM-first with a grace window would be cleaner but `claude -p` does not appear to do post-SIGTERM cleanup that matters for our use case (no on-disk session state when `--no-session-persistence`), so SIGKILL via drop is fine and is the simplest correct behavior.

**Alternatives considered**:
- SIGTERM + grace period: rejected — adds code, no observable benefit at our scope.
- No kill (just let it run): rejected — defeats the hang detector.

## R-9: Output envelope schema

**Decision**: Treat the `claude -p --output-format json` envelope as having this minimum shape; ignore extra fields:

```jsonc
{
  "type": "result",
  "subtype": "success" | "error",
  "is_error": bool,
  "api_error_status": null | string,
  "duration_ms": number,
  "duration_api_ms": number,
  "ttft_ms": number,
  "result": string,                 // the model's reply text — JSON-encoded when --json-schema is set
  "stop_reason": string,
  "session_id": string,
  "total_cost_usd": number,
  "usage": {
    "input_tokens": number,
    "output_tokens": number,
    "cache_creation_input_tokens": number,
    "cache_read_input_tokens": number,
    "service_tier": string
  },
  "modelUsage": { "<model-id>": { ... } },
  "permission_denials": Array
}
```

The struct deserialized by `cc_client` uses `#[serde(default)]` on every optional/numeric field. Unknown fields are accepted via the default Serde behavior (no `deny_unknown_fields`).

**Rationale**: This shape was observed directly in the live validation test executed during specification. Treating it as a forward-compatible envelope (extra fields tolerated; absent fields default) is robust against Claude Code adding fields, which is the most likely future change.

**Alternatives considered**:
- `deny_unknown_fields`: rejected — fragile; one CC update adds a field and we hard-fail.
- Pinning to the full schema from CC's docs: rejected — that schema is not contractually frozen; tolerating extras is safer.

## R-10: Persisting structured per-call metadata

**Decision**: Add a single new column `inference_metadata TEXT` to each of the run-record tables (`learning_runs` and the equivalent memory-optimization run table). The column holds a JSON blob — an array of one object per inference invocation made during that run, each object carrying the structured fields named in FR-016 (input/output tokens, cache stats, model id, durations, cost, stop reason, permission denials, plus a synthetic `phase` tag such as `"stream_a"`, `"stream_b"`, `"stream_c"`, `"synthesis"`, `"memory_optimizer"`, `"prose_compression"`). Existing `logs` TEXT column is preserved for text-mode messages.

**Rationale**: SC-009 requires structured metadata for every CC invocation persisted on the run record. A single TEXT column with a JSON-encoded array is the lowest-friction migration (additive, no row-shape changes) and matches the existing "store complex structured payloads as JSON in TEXT" pattern that `learning_runs.phases` and similar already use. Per-call rows in a separate table would be cleaner relational design but is overkill for ≤4 calls per run.

**Alternatives considered**:
- A new `inference_call_metadata` table joined by `run_id`: rejected — relational overhead not justified at this scale; introduces a migration coordination risk for a feature that is otherwise additive.
- Append metadata as text inside `logs`: rejected — couples text presentation to machine-readable data; harder to query later.

## R-11: Minimum Claude Code version

**Decision**: At call time, treat any required flag's absence as `ClaudeCodeTooOld` via the same error path (R-7). Do **not** hard-pin a version number in code. On first failed invocation that smells like a version mismatch, also run `claude --version` once and include the discovered version in the user-visible error.

**Rationale**: Feature-detection by behavior is more robust than version-pinning, especially when the CLI's flag set is the actual contract (not the version number). The live test ran successfully on `2.1.142`; the `--json-schema` flag was documented at least as far back as the version installed on this dev box. Hard-coding a floor invites maintenance churn each time CC ships.

**Alternatives considered**:
- Pin `MINIMUM_CC_VERSION = "2.1.142"` and check at startup: rejected — adds a startup probe, blocks app launch on misconfiguration, and is fragile if CC version strings change format.
- No check at all: rejected — FR-010 requires identifying the cause of failure clearly.

## R-12: Discovering the `claude` executable

**Decision**: Resolve via `which::which("claude")` once at first inference call per app process; cache the resolved absolute path. If the lookup fails, return `ClaudeCodeMissing` for every call until the next app restart (no retry, no PATH re-watch).

**Rationale**: `which` is already in our dependency tree (transitive). One-shot lookup with caching matches the user expectation that "if it works once it works again" and the no-retry stance from FR-011. A PATH watcher would be over-engineered.

**Alternatives considered**:
- Lookup on every call: rejected — slow on cold PATH lookups, especially on Windows.
- Cache for the lifetime of the process and never refresh: same as decided.
- Try `~/.local/bin/claude` / `~/.claude/local/claude` as fallback: rejected — let `which` handle PATH semantics correctly; users who modify PATH between launches can restart the app.

## R-13: How to inhibit Claude Code's own retry behavior (if any)

**Decision**: Do nothing app-side. `claude -p` is a one-shot mode; if Claude Code performs any internal retries during a single invocation, those retries happen inside the 300s hang detector window and are invisible to us — exactly the FR-011 contract. No flag in `claude --help` exposes "disable internal retries"; the no-retry policy is the app's, not CC's.

**Rationale**: The spec already encodes that whatever CC does internally during one invocation is opaque and acceptable.

**Alternatives considered**:
- Try to set a hidden env var to suppress retries: rejected — speculative and unsupported.

## R-14: Working directory of the subprocess

**Decision**: Spawn each `claude` subprocess with `current_dir` set to a stable, app-controlled location: the app's `state_dir()` (typically `~/.local/share/com.quilltoolkit.app/`). Specifically NOT the user's project directory.

**Rationale**: Even with `--setting-sources ""` and `--exclude-dynamic-system-prompt-sections`, the CWD affects a few CC behaviors (its on-disk `.claude/` directory discovery, file-operation tools — which we disable, but defense in depth — and any future cwd-derived state). A stable app-owned location decouples inference reproducibility from whatever the user's frontend is doing.

**Alternatives considered**:
- User's project root: rejected — couples inference to whatever's in that directory.
- `/tmp` (or platform equivalent): rejected — anti-virus quarantine risk on Windows, harder to debug if something goes wrong.

## Summary of resolved unknowns

Every NEEDS CLARIFICATION raised by the Technical Context section has been resolved above. The remaining open items (lifecycle of `--fallback-model`, BYOK toggle UI, structured-metadata UI surfacing) are explicitly out of scope per the spec and need not block planning.
