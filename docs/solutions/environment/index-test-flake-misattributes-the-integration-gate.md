---
title: A concurrency-flaky index test misattributes the integration gate
date: 2026-08-20
last_updated: 2026-08-20
component: session-search
tags: [tests, concurrency, tantivy, flake, integration-gate, rail, attribution]
problem_type: environment
---

# A concurrency-flaky index test misattributes the integration gate

## Problem

`verify-integration --gates` exited 10 while integrating `quill-fiqx`, a
pure-refactor commit. Exit 10 means the gate failed on main and the rail
attributes the breakage to the task that just landed, so the default reading is
"the commit you just made broke main".

```text
test sessions::tests::opening_index_removes_only_legacy_pi_non_conversation_documents ... FAILED
thread '...' panicked at src/sessions.rs:6071:63
test result: FAILED. 507 passed; 1 failed; 6 ignored
```

That reading was wrong. The failing test lives in `src-tauri/src/sessions.rs`,
which the landing commit never touched.

## Root cause

`src/sessions.rs:6071` is
`let index = SessionIndex::open_or_create(temp.path()).expect("reopen index");`
— column 63 is the `.expect`, so `open_or_create` returned `Err`.

`SessionIndex::open_or_create` ends in `index.writer(50_000_000)`
(`src-tauri/src/sessions.rs:1169-1170`). Every test that opens a `SessionIndex`
therefore reserves a 50 MB tantivy writer heap and spawns tantivy indexing
threads. The test is plain `#[test]`, not `#[serial]`, so several such tests
run concurrently under the default harness parallelism and reserve that budget
at the same time. On a loaded host the writer allocation or its thread spawn
can fail.

50 MB is a production figure. These tests index a handful of documents.

The host at the time was running a live dev Quill, a `tauri dev` session, and
`vite`, at load average around 5.7 — the same class of host pressure recorded
in `loaded-host-invalidates-p95-qualification.md`.

## How the attribution was settled

Four cheap checks, in the order that resolves fastest:

1. Run the single test in isolation — `cargo test <name>` passed in 0.49s.
2. Check whether the landing commit touches the failing file at all — it did
   not; `git show --stat` listed four unrelated files.
3. Check whether the commit could have changed test interleaving. It had
   consolidated `#[serial]` env guards, so this mattered. Comparing
   `git show <before>:<file> | grep -c serial` against the same for the new
   commit showed 4 → 5: the commit *increased* serialization, so it could not
   have raised concurrency.
4. Re-run the full suite. Three consecutive runs passed 508/508.

Only then re-run `verify-integration --gates`, which passed and released the
lock normally.

## Fix

None landed. Filed as `quill-l459` (P2), with the reduced test-only writer
heap as the cheapest candidate, `#[serial]` on the index tests as the fallback,
and capturing the real `Err` string instead of `.expect(...)` so the next
occurrence names its own cause.

## Prevention

- A gate failure in a file the landing commit never touched is a flake
  hypothesis, not a conclusion. Run the four checks above before either
  reverting the commit or waving the failure through — both shortcuts are
  wrong, and the rail deliberately keeps the lock so there is time to look.
- Never hold a production-sized resource budget in a test that runs under
  default parallelism. A 50 MB writer heap times N concurrent tests is a
  resource reservation nobody chose.
- When a test can only fail under concurrency, `.expect("...")` hides the
  reason. Capture the error into the panic message so one occurrence is enough
  to diagnose.
- Record the host's load and what else was running beside any intermittent
  test failure, the same way tight p95 qualification results already require.
