---
lat:
  require-code-mention: true
---
# Indicator Tests

Backend indicator tests protect provider choice and quota-window selection across direct and CPA usage sources.

## Claude CPA Pool Precedence

Claude's title and reset metadata use its aggregate CPA pool instead of a conflicting account-qualified bucket.

## Codex CPA-Only Resolution

Codex resolves title percentages and reset metadata from canonical CPA pool windows without native buckets.

## Configured CPA Provider Availability

A configured provider resolves from its CPA pool even when its native status is missing or disabled.

## Automatic CPA Provider Selection

Auto selects an available CPA pool when no enabled native provider has indicator metrics.

## Direct Native Fallback

Direct Claude buckets remain the indicator source only when no matching CPA pool exists and native status is enabled.
