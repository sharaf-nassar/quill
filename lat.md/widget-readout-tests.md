---
lat:
  require-code-mention: true
---
# Widget Readout Test Specs

These specs protect the readout grid's activity figures from reading a missing measurement as a zero.

## Activity Stats Denominators

[[src-tauri/src/storage.rs#Storage#get_widget_activity_stats]] reports each secondary figure with its own measured denominator and leaves a dimension nobody reported as absent.

A range with tool rows carrying `is_error` evidence, one unmeasured row, and one row outside the window counts every in-range call while the error denominator counts only rows with evidence. Root-chain `user_text` events count as prompts across distinct provider-qualified sessions, and a sidechain prompt stays out because it is the agent's message rather than the operator's. With no turn in range the reasoning total is `None`, not zero; once one turn reports reasoning beside one that does not, the total is that turn's reasoning and the share denominator is that turn's output alone. Every series sums to the total it accompanies.
