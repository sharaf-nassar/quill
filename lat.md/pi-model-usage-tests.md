---
lat:
  require-code-mention: true
---
# Pi Model Usage Test Specs

Pi usage tests pin pushed-message storage and its safe coexistence with retained transcript evidence.

## All Branch Usage

Every Pi assistant message contributes direct token dimensions to aggregate totals, with its upstream provider and model retained as one provider-qualified model id.

## Version And Diagnostic Tolerance

Pi v2 and v3 sessions parse through the shared tolerant parser, while malformed records and invalid model or token fields produce bounded diagnostics instead of aborting later usage.

## Native Session Identity

Pi model-source identity comes from the session header id and cwd, independent of filenames and tree entry content.

## Legacy All-Branch Totals

Replacing one legacy Pi source twice stays idempotent, and every session/model read reports all stored branches without an active-branch scope label.

## Pushed Usage Migration

Opening a schema-42 database adds nullable event identity and five native cost fields, preserves existing observations, creates the Pi-only dedupe index, and records schema 43 once.

## Replay And Cost Storage

Live delivery and replay of one Pi usage event retain one observation while preserving every token and native per-field plus total cost value.

## Tracking Replay And Live Totals

Replaying one accepted usage envelope leaves one Models contribution and one cumulative LiveTracker token increment.

## Upgrade Coexistence

A resumed session with pushed rows excludes its legacy adapter rows from raw and completed-rollup Models reads, preventing double count across an upgrade.

## Push Source Prune Exemption

Retention never selects observations owned by a synthetic `pi-push:<session-id>` source, even when their timestamps predate the cutoff.
