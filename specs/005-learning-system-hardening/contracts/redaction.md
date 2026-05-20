# Contract: Redaction & Sanitization (R-1 / C-1, H-3)

Internal module contract. Not a public API.

## `src-tauri/src/redaction.rs`

```rust
/// Mask every recognized secret/PII literal while preserving structural frame.
/// Idempotent: redact(redact(x)) == redact(x). Single mask token `‹redacted›`.
pub fn redact(input: &str) -> String
```

### Detector layers (ordered, all → `‹redacted›`)
1. Anchored cred patterns (existing set, kept; key-alternation broadened with
   `compress_prose/detect.rs` `SENSITIVE_NAME_TOKENS`).
2. Connection-string / URL userinfo: mask only `:password@`, keep scheme/host.
3. Shannon-entropy fallback: tokens ≥24 chars, base64/hex charset,
   entropy ≥ 4.0 bits/char; gated to skip git hashes, file paths, decimals.
4. Email: mask local-part, keep `@domain`.

### Invariants
- **Order**: `redact` runs BEFORE any lossy `compress_observation`/
  `safe_truncate`/`sanitize_for_prompt`, on every path.
- **Structure preserved** (FR-006): key names, URL scheme/host, `@domain`,
  `Bearer ` kept; only the secret value masked.
- **Out of scope**: personal-name detection (no reliable offline detector;
  would shred behavioral signal) — committed decision, not a gap.

### Call-site adoption (every inference input — FR-002)
| Site | Contract |
|---|---|
| `server.rs` `post_observation` | redact `tool_input/output/cwd` BEFORE `store_observation_in_background`; `202` still returned synchronously after |
| `storage.rs` `store_observation` | defense-in-depth redact before INSERT — no plaintext at rest |
| Stream A | none required (capture guarantees it) |
| Stream B `git_analysis.rs` | redact commit msgs/diffs BEFORE `compress_git_data` AND before `git_snapshots.raw_data` cache write |
| Synthesis `learning.rs:997-1028` | redact `memory_context`/`instruction_context` before sanitize/truncate |
| `memory_optimizer::build_prompt` | redact each content field before escape/truncate |
| Stream C `learning.rs:142` | invert to `compress(redact(raw))` |

### H-3 — reconcile/promote sanitization
`reconcile_learned_rules` steps 3a/3c and `promote_learned_rule` store
`sanitize_rule_content(redact(body))` into `learned_rules.content`
(`content_hash` still computed over raw file bytes for change detection).
`sanitize_rule_content` fixed to actually strip code fences per its
doc-comment; doc corrected to "injection-hardening only".

### One-time backfill
Migration-time pass redacts existing `observations` rows +
`git_snapshots.raw_data` (idempotent; guarded by a `settings` sentinel).

### Acceptance
SC-001: seeded secret/PII corpus across every capture path → 0 unredacted
tokens at rest or in any inference input; rules still produced (signal
preserved).
