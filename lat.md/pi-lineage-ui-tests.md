---
lat:
  require-code-mention: true
---
# Pi Lineage UI Tests

These tests pin Pi-specific lineage navigation and live linked-session wording.

## Search Parent Navigation

Pi child search results render accessible parent-session navigation using stable header ids rather than transcript paths.

## Pushed Search Parent

Pi notify indexing uses the extension's pushed parent id for generic links and explicit agents, while root or unresolved proof clears transcript-derived parents.

## Immediate Search Input

Search input updates parent query state on each change so controlled text remains visible while backend dispatch stays debounced.

## Live Linked Session Copy

Two generic live Pi children render as linked sessions without agent wording; explicitly marked Pi subagents use the agent rail instead.

## Linked Session Model Label

A live linked child's model id renders through the same short family label as the agent rail immediately above it, keeping the raw id in the chip's accessible label. A child with no known model still falls back to its truncated session id.

## Singular Linked Session Copy

One live Pi child uses the singular label `live linked session`.

## Unresolved Lineage Reason

An unresolved Pi parent renders as explicitly unlinked with its pushed reason and never renders root or parent-navigation copy.

Recovering durable rows use the same truthful unlinked treatment but do not claim live activity until same-process proof arrives.

## Agent Role Identity

A validated Pi launcher role is carried as the observed agent type. The compact rail retains model-family ordering while its instant tooltip and accessible label name both role and raw model when both exist.
