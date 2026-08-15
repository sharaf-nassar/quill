---
lat:
  require-code-mention: true
---
# Integration Action Error Tests

These tests keep provider mutation failures local to one Settings row without weakening the initial-load states.

## Provider-local enable timeout

A rejected Codex enable keeps every provider control visible, reports the timeout only on Codex, and offers a keyboard-accessible retry that clears only Codex's action error after success.

## Successful retry isolation

A successful Codex retry removes its action error without clearing an error retained for another provider.

## Initial request states

Initial loading and genuine initial-load failures continue to replace the provider list with the established Settings feedback states.
