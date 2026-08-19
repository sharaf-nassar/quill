---
title: Model chart splits one model across CLIs
date: 2026-08-22
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
unlanded as of this writing. Tier-2 (catalog/alias knowledge not derivable
from the string, e.g. mapping dated `claude-haiku-4-5-20251001` to a
family, or vendor-labeling cross-routed traffic) requires a sibling
derived column with versioned re-stamping — the migration-29 playbook.

## Prevention

Raw ids stay stored and visible; normalized names are derived facets.
Before adding an identity mapping, ask: is it a pure function of stored
evidence (compute at read time) or external knowledge (sibling derived
column, never a rewrite)? Never repurpose an aggregation-grain column for
display semantics.
