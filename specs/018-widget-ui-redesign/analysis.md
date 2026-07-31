# Analysis: widget-ui-redesign

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|--------------------------|---------------------------|--------|
| US1 — 360px always-on-top widget, no split-pane, tabular/no-shift, reduced motion | Affected Components (shell, tauri.conf), Testing 2 cross-cutting checks | full |
| US2 — chrome contract: update button centered, version+What's-new in settings, AOT single source, close-to-tray, drag, right-click menu; no pace band; slate degraded pill; per-band empty states | WidgetTitleBar, GeneralTab ADD, lib.rs, view content contracts, Testing 3 | full |
| US3 — per-provider LIMITS rows, severity 50/80, stale suppression, absent when no provider, fixed hues | LimitsSection, provider-hue tokens, Testing 3 | full |
| US4 — chart overlay value+delta, hover legend, range re-scopes everything | viz/AreaChart, 6h plumbing enumeration, per-metric sources | full |
| US5 — dropdown with 5 views, in-widget swap below LIMITS, old views deleted | ViewSwitcher, views/*, Deleted list, Sequencing 6–9 | full |
| US6 — six metric hues on swatch/spark/endpoint, real series | tokens, viz/, per-metric source table | full |
| US7 — five breakdown modes, row fields designed, disclosures homed | Breakdown row-field table, Hooks `?`/retention lines | full |
| US8 — footer totals track range, Manage ⌘M app-scoped, no editable settings | UsageView footer, main.tsx accelerator | full |
| US9 — legacy deletion incl. all localStorage keys, dead-code sweep, docs truthful + lat check | Deleted list, Testing 4/6, Docs, Sequencing 10–12 | full |
| Constraints — geometry package, data contract, 8px floor, color law, a11y, gates | Modified (tauri.conf/lib.rs), Data Model, a11y semantics block, Testing 1/5 | full |
| Clarifications Q1–Q7 | Folded throughout (see plan Alignment log) | full |
| Perf budgets (constitution #10) | Testing 5 + remediation path | full |
| Follow-ups (marketing/README screenshots, capture script, global dead-code, insight rotation, polish soak) | Sequencing 13 (beads created, not executed) | full |

## Backlog Disposition

| Source P4 id | Plan work item(s) / non-goal | Disposition | Ready to resolve? |
|--------------|------------------------------|-------------|-------------------|
| — (none) | No P4 sources in hierarchy/provenance closure | n/a | n/a |

Related non-P4: `quill-qwx` (P1 design tracker) → superseded by the new epic
at create-beads with a link preserving its decision notes.

## Target Epic

New epic (created at bead-creation). No candidates existed; no ambiguity.

## Remaining Risks

- **Four compact views are net-new design** executed inside implementation
  beads (fixture-first pass per view). Mitigated by content contracts in the
  plan and the Usage view establishing the visual language first; residual
  risk is iteration time, not direction.
- **Rust range refactor breadth** — six commands + eight match arms + TS
  mirrors; mitigated by the grep acceptance check ("no `\"1h\"` arm without a
  `\"6h\"` sibling") and the back-compat wrapper being deleted in-feature.
- **Upgrade behavior** — window-state SIZE exclusion and preference deletion
  are one-way; mitigated by release-notes entry; rollback = revert the single
  squash commit.
- **Unconditional analytics IPC** — budgeted (Testing 5) with a named
  remediation (bound `get_code_stats_history`) if missed.

## Unresolved Questions

None blocking. Two intentionally deferred into their work items: exact
min/max height bounds for the content-driven window (settled during shell
work) and knip configuration scope (settled when the baseline is captured).

## Constitution Check

1. Local source-backed truth — pass (shared-WHERE aggregates; enumerated 6h
   arms prevent silent 24h fallbacks; disclosures kept).
2. Established stack — pass (extends Rust/Tauri + React TS; removes a dep).
3. Responsive execution — pass (read-side SQL, staggered fetch).
4. Recoverable mutation — pass (settings via existing transactional path;
   marker-guarded seed).
5. Typed failure boundaries — pass (degraded states as typed pill variants;
   AOT failure surfaces).
6. Zero-warning gates — pass (enumerated; knip scoped with baseline).
7. Authorized behavior testing — pass (no new test code).
8. Architecture traceability — pass (lat.md item + lat check gate).
9. Glass Cockpit discipline — tension, resolved in-feature: DESIGN.md itself
   is rewritten by this feature; old doc and new UI disagree only within the
   feature branch, never on main.
10. Measured performance — pass (budgets + method + remediation).
11. Explicit external transmission — pass (none).
12. Gated delivery — pass (worktree, squash, no push).

## Recommendation

**GO.** Every user story, constraint, and clarification answer maps to a
concrete plan item; there are no P4 backlog sources; the target epic is
unambiguous (new); the one constitution tension (#9) is definitionally
resolved by the feature itself. The plan's riskiest area — four unmocked
compact views — is bounded by content contracts and an established visual
language rather than left open.
