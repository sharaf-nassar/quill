---
lat:
  require-code-mention: true
---
# Frontend Invoke Cache Tests

These tests protect cache identity, lifecycle, and the widget's bounded ingest-refresh cadence.

## Fresh remount skips fan-out

A fresh module entry restores data after unmount without another request; an event received while unmounted marks it stale, preserves its render data, and schedules background revalidation.

## Concurrent subscribers coalesce

Two subscribers for one command and argument scope share exactly one in-flight promise and accepted result.

## Listener setup stays query-silent

Resolving async push-listener registrations causes no cache invalidation; only an emitted event starts the mounted refresh fan-out.

## Ingest storms keep one cadence

Continuous one-second invalidations produce complete mounted fan-outs no more often than every 5,000 ms, without starvation or cancellation when a sibling subscriber leaves.

## Transcript runtime refresh is immediate

A transcript event refreshes mounted session breakdown data immediately, coalesces updates during an in-flight read, and leaves sibling analytics on the shared 5,000 ms cadence.

## Arguments isolate cache entries

Stable serialization coalesces equivalent object key order while distinct ranges remain distinct cache entries.

## Errors retry without poisoning cache

A rejected refresh retains prior data alongside the error, and an immediate explicit retry can replace it with a successful result.

## Strict Mode cleanup releases resources

The setup-cleanup-setup lifecycle shares initial work and releases late async listener registrations plus pending module timers exactly once.
