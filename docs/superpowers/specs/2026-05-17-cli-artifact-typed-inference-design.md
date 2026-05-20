# Design: Artifact-File Typed Inference via the Claude Code CLI

**Date:** 2026-05-17
**Status:** Approved (brainstorming) — pending implementation plan
**Supersedes (in part):** the spec-003 `--json-schema` typed-inference premise; the in-flight debugging patches (structured_output strict parse, defensive `result`-fence fallback) are reconciled/removed by this design.

## Problem

`invoke_typed` (spec-003) assumed `claude -p --json-schema <schema>` yields schema-conforming structured output, a drop-in for the pre-003 direct-API path (`rig` `prompt_typed`, real tool-use enforcement). Evidence from learning runs #44–#47 disproves this for `claude` 2.1.143:

- `structured_output` is populated nondeterministically (often absent with `stop_reason=end_turn`).
- When JSON arrives via `result`, the model ignores the schema: missing required fields (`evidence`, `name`), wrong types (array where string expected), invented fields (`type`).
- `--json-schema` is best-effort *validation*, not enforcement — no CLI-side retry/repair.

Four layered patches (structured_output read, Stream C `FromStr`, Sonnet 4.6 swap, defensive `result` extraction) each peeled one symptom to reveal the next — the systematic-debugging "wrong architecture" signal. The learning feature worked pre-spec-003 on the enforced-structured-output API path; spec-003 traded that for the supported CLI path for ToS-compliance + supportability reasons (`specs/003-cc-inference-migration/spec.md` §10–14), which remain valid and are a hard constraint.

## Decision

Stop constraining the model's *text*. The headless agent **delivers the typed result as a JSON file written via a `Write` tool action** into a sandboxed per-call temp dir; Quill reads it and the existing `serde_json::from_str::<T>` **is** the validation and trust boundary. Tool-action output is structurally reliable (the model commits an artifact); free-text / `--json-schema` is not. 100% supported CLI; no MCP; no direct Anthropic API; no OAuth-for-inference.

## Architecture

For every `invoke_typed::<T>` call:

1. Create a unique per-call temp directory.
2. Build the prompt: existing task prompt + the exact `schemars::schema_for!(T)` JSON + one filled example + an explicit instruction to write *only* the JSON to `<tmp>/out.json` and re-read/verify it before finishing.
3. Spawn `claude` headless, granted *only* the `Write` tool, CWD and `--add-dir` confined to `<tmp>`.
4. On exit: parse the `--output-format json` envelope **for per-call metadata only** (tokens/cost/model/durations).
5. Read `<tmp>/out.json`; `serde_json::from_str::<T>` it — this is the sole typed-validation step.
6. Delete `<tmp>` unconditionally (success, failure, or timeout) via a drop guard.
7. Return `InvokeOutcome { value, metadata }`.

`invoke_text` (free-form, no schema) is **unchanged** — it legitimately returns `envelope.result`.

### Invocation change (`cc_client::build_command` / `invoke_typed`)

- **Remove:** `--tools ""`; `--json-schema` (typed path).
- **Add:** `--allowedTools "Write"`; a broad `--disallowedTools` deny (belt-and-suspenders); `--permission-mode acceptEdits`; `--add-dir <tmp>`.
- **Change:** CWD from the shared `state_dir()` to the unique per-call temp dir.
- **Keep:** `-p --output-format json`, `--model`, `--append-system-prompt`, `--disable-slash-commands`, `--no-session-persistence`, `--setting-sources ""`, `--exclude-dynamic-system-prompt-sections`, R-6 env-scrub, `kill_on_drop`, the 300 s hang-detector timeout.

## Deliberate decision: the file is the sole typed channel

The earlier defensive `result`-fence extraction is **removed** from `invoke_typed`. That fragile prose path is precisely what this design eliminates; retaining it as a fallback would re-introduce the unreliability and create dual code paths. Missing or invalid `out.json` → `InferenceError::SchemaValidationFailed` with a specific cause. **No app-side retry** (spec-003 FR-011 preserved); the agent's own write→self-check loop is the only iteration.

## Isolation / safety — scoped spec-003 R-5 deviation

This consciously, narrowly reverses spec-003 R-5's *total* tool isolation:

- Grant is `Write` only; everything else denied (`--disallowedTools`). No Bash/Read/Edit, no network, no MCP, no slash commands.
- CWD and the only writable directory (`--add-dir`) are the unique per-call temp dir; nothing else is reachable.
- R-6 env-scrub, `--no-session-persistence`, `--setting-sources ""`, `--exclude-dynamic-system-prompt-sections`, `kill_on_drop`, and the 300 s hang-detector are all retained.
- The temp dir is destroyed unconditionally after the call.

Rationale (recorded for the spec-003 deviation log): the supported-CLI premise is non-negotiable (ToS/supportability), but the CLI cannot enforce structured output; a minimal sandboxed `Write` grant is the smallest capability that makes the supported path sound. The 300 s timeout is retained unchanged — a write plus a self-recheck is a small number of agent turns, comfortably within budget; revisit only if evidence shows otherwise.

## Scope

**Unchanged:** pipeline shape and 3-stream parallelism (FR-014), per-stream inference metadata, the generalized synthesis decision, Stream C session selection/digests, the Stream C provider `FromStr` fix, Sonnet 4.6 model routing (orthogonal — retained), `invoke_text`. The change is to the shared `invoke_typed` mechanism, so it applies uniformly to Stream A/B/C, synthesis, the memory optimizer, and prose compression.

**Non-goals / out of scope:** broader pipeline redesign; the single-holistic-session approach (rejected); reopening the direct-API path (rejected — ToS).

## Carried-over follow-up (flagged, not bundled)

`failed_metadata` records `model=None` (it has no envelope access on the failure path) — a real observability gap that hampered debugging. Tracked as the recommended next change; **not** part of this design.

## Testing / verification

Per project/user policy, no automated test code unless explicitly requested. Verification: a live Analyze run (Stream A/B/C produce findings; run completes; per-stream `stream_*` metadata persisted), `cargo check`/`cargo test --lib` green, `lat check` green, and confirmation that no `claude /insights` or `--json-schema` typed path remains.

## Documentation impact

`lat.md/backend.md` (Claude Code Inference Client — invocation flags, the structured-output story), `specs/003-cc-inference-migration/contracts/cc-client.md` (typed output contract), and the spec-003 deviation note must be updated by the implementation to reflect the artifact-file mechanism and the R-5 deviation; `lat check` must pass.
