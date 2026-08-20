---
title: Loaded hosts invalidate tight p95 release qualification
date: 2026-08-18
last_updated: 2026-08-20
component: pi-tracking-qualification
tags: [benchmark, p95, rss, scheduler, swap, qualification, ports]
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

## Preflight check before dispatching qualification

Added 2026-08-20, during `implement-ready` run `run-20260820T050427`. The
qualification bead `quill-oyie.9` came up ready and was **not** dispatched,
because a two-command preflight showed the host was disqualified on both
counts at once:

```bash
ss -ltnp | grep -E "19876|19877"   # ports the harness must never bind
uptime                              # load average
```

A live dev Quill (`target/debug/quill`, pid 3473342) held **both** 19876 and
19877, alongside a `tauri dev` and a `vite` process, at load average 5.72.

The port dimension is separate from the load dimension recorded above and can
invalidate a run just as completely. Since the handshake consolidation moved
every identity onto the published 19876/19877 pair, the harness must pass
`QUILL_PORT`/`QUILL_CONTEXT_PORT` overrides and must never bind the ports a
live Quill already holds — contending for them can disturb the running app as
well as the measurement.

Run both checks before dispatch, not after a failed attempt. This bead had
already burned at least three attempts, one of them (attempt 3) failing purely
on RSS p95 for the load reason documented here; each of those attempts was
cheap to avoid and expensive to run.

## Prevention

- Run the two-command preflight above before dispatching qualification, and
  treat either signal as disqualifying on its own.
- Record load, swap, and top CPU consumers beside every tight p95 result.
- Reject a qualification sample before comparison when the host is outside the
  baseline's environment assumptions.
- Never convert run-to-run variance into a higher threshold or a retry-until-pass
  loop.
- Keep benchmark-method experiments uncommitted until they pass the unchanged
  acceptance gates on a controlled host.
