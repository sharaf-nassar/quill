---
lat:
  require-code-mention: true
---
# Widget Usage Tests

These checks protect persisted Usage-graph controls from silently changing the graph's meaning.

## Chart dimension preference

The chart starts in Models mode, rejects invalid saved values, and persists a valid CLI or LLM choice.

## Chart group preservation

CLI, LLM, and Models group the same model-evidence buckets without dropping unattributed tokens.
