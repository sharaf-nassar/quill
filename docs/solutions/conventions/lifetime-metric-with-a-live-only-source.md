---
title: A lifetime metric fed only by live state collapses when the live source exits
date: 2026-08-22
component: live-tracker-overlay
tags: [overlay, pi, agent-count, lifetime-metrics, sidechains, session-breakdown]
problem_type: convention
---

# A lifetime metric fed only by live state collapses when the live source exits

## Problem

The Sessions row's agent group — purple count, bot icon, agent runtime — renders
while a Pi sub-agent is running and vanishes entirely the moment that agent
finishes, even though the session is still live and the group is labelled a
lifetime total ("N total agents run during this session").

It reads as a rendering bug in the group's visibility predicate. It is not. The
predicate is correct and the value it is handed is zero.

The louder half of the same defect is easy to miss: while the group *is*
visible, its number is wrong. The Pi root session
`01a02801-4dae-70fb-bd52-d06f3a197735` rendered `1` while
`pi_session_lifecycle` held 31 agent children for it. The live count was wearing
the lifetime label the whole time; the disappearance is just the moment the live
count reaches zero.

## Root cause

`SessionBreakdown.agent_count` has two producers, merged in
`src-tauri/src/storage.rs:16034`:

- the retained side counts DB chains where `chain.is_sidechain`
  (`src-tauri/src/storage.rs:16005-16009`);
- the live overlay pushes its current open-agent snapshot
  (`src-tauri/src/live_tracker.rs:1468`).

For Claude and Codex that first producer is the lifetime answer, because their
sub-agents are sidechains recorded inside the parent's own transcript. **Pi
sub-agents are separate top-level sessions**, so they never produce a sidechain
row: on a real install, `session_events` and `transcript_analytics_sources` hold
zero rows with `is_sidechain = 1` for `provider = 'pi'`. The retained producer
is structurally always `0` for Pi.

That leaves the overlay as the sole source, and it is live-only by
construction. `src-tauri/src/live_tracker.rs:1465-1474` writes `agent_count`,
`agent_runtime_secs` and the agent share of `active_runtime_secs` inside
`if let Some(agents) = agents_by_parent.get(&key)`, where `agents_by_parent`
is built only from sessions still present in `state.sessions` — and a Pi
`SessionEnd` removes the session at `src-tauri/src/live_tracker.rs:931-932`.

Last agent exits → pushed becomes `None` → the merge yields `Some(0)` →
`hasAgentTotals` is false (`src/components/widget/views/UsageView.tsx:564-566`)
→ the whole group unmounts. Nothing in the pipeline is broken; the lifetime
metric simply never had a lifetime source for this provider.

The contract already said so, in two places that disagree with each other.
`lat.md/frontend.md:299` states the invariant the user expects — *"Positive
totals remain visible after every agent closes"*. `lat.md/frontend.md:301` then
defines the Pi input as *"retained sidechains plus current explicit Pi agents"*,
which cannot satisfy it. A test encodes the losing side:
`pi_nested_agents_flatten_with_roles_and_unresolved_edges_stay_visible` asserts
`agent_count` dropping from `Some(3)` to `Some(2)` after an agent ends
(`src-tauri/src/live_tracker.rs:3178`), so the suite was green on the bug.

## What didn't work

- **Reading the frontend first.** `hasAgentTotals` looks like the obvious
  suspect and is a one-line predicate, so it invites a "just show it when
  agents exist" patch. It is already correct; a change there would have made
  the row render a wrong zero instead of hiding it.
- **Assuming the overlay assignment was the whole story.** The overlay does
  clobber rather than merge, which matches an earlier learning
  (`docs/solutions/conventions/unconditional-overlay-erases-a-siblings-new-source.md`),
  and guarding it the same way changes nothing here — the sibling producer it
  would preserve is a structural `0`, not a real value. Same shape, different
  defect: there the second source existed and was erased, here it never
  existed.

## Fix

Filed as `quill-evxs`, unlanded as of this writing. The approach is to give Pi
a durable lifetime source instead of promoting live state: `pi_session_lifecycle`
already carries 317 rows with `lineage_state = 'agent'` and a populated
`direct_parent_session_id`. Its `visible_root_session_id` is `NULL` on every
row today, so the direct-parent edges have to be walked to the visible root with
the same rules `resolve_pi_root` applies live, and the durable set unioned by
session id with the open agents so a currently-running agent counts once.

## Prevention

- **A metric named "lifetime" needs a durable producer per provider, not one
  that happens to cover the providers you tested.** Before shipping a
  cross-provider aggregate, check each provider's contribution independently;
  a provider whose contribution is structurally constant is invisible in any
  test that only exercises the live path.
- **The fastest confirmation is a query, not a code read.** One
  `GROUP BY provider, is_sidechain` over `session_events` settled a question
  that three files of overlay logic only hinted at. When a metric is
  provider-shaped, count the rows per provider first.
- **A group that disappears is the same bug as a group showing the wrong
  number.** "It vanishes when the count hits zero" and "it shows 1 instead of
  31" are one mechanism. Chasing only the reported symptom — visibility — leads
  straight to the frontend predicate and away from the cause.
- **When two `lat.md` sentences about one field disagree, that gap is the
  finding.** Here the invariant and the input definition sat two paragraphs
  apart in the same section, and the code implemented the input definition.
- **A green test asserting the buggy value is negative evidence, not
  reassurance.** The dropping-count assertion was written deliberately for
  nested-agent flattening; it silently became the specification for a defect.
