---
lat:
  require-code-mention: true
---
# Transcript Memory Test Specs

Focused incident regressions for quill-r93f use synthetic transcripts and isolated SQLite/Tantivy directories, never production session contents.

## Source Lifetime And Fairness

One source budget covers decoding through stalled consumption; the fixed decoder survives parser failure.

`transcript_work::tests::source_lifetime_is_bounded_through_stalled_consumption_and_decoder_survives_panic` holds one normalized source output across a deliberately stalled consumer. A second source cannot decode until that output drops; consecutive decodes use the same named thread, and a parser panic releases admission without killing that thread. Self-submission and source admission from the decoder thread must panic promptly rather than deadlock, and a subsequent ordinary decode must still succeed. FIFO source admission covers decoding through commit/drop; root inventory and graph orchestration do not occupy the decoder for a whole root.

## Shared Source Versions And Retry

Overlapping source consumers preserve appends and last-good data while unchanged versions skip decoding.

`transcript_analytics::tests::shared_notify_sweep_versions_append_hints_and_failures_preserve_both_consumers` overlaps two real owning calls while a Tantivy writer stalls the first, appends during that work, and checks the appended message survives. Subsequent identical admissions perform zero decodes. New Search hints re-extract Search only, identical hints do nothing, and watcher admission cannot erase hints. An injected SQLite insert failure preserves previous analytics while Search succeeds; analytics retry leaves the successful Search checkpoint unchanged. A sparse Pi source exceeding the 4 GiB streaming input budget preserves both last-good consumers and settles as a durable rejection rather than retryable work.

## Durable Rejections

A failed source version is remembered separately from successful Search and analytics checkpoints; identical rejected versions perform no decode or diagnostic rewrite.

`transcript_analytics::tests::rejected_versions_preserve_success_and_rearm_on_change` checks successful rows and fingerprints survive rejection, Search rejection survives reopen, and changed valid content clears both rejections. Checkpoints include canonical path, nanosecond mtime, size, and parser policy; legacy failures require revalidation. Infrastructure failures remain retryable.

## Empty Codex Recovery

An empty rollout is rejected without inventing native identity; unchanged rejection survives restart, and the producer's first valid append rearms both consumers.

`model_usage::tests::empty_codex_rejection_survives_reopen_and_recovers_after_append` checks durable model and Search rejection, no successful empty Search checkpoint, preservation through a new model inventory generation without rewriting attempt diagnostics, and normal replacement after `session_meta` arrives.

## Rejection Policy Invalidation

Negative checkpoints are valid only for their canonical path, source fingerprint, and decoder policy; diagnostics are bounded independently of input size.

`transcript_identity::rejection_tests::changed_source_or_parser_policy_rearms_a_rejection` checks policy upgrades, path changes, changed bytes, the 1024-character diagnostic cap, and refusal to record a raced source.

## Finite Live Retry Budget

Six failed attempts retire one queued revision so a broken dependency cannot keep the immediate drain alive forever; recovery can admit a fresh revision.

`tests::retained_source_retry_budget_yields_to_recovery` exercises the real queue without sleeps and verifies a later admission resets the failure count.

## Large Streamed Pi Source

A 400 MiB synthetic Pi file must preserve complete searchable messages, tool-result previews, and native usage while staying below 2 GiB peak RSS in an isolated process.

Qualification on 2026-09-19 (Linux, default allocator, isolated synthetic data): the 400 MiB tool-output case passed in 31.32 seconds with 164752 KiB peak RSS. The existing 244 MiB text-heavy case passed four growing versions, peaking at 857228 KiB RSS with zero swap and one decode per changed version. These are workload-specific measurements, not a universal RSS guarantee.

`transcript_analytics::tests::large_pi_stream_preserves_search_tools_and_usage_under_memory_budget` is ignored by default. Run alone with `uv run --locked --project claude-integration/mcp cargo test large_pi_stream_preserves_search_tools_and_usage_under_memory_budget -- --ignored --nocapture --test-threads=1` from `src-tauri/`. Twenty 20 MiB tool results exercise raw input beyond the former 256 MiB cap without retaining their discarded tails. This qualifies large tool-output-heavy sources, not unlimited conversation text or arbitrary JSON expansion.

## Fanout Hint And Failure Ownership

Ancestor reconciliation preserves each descendant's Search hints and records classification failures against the failing source without erasing last-good analytics.

`transcript_analytics::tests::fanout_preserves_descendant_hints_and_records_its_classification_failure` reconciles a Codex ancestor that changes its child's resolved root, then repeats with an unreadable child. Only the notifying source receives its hints; the child retains its metadata, receives its own failure record, and keeps the last committed analytics root.

## Cache Lifetime

Expired analytics buckets are evicted on every access, and each typed cache holds at most 64 request variants.

`storage::tests::analytics_cache_evicts_expired_keys_even_on_probe_failure_and_caps_variants` exercises the real cache primitive with 80 distinct bucket keys. At most 64 remain. Expiring every entry and deliberately failing the SQL version probe must still release all expired payloads before uncached computation.

## Model Plan Lifetime And Drift

Model preparation retains only compact identities; commit revalidates bytes before replacing source-owned rows.

`model_usage::tests::model_plan_retains_only_identity_and_refuses_commit_time_drift` prepares 16 changed sources and proves the plan owns native identities/counts but no observations or diagnostic payloads. One real commit reparses its source successfully. Mutating that source before another commit fails revalidation, preserves last-good observations, and cannot complete the plan or authorize pruning.

## Search Checkpoint Compatibility

Old Search checkpoints remain readable without newly added hint and path fields.

`sessions::tests::old_search_checkpoint_deserializes_without_hints_or_canonical_path` decodes the prior JSON shape. Missing hints remain absent; a missing canonical path requires one successful source revalidation, never a guessed path or lost documents.

## Checkpoint Batch Persistence

Committed source checkpoints are persisted at their owning batch boundary instead of rewriting the full map per source.

`sessions::checkpoint_tests::checkpoint_replacements_flush_once_and_survive_reopen` commits 32 synthetic sources and measures intermediate checkpoint writes and bytes. No per-source write is allowed. One final flush must preserve every in-memory checkpoint and all searchable documents after reopening the index. Individual Tantivy commits remain unchanged.

## Checkpoint Atomic Replacement

Checkpoint readers observe complete old or new files, never a truncated rewrite.

`sessions::checkpoint_tests::checkpoint_readers_see_complete_replacements` holds an open reader across a checkpoint replacement. That reader must retain the complete original snapshot while reopening the path yields the new checkpoint. This verifies atomic replacement, not directory durability after power loss.

## Legacy Notify Failure Retry

Legacy notify failures retain their queued payload and use capped backoff instead of acknowledging failed checkpoint persistence.

`server::observed_subagent_tests::legacy_notify_failure_keeps_pending_payload_and_newer_generation` checks failed work requests retry, an older completion cannot clear a newer generation, and only a successful matching completion removes the pending entry. Search invalidation is emitted even if checkpoint persistence follows a successful document commit with an error.

## Search Prune Proof

Search pruning must not erase a live source committed after the sweep's inventory was taken.

`sessions::tests::search_prune_does_not_erase_a_live_commit_newer_than_inventory` commits a newly created source against an older empty complete inventory. Its still-existing in-root canonical path prevents pruning. Removing the file then allows the same inventory to prune its documents and checkpoint.

## Pi Header Replacement Prune

An existing Pi path proves a live source still exists only while its supported header retains that native identity.

`sessions::tests::search_prune_distinguishes_a_live_pi_source_from_replaced_header_identity` keeps a matching Pi checkpoint despite an older empty inventory, then replaces the file's supported header ID. The old source is pruned even though the path remains, preventing the live-race guard from retaining stale identity aliases forever.

## Unchanged Sweep Isolation

An unchanged Search inventory must not perform per-source live analytics registry work.

`sessions::tests::unchanged_search_sweep_does_not_refresh_analytics_but_recovery_still_runs` invokes the production sweep helper against an already-current Search source. An impossible generation sentinel and empty analytics registry remain untouched, proving the helper never begins live root reconciliation. Whole-root recovery then ingests the missing analytics, independently of Search freshness.

## Learning Digest Ownership

Learning retains source admission through its existing redaction-before-compression operation, returning only the budgeted digest.

`learning::tests::learning_digest_keeps_fetch_and_compaction_under_source_admission` supplies a large synthetic string with a fake secret, checks admission during fetch and release afterward, and verifies the exact existing redact/compress result under the 48 KiB budget. On a single-thread Tokio runtime, a sibling `join!` future must release the blocked digest worker; synchronous construction or `block_in_place` would stall that sibling and fail the bounded channel wait. No truncation-before-redaction or new summary semantics are introduced.

## Context Ownership And Errors

Context extraction drops raw trees and unrelated source rows under admission, then transfers the explicitly requested wire response to its caller.

`sessions::tests::context_extreme_window_does_not_overflow_and_read_failures_are_errors` exercises a real fixture with `usize::MAX` window, preserving both requested messages without arithmetic overflow. Oversized input and invalid UTF-8 return errors, not empty successful contexts. Response-byte limits and client context-cache policy remain separate concerns: no new context truncation is implied by source admission.

## Dense Overlapping Owning Benchmark

A dense synthetic transcript measures large-arena replication across overlapping real consumers and retained rotating caller threads.

`sessions::tests::dense_pi_transcript_overlapping_owning_paths` is ignored by default. Each generated group contains native user, assistant/tool-call, and tool-result records with 128 small nested objects in both arguments and details. Eight rounds alternate growing and identical versions while three fresh callers per round overlap canonical notify, Search sweep, and whole-root analytics owning paths; the 24 caller threads stay alive until measurement ends to model parked pool arenas. Complete Search, event, tool, and model-row counts are asserted each round. The fixed path must use one decoder thread, at most one full extraction plus a root identity pass per changed version, and zero decodes on identical versions.

From `src-tauri`, run alone with `QUILL_DENSE_BENCH_MIB=32 QUILL_DENSE_BENCH_ROUNDS=8 uv run --locked --project claude-integration/mcp cargo test dense_pi_transcript_overlapping_owning_paths -- --ignored --nocapture --test-threads=1`. Optional `QUILL_DENSE_BENCH_ARTIFACT_DIR` preserves read-only glibc `malloc_info` XML snapshots. RSS, allocator-live estimates, total arenas, and arenas at least 64 MiB are separate measurements; neither flat total arena count nor a universal 2 GiB bound for arbitrary JSON expansion is asserted.

A scratch-HEAD adapter exercises the original three owners against the identical fixture. The complete like-for-like comparison uses 4 MiB, eight rounds, and identical external 4 GiB max/3 GiB high/no-swap limits. The original 8 MiB attempt stalled under those limits and timed out; it is explicitly incomplete and is not a performance comparison. The complete fixed 32 MiB run establishes the larger allocation-dense case. Recipes, adapter patch, binary identities, XML, results, and limitations live in `~/.cache/quill/diagnostics/quill-r93f-implementation-20260914/`.

## Large Owning Path Benchmark

A fresh-process synthetic benchmark measures repeated growing Pi sources through real Search and analytics consumers.

`sessions::tests::repeated_large_pi_transcript_owning_paths` is ignored by default. Run from `src-tauri` with `QUILL_TRANSCRIPT_BENCH_MIB=244 uv run --locked --project claude-integration/mcp cargo test repeated_large_pi_transcript_owning_paths -- --ignored --nocapture --test-threads=1`, alone in a fresh process with the default allocator. The default 32 MiB scale is a faster check. Four growing versions must each commit complete Search/analytics output, unchanged admissions must not decode, and document counts must match. Linux reports RSS/high-water/swap separately from read-only glibc allocated/free/system-byte accounting. No trim, arena cap, production transcript, or application reset is part of this recipe.
