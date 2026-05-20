# Artifact-File Typed Inference Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Make `invoke_typed` reliable by having the headless `claude` agent write the result as a sandboxed JSON file (delivered via a `Write` tool action) that Quill reads and typed-deserializes, instead of parsing an unenforced `--json-schema` envelope.

**Architecture:** `build_command` branches: typed calls get a `Write`-only grant sandboxed to a per-call `tempfile::TempDir`; free-form (`invoke_text`) keeps spec-003 total isolation. `invoke_typed` embeds `schema_for!(T)` in the prompt, instructs the agent to write `<tmp>/out.json`, then `std::fs::read_to_string` + `serde_json::from_str::<T>` is the sole validation. `TempDir` drop = unconditional cleanup. The defensive `result`-fence path and `extract_json` are deleted.

**Tech Stack:** Rust, `tempfile` 3.27, `serde_json`, `schemars`, the `claude` CLI (2.1.143) headless mode.

**Testing note (per project/user policy + design doc):** no automated test code is written unless explicitly requested. The skill's TDD default is intentionally waived. Verification per task = `cargo check --lib` / `cargo test --lib` (existing tests stay green) + `lat check`; end-to-end = a live Analyze run (owner-run, can't be driven headlessly here).

---

## File Structure

- Modify `src-tauri/src/cc_client.rs`: `build_command` (branch on artifact dir), `invoke_raw` (param `json_schema: Option<&str>` → `artifact_dir: Option<&Path>`), `invoke_typed` (artifact-file flow), delete `extract_json`. `invoke_text` body unchanged (its `None` arg now means "no artifact / total isolation").
- Modify `lat.md/backend.md`, `specs/003-cc-inference-migration/contracts/cc-client.md`: reflect artifact-file mechanism + R-5 deviation.
- No other files. Stream A/B/C, synthesis, memory optimizer, prose compression call `invoke_typed` unchanged.

---

## Task 1: Branch `build_command` on artifact dir

**Files:** Modify `src-tauri/src/cc_client.rs` (`fn build_command`, currently ~340–381).

- [ ] **Step 1: Replace `build_command` entirely**

Replace the whole `fn build_command(...) { ... }` with:

```rust
fn build_command(args: &InvokeArgs, artifact_dir: Option<&Path>, claude_path: &Path) -> Command {
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
        // bounded reversal of spec-003 R-5 total tool isolation (see
        // the R-5 deviation note in the 003 contract). No
        // `--json-schema` (the CLI does not enforce it; the schema is
        // embedded in the prompt by invoke_typed instead).
        Some(dir) => {
            cmd.arg("--allowedTools").arg("Write");
            cmd.arg("--disallowedTools")
                .arg("Bash Edit Read WebFetch WebSearch Glob Grep");
            cmd.arg("--permission-mode").arg("acceptEdits");
            cmd.arg("--add-dir").arg(dir);
            cmd.current_dir(dir);
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

    cmd
}
```

- [ ] **Step 2: Build does not need to pass yet (invoke_raw still passes old arg).** Skip standalone check; verified at end of Task 2.

---

## Task 2: Artifact-file `invoke_typed` + `invoke_raw` signature + delete `extract_json`

**Files:** Modify `src-tauri/src/cc_client.rs` (`invoke_raw` ~633, `invoke_typed` ~551–595, `extract_json` ~597–605).

- [ ] **Step 1: Change `invoke_raw` signature + its `build_command` call**

Find `async fn invoke_raw(args: &InvokeArgs, json_schema: Option<&str>) -> Result<Envelope, InferenceError> {` and change the signature to:

```rust
async fn invoke_raw(
    args: &InvokeArgs,
    artifact_dir: Option<&Path>,
) -> Result<Envelope, InferenceError> {
```

Inside `invoke_raw`, find the `build_command(args, json_schema, &claude_path)` call and change it to:

```rust
    let mut cmd = build_command(args, artifact_dir, &claude_path);
```

(Keep the rest of `invoke_raw` — spawn, stdin write, timeout, envelope parse — exactly as-is.)

- [ ] **Step 2: Replace `invoke_typed` body**

Replace the entire `pub async fn invoke_typed<T>(...) { ... }` with:

```rust
pub async fn invoke_typed<T>(args: InvokeArgs) -> Result<InvokeOutcome<T>, InferenceError>
where
    T: for<'de> Deserialize<'de> + schemars::JsonSchema + Send + Sync + 'static,
{
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

    let envelope = invoke_raw(&args, Some(dir.path())).await?;

    // std::fs (tokio "fs" feature is not enabled); out.json is a tiny
    // local file so the brief blocking read is acceptable.
    let raw = std::fs::read_to_string(&out_path).map_err(|e| {
        InferenceError::SchemaValidationFailed {
            details: format!(
                "agent did not produce {out} ({e}); stop_reason={sr:?}, \
                 result preview: {rp}",
                out = out_path.display(),
                sr = envelope.stop_reason,
                rp = truncate(&envelope.result, 256),
            ),
        }
    })?;
    let value: T = serde_json::from_str(&raw).map_err(|e| {
        InferenceError::SchemaValidationFailed {
            details: format!(
                "artifact did not match target type: {e} (first 256 chars: {})",
                truncate(&raw, 256)
            ),
        }
    })?;
    let metadata = metadata_from_envelope(args.phase, args.max_tokens, &envelope);
    Ok(InvokeOutcome { value, metadata })
}
```

- [ ] **Step 3: Delete `extract_json`**

Delete the entire `fn extract_json(text: &str) -> Option<&str> { ... }` and its doc comment (immediately follows the old `invoke_typed`). It now has no callers.

- [ ] **Step 4: Verify compile**

Run: `cd src-tauri && cargo check --lib`
Expected: `Finished` with no errors. (Pre-existing unrelated warnings OK; no `extract_json`/`json_schema` errors.)

- [ ] **Step 5: Verify existing tests + no stale refs**

Run: `cd src-tauri && cargo test --lib 2>&1 | tail -3`
Expected: `test result: ok.` (all pass).
Run: `grep -n "extract_json\|json_schema\|--json-schema" src-tauri/src/cc_client.rs`
Expected: no matches (defensive path fully gone).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/cc_client.rs
git commit -m "fix(inference): deliver typed output via sandboxed artifact file" -m "invoke_typed now has the headless claude agent write the result as
JSON to a per-call tempfile sandbox via a Write-only tool grant;
Quill reads it and serde-deserializes T as the sole validation. The
unenforced --json-schema path and the defensive result-fence parser
are removed (file is the sole typed channel; missing/invalid yields
SchemaValidationFailed, no app-side retry). build_command branches:
typed = Write-only sandboxed to the temp dir (scoped R-5 deviation);
invoke_text keeps total isolation. Reconciles the prior debugging
patches: keeps the Stream C FromStr fix, Sonnet 4.6 routing, and the
log-literal corrections; removes the defensive fence fallback."
```

---

## Task 3: Documentation sync

**Files:** Modify `lat.md/backend.md`, `specs/003-cc-inference-migration/contracts/cc-client.md`.

- [ ] **Step 1: `lat.md/backend.md` — Claude Code Inference Client section**

Replace the sentence describing `--json-schema`/`structured_output` parsing (currently "`--json-schema` does **not** deterministically populate `structured_output` … falls back to extracting the JSON value from `result` …") with:

```
`--json-schema` is unreliable (the CLI does not enforce it), so typed calls do not use it. `invoke_typed` instead embeds the JSON Schema in the prompt, grants the headless agent a `Write`-only tool sandboxed to a per-call temp dir, and has it write the result to `out.json`; Quill reads that file and `serde_json::from_str::<T>` is the sole validation (missing/invalid → `SchemaValidationFailed`, no app-side retry). `invoke_text` is unchanged (free-form, total tool isolation).
```

- [ ] **Step 2: `specs/003-cc-inference-migration/contracts/cc-client.md` — output contract + R-5 note**

Replace the `invoke_typed` output-contract bullet (the one starting "For `invoke_typed`: `--json-schema` is *supposed* to place…") with:

```
- For `invoke_typed`: the JSON Schema is embedded in the prompt (not `--json-schema`, which the CLI does not enforce). The agent is granted a `Write`-only tool sandboxed to a per-call temp dir and writes the result to `out.json`; `T` is obtained via `serde_json::from_str::<T>` of that file. Missing/unreadable/un-deserializable file → `InferenceError::SchemaValidationFailed`. No app-side retry.
```

Then append to the same file, under a new trailing section:

```
## R-5 deviation (feature: artifact-file typed inference, 2026-05-17)

spec-003 R-5 specified total tool isolation (`--tools ""`). Typed inference now requires a narrow, bounded exception: `invoke_typed` grants `--allowedTools "Write"` (everything else `--disallowedTools`-denied), `--permission-mode acceptEdits`, and `--add-dir`/CWD confined to a unique per-call `tempfile::TempDir` that is destroyed unconditionally on drop. Rationale: the supported-CLI premise is non-negotiable but `--json-schema` is unenforced; a minimal sandboxed `Write` grant is the smallest capability that makes the supported path sound. R-6 env-scrub, `--no-session-persistence`, `--setting-sources ""`, `--exclude-dynamic-system-prompt-sections`, `kill_on_drop`, and the 300 s timeout are all retained. `invoke_text` keeps full R-5 isolation.
```

- [ ] **Step 3: Validate docs**

Run: `lat check`
Expected: `All checks passed`.

- [ ] **Step 4: Commit**

```bash
git add lat.md/backend.md specs/003-cc-inference-migration/contracts/cc-client.md
git commit -m "docs: sync inference docs to artifact-file typed path" -m "Updates lat.md and the 003 contract to describe the artifact-file
mechanism and records the scoped R-5 tool-isolation deviation."
```

---

## Task 4: Final verification

- [ ] **Step 1: Full check**

Run: `cd src-tauri && cargo check --lib && cargo test --lib 2>&1 | tail -2`
Expected: compiles; `test result: ok.`

- [ ] **Step 2: Sweep**

Run: `grep -rn "extract_json\|--json-schema\|structured_output" src-tauri/src/cc_client.rs`
Expected: only `structured_output` may remain as an `Envelope` field (now unused by `invoke_typed` but harmless for metadata/forward-compat); no `extract_json`, no `--json-schema`.

- [ ] **Step 3: Manual verification handoff (owner)**

Document for the owner: rebuild + relaunch the app, run Analyze. Expect: Stream A/B/C log "calling Sonnet 4.6", the agent writes `out.json` in a `quill-cc-*` temp dir, run completes, `learning_runs.inference_metadata` has `stream_a/b/c` entries, rules created > 0. (Cannot be driven from this non-interactive context.)

---

## Self-Review

- **Spec coverage:** artifact-file delivery (T2), Write-only sandbox + flag changes (T1), schema-in-prompt (T2 S2), sole-channel/remove-fence (T2 S2–3), TempDir drop-guard (T2 S2), `invoke_text` unchanged (T1 None branch), docs + R-5 deviation (T3), reconcile working tree — keep FromStr/Sonnet46/log, drop fence (T2 commit msg + S3). Covered.
- **Spec refinement (flagged):** the design said "+ one filled example"; a generic literal example per arbitrary `T` is not feasible without per-type machinery (YAGNI, and callers weren't to change). The plan instead embeds `schema_for!(T)` + an explicit field-conformance instruction — same intent (a strong prompt contract), no per-caller work. Acceptable, documented here.
- **Placeholder scan:** none — all steps have exact code/commands/paths.
- **Type consistency:** `build_command(args, artifact_dir: Option<&Path>, claude_path)`, `invoke_raw(args, artifact_dir: Option<&Path>)`, `invoke_typed` consumes `args` (rebinds `let mut args`), `std::fs::read_to_string` (tokio fs feature absent), `tempfile::Builder` (dep present). Consistent.
