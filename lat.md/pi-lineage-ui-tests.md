---
lat:
  require-code-mention: true
---
# Pi Lineage UI Tests

These tests pin Pi-specific lineage navigation and live linked-session wording.

## Search Parent Navigation

Pi child search results render accessible parent-session navigation using stable header ids rather than transcript paths.

## Immediate Search Input

Search input updates parent query state on each change so controlled text remains visible while backend dispatch stays debounced.

## Live Linked Session Copy

Two live Pi children render as live linked sessions without subagent, native-agent, or total-agent wording.

## Singular Linked Session Copy

One live Pi child uses the singular label `live linked session`.
