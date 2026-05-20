# Pre-Remediation Baseline — Learning System Hardening

**Captured**: 2026-05-17 on branch `005-learning-system-hardening` (HEAD before
any pipeline change) | Tasks: **T006** (SC-011 anchor) + **T005** (build baseline)

This is the **only** SC-011 comparison anchor. It is captured before any
Phase 2+ change touches the rule pipeline (the legacy archive-wipe, T032,
destroys legacy rules). Do not regenerate after changes land.

## SC-011 learning-value baseline (read-only DB + filesystem snapshot)

Source: `~/.local/share/com.quilltoolkit.app/usage.db` (opened read-only) and
the on-disk learned-rule directories.

- **`schema_version` MAX = 24** → confirms migration **25** is next (validates
  research.md R-0; the stale "21" was corrected).
- **On-disk active rules**: 0 in every scope
  (`~/.claude/rules/learned/`, `~/.config/quill/learned-rules/{shared,codex}/`
  all contain 0 `.md`). No rule is currently globally active.
- **`learned_rules` rows**: **20 total** — 0 active on-disk, **20
  discovered-only** (`file_path` empty).
- **By provider scope**: `["claude"]` = 14, `["claude","codex"]` (shared) = 6.
- **By stored state**: `emerging` = 11, `suppressed` = 9.
- **Anti-patterns** (`is_anti_pattern=1`): 6.

### Rule inventory (top 15 by observation_count)

```
tabs-over-spaces
validate-inputs-at-boundary
comprehensive-source-gathering-before-authorship
post-write-validation-verification-loop
regex-syntax-trial-and-error-approach
line-number-extraction-for-code-review
post-edit-line-range-verification-iterations
repeated-ripgrep-dependency-discovery
composite-pre-commit-verification-sweep
runtime-validation-gap-static-gates-pass
mcp-credit-exhaustion-parallel-dispatch
prefer-immutable-updates
avoid-blanket-exceptions
use-async-await
native-python-types
```

### Reviewer-usefulness sample (maintainer judgment — REQUIRED at T068)

SC-011 compares **count AND reviewer-judged usefulness** of *genuinely useful*
rules. The count baseline is above (20 discovered; 11 non-suppressed
`emerging`). The **usefulness** half is an inherently manual maintainer
assessment (spec Assumptions: "assessed by a maintainer against a
representative sample") and is recorded here as a template to be filled by the
owner **now** (before changes) so T068 has a real comparison anchor:

| Rule name | Genuinely useful? (Y/N) | Note |
|-----------|--------------------------|------|
| tabs-over-spaces | _TBD by maintainer_ | |
| validate-inputs-at-boundary | _TBD_ | |
| comprehensive-source-gathering-before-authorship | _TBD_ | |
| post-write-validation-verification-loop | _TBD_ | |
| prefer-immutable-updates | _TBD_ | |
| avoid-blanket-exceptions | _TBD_ | |
| use-async-await | _TBD_ | |
| native-python-types | _TBD_ | |
| _(remaining 12 of 20 — extend as needed)_ | _TBD_ | |

> Owner action: mark Y/N before Phase 2. T068 re-runs the same judgment
> post-remediation and asserts count + useful-count ≥ this baseline (SC-011).

## T005 build baseline

- Toolchain: `cargo 1.94.0 (85eff7c80 2026-01-15)`.
- Pre-change compile + warning baseline (Setup phase, with the T001/T002
  stub modules + `lib.rs` mod lines in place):

```
cargo check  : Finished dev in 4.22s  — exit 0, 0 errors, 0 warnings
cargo clippy : exit 0 — 0 warning/error lines
```

Baseline is **clean** (0 warnings). Any warning introduced by later phases is
a regression against this anchor; T069 must restore clippy-clean before the
T056 `-D warnings` CI gate.

## T068 SC-011 tuning review (post-remediation)

**Promotion-gate defaults (committed, with rationale):**

| Knob | Default | Basis |
|---|---|---|
| `learning.min_eligibility` | **0.6** (Wilson scale) | = the existing `compute_state` `confirmed` cutpoint, so "eligible for review" ≡ "would reach `confirmed`" — an already-tuned, coherent cutpoint, not an invented constant (research R-6 Decision 1). Legacy `learning.min_confidence` honored as fallback. |
| min evidence cluster | **≥3 resolved distinct citations AND ≥1 distinct source** | "a pattern seen once does not become a permanent global rule" (spec.md); uniform across Stream A/B/C (research R-6 Decision 3). |
| redaction entropy | **4.0 bits/char** (charset+length+git-hash/path gated) | base64 max is 6.0; prose/identifiers sit ~3.0–3.7 — 4.0 with gating catches real unprefixed secrets while sparing identifiers (research R-1 Decision 2; the single biggest SC-006 quality lever). |

**As-built behavioral note (validated by the test suite, not a regression):**
a brand-new rule meeting only the bare 3-citation minimum peaks at a Wilson
lower bound ≈0.554 (`evidence_scale` floors at 5), i.e. **below 0.6** — so
eligibility intentionally requires evidence *accumulated across runs* or human
`accept` feedback (`W_op` dominates), not a single bare batch. This is the
designed anti-one-off behavior (R-6), exercised green by the US3/US4
learning-logic tests (`eligible_for_review_*`, grounding/cluster/verdict,
synthesis-decision matrix; full lib suite 107 passed / 0 failed).

**SC-011 comparison status:** the pre-remediation **count** anchor is recorded
above (20 discovered rules; 0 active; 14 claude / 6 shared; 11 emerging / 9
suppressed). The **reviewer-judged-usefulness** half is, per spec Assumptions,
an inherently manual maintainer assessment — the owner fills the
"Reviewer-usefulness sample" table above and re-runs the same judgment
post-remediation as **quickstart V8 (T070)**, asserting count + useful-count ≥
this baseline. The tuning defaults are not expected to reduce genuinely-useful
rules (the gate down-weights one-off/hallucinated/contradicted candidates and
routes survivors to human review, rather than discarding evidence); confirm
empirically at V8 and lower `min_eligibility`/`min_evidence_count` only if a
real regression is observed.
