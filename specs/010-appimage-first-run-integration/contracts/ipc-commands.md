# IPC Contract: AppImage Integration Commands

Two Tauri commands back this feature. Both are inert outside an AppImage runtime.
The first-run prompt's accept path and the Settings button invoke the **same**
internal integration routine — these commands are the externally-observable surface.

## `get_appimage_integration_status`

- **Direction**: frontend → backend. Read-only; no side effects.
- **Request**: none.
- **Response**:

  ```jsonc
  { "is_appimage": boolean, "integrated": boolean }
  ```

  - `is_appimage`: `true` iff the `APPIMAGE` env var is set.
  - `integrated`: `true` iff the `appimage.integration` setting equals `done`.
- **Errors**: none. On any uncertainty it returns `{ is_appimage: false, integrated: false }` (the Settings row then renders nothing).
- **Consumer**: `GeneralTab` — decides whether to render the install row and which state to show.

## `integrate_appimage`

- **Direction**: frontend → backend (action). Also called by the first-run prompt's accept handler via the shared routine.
- **Request**: none.
- **Response**: `Result<(), String>` — `Ok(())` on success; `Err(message)` on failure (the frontend shows the message as an error toast).
- **Behavior**: ensure `~/Applications/`; copy `$APPIMAGE` → `~/Applications/Quill.AppImage` (executable); write `~/.local/share/applications/quill.desktop`; install the icon PNG; best-effort `update-desktop-database`/`gtk-update-icon-cache`; set `appimage.integration=done` + `appimage.integration_path`. **Idempotent** — re-running overwrites in place with no duplicates.
- **Error cases** (returned as `Err`, state left unrecorded for retry): not running as an AppImage; target directory not writable; copy/write failure (no space, permissions).
- **Consumer**: the Settings "Install to applications menu" button.

## Shared invariants

- Exactly one internal integration function backs both the prompt and `integrate_appimage`.
- No privilege escalation; all paths are user-space.
- Neither command touches a previously-installed `.deb` (out of scope).
