---
title: Model chart splits one model across CLIs
date: 2026-08-22
last_updated: 2026-08-24
component: model analytics identity
tags: [widget, usage, model-analytics, normalization, constitution]
problem_type: convention
---

## Problem

The Usage chart's Model grouping shows one model as up to three series
(`gpt-5.6-sol` under claude and codex, `cliproxyapi/gpt-5.6-sol` under pi),
and it is not obvious where model-name standardization is allowed to live.
Opposite failure shape of
`docs/solutions/conventions/usage-graph-collapses-model-dimensions.md`
(pre-`quill-zddt` collapse into one PI series).

## Root cause

The Model dimension keys on exact provider-qualified identity
(`src/components/widget/chartDimensions.ts:50`). Pi ids embed the upstream
gateway prefix, and cross-CLI use of one model carries different provider
qualifiers, so equal models never merge. That was deliberate: `quill-zddt`
specified exact-identity Model mode and banned inferring a vendor from an
id.

## What didn't work

Two tempting approaches are wrong:

- **Normalizing into `derived_model_id`.** It is the rollup grain
  (`model_usage_hourly` primary key) and, per this investigation of the
  live schema, the only identity surviving raw-observation pruning
  (`raw_pruned` flag). Overwriting it with a normalized name destroys
  provider-qualified granularity irreversibly for pruned hours.
- **Amending the constitution first.** Constitution #1 bans *inventing*
  data, not deriving groupings. The "no catalog, no alias" rule lives in
  feature design docs (`lat.md/features.md:193`, `DESIGN.md` Model-Shade
  Rule, `ModelsView.tsx` header), which are amendable per change.
  Precedent: `derived_model_id` (migration 29) added derived attribution
  with no constitution change.

## Fix

Tier-1 normalization is a pure function of the stored string (strip Pi's
`{upstream}/` prefix, merge equal residue), so it needs no schema, no
backfill, and no backend field while its only consumer is chart grouping —
`get_model_sessions` has no frontend caller. Frontend
`normalizeModelId()` + Model-dimension regroup, filed as `quill-dbs1`,
landed in `844ceaf`: the helper lives at
`src/components/widget/chartDimensions.ts:35` and Model grouping keys on
`model:${normalized}` at `:60`, with no schema or backend change. Tier-2 (catalog/alias knowledge not derivable
from the string, e.g. mapping dated `claude-haiku-4-5-20251001` to a
family, or vendor-labeling cross-routed traffic) requires a sibling
derived column with versioned re-stamping — the migration-29 playbook.

## Scope of the no-family-parsing rule

`lat.md/features.md:193` bans "model catalog, family parsing, alias,
friendly name" — that sentence describes the **Models view** only, not the
repo. Session rails do the opposite by design: `agentModelFamily`
(`src/utils/format.ts:35`) parses `Opus`/`Sonnet`/`Haiku`/`Fable` and
`Sol`/`Terra`/`Luna` out of raw ids for the agent rail, shipped by closed
bead `quill-pe96` and pinned as a contract in
`lat.md/live-subagent-count-tests.md:61-65`. A reader who applies :193
globally will either stall on a rail label change or "fix" the rails by
removing shipped behavior.

The distinction is what the label is *for*: the Models view attributes
usage and must not merge distinct identities, so it keys on exact
provider-qualified strings. A rail is a 100 px status chip whose job is
recognition, and it keeps the raw id in its tooltip/ARIA — nothing is lost,
nothing is invented. Same test as the Prevention section below: derived at
read time from stored evidence, displayed beside the evidence, never
substituted for it.

Surfaced while triaging `quill-ihbn` (root session rows carry no model
label; unlanded as of this writing) — its follow-ups `quill-0fhp` and
`quill-nm0h` extend the same rail vocabulary.

## Prevention

Raw ids stay stored and visible; normalized names are derived facets.
Before adding an identity mapping, ask: is it a pure function of stored
evidence (compute at read time) or external knowledge (sibling derived
column, never a rewrite)? Never repurpose an aggregation-grain column for
display semantics.
