---
title: Usage graph collapses model dimensions into Pi
date: 2026-08-19
component: widget usage graph
tags: [widget, usage, pi, model-analytics]
problem_type: bug
---

## Problem

The widget graph can show one large `PI` area although Pi ran several models.

## Root cause

`UsageView` requests `useProviderTokenSeries` at
`src/components/widget/views/UsageView.tsx:737`. Its storage query groups
`token_snapshots` by the CLI `provider` alone at
`src-tauri/src/storage.rs:14875-14907`.

Pi reconciliation records the upstream provider and model as one validated
`provider/model` identity at `src-tauri/src/transcript_analytics.rs:1885-1896`.
The graph does not read that identity, so it cannot separate Pi traffic by
upstream LLM provider or model.

Per this investigation, the production 24-hour snapshot had one Pi graph
series while its model observations contained separate
`cliproxyapi/claude-opus-5`, `cliproxyapi/gpt-5.6-sol`,
`cliproxyapi/gpt-5.6-terra`, and `cliproxyapi/claude-sonnet-5` rows.

## What didn't work

Changing labels or splitting the existing `token_snapshots` result cannot add
LLM or model dimensions. That table stores only the CLI provider for this
query.

## Fix

Fix filed as `quill-zddt`, unlanded as of this writing. It will provide one
bucketed model-evidence aggregate with CLI, recorded upstream LLM, and exact
model groupings. `Models` will be the persisted default. The graph headline,
delta, legend, insight split, and footer must use the same selected aggregate.

## Prevention

When adding a graph grouping, verify the source table contains that dimension.
Do not claim a common total when the graph and related readouts use different
aggregates. Document the grouping and total invariant in `lat.md` with the
implementation.
