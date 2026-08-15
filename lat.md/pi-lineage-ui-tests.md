---
lat:
  require-code-mention: true
---
# Pi Lineage UI Tests

These tests pin Pi-specific lineage navigation and live linked-session wording.

## Search Parent Navigation

Pi child search results render accessible parent-session navigation using stable header ids rather than transcript paths.

## Pushed Search Parent

Pi notify indexing prefers the extension's pushed parent id and clears transcript-derived parents for pushed root or unresolved proof.

## Immediate Search Input

Search input updates parent query state on each change so controlled text remains visible while backend dispatch stays debounced.

## Live Linked Session Copy

Two live Pi children render as live linked sessions without subagent, native-agent, or total-agent wording.

## Singular Linked Session Copy

One live Pi child uses the singular label `live linked session`.

## Unresolved Lineage Reason

An unresolved Pi parent renders as explicitly unlinked with its pushed reason and never renders root or parent-navigation copy.
