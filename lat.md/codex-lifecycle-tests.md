---
lat:
  require-code-mention: true
---
# Codex Lifecycle Test Specs

These specs protect Codex integration mutations from model-provider authentication while preserving user configuration and hook trust behavior.

## Hook Discovery Auth Isolation

Hook discovery uses a cliproxyapi-shaped fixture with no ChatGPT login and must return without provider authentication or any config-file mutation.
