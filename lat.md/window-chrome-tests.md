---
lat:
  require-code-mention: true
---
# Window Chrome Test Specs

These tests lock Quill's cross-platform resize policy and macOS configuration merge contract.

## Platform policy

macOS uses native overlay chrome while Linux and Windows retain decorationless windows and HTML resize handles.

## macOS main-window override

The macOS platform config repeats every base main-window field and enables overlay chrome and hidden title text, while the merged base config supplies private transparency support.
