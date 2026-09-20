---
lat:
  require-code-mention: true
---
# Pipeline Recovery Test Specs

Regression coverage for `quill-us5x.2` uses synthetic transcript roots and isolated SQLite databases only.

## Inventory-To-Prune Serialization

Whole-root transcript recovery holds every provider/root permit from filesystem inventory through absence pruning.

A source created after that locked inventory may wait for the pass and commit afterward; the stale inventory can never prune a completed live commit.

`transcript_analytics::tests::live_commit_after_locked_inventory_cannot_be_pruned_by_that_inventory` pauses recovery after an empty locked inventory, creates and starts live reconciliation for a valid source, proves the live commit waits, then releases recovery and requires the source registry row to survive. `transcript_analytics::tests::stale_explicit_inventory_revalidates_a_completed_live_commit` pins compatibility for explicit-inventory callers by committing a source between their walk and permit acquisition, then requiring last-moment source validation to preserve it.

## Per-Source Model Outcomes

Live model reconciliation carries each source's durable outcome back to the source-keyed queue.

Healthy siblings settle independently, transient failures alone consume retry attempts, and stable versioned content rejections settle until their rejection fingerprint changes.

`model_usage::tests::transient_model_read_failure_remains_retryable` distinguishes an unversioned read failure from the versioned rejection assertions in `model_usage::tests::empty_codex_rejection_survives_reopen_and_recovers_after_append`. `tests::model_batch_outcomes_settle_healthy_and_permanent_siblings_only` applies a mixed batch to the real coordinator and requires only the transient source to remain queued.

## Durable Model Recovery

Periodic retained recovery selects bounded Claude/Codex model work from complete inventory and durable registry state, independently of mtime advancement.

Never-registered, pending, stale, and failed sources without a current rejection are eligible. Successful or suppressed sources whose current stat differs are also eligible, covering backwards clocks. Current version rejections are not. Missing registry rows sort before retries, and retries sort by oldest attempt, so imported sources and exhausted unchanged failures are reconsidered without one source monopolizing a pass.

`transcript_watcher::tests::durable_model_recovery_ignores_mtime_and_skips_versioned_rejections` pins never-seen admission, backwards-mtime rearming, unchanged failed-source recovery, and permanent rejection settlement. `tests::exhausted_model_retry_rearms_without_a_source_append` exhausts the six-attempt model budget and requires admission of the identical source revision to create runnable work again.

## Watcher Initialization Recovery

Watcher construction or channel failure requests retained recovery immediately and retries watcher initialization after the bounded interval.

The live-fold fallback remains active, and the watcher no longer logs that periodic recovery survives after dropping its scheduler.

`transcript_watcher::tests::watcher_initialization_failure_requests_recovery_before_retry` injects one initialization failure and requires recovery to run before the successful retry.
