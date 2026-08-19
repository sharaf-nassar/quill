---
lat:
  require-code-mention: true
---
# Widget Usage Tests

These checks protect persisted Usage-graph controls from silently changing the graph's meaning.

## Chart dimension preference

The chart starts in Model mode, rejects invalid saved values, and persists a valid CLI or Provider choice. Provider retains the internal `llm` storage value for preference compatibility.

## Chart group preservation

CLI, Provider, and Model group the same model-evidence buckets without dropping unattributed tokens.

Model further merges one model's evidence across CLIs by normalized id, stripping Pi's `{upstream}/` prefix before grouping, so `cliproxyapi/gpt-5.6-sol` and a directly-routed `gpt-5.6-sol` render as one series.
