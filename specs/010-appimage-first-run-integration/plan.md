# Implementation Plan: AppImage First-Run Self-Integration

**Branch**: `010-appimage-first-run-integration` | **Date**: 2026-06-03 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `specs/010-appimage-first-run-integration/spec.md`

## Summary

Give the Linux AppImage first-run self-integration. At startup, the backend
detects an un-integrated AppImage and shows a one-time native prompt to add Quill
to the applications menu; on accept it copies the AppImage to `~/Applications`,
writes a `~/.local/share/applications` launcher entry, installs an icon, and
records the decision. The same integration routine is exposed as a manual control
in the General settings tab (shown only for AppImage runtimes). The updater is
unchanged: because the menu launches the integrated copy, future updates land on
that copy in place. Everything runs in user space (no privilege escalation), and
the logic is entirely inert on dev / non-AppImage builds.

## Technical Context

**Language/Version**: Rust (toolchain 1.95.0) for the Tauri backend; TypeScript + React for the frontend.
**Primary Dependencies**: Tauri 2.10.3; `tauri-plugin-dialog` (native prompt, already used by `check_for_update`); `tauri-plugin-updater` (unchanged). No new crates anticipated; `.desktop`/icon writing uses `std::fs`.
**Storage**: SQLite settings table via `storage.get_setting` / `set_setting` (key `appimage.integration`).
**Testing**: `cargo test` — pure-logic unit tests (eligibility, path computation, `.desktop` text) plus a temp-dir side-effect harness with injected `$HOME`/`$APPIMAGE`; manual smoke on GNOME + KDE.
**Target Platform**: Linux (AppImage). Inert on macOS, Windows, and `tauri dev`.
**Project Type**: desktop-app (Tauri: Rust backend in `src-tauri/`, React frontend in `src/`).
**Performance Goals**: detection runs async from `.setup()` and never blocks GTK/webview startup; the one-time copy (tens of MB) happens only on user accept.
**Constraints**: no startup blocking; idempotent; fail-safe (non-fatal on error, retryable); strictly user-space paths (`~/Applications`, `~/.local/share/...`); no `pkexec`/`sudo`.
**Scale/Scope**: one new Rust module, two IPC commands, one `.setup()` hook, one settings row, one `lat.md` section.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` is the unpopulated template — no project-specific
principles or gates are defined, so there is nothing to enforce or violate. The
design nonetheless follows the practices already established in this codebase:

- **Isolation**: one new module (`appimage_integration.rs`) with a single purpose; `lib.rs` only gains a small spawn in `.setup()` and two command registrations.
- **Testability**: side-effecting work is separated from pure logic (eligibility, path/`.desktop` generation) so the core is unit-testable with injected paths.
- **No startup blocking**: detection/prompt is spawned async, mirroring the tray `check_for_update` path.
- **No new dependencies / no privilege escalation.**

**Result**: PASS (no gates defined; no complexity deviations). Re-checked post-Phase 1: still PASS.

## Project Structure

### Documentation (this feature)

```text
specs/010-appimage-first-run-integration/
├── plan.md              # This file
├── research.md          # Phase 0 — decisions & rationale (R-A..R-I)
├── data-model.md        # Phase 1 — state + entities
├── quickstart.md        # Phase 1 — maintainer verification walkthrough
├── contracts/
│   └── ipc-commands.md  # Phase 1 — get_appimage_integration_status + integrate_appimage
└── checklists/
    └── requirements.md  # /speckit-specify quality checklist
```

### Source Code (repository root)

```text
src-tauri/src/
├── appimage_integration.rs   # NEW: detection ($APPIMAGE), integration routine,
│                             #      .desktop + icon generation, state read/write
├── lib.rs                    # MOD: .setup() spawns first-run detect+prompt (async);
│                             #      register get_appimage_integration_status + integrate_appimage
└── storage.rs                # REUSE: get_setting/set_setting for `appimage.integration`

src/
├── components/settings/GeneralTab.tsx   # MOD: conditional "Install to applications menu" SettingRow
└── hooks/useAppImageIntegration.ts      # NEW (small): query status + invoke integrate, with toasts

lat.md/
└── features.md                          # MOD: document first-run integration + Settings control
```

**Structure Decision**: Single Tauri desktop-app repo. The feature is
backend-owned (detection, filesystem side effects, persisted state) with a thin,
conditionally-rendered frontend control — the same split already used by
`install_app_update` (Rust owns the action) and the updater UI (frontend renders
state). No new top-level structure is introduced.

## Complexity Tracking

> No constitution gate violations and no added dependencies — nothing to justify.

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|--------------------------------------|
| (none)    | —          | —                                    |
