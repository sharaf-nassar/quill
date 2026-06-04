# Quickstart: Verifying AppImage First-Run Integration (maintainer)

End-to-end verification. Requires a built AppImage (CI artifact
`Quill_<ver>_linux_amd64.AppImage`, or a local Tauri build whose AppImage lands
under `src-tauri/target/release/bundle/appimage/`). `chmod +x` it first.

## 1. First-run prompt — accept

1. Run the AppImage from `~/Downloads`. Expect the native prompt: *"Add Quill to your applications menu?"*
2. Click **Add**, then verify:
   - `~/Applications/Quill.AppImage` exists and is executable.
   - `~/.local/share/applications/quill.desktop` exists with `Exec` pointing at it.
   - `~/.local/share/icons/hicolor/256x256/apps/quill.png` exists.
   - Quill appears in the applications menu — check **GNOME and KDE**.
   - A toast notes the original download can be deleted.
3. Launch Quill from the menu → it opens against the same data dir (`~/.local/share/com.quilltoolkit.app`): usage history, auth, and settings intact.

## 2. Prompt does not recur

Relaunch the integrated copy → no prompt appears.

## 3. Decline path

1. Reset state (clear the `appimage.integration` setting or use a fresh profile). Run the AppImage and click **Not now**.
2. Verify no files changed and relaunching does not re-prompt.
3. Open Settings → General → an active **Install to applications menu** button is present → click it → integration completes as in step 1.

## 4. Settings control states

- Integrated → row shows **Installed ✓** (disabled).
- Not integrated, AppImage runtime → active **Install to applications menu** button.

## 5. Non-AppImage build

Run a dev build (`npm run tauri dev`) or any non-AppImage binary → no prompt
fires, the Settings row is absent, and `get_appimage_integration_status` reports
`is_appimage: false`.

## 6. Idempotency & failure

- Click **Install** twice → no duplicate menu entry.
- Make `~/Applications` read-only and trigger integration → a non-fatal error toast appears, the `appimage.integration` setting stays unset, and retrying after restoring permissions succeeds.

## 7. Docs

`lat check` passes and the `lat.md` features section documents detection, the
prompt, copy-to-`~/Applications`, the launcher entry/icon, the settings flag, the
Settings control, and the updater interaction.
