//! Transactional deployment of Quill-managed assets into user config dirs.
//!
//! One backup directory per transaction (`.quill-deploy-backup`, created
//! beside the deployment targets) holds copies of provider configuration
//! files plus any target trees moved aside during publish. Commit atomically
//! renames the backup away; a failure or crash leaves it in place so
//! [`recover_staged_batch`] can restore every original from it. Staged trees
//! are built in temp dirs beside their targets so publication is a
//! same-filesystem rename per target.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

const STAGING_PREFIX: &str = ".quill-staging-";
const BACKUP_DIR: &str = ".quill-deploy-backup";
const BACKUP_TARGETS_DIR: &str = "targets";
const ABSENT_SUFFIX: &str = ".was-absent";
const BACKUP_MANIFEST: &str = "manifest.json";
const BACKUP_VERSION: u32 = 1;
const DEPLOY_STAMP_FILE: &str = ".quill-deploy-stamp";
const DEPLOY_STAMP_VERSION: u32 = 1;

const REQUIRED_MCP_FILES: [&str; 6] = [
    "server.py",
    "dependencies.py",
    "pyproject.toml",
    "uv.lock",
    "tools/__init__.py",
    "tools/search.py",
];

/// A complete replacement directory built beside its eventual target.
pub(crate) struct StagedDirectory {
    target: PathBuf,
    staging: Option<TempDir>,
}

/// A published directory batch whose backup remains live until provider setup succeeds.
#[must_use = "published deployment batches must be committed or rolled back"]
pub(crate) struct PublishedBatch {
    backup: PathBuf,
}

/// Exact pre-install snapshots of provider configuration and instruction files.
#[must_use = "file snapshots must be retained until provider setup succeeds"]
pub(crate) struct FileSnapshots {
    targets: Vec<PathBuf>,
    backup: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupManifest {
    version: u32,
    files: Vec<BackupFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupFile {
    path: PathBuf,
    /// Snapshot data file name inside the backup directory; `None` when the
    /// path did not exist at capture time.
    data: Option<String>,
    mode: Option<u32>,
}

impl StagedDirectory {
    pub(crate) fn new(target: PathBuf) -> Result<Self, String> {
        let parent = target.parent().ok_or_else(|| {
            format!(
                "Cannot determine parent directory for deployment target {}",
                target.display()
            )
        })?;

        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create deployment parent {}: {err}",
                parent.display()
            )
        })?;

        let staging = Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(parent)
            .map_err(|err| {
                format!(
                    "Failed to create staging directory beside {}: {err}",
                    target.display()
                )
            })?;

        Ok(Self {
            target,
            staging: Some(staging),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.staging
            .as_ref()
            .expect("staging directory is unavailable after publication")
            .path()
    }
}

/// Recursively copy every file from `src` into `dst`.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }

    fs::create_dir_all(dst)
        .map_err(|err| format!("Failed to create directory {}: {err}", dst.display()))?;

    for entry in walkdir::WalkDir::new(src).min_depth(1).follow_links(true) {
        let entry = entry.map_err(|err| format!("Failed to walk {}: {err}", src.display()))?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .map_err(|err| format!("Failed to strip prefix: {err}"))?;
        let target = dst.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .map_err(|err| format!("Failed to create dir {}: {err}", target.display()))?;
        } else {
            fs::copy(entry.path(), &target).map_err(|err| {
                format!(
                    "Failed to copy {} -> {}: {err}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
    }

    Ok(())
}

impl PublishedBatch {
    /// Make the published trees authoritative and discard the backup.
    pub(crate) fn commit(self) -> Result<(), String> {
        commit_backup(&self.backup)
    }

    /// Restore every pre-publication tree and configuration file.
    pub(crate) fn rollback(self) -> Result<(), String> {
        restore_backup(&self.backup)
    }

    pub(crate) fn rollback_with_error(self, primary: String) -> String {
        match self.rollback() {
            Ok(()) => primary,
            Err(rollback) => format!("{primary}; installation rollback failed: {rollback}"),
        }
    }
}

impl FileSnapshots {
    /// Recover any interrupted install, then open a new transaction by copying
    /// every configuration file in `paths` into a fresh backup directory.
    pub(crate) fn capture(targets: &[PathBuf], paths: &[PathBuf]) -> Result<Self, String> {
        recover_staged_batch(targets)?;
        let parent = batch_parent(targets)?;
        fs::create_dir_all(&parent).map_err(|err| {
            format!(
                "Failed to create deployment parent {}: {err}",
                parent.display()
            )
        })?;
        let backup = parent.join(BACKUP_DIR);
        fs::create_dir(&backup).map_err(|err| {
            format!(
                "Failed to create deployment backup {}: {err}",
                backup.display()
            )
        })?;
        set_private_directory_permissions(&backup)?;

        if let Err(err) = snapshot_files(&backup, paths) {
            return Err(match remove_path(&backup) {
                Ok(()) => err,
                Err(cleanup) => format!(
                    "{err}; failed to remove partial deployment backup {}: {cleanup}",
                    backup.display()
                ),
            });
        }

        Ok(Self {
            targets: targets.to_vec(),
            backup,
        })
    }

    /// Restore the captured configuration and any assets published by this transaction.
    pub(crate) fn restore(self) -> Result<(), String> {
        restore_backup(&self.backup)
    }

    pub(crate) fn restore_with_error(self, primary: String) -> String {
        match self.restore() {
            Ok(()) => primary,
            Err(rollback) => format!("{primary}; installation rollback failed: {rollback}"),
        }
    }

    /// Commit a configuration-only transaction that did not publish asset trees.
    pub(crate) fn commit(self) -> Result<(), String> {
        commit_backup(&self.backup)
    }
}

/// Recover a prior interrupted transaction before opening a new one: restore
/// everything recorded in a leftover backup directory, then sweep stale
/// staging directories left behind by crashes.
pub(crate) fn recover_staged_batch(targets: &[PathBuf]) -> Result<(), String> {
    let parent = batch_parent(targets)?;
    let backup = parent.join(BACKUP_DIR);
    if path_exists(&backup)? {
        // ponytail: a backup that cannot be restored (e.g. corrupt manifest)
        // fails closed and blocks installs until the directory is removed by
        // hand; bring back quarantine machinery if this generates support load.
        restore_backup(&backup)
            .map_err(|err| format!("Failed to recover interrupted deployment: {err}"))?;
    }
    cleanup_stale_staging(&parent)
}

/// Move every staged tree into place, saving whatever each target held before
/// into the transaction backup so failure or crash can restore it.
pub(crate) fn publish_staged_batch(
    mut stages: Vec<StagedDirectory>,
    snapshots: FileSnapshots,
) -> Result<PublishedBatch, String> {
    let FileSnapshots { targets, backup } = snapshots;
    let staged_targets: Vec<PathBuf> = stages.iter().map(|stage| stage.target.clone()).collect();
    if staged_targets != targets {
        let primary = "Staged deployment targets do not match the open transaction".to_string();
        return Err(fail_with_restore(primary, &backup));
    }
    match path_exists(&backup) {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "Deployment backup is missing at {}",
                backup.display()
            ));
        }
        Err(err) => return Err(fail_with_restore(err, &backup)),
    }
    let saved = backup.join(BACKUP_TARGETS_DIR);
    if let Err(err) = fs::create_dir_all(&saved) {
        let primary = format!("Failed to create {}: {err}", saved.display());
        return Err(fail_with_restore(primary, &backup));
    }

    for stage in &mut stages {
        if let Err(err) = publish_stage(stage, &saved) {
            return Err(fail_with_restore(err, &backup));
        }
        let staging = stage
            .staging
            .take()
            .expect("staging directory disappeared during publication");
        let _ = staging.keep();
    }

    Ok(PublishedBatch { backup })
}

/// Move the target aside into the backup (or record that it was absent),
/// then rename the staged tree into place.
fn publish_stage(stage: &StagedDirectory, saved: &Path) -> Result<(), String> {
    let name = stage.target.file_name().ok_or_else(|| {
        format!(
            "Cannot determine file name for deployment target {}",
            stage.target.display()
        )
    })?;
    if path_exists(&stage.target)? {
        fs::rename(&stage.target, saved.join(name))
            .map_err(|err| format!("Failed to back up {}: {err}", stage.target.display()))?;
    } else {
        let mut sentinel = name.to_os_string();
        sentinel.push(ABSENT_SUFFIX);
        fs::write(saved.join(sentinel), b"").map_err(|err| {
            format!(
                "Failed to record absent deployment target {}: {err}",
                stage.target.display()
            )
        })?;
    }
    fs::rename(stage.path(), &stage.target).map_err(|err| {
        format!(
            "Failed to publish staged directory to {}: {err}",
            stage.target.display()
        )
    })
}

pub(crate) fn validate_staged_mcp(
    mcp_root: &Path,
    context_preservation: bool,
) -> Result<(), String> {
    for relative in REQUIRED_MCP_FILES {
        require_staged_file(mcp_root, relative)?;
    }
    if context_preservation {
        require_staged_file(mcp_root, "tools/context.py")?;
    }
    Ok(())
}

fn require_staged_file(root: &Path, relative: &str) -> Result<(), String> {
    let path = root.join(relative);
    if path.is_file() {
        return Ok(());
    }
    Err(format!("Staged MCP file is missing at {}", path.display()))
}

/// Copy every configuration file into the backup and write the manifest last,
/// via rename, so a crash can never leave a half-written manifest.
fn snapshot_files(backup: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let mut files = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let entry = match fs::read(path) {
            Ok(contents) => {
                let data = format!("data-{index:04}");
                write_private_file(&backup.join(&data), &contents)?;
                BackupFile {
                    path: path.clone(),
                    data: Some(data),
                    mode: capture_mode(path),
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => BackupFile {
                path: path.clone(),
                data: None,
                mode: None,
            },
            Err(err) => return Err(format!("Failed to snapshot {}: {err}", path.display())),
        };
        files.push(entry);
    }

    let manifest = serde_json::to_vec(&BackupManifest {
        version: BACKUP_VERSION,
        files,
    })
    .map_err(|err| format!("Failed to serialize deployment backup manifest: {err}"))?;
    let scratch = backup.join("manifest.tmp");
    write_private_file(&scratch, &manifest)?;
    fs::rename(&scratch, backup.join(BACKUP_MANIFEST))
        .map_err(|err| format!("Failed to finalize deployment backup manifest: {err}"))
}

fn capture_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .ok()
            .map(|metadata| metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Restore every original recorded in `backup`: moved-aside target trees go
/// back, targets that were absent are removed, and configuration files get
/// their captured bytes and mode. Removes the backup directory only when
/// everything restored, so a partial restore can be retried.
fn restore_backup(backup: &Path) -> Result<(), String> {
    if !path_exists(backup)? {
        return Ok(());
    }
    let parent = backup.parent().ok_or_else(|| {
        format!(
            "Cannot determine parent for deployment backup {}",
            backup.display()
        )
    })?;

    let mut errors = Vec::new();
    restore_saved_targets(backup, parent, &mut errors);
    restore_snapshot_files(backup, &mut errors);

    if errors.is_empty() {
        remove_path(backup).map_err(|err| {
            format!(
                "Failed to remove deployment backup {}: {err}",
                backup.display()
            )
        })
    } else {
        Err(errors.join("; "))
    }
}

fn restore_saved_targets(backup: &Path, parent: &Path, errors: &mut Vec<String>) {
    let saved = backup.join(BACKUP_TARGETS_DIR);
    let entries = match fs::read_dir(&saved) {
        Ok(entries) => entries,
        // No target was ever moved aside: publish never ran.
        Err(err) if err.kind() == ErrorKind::NotFound => return,
        Err(err) => {
            errors.push(format!(
                "Failed to read saved targets {}: {err}",
                saved.display()
            ));
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if let Some(original) = name.to_string_lossy().strip_suffix(ABSENT_SUFFIX) {
            // The target did not exist before publish: remove what we installed.
            if let Err(err) = remove_path(&parent.join(original)) {
                errors.push(format!(
                    "Failed to remove published target {original}: {err}"
                ));
            }
        } else {
            let target = parent.join(&name);
            if let Err(err) = remove_path(&target) {
                errors.push(format!(
                    "Failed to remove published target {} before restore: {err}",
                    target.display()
                ));
                continue;
            }
            if let Err(err) = fs::rename(entry.path(), &target) {
                errors.push(format!("Failed to restore {}: {err}", target.display()));
            }
        }
    }
}

fn restore_snapshot_files(backup: &Path, errors: &mut Vec<String>) {
    let manifest_path = backup.join(BACKUP_MANIFEST);
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        // Crash before the manifest was finalized: nothing was mutated yet.
        Err(err) if err.kind() == ErrorKind::NotFound => return,
        Err(err) => {
            errors.push(format!(
                "Failed to read deployment backup manifest {}: {err}",
                manifest_path.display()
            ));
            return;
        }
    };
    let manifest: BackupManifest = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            errors.push(format!(
                "Failed to parse deployment backup manifest {}: {err}",
                manifest_path.display()
            ));
            return;
        }
    };
    if manifest.version != BACKUP_VERSION {
        errors.push(format!(
            "Unsupported deployment backup version {} at {}",
            manifest.version,
            manifest_path.display()
        ));
        return;
    }
    for file in manifest.files {
        if let Err(err) = restore_snapshot_file(backup, &file) {
            errors.push(err);
        }
    }
}

fn restore_snapshot_file(backup: &Path, file: &BackupFile) -> Result<(), String> {
    let Some(data) = &file.data else {
        return remove_path(&file.path).map_err(|err| {
            format!(
                "Failed to remove newly created {}: {err}",
                file.path.display()
            )
        });
    };
    let contents = fs::read(backup.join(data)).map_err(|err| {
        format!(
            "Failed to read snapshot data for {}: {err}",
            file.path.display()
        )
    })?;
    remove_path(&file.path).map_err(|err| {
        format!(
            "Failed to remove current {} before restore: {err}",
            file.path.display()
        )
    })?;
    create_parent(&file.path)?;
    fs::write(&file.path, &contents)
        .map_err(|err| format!("Failed to restore {}: {err}", file.path.display()))?;
    apply_mode(&file.path, file.mode)
}

fn apply_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|err| format!("Failed to restore permissions on {}: {err}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// Commit point: atomically rename the backup away, then delete it
/// best-effort (a leftover carries the staging prefix and is swept by the
/// next recovery). On failure the transaction is rolled back instead so the
/// tree is never left half-committed.
fn commit_backup(backup: &Path) -> Result<(), String> {
    let result = (|| {
        let parent = backup.parent().ok_or_else(|| {
            format!(
                "Cannot determine parent for deployment backup {}",
                backup.display()
            )
        })?;
        let trash = Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(parent)
            .map_err(|err| {
                format!(
                    "Failed to commit deployment transaction {}: {err}",
                    backup.display()
                )
            })?;
        fs::rename(backup, trash.path().join("committed")).map_err(|err| {
            format!(
                "Failed to commit deployment transaction {}: {err}",
                backup.display()
            )
        })
    })();
    result.map_err(|err| fail_with_restore(err, backup))
}

fn fail_with_restore(primary: String, backup: &Path) -> String {
    match restore_backup(backup) {
        Ok(()) => primary,
        Err(rollback) => format!("{primary}; rollback failed: {rollback}"),
    }
}

fn cleanup_stale_staging(parent: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "Failed to inspect deployment parent {}: {err}",
                parent.display()
            ));
        }
    };
    let mut errors = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            continue;
        }
        if let Err(err) = remove_path(&entry.path()) {
            errors.push(format!(
                "Failed to remove stale staging {}: {err}",
                entry.path().display()
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn batch_parent(targets: &[PathBuf]) -> Result<PathBuf, String> {
    let first = targets
        .first()
        .ok_or_else(|| "Cannot deploy an empty directory batch".to_string())?;
    let parent = first.parent().ok_or_else(|| {
        format!(
            "Cannot determine parent directory for deployment target {}",
            first.display()
        )
    })?;

    for target in &targets[1..] {
        if target.parent() != Some(parent) {
            return Err(format!(
                "Deployment targets must share parent {} but {} does not",
                parent.display(),
                target.display()
            ));
        }
    }

    Ok(parent.to_path_buf())
}

fn create_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create parent {}: {err}", parent.display()))
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("Failed to create private file {}: {err}", path.display()))?;
    file.write_all(contents)
        .map_err(|err| format!("Failed to write private file {}: {err}", path.display()))
}

fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
            format!(
                "Failed to secure deployment backup {}: {err}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Compute the current deployment stamp for a provider: a hash of every bundled
/// source tree combined with the caller's `inputs` signature (feature flags and
/// app version). `verify()` cannot see stale managed *contents*; this stamp can,
/// so startup repair can skip the full transactional reinstall when it matches.
pub(crate) fn deployment_stamp_current(roots: &[&Path], inputs: &str) -> Result<String, String> {
    let bundle_hash = hash_source_trees(roots)?;
    Ok(format!(
        "{DEPLOY_STAMP_VERSION}\u{1f}{inputs}\u{1f}{bundle_hash}"
    ))
}

/// Hash of the bundled source trees, walked deterministically so identical bytes
/// always produce an identical digest: relative paths are sorted, and each file
/// contributes its relative path, length, and bytes. The absolute root location
/// is deliberately excluded so the digest is stable across launches even when
/// resources mount at a different path each run (e.g. AppImage).
fn hash_source_trees(roots: &[&Path]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for root in roots {
        for (relative, path) in collect_files_sorted(root)? {
            let bytes = fs::read(&path)
                .map_err(|err| format!("Failed to hash bundled file {}: {err}", path.display()))?;
            hasher.update((relative.len() as u64).to_le_bytes());
            hasher.update(relative.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
        hasher.update([0xff]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_files_sorted(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(format!(
                    "Failed to read {} for hashing: {err}",
                    dir.display()
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|err| format!("Failed to read bundle entry: {err}"))?;
            let file_type = entry
                .file_type()
                .map_err(|err| format!("Failed to inspect bundle entry: {err}"))?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|err| format!("Failed to relativize bundle path: {err}"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((relative, path));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// True when the stored stamp at `parent` equals `current`. A missing or
/// unreadable stamp compares unequal, so repair falls back to a full reinstall.
pub(crate) fn deployment_stamp_matches(parent: &Path, current: &str) -> bool {
    fs::read_to_string(parent.join(DEPLOY_STAMP_FILE)).is_ok_and(|stored| stored == current)
}

/// Persist the deployment stamp after a successful commit so the next startup
/// repair can take the fast path. Callers treat failure as best-effort.
pub(crate) fn write_deployment_stamp(parent: &Path, current: &str) -> Result<(), String> {
    let path = parent.join(DEPLOY_STAMP_FILE);
    fs::write(&path, current)
        .map_err(|err| format!("Failed to write deployment stamp {}: {err}", path.display()))
}

pub(crate) fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("Failed to inspect {}: {err}", path.display())),
    }
}

pub(crate) fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    let file_type = metadata.file_type();
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if file_type.is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }

    if file_type.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn read_file(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// The transaction left nothing behind: no backup dir, no staging leftovers.
    fn assert_clean(parent: &Path) {
        assert!(!parent.join(BACKUP_DIR).exists(), "backup dir must be gone");
        for entry in fs::read_dir(parent).unwrap().flatten() {
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(STAGING_PREFIX),
                "staging leftover: {}",
                entry.path().display()
            );
        }
    }

    struct Harness {
        _dir: TempDir,
        parent: PathBuf,
        existing: PathBuf,
        absent: PathBuf,
        config: PathBuf,
        created: PathBuf,
    }

    /// Capture a transaction over one pre-existing and one absent target plus
    /// one config file, mutate the config, create a new config file, and
    /// publish staged replacements for both targets.
    fn publish_full_batch() -> (Harness, PublishedBatch) {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();
        let existing = parent.join("scripts");
        let absent = parent.join("templates");
        let config = parent.join("config.toml");
        let created = parent.join("agents.md");

        write_file(&existing.join("old.txt"), b"old");
        write_file(&config, b"original config");

        let snapshots = FileSnapshots::capture(
            &[existing.clone(), absent.clone()],
            &[config.clone(), created.clone()],
        )
        .unwrap();
        write_file(&config, b"new config");
        write_file(&created, b"created by install");

        let staged_existing = StagedDirectory::new(existing.clone()).unwrap();
        write_file(&staged_existing.path().join("new.txt"), b"new");
        let staged_absent = StagedDirectory::new(absent.clone()).unwrap();
        write_file(&staged_absent.path().join("fresh.txt"), b"fresh");
        let published =
            publish_staged_batch(vec![staged_existing, staged_absent], snapshots).unwrap();

        (
            Harness {
                _dir: dir,
                parent,
                existing,
                absent,
                config,
                created,
            },
            published,
        )
    }

    fn assert_restored(harness: &Harness) {
        assert_eq!(read_file(&harness.existing.join("old.txt")), "old");
        assert!(!harness.existing.join("new.txt").exists());
        assert!(!harness.absent.exists(), "absent target must be removed");
        assert_eq!(read_file(&harness.config), "original config");
        assert!(!harness.created.exists(), "created file must be removed");
        assert_clean(&harness.parent);
    }

    #[test]
    fn publish_and_commit_installs_all_targets_and_removes_backup() {
        let (harness, published) = publish_full_batch();

        published.commit().unwrap();

        assert_eq!(read_file(&harness.existing.join("new.txt")), "new");
        assert!(!harness.existing.join("old.txt").exists());
        assert_eq!(read_file(&harness.absent.join("fresh.txt")), "fresh");
        assert_eq!(read_file(&harness.config), "new config");
        assert_eq!(read_file(&harness.created), "created by install");
        assert_clean(&harness.parent);
    }

    #[test]
    fn rollback_after_publish_restores_originals() {
        let (harness, published) = publish_full_batch();

        published.rollback().unwrap();

        assert_restored(&harness);
    }

    #[test]
    fn crash_before_commit_recovers_and_is_idempotent() {
        let (harness, published) = publish_full_batch();
        drop(published); // crash before commit/rollback: the backup dir persists

        let targets = [harness.existing.clone(), harness.absent.clone()];
        recover_staged_batch(&targets).unwrap();
        assert_restored(&harness);

        // Running recovery again converges to a clean no-op.
        recover_staged_batch(&targets).unwrap();
        assert_restored(&harness);
    }

    #[test]
    fn configuration_only_commit_keeps_mutation_and_clears_backup() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("restart-transaction");
        let config = dir.path().join("settings.json");
        write_file(&config, b"old");

        let snapshots =
            FileSnapshots::capture(std::slice::from_ref(&target), std::slice::from_ref(&config))
                .unwrap();
        write_file(&config, b"new");
        snapshots.commit().unwrap();

        assert_eq!(read_file(&config), "new");
        assert_clean(dir.path());
    }

    #[cfg(unix)]
    #[test]
    fn config_restore_roundtrips_contents_and_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("scripts");
        let config = dir.path().join("config.toml");
        write_file(&config, b"original");
        fs::set_permissions(&config, fs::Permissions::from_mode(0o400)).unwrap();

        let snapshots =
            FileSnapshots::capture(std::slice::from_ref(&target), std::slice::from_ref(&config))
                .unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
        write_file(&config, b"modified");
        snapshots.restore().unwrap();

        assert_eq!(read_file(&config), "original");
        let mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400, "read-only permission must be restored");
        assert_clean(dir.path());
    }

    #[test]
    fn stale_staging_directories_are_removed() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();
        let target = parent.join("scripts");
        write_file(&target.join("live.txt"), b"live");
        let staging = parent.join(format!("{STAGING_PREFIX}xyz"));
        write_file(&staging.join("scratch"), b"scratch");

        recover_staged_batch(std::slice::from_ref(&target)).unwrap();

        assert!(!staging.exists());
        assert_eq!(read_file(&target.join("live.txt")), "live");
    }

    #[test]
    fn deployment_stamp_reflects_bundle_and_inputs() {
        let bundle_dir = TempDir::new().unwrap();
        let bundle = bundle_dir.path();
        write_file(&bundle.join("scripts/a.sh"), b"echo a");
        write_file(&bundle.join("mcp/server.py"), b"print('hi')");

        let stamp_dir = TempDir::new().unwrap();
        let parent = stamp_dir.path();

        let base = deployment_stamp_current(&[bundle], "v1|features").unwrap();
        assert_eq!(
            base,
            deployment_stamp_current(&[bundle], "v1|features").unwrap(),
            "identical tree and inputs hash deterministically"
        );

        assert!(!deployment_stamp_matches(parent, &base));
        write_deployment_stamp(parent, &base).unwrap();
        assert!(deployment_stamp_matches(parent, &base));

        let changed_inputs = deployment_stamp_current(&[bundle], "v2|features").unwrap();
        assert_ne!(changed_inputs, base);
        assert!(!deployment_stamp_matches(parent, &changed_inputs));

        write_file(&bundle.join("scripts/a.sh"), b"echo CHANGED");
        let changed_bundle = deployment_stamp_current(&[bundle], "v1|features").unwrap();
        assert_ne!(
            changed_bundle, base,
            "changed bundle bytes change the stamp"
        );
        assert!(!deployment_stamp_matches(parent, &changed_bundle));
    }
}
