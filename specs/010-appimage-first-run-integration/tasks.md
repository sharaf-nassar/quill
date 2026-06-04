# Tasks: AppImage First-Run Self-Integration

**Input**: Design documents from `specs/010-appimage-first-run-integration/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/ipc-commands.md

**Tests**: Included — the spec's Testing section explicitly requests unit + side-effect tests, and the repo's CI gate runs `cargo clippy --all-targets -- -D warnings` and `cargo test`.

**Organization**: Tasks are grouped by user story (US1–US3 from spec.md) so each story is independently implementable and testable.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task).
- **[Story]**: US1 / US2 / US3 (Setup, Foundational, and Polish tasks have no story label).
- Exact file paths are included in each description.

## Path Conventions

Tauri desktop app: Rust backend in `src-tauri/src/`, React frontend in `src/`, docs in `lat.md/`.

---

## Phase 1: Setup

**Purpose**: Create the feature module and wire it into the build.

- [X] T001 Create `src-tauri/src/appimage_integration.rs` (empty module with doc comment) and register `mod appimage_integration;` in `src-tauri/src/lib.rs`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Detection, persisted state, and the shared integration routine — required by every user story. All in `src-tauri/src/appimage_integration.rs` (single file → sequential).

- [X] T002 Implement `running_as_appimage() -> bool` (reads the `APPIMAGE` env var) and the pure eligibility helper `should_prompt(state, is_appimage) -> bool` in `src-tauri/src/appimage_integration.rs`.
- [X] T003 Implement integration-state read/write helpers for settings keys `appimage.integration` (`done` | `declined`) and `appimage.integration_path`, via `storage.get_setting`/`set_setting`, in `src-tauri/src/appimage_integration.rs`.
- [X] T004 Implement pure target-path computation (`~/Applications/Quill.AppImage`, `~/.local/share/applications/quill.desktop`, `~/.local/share/icons/hicolor/256x256/apps/quill.png`) and `.desktop` content generation (`fn desktop_entry(exec_path, icon_name) -> String`) in `src-tauri/src/appimage_integration.rs`.
- [X] T005 Implement the shared `integrate(app) -> Result<(), String>` routine (ensure `~/Applications`; copy `$APPIMAGE` → target and mark executable; write the `.desktop`; extract+install the icon from the running AppImage; best-effort `update-desktop-database`/`gtk-update-icon-cache`; persist `done` + path; idempotent overwrite; non-fatal errors leave state unset) in `src-tauri/src/appimage_integration.rs` (depends on T002–T004).
- [X] T006 Add `#[cfg(test)]` unit tests (detection, `should_prompt`, path computation, `.desktop` text) and a temp-dir side-effect harness (with injected `$HOME`/`$APPIMAGE`: `integrate` writes the copy/`.desktop`/icon; idempotent re-run produces no duplicates; a forced failure leaves state unset) in `src-tauri/src/appimage_integration.rs` (depends on T005).

**Checkpoint**: detection, state, and the integration routine exist and are tested — user stories can now build on this.

---

## Phase 3: User Story 1 — First-run integration prompt (Priority: P1) 🎯 MVP

**Goal**: A new user is offered one-time integration on first AppImage launch.
**Independent test**: Launch a freshly downloaded AppImage → the prompt appears → Add produces a working menu entry + icon → the prompt never returns.

- [X] T007 [US1] In the `.setup()` closure of `src-tauri/src/lib.rs`, spawn an async task that, when `should_prompt(...)` is true, shows the `tauri-plugin-dialog` confirmation ("Add Quill to your applications menu?"); on **Add** → call `integrate(app)` and toast success (noting the original download can be deleted); on **Not now** → persist `declined`. Spawn async so startup is never blocked (depends on T005).

**Checkpoint**: US1 is fully functional and independently shippable — this is the MVP.

---

## Phase 4: User Story 2 — Manual install / re-install from Settings (Priority: P2)

**Goal**: Users who declined (or need a repair) can integrate from Settings.
**Independent test**: With state unset/declined, open Settings → click the install control → integration runs → the control shows "Installed ✓".

- [X] T008 [US2] Implement the `get_appimage_integration_status() -> { is_appimage, integrated }` and `integrate_appimage() -> Result<(), String>` Tauri commands (the latter delegating to the shared `integrate()`), and register both in the `invoke_handler` in `src-tauri/src/lib.rs` (depends on T005).
- [X] T009 [P] [US2] Create `src/hooks/useAppImageIntegration.ts` — query status on mount via `get_appimage_integration_status`, expose an `integrate()` that invokes `integrate_appimage` with success/error toasts and refreshes status afterward (depends on the T008 contract).
- [X] T010 [US2] Add a conditional "Install to applications menu" `SettingRow` near "Always on top" in `src/components/settings/GeneralTab.tsx` — rendered only when `is_appimage`; an active button when not integrated, a disabled "Installed ✓" when integrated; wired to `useAppImageIntegration` (depends on T009).
- [X] T011 [US2] Add tests for command status reporting and idempotent re-integration (a second `integrate_appimage` leaves a single `.desktop`/menu entry) in `src-tauri/src/appimage_integration.rs` (depends on T008).

**Checkpoint**: US2 layered on top of US1; both work independently.

---

## Phase 5: User Story 3 — Non-AppImage builds are inert (Priority: P1)

**Goal**: No prompt, no Settings control, and no behavior change off AppImage.
**Independent test**: Run a dev build → no prompt fires, the Settings row is absent, and status reports `is_appimage: false`.

- [X] T012 [US3] Add a test asserting that with `APPIMAGE` unset `running_as_appimage()` is `false`, `should_prompt(...)` is `false`, and `get_appimage_integration_status` returns `{ is_appimage: false, integrated: false }`; and confirm `src/components/settings/GeneralTab.tsx` does not render the row when `is_appimage` is false (depends on T008, T010).

**Checkpoint**: gating verified end to end.

---

## Phase 6: Polish & Cross-Cutting Concerns

- [X] T013 [P] Document the feature in `lat.md/features.md` (detection, first-run prompt, copy-to-`~/Applications`, launcher/icon, settings flag, the Settings control, updater interaction, and the pre-launch caveat) and run `lat check`.
- [X] T014 [P] Update the `README.md` Linux section to note that the AppImage offers to add itself to the applications menu on first run (no manual menu setup needed).
- [X] T015 Run the backend gate from `src-tauri/`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`; fix any findings (depends on T001–T012).
- [ ] T016 Manual smoke per `quickstart.md` on GNOME + KDE: first-run accept, decline→Settings, idempotent re-run, and non-AppImage inert (depends on a built AppImage).

---

## Dependencies

- **Setup → Foundational → Stories**: T001 → T002–T006 → user-story phases.
- **US1 (T007)** depends only on the foundational routine (T005). It is the MVP and ships alone.
- **US2 (T008–T011)** depends on T005; runtime-independent of US1 (shares the `integrate()` routine). T009 → T010; T011 → T008.
- **US3 (T012)** depends on T008 + T010 (asserts the gating already built in).
- **Polish (T013–T016)** after implementation; T015 gates the whole change.

## Parallel Execution Examples

- T013 (`lat.md/features.md`) and T014 (`README.md`) edit different files with no dependency → run in parallel.
- T009 (`src/hooks/useAppImageIntegration.ts`) can be drafted in parallel with T008 (Rust commands) against the documented IPC contract, since they live in different files.
- Within `src-tauri/src/appimage_integration.rs`, foundational tasks (T002–T006) edit one file → keep sequential.

## Implementation Strategy

- **MVP = Phase 1 + Phase 2 + US1 (T001–T007)**: first-run prompt + working integration; independently shippable and demonstrable.
- **Increment 2 = US2 (T008–T011)**: the Settings control / recovery path.
- **Increment 3 = US3 (T012)**: verify inert behavior off AppImage.
- **Finish with Polish (T013–T016)**: docs, README, the `fmt`/`clippy`/`test` gate, and GNOME/KDE smoke.
