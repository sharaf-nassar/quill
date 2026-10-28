---
lat:
  require-code-mention: true
---
# Development Handoff Tests

Isolated fixtures verify development handoff without quitting, relaunching or manipulating the user's live Quill.

## Lifecycle Ordering

Production remains available during web asset preparation. Graceful quit and port release precede serving dev; frontend shutdown precedes production restoration.

## Failure Recovery

Build failure leaves an existing production process untouched. Partial quit, occupied ports and frontend failure retain restoration responsibility. An early CLI exit never initiates a handoff, and an initially absent production app can be restored.

## Exclusive Ownership

A live supervisor owns one machine-local lease across checkouts. Dead owners can be recovered without stealing a live lease, and process identity includes start time to reject a reused PID.

## Port Release Barrier

An occupied listener prevents restoration until the port is free. A timeout reports the blocked handoff instead of starting a competing production process.

## Native Quit And Clean Relaunch

Linux resolves an enabled native Quit action, not a process-name kill. Production launch drops dev identity, data, port and loader overrides while preserving desktop-session environment.

## Mac Production Identity

macOS discovery restricts process enumeration to the current user, checks bundle identity, ignores unbundled dev binaries and preserves application paths containing spaces. Command fixtures cover discovery without sending Apple events.

## Detached Supervisor Lifetime

Real subprocess fixtures prove the supervisor outlives forced hook cleanup and does not restore production while the CLI remains alive. Normal exit and SIGINT both trigger restoration after the CLI exits.
