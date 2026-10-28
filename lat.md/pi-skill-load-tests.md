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

## Pi Scoped Migration

Migration 50 preserves existing skill rows and arms only Pi analytics replay. Reopening after the marker is consumed does not rearm it, so recovery uses the existing bounded reconciliation lifecycle.
