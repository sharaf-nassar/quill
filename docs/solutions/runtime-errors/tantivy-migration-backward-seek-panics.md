---
title: Sparse project postings crash the session-index migration
date: 2026-09-20
component: session-index
tags: [tantivy, migration, startup, postings]
problem_type: runtime-errors
---

# Sparse project postings crash the session-index migration

## Symptom

`npx tauri dev --config src-tauri/tauri.dev.conf.json` builds successfully,
then panics during startup in Tantivy 0.25.0 `SegmentPostings::seek`:

```text
assertion failed: self.doc() <= target
```

## Cause

Schema-10 migration reconstructs nonstored exact cwd fields by scanning each
project's postings for each 256-document migration batch. A newly opened
posting list already points at its first matching document. If that document
is later than the batch start, unconditional `seek(start)` attempts a backward
seek and trips Tantivy's debug assertion.

The original fixtures gave every document the same cwd. Their posting list
started at document zero, so they never exercised this condition.

## Fix and verification

Read `postings.doc()` first and seek only when it is less than the batch start.
Keep the existing end-of-batch and terminated-list checks.

The regression in `src-tauri/src/sessions/pipeline_migration_tests.rs` uses
771 documents grouped across three cwd terms. It reproduces the original
panic, then verifies all 257 documents per cwd survive migration and reopen.

The observed panic happened while building the staged index, before directory
promotion. The original schema-9 index remained intact. Do not delete the live
index to work around this failure; corrected migration retries staging.
