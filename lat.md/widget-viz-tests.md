---
lat:
  require-code-mention: true
---
# Widget Viz Tests

These tests protect direct interaction with the widget's dependency-free SVG charts.

## Pointer Scrub Bucket Mapping

Horizontal pointer positions select every equal-width chart bucket. The measured floating legend follows both pointer axes, flips horizontally, and clamps vertically inside the plot; invalid geometry returns no anchor.
