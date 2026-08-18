---
title: Loaded hosts invalidate tight p95 release qualification
date: 2026-08-18
component: pi-tracking-qualification
tags: [benchmark, p95, rss, scheduler, swap, qualification]
problem_type: environment
---

# Loaded hosts invalidate tight p95 release qualification

## Problem

Pi release qualification requires RSS, event-loop, reconciliation, and Sessions
p95 values to remain within 10% of the frozen baseline, with recovery below
31.5 seconds. Runtime correctness stayed green, but repeated comparison runs
crossed different p95 gates without source changes.

The final controlled sequence recorded RSS growth of 62,238,720, 73,969,664,
and 64,229,376 bytes against a 61,852,876.8-byte limit. Sessions overlay crossed
its limit in two runs. Other runs on the same implementation passed those same
gates.

## Root cause

The host was not a valid low-noise qualification environment. Per this
investigation, load average ranged from 12 to 17, `dockerd` and `containerd`
each consumed several CPU cores, several Pi/Quill/Scribe processes were active,
and all 8 GiB of swap was used. The harness measures wall-clock micro-latency
and process RSS, both of which move with scheduler and allocator pressure.

The frozen baseline in `specs/028-pi-agent-tracking-hardening.md` records one
machine profile and explicitly requires equivalent-environment comparison. The
current evidence does not isolate implementation cost from host contention.

## What didn't work

- Moving protocol fixture construction out of long-lived async closures changed
  allocation lifetime but did not make three runs pass.
- A fresh fleet subprocess and baseline-equivalent handler preconditioning still
  crossed RSS and Sessions limits.
- CPU affinity and `MALLOC_ARENA_MAX=1` did not stabilize the gates.
- Retrying until three lucky samples passed was rejected because it cherry-picks
  nondeterministic evidence.
- Changing thresholds or workloads would erase the approved release contract
  rather than diagnose it.

## Fix

No code fix has landed. Recovery is held in `quill-oyie.18` until a human
reduces background load and clears swap, or provides a controlled qualification
host. The next run must execute three consecutive ordinary comparisons without
sample selection.

If controlled runs still fail, file separate measured production performance
bugs for RSS and Sessions. Do not attribute loaded-host results to production
code before that reproduction.

## Prevention

- Record load, swap, and top CPU consumers beside every tight p95 result.
- Reject a qualification sample before comparison when the host is outside the
  baseline's environment assumptions.
- Never convert run-to-run variance into a higher threshold or a retry-until-pass
  loop.
- Keep benchmark-method experiments uncommitted until they pass the unchanged
  acceptance gates on a controlled host.
