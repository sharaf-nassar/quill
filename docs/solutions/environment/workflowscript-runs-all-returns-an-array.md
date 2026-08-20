---
title: workflowScript runs.all returns an array, not a key map
date: 2026-08-20
last_updated: 2026-08-20
component: implement-ready
tags: [subagents, workflowscript, orchestration, rail, worker-output]
problem_type: workflow
---

# workflowScript runs.all returns an array, not a key map

## Problem

A three-worker `implement-ready` wave completed successfully — all three
children committed real work — and the orchestrating workflow then reported:

```text
TypeError: Cannot read properties of undefined (reading 'output')
    at workflow-script.js:156:25
```

The whole wave's structured results were lost. The parent had to reconstruct
every worker's result JSON from saved artifacts before it could feed the rail.

## Root cause

The workflow script launched children with stable keys and then indexed the
result by those keys:

```js
const out = await runs.all([
  { key: "fqwp", agent: "worker", context: "fresh", task: fqwp },
  { key: "ppbv", agent: "worker", context: "fresh", task: ppbv },
  { key: "ihbn", agent: "worker", context: "fresh", task: ihbn },
]);

return { fqwp: out.fqwp.output, ... };   // out.fqwp is undefined
```

`runs.all` resolves to an **array** of results in input order. The `key` is
used for trace and steering identity, not for indexing the return value. So
`out.fqwp` is `undefined` and the property read throws — after every child has
already finished and committed.

The failure is maximally late and maximally expensive: children are launched,
run to completion, and only the final return line fails, so the cost of the
wave is fully paid and none of its output is delivered.

## What didn't work

- Re-reading the run's `events.jsonl`: it carries
  `subagent.workflow.trace` entries proving each child reached
  `state: "completed"` with its own `runId`, but no child output.
- `subagent({ action: "status", id: <childRunId> })` through a scripted MCP
  call returned `ok: false` with no data.

## Fix

Index positionally:

```js
return out.map((r) => r.output).join("\n\n=====\n\n");
```

To recover a wave that already crashed this way, read the per-child artifact
that pi-subagents writes regardless of the workflow's own outcome:

```text
~/.pi/agent/sessions/<project>/subagent-artifacts/<childRunId>_worker_0_output.md
```

`subagent({ action: "status", id: <runId>, view: "transcript" })` names that
path, and the trace events in the run directory's `events.jsonl` name each
child's `runId`.

## Prevention

- Treat `runs.all`'s return as an ordered array. Use `key` for steering and
  trace identity only.
- Keep the final `return` of a workflow script trivial. Any expression that can
  throw there discards an entire completed wave; shaping and joining is cheaper
  to do in the parent after the results arrive.
- Workers should be told to emit their result JSON as their whole final
  message, which is what made artifact-based recovery possible here. Prose
  wrapped around the JSON still parses, but costs a regex per worker.
