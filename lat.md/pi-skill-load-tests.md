---
lat:
  require-code-mention: true
---
# Pi Skill Load Tests

Pi skill analytics count persisted load evidence without treating restored instructions as fresh activity.

## Confirmed Tool Results

Successful `skill_context` results use resolved skill metadata, deduplicate repeated name/path pairs, and reject failed or unconfirmed results. Missing metadata permits leading skill-block fallback; malformed or empty metadata does not.

## Explicit Invocation Boundaries

Native user skill envelopes and exact `proper-base-skill-context` load records count. Quotes, malformed envelopes, restoration prefixes, assistant text, summaries, unknown custom types, and list/remove/clear controls do not.

## Evidence Before Truncation

The persisted parser extracts every skill before output and details caps apply. Multi-skill bodies and oversized metadata survive as compact load evidence without creating searchable messages or extra runtime events.

## Owned Replay And Analytics Neutrality

Retained snapshots attribute loads to their source, host and project.

Repeated event IDs and repeated snapshot replacement do not inflate counts; separate explicit invocations count separately. Replacement removes vanished evidence without synthetic tool activity.

## Bounded Identity Discovery

Pi inventory reads the native header without decoding the body. Malformed tracking evidence still fails full parsing before a snapshot can commit, so faster inventory never weakens validation.

## Recent First Resumable Replay

Recovery commits recently modified sources before old history. A transient failure retains last-good rows and completed-source fingerprints; a later pass retries only unfinished work rather than restarting the corpus.

## Snapshot Writer Admission

A competing SQLite writer makes snapshot replacement wait before reading generation guards. It then commits successfully rather than failing a deferred read-to-write upgrade and leaving current analytics stale.

## Pi Scoped Migration

Migration 51 converts an unfinished Pi marker into per-source fingerprint invalidation without clearing skill rows. Suppressed/live sources and other providers remain untouched. A completed version-50 replay does not rearm.
