# Cached CPA quotas lose their colors

## Cause

Account toggles confirm routing state without fetching new quota observations.
The mutation initially forced the account quota to Cached, discarded the live
usage cache, and rebuilt the response with `load_cached_usage_data`, which marked
all CPA accounts cached. Then `cpaRows` forced pool cells to stale whenever a
provider error existed and account cells to stale whenever quota state was not
live. One toggle therefore grayed unrelated meters.

## Fix

Keep `numericCell` as the severity owner for direct, CPA pool, and account rows.
Colors follow utilization and actual reset expiry, not cache provenance or source
errors. Missing values and elapsed windows remain neutral. Preserve cached/error
metadata and report it through the existing sync control rather than inventing a
live read to recover the colors.

The mutation must also preserve quota metadata in storage and patch the current
endpoint-matched usage snapshot rather than replacing it with a cache-only read.
Recompute pool membership, but retain existing source errors and the cache's
refresh timestamp. Return routing changes directly to the widget and emit only
the indicator update, so toggles do not restart the quota sync clock. A cold
cache still uses the persisted fallback; a failed confirmation invalidates the
process cache without fabricating an observation.

## Verification

The existing widget quota regression checks utilization severity for cached
account and pool cells while still rejecting a live sync label for source errors.
