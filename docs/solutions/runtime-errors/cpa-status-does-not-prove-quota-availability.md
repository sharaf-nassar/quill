# CPA status does not prove quota availability

## Symptom

A Codex pool showed 61% while its disclosed accounts showed 21%, unavailable,
and 100%. The exhausted account also supplied the pool's earlier reset.

## Cause

`src-tauri/src/cpa/aggregate.rs` checked CPA lifecycle status and credential
flags, but not returned quota windows. An account can still report `active`
or `ready` when its quota is exhausted. The no-healthy-account fallback also
averaged exhausted, quota-readable snapshots.

## Fix

Exclude disabled, unavailable, and quota-exhausted snapshots before counting
healthy contributors or selecting the fallback. An account-wide window at or
above 100% excludes the entire account from every aggregate window and reset.
For Claude, only `five_hour` and `seven_day` are account-wide. Exhausted Fable,
Sonnet, Opus, or surface-scoped quotas must not suppress general Claude totals.
Codex's returned rate-limit windows are account-wide. Keep all accounts in the
inventory total and individual disclosure rows.

The initial fix treated every quota window as account-wide. This incorrectly
blanked Claude totals when both accounts had exhausted Fable but still had
general quota. Classify quota scope before deciding account eligibility.

An entirely exhausted pool has zero healthy accounts and no numeric aggregate.
Missing windows remain gaps, not zero utilization. Live and cached usage both
call `compute_cpa_pools`, so the filter belongs there rather than in rendering.

## Verification

Regression coverage in `src-tauri/src/cpa/aggregate.rs` checks the 21% result,
cross-window exclusion, reset provenance, lifecycle variants, both providers,
entirely exhausted pools, and Claude totals with exhausted scoped quotas.
