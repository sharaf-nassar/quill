---
title: An unconditional overlay assignment erases a sibling task's new source
date: 2026-08-20
last_updated: 2026-08-20
component: live-tracker-overlay
tags: [overlay, precedence, parallel-tasks, session-breakdown, model-id, review]
problem_type: design
---

# An unconditional overlay assignment erases a sibling task's new source

## Problem

Two beads implemented in parallel each landed individually correct, passed the
full gate on main separately, and together produced a user-visible regression
that no per-task review could see.

`quill-ihbn` (`dbe2b39`) added `SessionBreakdown.model_id` and populated it from
the live fold. `quill-0fhp` (`3e49a00`) then made the retained SQL side supply
the same field from a ranked primary model. After both landed, a session that
was live but had not yet folded a model rendered **no** model at all, even
though its recorded usage evidence existed and the retained query had just
supplied it.

## Root cause

The overlay assignment was unconditional. Before the fix, at
`src-tauri/src/live_tracker.rs:1461` (as of `dbe2b39`):

```rust
row.model_id = session.model.clone();
```

That is safe only while the other producer is a constant. When `quill-ihbn`
wrote it, `session_breakdown_query` hardcoded `model_id: None`, so overwriting
with `None` cost nothing. `quill-0fhp` then made that same field carry a real
value, and the identical line became an eraser.

Both written contracts already specified the correct precedence, so the record
was right and only the code was wrong:

- `src-tauri/src/models.rs:493-496` — "the retained primary model ranked from
  its recorded usage, replaced by the live fold's own model when the fold knows
  one".
- `lat.md/backend.md:1524` — "the overlay's own live-folded model still
  outranks it whenever the fold has folded one".

The two sibling assignments in the same loop already guarded, which is what
made the outlier obvious once the whole diff was read at once: `row.pi_lineage`
used `.or_else(...)` and `total_tokens` used `if let Some(total_tokens) =
session.live_tokens`.

The damage concentrated on exactly the rows a user watches. The no-model window
is spawn to the child's first assistant message being folded, and `quill-w5bu`
(`a5b9aaa`) had just removed every placeholder by deliberate decision, so the
erased value degraded to blank rather than to a visible fallback.

## Why the gates missed it

Nothing was wrong with the gates. Each task ran the full suite green on a main
that contained the other task's change, and the suite still passed, because the
only existing precedence test covered the fold-**has**-a-model direction. The
missing direction — fold has none, retained has one — had no test, since before
`3e49a00` it was not a reachable state.

## Fix

Guard the assignment, matching its siblings (`00ae490`, bead `quill-qmom`,
now at `src-tauri/src/live_tracker.rs:1462`):

```rust
if let Some(model) = &session.model {
    row.model_id = Some(model.clone());
}
```

Regression test at
`src-tauri/src/live_tracker.rs:4996`,
`a_fold_with_no_model_keeps_the_retained_primary_model`, which fails against
the unguarded code with `left: None, right: Some("retained-model-x")`.

## Prevention

- When one task adds a field fed by a single source, and a later task adds a
  second source for the same field, revisit the first task's write site. An
  assignment that was total is now a precedence decision.
- A precedence rule stated in prose (`models.rs`, `lat.md/backend.md`) is not
  enforced by anything. Both directions need a test, including the direction
  that is currently unreachable — it becomes reachable the moment the second
  source ships.
- In an overlay loop, an unconditional assignment sitting next to guarded
  siblings (`.or_else`, `if let Some`) is the shape to look for. The
  inconsistency within one loop body is the cheapest available signal.
- Review the combined range across parallel tasks, not just per-task diffs.
  Each worker branches from main and cannot see its siblings' unlanded work, so
  the interaction is invisible from any single task's vantage point. This
  defect was found by exactly that pass over `3b621a2..a5b9aaa`.
