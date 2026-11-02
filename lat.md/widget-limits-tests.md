---
lat:
  require-code-mention: true
---
# Widget Limits Tests

Behavior specifications for provider and account state cues in the widget LIMITS band.

## Claude 5-hour cell follows the data

A Claude CPA row whose accounts return only weekly windows has no 5-hour cell; one account returning `five_hour` adds the shared cell, filled on that account and dashed on the others.

## Collapsed all-cooling pool

A collapsed CPA provider labels `COOLING` only when its non-empty account set is entirely cooling; expansion or any ready, disabled, or unavailable account suppresses the pool cue.
