# Phase 0 Research: AppImage First-Run Self-Integration

All design clarifications were resolved during brainstorming; this records the
decisions, rationale, and rejected alternatives. No `NEEDS CLARIFICATION` remain.

## R-A: AppImage runtime detection

- **Decision**: Detect via the `APPIMAGE` environment variable (set by the AppImage `AppRun`). Treat its presence as "running as an AppImage."
- **Rationale**: Canonical AppImage signal; it is exactly what `tauri-plugin-updater` itself relies on for Linux. Cheap, no I/O.
- **Alternatives**: Inspecting `/proc/self/mountinfo` or `current_exe()` paths — fragile and version-dependent.

## R-B: Copy vs. move into the applications location

- **Decision**: **Copy** `$APPIMAGE` → `~/Applications/Quill.AppImage` (chmod +x), leaving the original in place.
- **Rationale**: Copy keeps the running session's `$APPIMAGE` path valid, so no jarring auto-restart is needed; it is also robust across filesystems (e.g. a `/tmp` download). The success toast tells the user they may delete the original download.
- **Alternatives**: Move — cleaner (no duplicate) but requires relaunching from the new path mid-session (jarring) and breaks on cross-filesystem renames.

## R-C: Where the AppImage and launcher entry live

- **Decision**: AppImage → `~/Applications/Quill.AppImage`. Launcher → `~/.local/share/applications/quill.desktop`. Icon → `~/.local/share/icons/hicolor/256x256/apps/quill.png`.
- **Rationale**: `~/Applications` matches the existing README install convention; the XDG user-level `applications`/`icons` paths are the standard menu-integration locations and need no privileges.
- **Alternatives**: `~/.local/bin` (not menu-standard); system-wide `/usr/share` (needs root — rejected, defeats the user-space goal).

## R-D: Desktop database / icon cache refresh

- **Decision**: After writing the `.desktop` and icon, best-effort run `update-desktop-database ~/.local/share/applications` and `gtk-update-icon-cache`; ignore failures/absence.
- **Rationale**: Makes the entry appear immediately on environments that have the tools; harmless where they are missing (the catalog refreshes on its own).
- **Alternatives**: Depend on `appimaged`/AppImageLauncher — an external dependency we cannot assume; rejected.

## R-E: Icon source

- **Decision**: Extract the icon from the running AppImage (its `$APPDIR`/embedded resource) rather than referencing a repo path.
- **Rationale**: The shipped bundle, not the source tree, is what runs; the embedded icon is guaranteed present at runtime.
- **Alternatives**: Bundling a copy via Tauri resources also works but is redundant with the AppImage's own icon.

## R-F: Integration-state persistence

- **Decision**: Persist in the settings table: `appimage.integration` ∈ {`done`, `declined`} plus `appimage.integration_path` (the integrated path). Reuse `storage.get_setting`/`set_setting`.
- **Rationale**: Distinguishes "already integrated" (skip prompt) from "user declined" (skip prompt, but the Settings control still works) from "never asked." Reuses the existing settings store; no new schema.
- **Alternatives**: Infer state purely from the `.desktop` file's presence — cannot represent "declined," so the prompt would nag.

## R-G: Updater interaction

- **Decision**: No updater changes. Because the menu launches `~/Applications/Quill.AppImage`, the Tauri updater replaces that copy in place on future updates. The first session keeps running the original copy; that is acceptable.
- **Rationale**: Copy semantics (R-B) make the integrated copy the long-lived one without any relaunch or updater rewiring.

## R-H: Prompt mechanism & startup safety

- **Decision**: Use `tauri-plugin-dialog` for the native prompt (as `check_for_update` already does), spawned async from the `.setup()` hook.
- **Rationale**: Reuses an in-place plugin; async dispatch keeps GTK/webview startup unblocked (the codebase explicitly forbids blocking `.setup()`).

## R-I: Privilege boundary

- **Decision**: All operations are user-space (`~/Applications`, `~/.local/share/...`). No `pkexec`/`sudo`, and **removing any prior `.deb`** is explicitly out of scope (a separate migration concern).
- **Rationale**: Keeps the feature simple, prompt-free of password dialogs, and reliable across distros.
