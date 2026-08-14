---
lat:
  require-code-mention: true
---
# Pi Lifecycle Test Specs

These tests pin Pi detection, transactional file ownership, repair verification, and manager wiring.

## Version Gate

Pi 0.84.0 and newer pass detection, while older, unknown, and malformed versions produce an explicit error status.

## Transactional Round Trip

Install and uninstall preserve existing AGENTS.md bytes and unrelated Pi extensions while removing every Quill-owned lifecycle artifact.

## Crash Recovery

The next guarded mutation restores extension and instruction bytes left half-written by an interrupted Pi transaction.

## Semantic Verification

Verification rejects a stale stamp, changed extension payload, missing managed instruction block, unexpected marked extension, or invalid state file.

## Owned File Boundaries

Repair removes only structurally marked Quill extensions and refuses to overwrite a user-owned `quill.ts` file.

## Writable Extension Directory

Detection reports a typed error when Pi's extension directory cannot accept managed files.

## Packaged Assets

Tauri's resource manifest includes the Pi bundle and its required extension source exists at build time.

## Manager Wiring

Enabled Pi installs participate in startup repair and feature-triggered lifecycle synchronization.

## Typed Detection Errors

Saved enablement cannot hide a fresh version, path, or writability error reported by Pi detection.

## Context HTTP Setting

Install enables the setting-gated loopback context listener, uninstall clears it, and both changes share the recoverable deployment transaction.

## Feature-gated Payload

Deployment renders only the two Pi feature flags into `quill.ts`, and either flag changing invalidates the payload stamp until reinstall.
