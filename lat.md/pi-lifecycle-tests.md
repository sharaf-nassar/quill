---
lat:
  require-code-mention: true
---
# Pi Lifecycle Test Specs

These tests pin Pi detection, transactional file ownership, repair verification, and manager wiring.

## Full Shared Config Contract

The shared provider writer creates `config.json` with the main URL, context URL, hostname, and trimmed authentication secret required by every local integration.

## Local Config Drift

Rewriting a local contract heals port, hostname, and secret drift while retaining fields owned by newer or external consumers.

## Remote Config Preservation

A deliberately remote main URL remains byte-for-byte unchanged when a local provider is enabled or repaired.

## Shared Config Contract

A Pi-only install provisions all four shared config fields, and semantic verification makes stale local ports, hostname, or secret trigger repair.

## Shared Config Lifetime

Pi uninstall keeps the shared config while another local provider needs it and removes it when Pi is the last local provider.

## Shared Config Consumer Set

The last-provider decision counts Claude, Codex, and Pi but ignores service-only providers that never consume `config.json`.

## Version Gate

Pi 0.84.0 and newer pass detection, while older, unknown, and malformed versions produce an explicit error status.

## Transactional Round Trip

Install and uninstall preserve existing AGENTS.md bytes and unrelated Pi extensions while removing every Quill-owned lifecycle and extension-log artifact. Spool files remain under retirement sequencing rather than provider uninstall.

## Transactional repair rollback

A repair failure after config, reporter bytes, and database gates change restores exact prior config, extension, instruction/state bytes, and reporter/listener settings through [[src-tauri/src/integrations/pi.rs#restore_reporter_settings]].

## Crash Recovery

The next guarded mutation restores extension and instruction bytes left half-written by an interrupted Pi transaction.

## Semantic Verification

Verification rejects a stale stamp, changed extension payload, missing managed instruction block, unexpected marked extension, or invalid state file.

## Upgrade In Place

Startup repair detects an old deployment stamp and replaces its owned Pi extension with the current production payload without user action.

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

Install enables the setting-gated loopback context listener, uninstall persists it false, and both changes share the recoverable deployment transaction.

## Reload and disable status

Install and repair require a Pi reload until Quill observes the exact reporter generation.

Disable immediately gates every reporter channel and exposes typed disabled remediation without removing npm, project, development, or foreign files.

## Feature-gated Payload

Deployment renders Pi's context preservation, activity tracking, and context telemetry flags into `quill.ts`; changing any flag invalidates the payload stamp until reinstall.

The payload is formatter-owned, so rendering and bundle verification locate the `const FEATURES` declaration by its bounds and always emit the one-line form: a rewrapped declaration still deploys instead of failing closed and stranding the previously installed extension.
