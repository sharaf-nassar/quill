---
title: Usage graph collapses model dimensions into Pi
date: 2026-08-19
last_updated: 2026-08-19
component: widget usage graph
tags: [widget, usage, pi, model-analytics]
problem_type: bug
---

## Problem

The widget graph can show one large `PI` area although Pi ran several models.

## Root cause

Before `quill-zddt`, per this investigation, the graph grouped only
`token_snapshots.provider`. That turned all Pi-routed work into one `PI` area.

Current code groups model-evidence buckets in
`src/components/widget/views/UsageView.tsx:233-272` and reads them from the
shared model overview at `src/components/widget/views/UsageView.tsx:766-808`.
Pi reconciliation records upstream provider and model as one validated
`provider/model` identity in `src-tauri/src/transcript_analytics.rs:1885-1896`.

The original production 24-hour snapshot had one Pi graph series while model
evidence contained separate `cliproxyapi/claude-opus-5`,
`cliproxyapi/gpt-5.6-sol`, `cliproxyapi/gpt-5.6-terra`, and
`cliproxyapi/claude-sonnet-5` rows.

## What didn't work

Changing labels or splitting the old `token_snapshots` result could not add
LLM or model dimensions. That table stored only the CLI provider for that
query.

## Fix

`quill-zddt` replaced the CLI-only aggregate with model-evidence buckets.
The widget defaults to persisted `Models` grouping and can switch to CLI or
recorded upstream LLM grouping. Model-less rows remain unattributed. The graph
headline, delta, legend, insight split, and footer read the same aggregate.

## Prevention

When adding a graph grouping, verify the source table contains that dimension.
Do not claim a common total when the graph and related readouts use different
aggregates. Document the grouping and total invariant in `lat.md` with the
implementation.
