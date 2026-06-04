//! AppImage first-run self-integration (Linux only).
//!
//! When Quill is launched as a loose AppImage it has no desktop presence: no
//! menu entry, no icon, no launcher. This module detects that state and, on user
//! consent, integrates the running AppImage into the user's desktop environment
//! entirely in user space:
//!
//! - copies `$APPIMAGE` to `~/Applications/Quill.AppImage` (executable),
//! - writes a launcher to `~/.local/share/applications/quill.desktop`,
//! - installs an icon to `~/.local/share/icons/hicolor/256x256/apps/quill.png`,
//! - best-effort refreshes the desktop/icon caches,
//! - records the decision in the settings table.
//!
//! The same [`integrate`] routine backs both the first-run prompt (wired in
//! `lib.rs`'s `.setup()`) and the Settings "Install to applications menu"
//! control (the [`integrate_appimage`] command). All logic is inert on
//! non-AppImage runtimes (dev builds, future packages): [`running_as_appimage`]
//! returns `false`, so [`should_prompt`] is `false` and the status command
//! reports `is_appimage: false`.
//!
//! Side-effecting work is deliberately separated from pure logic (eligibility,
//! path/`.desktop` generation, icon-source selection) so the core is unit-tested
//! with explicit injected paths — tests never mutate process env.
//!
//! Design references: `specs/010-appimage-first-run-integration/` (R-A..R-I,
//! FR-001..FR-010).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::get_storage;

/// Settings key recording the integration decision: `"done"` or `"declined"`.
/// Unset means "never asked" (the only state that triggers the first-run prompt).
const INTEGRATION_KEY: &str = "appimage.integration";
/// Settings key recording the absolute path of the integrated AppImage copy.
const INTEGRATION_PATH_KEY: &str = "appimage.integration_path";

/// Settings value: the user integrated (prompt suppressed; status `integrated`).
const STATE_DONE: &str = "done";
/// Settings value: the user declined (prompt suppressed; Settings control still works).
const STATE_DECLINED: &str = "declined";

/// Product name used for the menu entry and window-class matching. Pinned to the
/// Tauri `productName` ("Quill" in `tauri.conf.json`); the GTK build derives the
/// runtime window class from this value, so `StartupWMClass` must match it.
const PRODUCT_NAME: &str = "Quill";
/// Icon name referenced from the `.desktop` `Icon=` field; resolves to the
/// installed `hicolor` PNG (`quill.png`).
const ICON_NAME: &str = "quill";

/// Status returned by [`get_appimage_integration_status`]. Never an error: on any
/// uncertainty both fields are `false` (the Settings row then renders nothing).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IntegrationStatus {
    /// `true` iff the `APPIMAGE` env var is set (running as an AppImage).
    pub is_appimage: bool,
    /// `true` iff the persisted decision equals `"done"`.
    pub integrated: bool,
}

/// True iff this process is running as an AppImage.
///
/// Detection uses the `APPIMAGE` env var, which the AppImage `AppRun` sets to the
/// path of the running image. This is the canonical signal `tauri-plugin-updater`
/// itself relies on for Linux (research R-A).
pub fn running_as_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some()
}

/// Pure eligibility check for the first-run prompt.
///
/// The prompt fires only when running as an AppImage **and** no decision has been
/// recorded yet (`decision` is `None`). Any recorded decision — `"done"` or
/// `"declined"` — suppresses it permanently (FR-004).
pub fn should_prompt(decision: Option<&str>, is_appimage: bool) -> bool {
    is_appimage && decision.is_none()
}

/// Read the persisted integration decision (`Some("done")`, `Some("declined")`,
/// or `None` when never asked / storage unavailable).
///
/// Calls the (synchronous) storage layer directly — no `run_blocking` /
/// `block_in_place` — so it is safe to invoke from any thread, not just a
/// Tokio worker.
fn read_decision() -> Option<String> {
    let storage = get_storage().ok()?;
    storage.get_setting(INTEGRATION_KEY).ok().flatten()
}

/// Persist the integration decision and (optionally) the integrated path.
///
/// Returns `Err` if the settings write fails, so callers that must keep the
/// decision retryable (the `integrate` success path) can surface the failure
/// rather than silently dropping it.
///
/// Write ordering is deliberate: the integrated path is persisted **first** and
/// the decision (`"done"`) **last**, so a crash between the two writes can never
/// leave a `done` decision without a recorded path. Calls storage directly
/// (synchronously), so this is safe on any thread.
fn write_decision(state: &str, path: Option<&str>) -> Result<(), String> {
    let storage = get_storage()?;
    if let Some(path) = path {
        storage.set_setting(INTEGRATION_PATH_KEY, path)?;
    }
    storage.set_setting(INTEGRATION_KEY, state)?;
    Ok(())
}

/// Record that the user declined integration (suppresses the prompt for good).
/// Best-effort: logs on failure rather than propagating, since a failed write
/// only means the prompt may reappear on a later launch.
pub fn record_declined() {
    if let Err(error) = write_decision(STATE_DECLINED, None) {
        log::warn!("Failed to persist AppImage integration decline: {error}");
    }
}

/// Computed user-space target paths for integration, derived from a home dir.
/// Taking an explicit `home` keeps these unit-testable without touching env.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetPaths {
    /// `~/Applications/Quill.AppImage`
    appimage: PathBuf,
    /// `~/.local/share/applications/quill.desktop`
    desktop: PathBuf,
    /// `~/.local/share/icons/hicolor/256x256/apps/quill.png`
    icon: PathBuf,
}

/// Compute the integration target paths under `home` (research R-C). Pure: no
/// I/O, no env access.
fn target_paths(home: &Path) -> TargetPaths {
    TargetPaths {
        appimage: home.join("Applications").join("Quill.AppImage"),
        desktop: home
            .join(".local")
            .join("share")
            .join("applications")
            .join("quill.desktop"),
        icon: home
            .join(".local")
            .join("share")
            .join("icons")
            .join("hicolor")
            .join("256x256")
            .join("apps")
            .join("quill.png"),
    }
}

/// Build a freedesktop `.desktop` launcher entry pointing at `exec_path`.
///
/// `Exec` appends `%U` so files/URLs passed by the launcher are forwarded.
/// `StartupWMClass` is pinned to [`PRODUCT_NAME`] so the menu entry associates
/// with Quill's runtime window (the GTK class derives from `productName`).
pub fn desktop_entry(exec_path: &str, icon_name: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={PRODUCT_NAME}\n\
         Comment=Token usage and session analytics for Claude Code and Codex\n\
         Exec={exec_path} %U\n\
         Icon={icon_name}\n\
         Categories=Utility;Development;\n\
         Terminal=false\n\
         StartupWMClass={PRODUCT_NAME}\n"
    )
}

/// Locate a PNG icon inside the running AppImage's mounted `$APPDIR`.
///
/// Preference order (most specific → least), all under `app_dir`:
///   1. `usr/share/icons/hicolor/256x256/apps/*.png` — the canonical packaged
///      256px icon Tauri's Linux bundle installs.
///   2. `<PRODUCT_NAME-lowercased>.png` / `<ICON_NAME>.png` at the AppImage root
///      — the top-level icon `AppRun` references.
///   3. `.DirIcon` — the AppImage thumbnail (PNG in practice for our bundle).
///   4. first `*.png` found anywhere under `usr/share/icons` — defensive
///      fallback so a layout change still yields a usable icon.
///
/// Returns `None` if `$APPDIR` is unset/missing or no PNG is found; the caller
/// then skips icon installation (non-fatal).
fn find_appdir_icon(app_dir: &Path) -> Option<PathBuf> {
    if !is_dir_nofollow(app_dir) {
        return None;
    }

    let hicolor_256 = app_dir
        .join("usr")
        .join("share")
        .join("icons")
        .join("hicolor")
        .join("256x256")
        .join("apps");
    if let Some(png) = first_png_in_dir(&hicolor_256) {
        return Some(png);
    }

    for name in [
        format!("{}.png", PRODUCT_NAME.to_lowercase()),
        format!("{ICON_NAME}.png"),
    ] {
        let candidate = app_dir.join(&name);
        if is_regular_file_nofollow(&candidate) {
            return Some(candidate);
        }
    }

    let dir_icon = app_dir.join(".DirIcon");
    if is_regular_file_nofollow(&dir_icon) {
        return Some(dir_icon);
    }

    // Defensive last resort: any PNG under the icons tree (depth-bounded).
    find_png_recursive(
        &app_dir.join("usr").join("share").join("icons"),
        MAX_ICON_WALK_DEPTH,
    )
}

/// Maximum directory depth for the defensive icon walk ([`find_png_recursive`]).
/// Past this depth the walk returns `None` rather than descending further — a
/// crafted `$APPDIR` cannot make us recurse without bound.
const MAX_ICON_WALK_DEPTH: usize = 6;

/// `true` iff `path` is a regular file, following **no** symlinks (lstat). Used
/// throughout icon discovery so a symlink planted in a crafted `$APPDIR` is not
/// traversed (which could escape `$APPDIR`).
fn is_regular_file_nofollow(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_file())
        .unwrap_or(false)
}

/// `true` iff `path` is a directory, following **no** symlinks (lstat).
fn is_dir_nofollow(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_dir())
        .unwrap_or(false)
}

/// First `*.png` directly inside `dir` (non-recursive), sorted for determinism.
/// File-type checks use lstat so symlinks are ignored, not followed.
fn first_png_in_dir(dir: &Path) -> Option<PathBuf> {
    let mut pngs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| has_png_extension(path) && is_regular_file_nofollow(path))
        .collect();
    pngs.sort();
    pngs.into_iter().next()
}

/// First `*.png` found by a bounded depth-first walk under `dir`. Directory
/// order is sorted so the result is deterministic. `depth` is the remaining
/// descent budget: at `0` the walk inspects the current directory's files but
/// does not recurse into subdirectories. File-type checks use lstat so symlinks
/// are neither matched as files nor traversed as directories — a crafted bundle
/// cannot redirect the walk outside `$APPDIR`.
fn find_png_recursive(dir: &Path, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in &entries {
        if has_png_extension(path) && is_regular_file_nofollow(path) {
            return Some(path.clone());
        }
    }
    if depth == 0 {
        return None;
    }
    for path in &entries {
        if is_dir_nofollow(path)
            && let Some(found) = find_png_recursive(path, depth - 1)
        {
            return Some(found);
        }
    }
    None
}

fn has_png_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("png"))
        .unwrap_or(false)
}

/// Copy `src` to `dest`, creating parent dirs, and mark `dest` executable.
///
/// `src` must be a **regular file** (verified via `symlink_metadata`/lstat so a
/// dangling symlink or special file — device, FIFO, socket — is rejected rather
/// than copied).
fn copy_appimage(src: &Path, dest: &Path) -> Result<(), String> {
    let src_meta = std::fs::symlink_metadata(src)
        .map_err(|e| format!("Failed to stat AppImage source {}: {e}", src.display()))?;
    if !src_meta.file_type().is_file() {
        return Err(format!(
            "AppImage source {} is not a regular file",
            src.display()
        ));
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    // Skip the copy when the source already IS the destination (e.g. installed
    // straight to ~/Applications by install.sh, or placed there manually):
    // std::fs::copy opens the dest with O_TRUNC, so it would truncate the file
    // onto itself. The executable bit is still re-applied below either way.
    let already_in_place = std::fs::canonicalize(src)
        .ok()
        .zip(std::fs::canonicalize(dest).ok())
        .map(|(resolved_src, resolved_dest)| resolved_src == resolved_dest)
        .unwrap_or(false);
    if !already_in_place {
        std::fs::copy(src, dest).map_err(|e| {
            format!(
                "Failed to copy AppImage {} -> {}: {e}",
                src.display(),
                dest.display()
            )
        })?;
    }
    // The executable bit only exists on Unix; Windows has no equivalent (and
    // integrate() never reaches here on Windows anyway, since $APPIMAGE is
    // unset). This is the module's only platform-specific call — gating it keeps
    // the rest of the module building on every target (the Windows release build
    // compiles this file too).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(dest, perms)
            .map_err(|e| format!("Failed to set executable bit on {}: {e}", dest.display()))?;
    }
    Ok(())
}

/// Write `contents` to `dest`, creating parent dirs first.
fn write_file(dest: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(dest, contents).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

/// Perform the filesystem side of integration against explicit paths.
///
/// Pulled out of [`integrate`] so the side effects are testable with a temp dir
/// and injected source paths — no env, no `AppHandle`, no settings writes. The
/// icon is optional: a missing `$APPDIR` icon is non-fatal (the launcher still
/// works, just without a custom icon until the catalog supplies a default).
///
/// Idempotent: every step overwrites in place, so a re-run leaves exactly one
/// copy / `.desktop` / icon.
fn perform_integration(
    appimage_src: &Path,
    icon_src: Option<&Path>,
    targets: &TargetPaths,
) -> Result<(), String> {
    copy_appimage(appimage_src, &targets.appimage)?;

    let exec_path = targets.appimage.to_string_lossy();
    let desktop = desktop_entry(&exec_path, ICON_NAME);
    write_file(&targets.desktop, desktop.as_bytes())?;

    if let Some(icon_src) = icon_src {
        let icon_bytes = std::fs::read(icon_src)
            .map_err(|e| format!("Failed to read icon {}: {e}", icon_src.display()))?;
        write_file(&targets.icon, &icon_bytes)?;
    } else {
        log::warn!("No icon found in $APPDIR; skipping icon install (launcher still created)");
    }

    Ok(())
}

/// Best-effort refresh of the desktop-entry and icon caches so the new launcher
/// appears immediately. Missing tools or non-zero exits are ignored (research
/// R-D): on environments without them the catalog refreshes on its own schedule.
fn refresh_desktop_caches(applications_dir: &Path, icon_root: &Path) {
    use std::process::Command;

    let _ = Command::new("update-desktop-database")
        .arg(applications_dir)
        .status();
    // `-q -f -t <theme-root>`: quiet, force, target the hicolor theme root.
    let _ = Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg("-f")
        .arg("-t")
        .arg(icon_root)
        .status();
}

/// Resolve the running AppImage path and (optional) bundled icon path from env.
///
/// Returns the `$APPIMAGE` path (required) and the best `$APPDIR` icon
/// (optional). Errors only when `$APPIMAGE` is unset — i.e. not running as an
/// AppImage, which makes integration meaningless (FR-001).
fn resolve_sources() -> Result<(PathBuf, Option<PathBuf>), String> {
    let appimage = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or_else(|| "Not running as an AppImage (APPIMAGE is unset)".to_string())?;
    let icon = std::env::var_os("APPDIR")
        .map(PathBuf::from)
        .and_then(|app_dir| find_appdir_icon(&app_dir));
    Ok((appimage, icon))
}

/// Integrate the running AppImage into the user's desktop environment.
///
/// Shared by the first-run prompt and the [`integrate_appimage`] command. Steps:
/// resolve `$APPIMAGE`/`$APPDIR`; ensure `~/Applications`; copy + chmod; write
/// the `.desktop`; install the icon; best-effort refresh caches; then persist
/// the integrated path followed by `appimage.integration=done`.
///
/// Idempotent (every write overwrites). On **any** error it returns `Err` and
/// does **not** persist state, so the decision stays unrecorded and a later
/// launch or Settings click can retry (FR-006/FR-007/FR-009). The `app` handle
/// is accepted for signature parity with the command layer and future use; the
/// routine itself needs only env + home dir.
///
/// Calls only synchronous storage (no `run_blocking` / `block_in_place`), so it
/// is a plain blocking function safe to run on any thread — callers off a Tokio
/// worker (e.g. the GTK dialog callback) must wrap it in `spawn_blocking`.
pub fn integrate(_app: &tauri::AppHandle) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())?;
    let (appimage_src, icon_src) = resolve_sources()?;
    let targets = target_paths(&home);

    perform_integration(&appimage_src, icon_src.as_deref(), &targets)?;

    // Cache refresh is best-effort and must not gate success.
    let icon_theme_root = home
        .join(".local")
        .join("share")
        .join("icons")
        .join("hicolor");
    if let Some(applications_dir) = targets.desktop.parent() {
        refresh_desktop_caches(applications_dir, &icon_theme_root);
    }

    // Persist only after the filesystem work fully succeeded.
    let integrated_path = targets.appimage.to_string_lossy().to_string();
    write_decision(STATE_DONE, Some(&integrated_path))?;
    log::info!("AppImage integrated at {integrated_path}");
    Ok(())
}

/// Read-only status for the Settings control. Never errors: on any uncertainty
/// both fields are `false` (see contract). `integrated` is `true` only when the
/// persisted decision equals `"done"`.
#[tauri::command]
pub async fn get_appimage_integration_status() -> IntegrationStatus {
    let is_appimage = running_as_appimage();
    let integrated = read_decision().as_deref() == Some(STATE_DONE);
    IntegrationStatus {
        is_appimage,
        integrated,
    }
}

/// Run integration on demand from the Settings control. Delegates to the shared
/// [`integrate`] routine on the blocking pool (it performs a multi-MB `fs::copy`
/// and must not block a Tokio worker); returns `Err(message)` on failure
/// (rendered as a toast) with the decision left unrecorded for retry.
#[tauri::command]
pub async fn integrate_appimage(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || integrate(&app))
        .await
        .map_err(|e| e.to_string())?
}

// The tests exercise Unix-only file APIs (mode bits, symlinks); they run on
// Linux/macOS CI and are skipped on Windows where those APIs don't exist.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // ---- Pure logic -------------------------------------------------------

    // detection / eligibility — running_as_appimage reflects env.
    #[test]
    fn running_as_appimage_reflects_env() {
        // The test binary is not an AppImage, so the var is unset here. We must
        // not mutate process env (racy across parallel tests), so we assert the
        // ambient state and exercise the var-driven branch via should_prompt's
        // truth table instead.
        assert!(!running_as_appimage());
    }

    #[test]
    fn should_prompt_truth_table() {
        // Prompt only when AppImage AND no decision recorded.
        assert!(should_prompt(None, true));
        // Already integrated or declined -> never prompt.
        assert!(!should_prompt(Some("done"), true));
        assert!(!should_prompt(Some("declined"), true));
        // Non-AppImage runtime -> never prompt, regardless of decision.
        assert!(!should_prompt(None, false));
        assert!(!should_prompt(Some("done"), false));
        assert!(!should_prompt(Some("declined"), false));
    }

    #[test]
    fn target_paths_are_user_space_xdg() {
        let home = Path::new("/home/tester");
        let t = target_paths(home);
        assert_eq!(
            t.appimage,
            Path::new("/home/tester/Applications/Quill.AppImage")
        );
        assert_eq!(
            t.desktop,
            Path::new("/home/tester/.local/share/applications/quill.desktop")
        );
        assert_eq!(
            t.icon,
            Path::new("/home/tester/.local/share/icons/hicolor/256x256/apps/quill.png")
        );
    }

    #[test]
    fn desktop_entry_has_required_fields() {
        let entry = desktop_entry("/home/tester/Applications/Quill.AppImage", ICON_NAME);
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Type=Application\n"));
        assert!(entry.contains("Name=Quill\n"));
        // Exec points at the integrated copy and forwards files/URLs.
        assert!(entry.contains("Exec=/home/tester/Applications/Quill.AppImage %U\n"));
        assert!(entry.contains("Icon=quill\n"));
        assert!(entry.contains("Categories=Utility;Development;\n"));
        assert!(entry.contains("Terminal=false\n"));
        // WMClass pinned to the product name so the menu entry maps to the window.
        assert!(entry.contains("StartupWMClass=Quill\n"));
    }

    #[test]
    fn png_extension_is_case_insensitive() {
        assert!(has_png_extension(Path::new("a/b/icon.png")));
        assert!(has_png_extension(Path::new("ICON.PNG")));
        assert!(!has_png_extension(Path::new("icon.svg")));
        assert!(!has_png_extension(Path::new("noext")));
    }

    // ---- Icon-source selection (temp dir, no env) -------------------------

    #[test]
    fn find_appdir_icon_prefers_hicolor_256() {
        let tmp = tempfile::tempdir().unwrap();
        let app_dir = tmp.path();
        let hicolor = app_dir.join("usr/share/icons/hicolor/256x256/apps");
        std::fs::create_dir_all(&hicolor).unwrap();
        std::fs::write(hicolor.join("quill.png"), b"png").unwrap();
        // A root-level icon and .DirIcon also exist but must lose to hicolor.
        std::fs::write(app_dir.join("quill.png"), b"png").unwrap();
        std::fs::write(app_dir.join(".DirIcon"), b"png").unwrap();

        let found = find_appdir_icon(app_dir).unwrap();
        assert_eq!(found, hicolor.join("quill.png"));
    }

    #[test]
    fn find_appdir_icon_falls_back_to_root_then_diricon() {
        // Only a root-level <name>.png present.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("quill.png"), b"png").unwrap();
        assert_eq!(
            find_appdir_icon(tmp.path()).unwrap(),
            tmp.path().join("quill.png")
        );

        // Only .DirIcon present.
        let tmp2 = tempfile::tempdir().unwrap();
        std::fs::write(tmp2.path().join(".DirIcon"), b"png").unwrap();
        assert_eq!(
            find_appdir_icon(tmp2.path()).unwrap(),
            tmp2.path().join(".DirIcon")
        );
    }

    #[test]
    fn find_appdir_icon_none_when_no_png() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_appdir_icon(tmp.path()).is_none());
        // Missing dir is also None, not a panic.
        assert!(find_appdir_icon(&tmp.path().join("nope")).is_none());
    }

    // ---- Side effects (temp dir, explicit paths) --------------------------

    fn fake_appimage(dir: &Path) -> PathBuf {
        let src = dir.join("Quill-download.AppImage");
        std::fs::write(&src, b"AI\x02fake-appimage-bytes").unwrap();
        src
    }

    // integrate writes copy + .desktop + icon under the target home.
    #[test]
    fn perform_integration_writes_copy_desktop_and_icon() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let src = fake_appimage(tmp.path());
        let icon = tmp.path().join("source-icon.png");
        std::fs::write(&icon, b"ICONDATA").unwrap();
        let targets = target_paths(&home);

        perform_integration(&src, Some(&icon), &targets).unwrap();

        // AppImage copied with identical bytes and the executable bit set.
        assert!(targets.appimage.is_file());
        assert_eq!(
            std::fs::read(&targets.appimage).unwrap(),
            std::fs::read(&src).unwrap()
        );
        let mode = std::fs::metadata(&targets.appimage)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "executable bits should be set");

        // .desktop written with the integrated Exec path.
        let desktop = std::fs::read_to_string(&targets.desktop).unwrap();
        assert!(desktop.contains(&format!("Exec={} %U\n", targets.appimage.to_string_lossy())));

        // Icon copied verbatim.
        assert_eq!(std::fs::read(&targets.icon).unwrap(), b"ICONDATA");
    }

    #[test]
    fn perform_integration_skips_icon_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let src = fake_appimage(tmp.path());
        let targets = target_paths(&home);

        perform_integration(&src, None, &targets).unwrap();

        assert!(targets.appimage.is_file());
        assert!(targets.desktop.is_file());
        // No icon source -> no icon file, but the launcher still exists.
        assert!(!targets.icon.exists());
    }

    // idempotent re-run leaves a single copy/.desktop/icon (T011).
    #[test]
    fn perform_integration_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let src = fake_appimage(tmp.path());
        let icon = tmp.path().join("source-icon.png");
        std::fs::write(&icon, b"ICONDATA").unwrap();
        let targets = target_paths(&home);

        perform_integration(&src, Some(&icon), &targets).unwrap();
        // Re-run with updated source bytes: overwrite in place.
        std::fs::write(&src, b"AI\x02updated-bytes").unwrap();
        perform_integration(&src, Some(&icon), &targets).unwrap();

        // Exactly one launcher entry in the applications dir.
        let apps_dir = targets.desktop.parent().unwrap();
        let desktops: Vec<_> = std::fs::read_dir(apps_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("desktop"))
            .collect();
        assert_eq!(desktops.len(), 1, "should be a single .desktop entry");

        // The integrated copy reflects the latest source bytes.
        assert_eq!(
            std::fs::read(&targets.appimage).unwrap(),
            b"AI\x02updated-bytes"
        );

        // Exactly one icon at the target.
        let icon_dir = targets.icon.parent().unwrap();
        let icons: Vec<_> = std::fs::read_dir(icon_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| has_png_extension(p))
            .collect();
        assert_eq!(icons.len(), 1, "should be a single installed icon");
    }

    // a forced failure leaves no partial menu entry / icon (retryable).
    #[test]
    fn perform_integration_fails_when_source_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let missing = tmp.path().join("does-not-exist.AppImage");
        let targets = target_paths(&home);

        let result = perform_integration(&missing, None, &targets);
        assert!(result.is_err(), "missing source must fail");
        // Copy failed before any .desktop/icon was written -> nothing left behind.
        assert!(!targets.appimage.exists());
        assert!(!targets.desktop.exists());
        assert!(!targets.icon.exists());
    }

    #[test]
    fn copy_appimage_sets_executable_bit() {
        let tmp = tempfile::tempdir().unwrap();
        let src = fake_appimage(tmp.path());
        let dest = tmp.path().join("nested/Quill.AppImage");
        copy_appimage(&src, &dest).unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }

    // a non-regular-file source (symlink to dir / dangling symlink) is rejected
    // before any copy, so crafted special files are never duplicated.
    #[test]
    fn copy_appimage_rejects_non_regular_source() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out/Quill.AppImage");

        // A directory is not a regular file.
        let dir_src = tmp.path().join("a-directory");
        std::fs::create_dir(&dir_src).unwrap();
        assert!(copy_appimage(&dir_src, &dest).is_err());
        assert!(!dest.exists());

        // A dangling symlink resolves to nothing but lstat sees a symlink.
        let dangling = tmp.path().join("dangling.AppImage");
        symlink(tmp.path().join("nowhere"), &dangling).unwrap();
        assert!(copy_appimage(&dangling, &dest).is_err());
        assert!(!dest.exists());
    }

    // src == dest (install.sh lands the AppImage at the integration target):
    // the copy is skipped so the file is not truncated onto itself.
    #[test]
    fn copy_appimage_skips_self_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Applications/Quill.AppImage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"AI\x02real-appimage-bytes").unwrap();
        copy_appimage(&path, &path).unwrap();
        // Bytes preserved (not truncated to zero) and still executable.
        assert_eq!(std::fs::read(&path).unwrap(), b"AI\x02real-appimage-bytes");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }

    // the defensive icon walk stops at MAX_ICON_WALK_DEPTH and ignores deeper PNGs.
    #[test]
    fn find_png_recursive_respects_depth_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Build root/d1/d2/.../d{N} with a PNG only at the deepest level.
        let mut deep = root.to_path_buf();
        for i in 0..(MAX_ICON_WALK_DEPTH + 2) {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep.png"), b"png").unwrap();

        // Past the cap the walk gives up rather than reaching the deep PNG.
        assert!(find_png_recursive(root, MAX_ICON_WALK_DEPTH).is_none());

        // A PNG within the budget is found.
        let shallow = root.join("d0").join("near.png");
        std::fs::write(&shallow, b"png").unwrap();
        assert_eq!(find_png_recursive(root, MAX_ICON_WALK_DEPTH), Some(shallow));
    }

    // icon discovery uses lstat: a symlinked PNG is not matched as a file.
    #[test]
    fn icon_discovery_ignores_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("icons");
        std::fs::create_dir_all(&dir).unwrap();

        // A real PNG outside the search dir, surfaced only via a symlink inside it.
        let real = tmp.path().join("outside.png");
        std::fs::write(&real, b"png").unwrap();
        symlink(&real, dir.join("link.png")).unwrap();

        // lstat sees a symlink, not a regular file -> no match.
        assert!(first_png_in_dir(&dir).is_none());
        assert!(find_png_recursive(&dir, MAX_ICON_WALK_DEPTH).is_none());

        // A genuine regular-file PNG is still found.
        std::fs::write(dir.join("real.png"), b"png").unwrap();
        assert_eq!(first_png_in_dir(&dir), Some(dir.join("real.png")));
    }

    // ---- Status struct shape (T012, non-AppImage inert) -------------------

    #[test]
    fn integration_status_serializes_expected_shape() {
        // The non-AppImage default the status command returns on uncertainty.
        let status = IntegrationStatus {
            is_appimage: false,
            integrated: false,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["is_appimage"], serde_json::json!(false));
        assert_eq!(json["integrated"], serde_json::json!(false));
    }
}
