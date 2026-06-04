# Feature Specification: AppImage First-Run Self-Integration

**Feature Branch**: `010-appimage-first-run-integration`
**Created**: 2026-06-03
**Status**: Draft
**Input**: User description: "new users installing the AppImage have a terrible first-run UX (download, chmod, manually place). Is there a way for us to auto-install the AppImage for the user instead?"

## Overview

Linux now ships only the AppImage (the `.deb` was dropped because the in-app
updater can only self-update AppImages, documented in
`lat.md/infrastructure.md`). An AppImage has no system integration by default: a
new user downloads a loose file and double-clicks it with no menu entry, icon, or
launcher presence — a poor first impression next to the old `.deb`.

This feature lets Quill **integrate itself on first run**: when it detects it is
running as an un-integrated AppImage, it offers to add itself to the user's
applications menu (place the file in the user's applications location, create a
launcher entry, install an icon). After that it behaves like an installed app and
the existing updater keeps it current in place. A manual control in Settings
covers users who decline or want to re-run integration.

This recovers the *post-launch* experience (menu entry, icon, updates). The
*pre-launch* step — a freshly downloaded file lacks an execute bit, so the user
must allow executing it and double-click once — cannot be fixed by an app that is
not yet running and is out of scope (see Assumptions).

## Clarifications

### Session 2026-06-03

- Q: How is integration triggered on first run? → A: A one-time prompt via a native confirmation dialog; the choice is remembered so it is asked at most once.
- Q: Is the AppImage moved or copied into the applications location? → A: Copied — this keeps the running session valid (no forced restart) and is robust across filesystems; the user is told they may delete the original download.
- Q: Where does the manual control live? → A: As a plain row in the General settings tab near "Always on top" (there is no Appearance tab or Window section today; creating one was declined).
- Q: What happens on non-AppImage builds (dev / future packages)? → A: No prompt, no Settings control; the integration logic is inert.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - First-run integration prompt (Priority: P1)

A new user downloads the Quill AppImage, allows it to execute, and launches it.
On first run Quill asks whether to add itself to the applications menu. Accepting
places Quill in the menu with an icon and a confirmation message; declining
dismisses the prompt for good. From then on the user launches Quill from their
normal applications menu and new versions update automatically.

**Why this priority**: This is the core of the feature — it converts the raw
AppImage into a first-class installed app and is what makes the AppImage-only
Linux story acceptable for new users.

**Independent Test**: Launch a freshly downloaded AppImage; confirm the prompt
appears, accepting produces a working menu entry + icon, and the prompt does not
return on subsequent launches.

**Acceptance Scenarios**:

1. **Given** a freshly downloaded, executable AppImage launched for the first time, **When** the user accepts the integration prompt, **Then** Quill is placed in the applications location, a launcher entry and icon appear in the desktop menu, and a confirmation message notes the original download can be deleted.
2. **Given** the same first launch, **When** the user declines, **Then** no files are changed and the prompt never appears again on later launches.
3. **Given** an already-integrated install, **When** Quill launches, **Then** no prompt is shown.

### User Story 2 - Manual install / re-install from Settings (Priority: P2)

A user who declined the prompt (or whose menu entry broke) opens Settings and
clicks a button to add Quill to the applications menu, achieving the same result
as the prompt.

**Why this priority**: Recovery path for the "Not now" choice and for repairs;
valuable but secondary to the first-run flow that reaches most users.

**Independent Test**: With integration not yet done, open Settings, click the
control, and confirm the menu entry appears and the control reflects the new
"installed" state.

**Acceptance Scenarios**:

1. **Given** a non-integrated AppImage, **When** the user clicks the Settings install control, **Then** integration runs identically to the prompt and the control then shows an "installed" state.
2. **Given** an already-integrated install, **When** the user opens Settings, **Then** the control shows an "installed" (disabled) state.

### User Story 3 - Non-AppImage builds are unaffected (Priority: P1)

A user running a development build or any non-AppImage binary sees no prompt and
no Settings control; integration behavior is entirely inert.

**Why this priority**: Prevents the feature from misbehaving or showing
irrelevant UI outside the AppImage context; protects existing workflows.

**Independent Test**: Run a non-AppImage build; confirm no prompt fires and the
Settings control is absent.

**Acceptance Scenarios**:

1. **Given** a non-AppImage runtime, **When** Quill starts, **Then** no integration prompt appears.
2. **Given** a non-AppImage runtime, **When** the user opens Settings, **Then** the install control is not shown.

### Edge Cases

- A user who previously declined is never re-prompted, yet the Settings control still works.
- Re-running integration when a previous menu entry/icon exists overwrites them cleanly (idempotent), with no duplicate entries.
- If integration fails (read-only location, no disk space), the user sees a clear non-fatal message and the install is left un-recorded so it can be retried later.
- On desktop environments without a desktop-database refresh tool, the menu entry still appears (the catalog refreshes on its own schedule).
- Pre-launch: a download lacking an execute bit cannot be auto-run; this is outside the feature's reach (see Assumptions).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST detect whether it is running as an AppImage and gate all integration behavior on that detection.
- **FR-002**: On first run as an un-integrated AppImage, the system MUST offer integration through a one-time native confirmation prompt, shown without blocking application startup.
- **FR-003**: On acceptance, the system MUST place the AppImage in the user's applications location, register a desktop launcher entry pointing at it, install an application icon, and best-effort refresh the desktop entry catalog.
- **FR-004**: The system MUST persist the user's decision (integrated or declined) and MUST NOT prompt again once a decision is recorded.
- **FR-005**: The system MUST provide a manual control in the General settings tab to run or repair integration; this control and the first-run prompt MUST perform the identical integration action, the control MUST reflect current state, and it MUST be shown only when running as an AppImage.
- **FR-006**: Integration MUST be idempotent — repeating it overwrites the placed file, launcher entry, and icon without creating duplicates.
- **FR-007**: Integration MUST copy (not relocate) the AppImage so the running session remains valid and no restart is forced, and MUST inform the user that the original download can be deleted.
- **FR-008**: After integration, automatic updates MUST apply to the integrated copy (the one launched from the menu).
- **FR-009**: On failure, the system MUST surface a clear, non-fatal message and leave the integration decision unrecorded so it can be retried.
- **FR-010**: On non-AppImage builds, the system MUST show no prompt and no Settings control and MUST otherwise behave exactly as before.

### Key Entities *(include if feature involves data)*

- **Integration decision**: a persisted record of whether the user integrated or declined, plus the path of the integrated AppImage.
- **Launcher entry**: the desktop menu record (name, command, icon) that points at the integrated AppImage.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A new user can add Quill to their applications menu from a freshly launched AppImage in a single confirmation, with zero terminal commands.
- **SC-002**: After integration, Quill appears in the applications menu with its icon on both GNOME and KDE.
- **SC-003**: A user who declines is never prompted again, yet can still integrate later from Settings in one click.
- **SC-004**: After integration, the next released version installs automatically without the user re-downloading anything.
- **SC-005**: On non-AppImage builds, no integration UI appears and the app runs unchanged.

## Assumptions

- The AppImage is the sole Linux distribution (the `.deb` was dropped); this feature targets Linux only — macOS and Windows already integrate at install time.
- The applications location is the user's home `Applications` directory, matching the existing README install convention.
- The running AppImage exposes its bundled icon for extraction into the desktop icon location.
- Pre-launch friction (marking the downloaded file executable and the first double-click) is out of scope — an application that is not yet running cannot remove it; only external mechanisms (a desktop AppImage integrator, or a system package) can.
- Re-adding the `.deb`, or adding Flatpak/Snap, is out of scope — evaluated and rejected (Flatpak's sandbox conflicts with Quill's host integration; the `.deb` cannot self-update).
