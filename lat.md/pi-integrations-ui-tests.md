---
lat:
  require-code-mention: true
---
# Pi Integrations UI Tests

These tests pin Pi setup controls and analytics presentation without adding Pi to subscription limits.

## Pi card states

The Integrations provider row distinguishes a detected Pi CLI, an enabled integration, and a setup error so operators can act on each state.

A detected-but-disabled provider reports itself as off or unconfigured instead of claiming Quill assets are missing; that language appears only when setup state confirms assets are actually absent.

## Extension health state machine

Pi extension reports transition from never connected to alive, idle, and stale using bounded last-report ages.

## Typed extension error detail

Integration detail names every typed reporter-health failure.

It covers exact mismatch, unknown session, configured-child gap, recovering source, reconciliation failure, rejected telemetry, and saturation while keeping affected counts, remediation, required generation, and verified recovery compact.

## Extension health presentation

The enabled Pi row renders compact health detail without red severity.

It includes versions, last report, typed failure, and intentional no-session absence. Sessions reuse their existing lineage datum to distinguish persisted-source recovery and configured-child gaps instead of adding a row.

## Missing health fallback

An enabled Pi integration with an older or incomplete status payload renders extension status unavailable in slate rather than disappearing or borrowing red severity.

## Executable extension consent

The Pi enable confirmation names `quill.ts`, its default and configured-directory locations, and Quill's stamped-file repair and self-update behavior.

## Provider breakdown counts

Skill and hook rows render Claude, Codex, and Pi sub-counts from their provider-specific payload fields.

## Excluded settings copy

The Brevity profile settings copy explicitly states that Pi and MiniMax do not receive the managed brevity block.

## Limits omission

The subscription Limits band drops Pi entirely instead of rendering a row, unavailable state, or N/A explanation.
