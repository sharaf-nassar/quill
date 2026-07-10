use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

const STAGING_PREFIX: &str = ".quill-staging-";
const ABSENT_BACKUP_PREFIX: &str = ".quill-absent-backup-";
const BACKUP_SUFFIX: &str = ".quill-backup";
const TRANSACTION_MARKER: &str = ".quill-deploy-transaction";
const SNAPSHOT_DIRECTORY: &str = ".quill-provider-snapshots";
const SNAPSHOT_MANIFEST: &str = "manifest.json";
const SNAPSHOT_VERSION: u32 = 1;
const ABSENT_TARGET_SENTINEL: &str = ".quill-target-was-absent-7d37b2f46f3a4df9a8f239e7d32cfde1";
const QUARANTINE_PREFIX: &str = ".quill-recovery-quarantine-";
const QUARANTINE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
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

/// A published directory batch whose backups remain live until provider setup succeeds.
#[must_use = "published deployment batches must be committed or rolled back"]
pub(crate) struct PublishedBatch {
    targets: Vec<PathBuf>,
    marker: PathBuf,
}

/// Exact pre-install snapshots of provider configuration and instruction files.
#[must_use = "file snapshots must be retained until provider setup succeeds"]
pub(crate) struct FileSnapshots {
    targets: Vec<PathBuf>,
    marker: PathBuf,
}

struct FileSnapshot {
    path: PathBuf,
    state: FileSnapshotState,
}

enum FileSnapshotState {
    Missing,
    File {
        contents: Vec<u8>,
        permissions: PersistedPermissions,
    },
    Symlink {
        target: PathBuf,
        referent: SymlinkReferentSnapshot,
        is_directory: bool,
    },
}

enum SymlinkReferentSnapshot {
    Missing,
    File {
        contents: Vec<u8>,
        permissions: PersistedPermissions,
    },
    Directory,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedSnapshots {
    version: u32,
    files: Vec<PersistedFileSnapshot>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedFileSnapshot {
    path: PathBuf,
    state: PersistedFileSnapshotState,
}

#[derive(serde::Serialize, serde::Deserialize)]
enum PersistedFileSnapshotState {
    Missing,
    File {
        contents: String,
        permissions: PersistedPermissions,
    },
    Symlink {
        target: PathBuf,
        referent: PersistedSymlinkReferentSnapshot,
        is_directory: bool,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
enum PersistedSymlinkReferentSnapshot {
    Missing,
    File {
        contents: String,
        permissions: PersistedPermissions,
    },
    Directory,
}

#[derive(Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
struct PersistedPermissions {
    readonly: bool,
    unix_mode: Option<u32>,
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

impl PublishedBatch {
    /// Make the published trees authoritative, then clean their old backups best-effort.
    pub(crate) fn commit(self) -> Result<(), String> {
        if let Err(err) = remove_existing_path(&self.marker) {
            let primary = format!(
                "Failed to commit deployment transaction {}: {err}",
                self.marker.display()
            );
            return Err(with_rollback_error(primary, &self.targets, &self.marker));
        }

        cleanup_backups_best_effort(&self.targets);
        cleanup_snapshots_best_effort(&self.marker);
        Ok(())
    }

    /// Restore every pre-publication tree and retain the marker if rollback is incomplete.
    pub(crate) fn rollback(self) -> Result<(), String> {
        rollback_transaction(&self.targets, &self.marker)
    }

    pub(crate) fn rollback_with_error(self, primary: String) -> String {
        match self.rollback() {
            Ok(()) => primary,
            Err(rollback) => format!("{primary}; installation rollback failed: {rollback}"),
        }
    }
}

impl FileSnapshots {
    /// Recover any interrupted install, persist snapshots, then open a new transaction.
    pub(crate) fn capture(targets: &[PathBuf], paths: &[PathBuf]) -> Result<Self, String> {
        recover_staged_batch(targets)?;
        let parent = batch_parent(targets)?;
        fs::create_dir_all(&parent).map_err(|err| {
            format!(
                "Failed to create deployment parent {}: {err}",
                parent.display()
            )
        })?;
        let marker = parent.join(TRANSACTION_MARKER);
        let directory = parent.join(SNAPSHOT_DIRECTORY);

        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            if files
                .iter()
                .any(|snapshot: &FileSnapshot| snapshot.path == *path)
            {
                continue;
            }
            files.push(capture_file_snapshot(path)?);
        }

        if let Err(err) = persist_file_snapshots(&directory, &files) {
            return Err(match remove_path(&directory) {
                Ok(()) => err,
                Err(cleanup_err) => format!(
                    "{err}; failed to remove partial provider snapshots {}: {cleanup_err}",
                    directory.display()
                ),
            });
        }
        if let Err(err) = create_transaction_marker(&marker) {
            let cleanup = remove_path(&directory).map_err(|cleanup_err| {
                format!(
                    "Failed to remove unused provider snapshots {}: {cleanup_err}",
                    directory.display()
                )
            });
            return Err(match cleanup {
                Ok(()) => err,
                Err(cleanup_err) => format!("{err}; {cleanup_err}"),
            });
        }

        Ok(Self {
            targets: targets.to_vec(),
            marker,
        })
    }

    /// Restore the persisted snapshots and any assets published by this transaction.
    pub(crate) fn restore(self) -> Result<(), String> {
        rollback_transaction(&self.targets, &self.marker)
    }

    pub(crate) fn restore_with_error(self, primary: String) -> String {
        match self.restore() {
            Ok(()) => primary,
            Err(rollback) => format!("{primary}; installation rollback failed: {rollback}"),
        }
    }
}

/// Recover a prior batch before inspecting deployment sources or constructing new stages.
///
/// Recovery is non-destructive and converges: an unrollbackable transaction is
/// quarantined rather than left to wedge every future guarded mutation, and
/// leftover user-data-bearing artifacts are quarantined rather than deleted.
pub(crate) fn recover_staged_batch(targets: &[PathBuf]) -> Result<(), String> {
    let parent = batch_parent(targets)?;
    let marker = parent.join(TRANSACTION_MARKER);

    // Bound quarantine disk growth before touching the transaction; best-effort.
    prune_stale_quarantines(&parent);

    if path_exists(&marker)? {
        // A marker means a real interrupted transaction. Roll it back; if that
        // cannot complete, quarantine the whole transaction so the app stays
        // usable and the un-restored data stays recoverable by hand instead of
        // wedging every future guarded mutation behind a fail-closed guard.
        return match rollback_transaction(targets, &marker) {
            Ok(()) => Ok(()),
            Err(rollback_err) => match quarantine_transaction_state(&parent, targets, &marker) {
                Ok(Some(quarantine)) => {
                    log::error!(
                        "Failed to roll back interrupted deployment ({rollback_err}); quarantined transaction state at {} for manual recovery",
                        quarantine.display()
                    );
                    Ok(())
                }
                Ok(None) => Err(format!(
                    "Failed to recover interrupted deployment: {rollback_err}"
                )),
                Err(quarantine_err) => Err(format!(
                    "Failed to recover interrupted deployment: {rollback_err}; quarantine also failed: {quarantine_err}"
                )),
            },
        };
    }

    // No marker. Either a clean tree, a crash after the commit point, or a user
    // who deleted the marker mid-recovery — the last two are indistinguishable.
    // Staging trees only ever hold copies of bundled assets, so they are safe to
    // delete; backups and snapshots may be the only remaining copy of un-restored
    // provider configuration, so they are quarantined, never deleted.
    let mut errors = Vec::new();
    if let Err(err) = cleanup_stale_temporary_directories(&parent) {
        errors.push(err);
    }
    match quarantine_orphaned_state(&parent, targets) {
        Ok(Some(quarantine)) => log::error!(
            "Quarantined orphaned deployment backups without a transaction marker at {}; delete after confirming provider configuration is intact",
            quarantine.display()
        ),
        Ok(None) => {}
        Err(err) => errors.push(err),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Move every still-present artifact of an interrupted transaction — marker,
/// snapshots, live backups, and staging trees — into a fresh quarantine
/// directory. Returns the quarantine path, or `None` when nothing remained.
fn quarantine_transaction_state(
    parent: &Path,
    targets: &[PathBuf],
    marker: &Path,
) -> Result<Option<PathBuf>, String> {
    let mut sources = vec![marker.to_path_buf(), parent.join(SNAPSHOT_DIRECTORY)];
    for target in targets {
        sources.push(backup_path(target)?);
    }
    sources.extend(list_temporary_directories(parent)?);
    quarantine_paths(parent, &sources)
}

/// Quarantine leftover backups and snapshots found without a transaction marker.
/// Only user-data-bearing artifacts land here; staging trees are cleaned
/// separately so they are never mistaken for recoverable configuration.
fn quarantine_orphaned_state(
    parent: &Path,
    targets: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    let mut sources = vec![parent.join(SNAPSHOT_DIRECTORY)];
    for target in targets {
        sources.push(backup_path(target)?);
    }
    quarantine_paths(parent, &sources)
}

/// Rename each existing source into a unique quarantine directory beside the
/// batch parent. Non-existent sources are skipped; when none exist no directory
/// is created. Renames stay on one filesystem so quarantine cannot partially
/// copy a tree.
fn quarantine_paths(parent: &Path, sources: &[PathBuf]) -> Result<Option<PathBuf>, String> {
    let mut present = Vec::new();
    for source in sources {
        if path_exists(source)? {
            present.push(source);
        }
    }
    if present.is_empty() {
        return Ok(None);
    }

    let quarantine = Builder::new()
        .prefix(QUARANTINE_PREFIX)
        .tempdir_in(parent)
        .map_err(|err| {
            format!(
                "Failed to create recovery quarantine beside {}: {err}",
                parent.display()
            )
        })?;
    let quarantine_path = quarantine.path().to_path_buf();

    let mut errors = Vec::new();
    for source in present {
        let Some(name) = source.file_name() else {
            errors.push(format!(
                "Cannot quarantine path without a file name: {}",
                source.display()
            ));
            continue;
        };
        let destination = quarantine_path.join(name);
        if let Err(err) = fs::rename(source, &destination) {
            errors.push(format!(
                "Failed to quarantine {} into {}: {err}",
                source.display(),
                destination.display()
            ));
        }
    }

    let _ = quarantine.keep();
    if errors.is_empty() {
        Ok(Some(quarantine_path))
    } else {
        Err(errors.join("; "))
    }
}

/// Best-effort removal of quarantine directories older than the retention window
/// so repeatedly failing recoveries cannot grow unbounded. Never fails recovery;
/// ages by directory mtime and logs what it prunes.
fn prune_stale_quarantines(parent: &Path) {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(QUARANTINE_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        let aged = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= QUARANTINE_MAX_AGE);
        if aged {
            match remove_path(&path) {
                Ok(()) => log::info!("Pruned aged deployment quarantine {}", path.display()),
                Err(err) => log::warn!(
                    "Failed to prune aged deployment quarantine {}: {err}",
                    path.display()
                ),
            }
        }
    }
}

/// Publish complete staged trees while retaining their backups for the caller to finalize.
pub(crate) fn publish_staged_batch(
    mut stages: Vec<StagedDirectory>,
    snapshots: FileSnapshots,
) -> Result<PublishedBatch, String> {
    let targets: Vec<PathBuf> = stages.iter().map(|stage| stage.target.clone()).collect();
    let FileSnapshots {
        targets: transaction_targets,
        marker,
    } = snapshots;
    let parent = match batch_parent(&targets) {
        Ok(parent) => parent,
        Err(err) => return Err(with_rollback_error(err, &transaction_targets, &marker)),
    };
    if transaction_targets != targets {
        let primary = "Staged deployment targets do not match the open transaction".to_string();
        return Err(with_rollback_error(primary, &transaction_targets, &marker));
    }
    let marker_exists = match path_exists(&marker) {
        Ok(exists) => exists,
        Err(err) => return Err(with_rollback_error(err, &targets, &marker)),
    };
    if marker.parent() != Some(parent.as_path()) || !marker_exists {
        let primary = format!(
            "Deployment transaction marker is missing at {}",
            marker.display()
        );
        return Err(with_rollback_error(primary, &targets, &marker));
    }

    for target in &targets {
        if let Err(err) = prepare_backup(target, &parent) {
            let primary = format!(
                "Failed to prepare deployment target {}: {err}",
                target.display()
            );
            return Err(with_rollback_error(primary, &targets, &marker));
        }
    }

    for stage in &mut stages {
        let staging_path = stage.path().to_path_buf();
        if let Err(err) = fs::rename(&staging_path, &stage.target) {
            let primary = format!(
                "Failed to publish staged directory {} to {}: {err}",
                staging_path.display(),
                stage.target.display()
            );
            return Err(with_rollback_error(primary, &targets, &marker));
        }

        let staging = stage
            .staging
            .take()
            .expect("staging directory disappeared during publication");
        let _ = staging.keep();
    }

    Ok(PublishedBatch { targets, marker })
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

fn capture_file_snapshot(path: &Path) -> Result<FileSnapshot, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(FileSnapshot {
                path: path.to_path_buf(),
                state: FileSnapshotState::Missing,
            });
        }
        Err(err) => return Err(format!("Failed to inspect {}: {err}", path.display())),
    };
    let file_type = metadata.file_type();

    let state = if is_symbolic_link(&file_type) {
        let target = fs::read_link(path)
            .map_err(|err| format!("Failed to read symbolic link {}: {err}", path.display()))?;
        let referent = capture_symlink_referent(path)?;
        let is_directory = symlink_is_directory(&file_type, &referent);
        FileSnapshotState::Symlink {
            target,
            referent,
            is_directory,
        }
    } else if file_type.is_file() {
        FileSnapshotState::File {
            contents: fs::read(path)
                .map_err(|err| format!("Failed to snapshot {}: {err}", path.display()))?,
            permissions: PersistedPermissions::capture(&metadata.permissions()),
        }
    } else {
        return Err(format!(
            "Cannot snapshot non-file provider configuration at {}",
            path.display()
        ));
    };

    Ok(FileSnapshot {
        path: path.to_path_buf(),
        state,
    })
}

fn capture_symlink_referent(path: &Path) -> Result<SymlinkReferentSnapshot, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(SymlinkReferentSnapshot::Missing);
        }
        Err(err) => {
            return Err(format!(
                "Failed to inspect symbolic-link target for {}: {err}",
                path.display()
            ));
        }
    };

    if metadata.is_file() {
        return Ok(SymlinkReferentSnapshot::File {
            contents: fs::read(path).map_err(|err| {
                format!(
                    "Failed to snapshot symbolic-link target for {}: {err}",
                    path.display()
                )
            })?,
            permissions: PersistedPermissions::capture(&metadata.permissions()),
        });
    }
    if metadata.is_dir() {
        return Ok(SymlinkReferentSnapshot::Directory);
    }

    Err(format!(
        "Cannot snapshot special symbolic-link target for {}",
        path.display()
    ))
}

fn persist_file_snapshots(directory: &Path, files: &[FileSnapshot]) -> Result<(), String> {
    fs::create_dir(directory).map_err(|err| {
        format!(
            "Failed to create provider snapshot directory {}: {err}",
            directory.display()
        )
    })?;
    set_private_directory_permissions(directory)?;

    let mut next_data_file = 0usize;
    let mut persisted_files = Vec::with_capacity(files.len());
    for snapshot in files {
        let state = match &snapshot.state {
            FileSnapshotState::Missing => PersistedFileSnapshotState::Missing,
            FileSnapshotState::File {
                contents,
                permissions,
            } => PersistedFileSnapshotState::File {
                contents: persist_snapshot_contents(directory, &mut next_data_file, contents)?,
                permissions: permissions.clone(),
            },
            FileSnapshotState::Symlink {
                target,
                referent,
                is_directory,
            } => PersistedFileSnapshotState::Symlink {
                target: target.clone(),
                referent: match referent {
                    SymlinkReferentSnapshot::Missing => PersistedSymlinkReferentSnapshot::Missing,
                    SymlinkReferentSnapshot::File {
                        contents,
                        permissions,
                    } => PersistedSymlinkReferentSnapshot::File {
                        contents: persist_snapshot_contents(
                            directory,
                            &mut next_data_file,
                            contents,
                        )?,
                        permissions: permissions.clone(),
                    },
                    SymlinkReferentSnapshot::Directory => {
                        PersistedSymlinkReferentSnapshot::Directory
                    }
                },
                is_directory: *is_directory,
            },
        };
        persisted_files.push(PersistedFileSnapshot {
            path: snapshot.path.clone(),
            state,
        });
    }

    let manifest = serde_json::to_vec(&PersistedSnapshots {
        version: SNAPSHOT_VERSION,
        files: persisted_files,
    })
    .map_err(|err| format!("Failed to serialize provider snapshots: {err}"))?;
    write_private_file(&directory.join(SNAPSHOT_MANIFEST), &manifest)
}

fn persist_snapshot_contents(
    directory: &Path,
    next_data_file: &mut usize,
    contents: &[u8],
) -> Result<String, String> {
    let name = format!("data-{:04}.bin", *next_data_file);
    *next_data_file += 1;
    write_private_file(&directory.join(&name), contents)?;
    Ok(name)
}

fn restore_persisted_snapshots(marker: &Path) -> Result<(), String> {
    let directory = snapshot_directory(marker)?;
    if !path_exists(&directory)? {
        return Ok(());
    }
    let manifest_path = directory.join(SNAPSHOT_MANIFEST);
    let manifest_bytes = fs::read(&manifest_path).map_err(|err| {
        format!(
            "Failed to read provider snapshot manifest {}: {err}",
            manifest_path.display()
        )
    })?;
    let persisted: PersistedSnapshots = serde_json::from_slice(&manifest_bytes).map_err(|err| {
        format!(
            "Failed to parse provider snapshot manifest {}: {err}",
            manifest_path.display()
        )
    })?;
    if persisted.version != SNAPSHOT_VERSION {
        return Err(format!(
            "Unsupported provider snapshot version {} at {}",
            persisted.version,
            manifest_path.display()
        ));
    }

    let mut errors = Vec::new();
    for persisted_snapshot in persisted.files.into_iter().rev() {
        match load_file_snapshot(&directory, persisted_snapshot).and_then(restore_file_snapshot) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn load_file_snapshot(
    directory: &Path,
    snapshot: PersistedFileSnapshot,
) -> Result<FileSnapshot, String> {
    let state = match snapshot.state {
        PersistedFileSnapshotState::Missing => FileSnapshotState::Missing,
        PersistedFileSnapshotState::File {
            contents,
            permissions,
        } => FileSnapshotState::File {
            contents: read_snapshot_contents(directory, &contents)?,
            permissions,
        },
        PersistedFileSnapshotState::Symlink {
            target,
            referent,
            is_directory,
        } => FileSnapshotState::Symlink {
            target,
            referent: match referent {
                PersistedSymlinkReferentSnapshot::Missing => SymlinkReferentSnapshot::Missing,
                PersistedSymlinkReferentSnapshot::File {
                    contents,
                    permissions,
                } => SymlinkReferentSnapshot::File {
                    contents: read_snapshot_contents(directory, &contents)?,
                    permissions,
                },
                PersistedSymlinkReferentSnapshot::Directory => SymlinkReferentSnapshot::Directory,
            },
            is_directory,
        },
    };
    Ok(FileSnapshot {
        path: snapshot.path,
        state,
    })
}

fn read_snapshot_contents(directory: &Path, name: &str) -> Result<Vec<u8>, String> {
    let relative = Path::new(name);
    if relative.components().count() != 1
        || !matches!(relative.components().next(), Some(Component::Normal(_)))
    {
        return Err(format!("Invalid provider snapshot data path: {name}"));
    }
    let path = directory.join(relative);
    fs::read(&path).map_err(|err| {
        format!(
            "Failed to read provider snapshot data {}: {err}",
            path.display()
        )
    })
}

impl PersistedPermissions {
    fn capture(permissions: &fs::Permissions) -> Self {
        #[cfg(unix)]
        let unix_mode = {
            use std::os::unix::fs::PermissionsExt;
            Some(permissions.mode())
        };
        #[cfg(not(unix))]
        let unix_mode = None;

        Self {
            readonly: permissions.readonly(),
            unix_mode,
        }
    }

    fn apply(&self, path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            match self.unix_mode {
                Some(mode) => fs::Permissions::from_mode(mode),
                None => {
                    let mut permissions = fs::metadata(path)
                        .map_err(|err| {
                            format!("Failed to inspect permissions on {}: {err}", path.display())
                        })?
                        .permissions();
                    permissions.set_readonly(self.readonly);
                    permissions
                }
            }
        };
        #[cfg(not(unix))]
        let permissions = {
            let mut permissions = fs::metadata(path)
                .map_err(|err| {
                    format!("Failed to inspect permissions on {}: {err}", path.display())
                })?
                .permissions();
            permissions.set_readonly(self.readonly);
            permissions
        };

        fs::set_permissions(path, permissions)
            .map_err(|err| format!("Failed to restore permissions on {}: {err}", path.display()))
    }
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
                "Failed to secure provider snapshot directory {}: {err}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn restore_file_snapshot(snapshot: FileSnapshot) -> Result<(), String> {
    let FileSnapshot { path, state } = snapshot;
    match state {
        FileSnapshotState::Missing => remove_path(&path)
            .map_err(|err| format!("Failed to remove newly created {}: {err}", path.display())),
        FileSnapshotState::File {
            contents,
            permissions,
        } => {
            if regular_file_matches(&path, &contents, &permissions) {
                return Ok(());
            }
            remove_path(&path).map_err(|err| {
                format!(
                    "Failed to remove current {} before restore: {err}",
                    path.display()
                )
            })?;
            create_parent(&path)?;
            write_restored_file(&path, &contents, &permissions, true)
        }
        FileSnapshotState::Symlink {
            target,
            referent,
            is_directory,
        } => restore_symlink_snapshot(&path, &target, referent, is_directory),
    }
}

fn restore_symlink_snapshot(
    path: &Path,
    target: &Path,
    referent: SymlinkReferentSnapshot,
    is_directory: bool,
) -> Result<(), String> {
    if !symlink_matches(path, target, is_directory)? {
        remove_path(path).map_err(|err| {
            format!(
                "Failed to remove current {} before restoring symbolic link: {err}",
                path.display()
            )
        })?;
        create_parent(path)?;
        create_symbolic_link(target, path, is_directory).map_err(|err| {
            format!(
                "Failed to restore symbolic link {} -> {}: {err}",
                path.display(),
                target.display()
            )
        })?;
    }

    match referent {
        SymlinkReferentSnapshot::Missing => {
            let resolved = resolve_link_target(path, target);
            if lexically_normalize(&resolved) == lexically_normalize(path) {
                return Ok(());
            }
            restore_absent_referent(path, &resolved)
        }
        SymlinkReferentSnapshot::File {
            contents,
            permissions,
        } => {
            if followed_file_matches(path, &contents, &permissions) {
                return Ok(());
            }
            if fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
                return Err(format!(
                    "Cannot restore file contents through directory symbolic link {}",
                    path.display()
                ));
            }
            write_restored_file(path, &contents, &permissions, false).map_err(|err| {
                format!(
                    "Failed to restore symbolic-link target for {}: {err}",
                    path.display()
                )
            })
        }
        SymlinkReferentSnapshot::Directory => {
            if fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
                Ok(())
            } else {
                Err(format!(
                    "Symbolic-link target for {} is no longer a directory",
                    path.display()
                ))
            }
        }
    }
}

/// Restore a symbolic link whose referent was absent at capture (a dangling
/// link) by clearing whatever the install wrote through it.
///
/// Invariant: recovery may run days after a crash, and the installers only ever
/// write regular files through their managed links. A directory at the referent
/// therefore cannot be this transaction's doing (the user may have populated the
/// path meanwhile, e.g. cloned dotfiles); deleting it with `remove_dir_all`
/// would destroy data the transaction never wrote, so it is left in place and
/// the restore is treated as satisfied. A regular file or symlink is the exact
/// shape an install writes through the link, so removing it is bounded to
/// undoing this transaction's own effect.
fn restore_absent_referent(path: &Path, resolved: &Path) -> Result<(), String> {
    match fs::symlink_metadata(resolved) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            log::warn!(
                "Leaving directory at symbolic-link referent {} while restoring {}: refusing to delete data this deployment did not write",
                resolved.display(),
                path.display()
            );
            Ok(())
        }
        Ok(_) => remove_path(resolved).map_err(|err| {
            format!(
                "Failed to restore dangling symbolic link {}: {err}",
                path.display()
            )
        }),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "Failed to inspect symbolic-link referent {}: {err}",
            resolved.display()
        )),
    }
}

fn regular_file_matches(path: &Path, contents: &[u8], permissions: &PersistedPermissions) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_file()
        && fs::read(path).is_ok_and(|current| current == contents)
        && PersistedPermissions::capture(&metadata.permissions()) == *permissions
}

fn followed_file_matches(path: &Path, contents: &[u8], permissions: &PersistedPermissions) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file()
        && fs::read(path).is_ok_and(|current| current == contents)
        && PersistedPermissions::capture(&metadata.permissions()) == *permissions
}

fn create_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create parent {}: {err}", parent.display()))
}

fn write_restored_file(
    path: &Path,
    contents: &[u8],
    permissions: &PersistedPermissions,
    create_new: bool,
) -> Result<(), String> {
    if !create_new && let Ok(_metadata) = fs::metadata(path) {
        #[cfg(unix)]
        let temporary_permissions = {
            use std::os::unix::fs::PermissionsExt;
            fs::Permissions::from_mode(0o200)
        };
        #[cfg(not(unix))]
        let temporary_permissions = {
            let mut temporary_permissions = _metadata.permissions();
            temporary_permissions.set_readonly(false);
            temporary_permissions
        };
        fs::set_permissions(path, temporary_permissions).map_err(|err| {
            format!(
                "Failed to secure {} before restoring its contents: {err}",
                path.display()
            )
        })?;
    }

    let mut options = OpenOptions::new();
    options.write(true).truncate(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(permissions.unix_mode.unwrap_or(0o600) & 0o7777);
    }

    let mut file = options
        .open(path)
        .map_err(|err| format!("Failed to open {} for restore: {err}", path.display()))?;
    file.write_all(contents)
        .map_err(|err| format!("Failed to restore {}: {err}", path.display()))?;
    drop(file);
    permissions.apply(path)
}

fn resolve_link_target(path: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        path.parent().unwrap_or_else(|| Path::new("")).join(target)
    }
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized
                    .file_name()
                    .is_some_and(|name| name != OsStr::new(".."))
                {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn symlink_matches(path: &Path, target: &Path, is_directory: bool) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("Failed to inspect {}: {err}", path.display())),
    };
    let file_type = metadata.file_type();
    if !is_symbolic_link(&file_type) {
        return Ok(false);
    }
    let current_target = fs::read_link(path)
        .map_err(|err| format!("Failed to read symbolic link {}: {err}", path.display()))?;
    if current_target != target {
        return Ok(false);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        Ok(file_type.is_symlink_dir() == is_directory)
    }
    #[cfg(not(windows))]
    {
        let _ = is_directory;
        Ok(true)
    }
}

#[cfg(unix)]
fn create_symbolic_link(target: &Path, path: &Path, _is_directory: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn create_symbolic_link(target: &Path, path: &Path, is_directory: bool) -> std::io::Result<()> {
    if is_directory {
        std::os::windows::fs::symlink_dir(target, path)
    } else {
        std::os::windows::fs::symlink_file(target, path)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symbolic_link(_target: &Path, _path: &Path, _is_directory: bool) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "symbolic-link restoration is unsupported on this platform",
    ))
}

#[cfg(windows)]
fn is_symbolic_link(file_type: &fs::FileType) -> bool {
    use std::os::windows::fs::FileTypeExt;
    file_type.is_symlink_dir() || file_type.is_symlink_file()
}

#[cfg(not(windows))]
fn is_symbolic_link(file_type: &fs::FileType) -> bool {
    file_type.is_symlink()
}

#[cfg(windows)]
fn symlink_is_directory(file_type: &fs::FileType, _referent: &SymlinkReferentSnapshot) -> bool {
    use std::os::windows::fs::FileTypeExt;
    file_type.is_symlink_dir()
}

#[cfg(not(windows))]
fn symlink_is_directory(_file_type: &fs::FileType, referent: &SymlinkReferentSnapshot) -> bool {
    matches!(referent, SymlinkReferentSnapshot::Directory)
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

fn create_transaction_marker(marker: &Path) -> Result<(), String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .map(|_| ())
        .map_err(|err| {
            format!(
                "Failed to create deployment transaction marker {}: {err}",
                marker.display()
            )
        })
}

fn prepare_backup(target: &Path, parent: &Path) -> Result<(), String> {
    let backup = backup_path(target)?;
    if path_exists(&backup)? {
        return Err(format!(
            "Deployment backup already exists at {}",
            backup.display()
        ));
    }

    if path_exists(target)? {
        return fs::rename(target, &backup).map_err(|err| {
            format!(
                "Failed to move {} to backup {}: {err}",
                target.display(),
                backup.display()
            )
        });
    }

    create_absent_target_backup(&backup, parent)
}

fn create_absent_target_backup(backup: &Path, parent: &Path) -> Result<(), String> {
    let placeholder = Builder::new()
        .prefix(ABSENT_BACKUP_PREFIX)
        .tempdir_in(parent)
        .map_err(|err| {
            format!(
                "Failed to create absent-target backup beside {}: {err}",
                backup.display()
            )
        })?;
    let sentinel = placeholder.path().join(ABSENT_TARGET_SENTINEL);
    fs::write(&sentinel, b"quill deployment target was absent\n").map_err(|err| {
        format!(
            "Failed to write absent-target sentinel {}: {err}",
            sentinel.display()
        )
    })?;
    fs::rename(placeholder.path(), backup).map_err(|err| {
        format!(
            "Failed to publish absent-target backup {}: {err}",
            backup.display()
        )
    })?;
    let _ = placeholder.keep();
    Ok(())
}

fn rollback_transaction(targets: &[PathBuf], marker: &Path) -> Result<(), String> {
    let mut errors = Vec::new();

    for target in targets.iter().rev() {
        let backup = match backup_path(target) {
            Ok(backup) => backup,
            Err(err) => {
                errors.push(err);
                continue;
            }
        };

        let backup_exists = match path_exists(&backup) {
            Ok(exists) => exists,
            Err(err) => {
                errors.push(err);
                continue;
            }
        };
        if !backup_exists {
            continue;
        }

        match is_absent_target_backup(&backup) {
            Ok(true) => {
                if let Err(err) = remove_path(target) {
                    errors.push(format!(
                        "Failed to remove newly published target {}: {err}",
                        target.display()
                    ));
                    continue;
                }
                if let Err(err) = remove_path(&backup) {
                    errors.push(format!(
                        "Failed to remove absent-target backup {}: {err}",
                        backup.display()
                    ));
                }
            }
            Ok(false) => {
                if let Err(err) = remove_path(target) {
                    errors.push(format!(
                        "Failed to remove published target {} before restore: {err}",
                        target.display()
                    ));
                    continue;
                }
                if let Err(err) = fs::rename(&backup, target) {
                    errors.push(format!(
                        "Failed to restore backup {} to {}: {err}",
                        backup.display(),
                        target.display()
                    ));
                }
            }
            Err(err) => errors.push(err),
        }
    }

    if let Err(err) = restore_persisted_snapshots(marker) {
        errors.push(format!("Failed to restore provider configuration: {err}"));
    }

    let snapshot_directory = snapshot_directory(marker)?;
    if errors.is_empty()
        && let Err(err) = remove_path(&snapshot_directory)
    {
        errors.push(format!(
            "Failed to remove provider snapshots {}: {err}",
            snapshot_directory.display()
        ));
    }

    if errors.is_empty()
        && let Err(err) = remove_path(marker)
    {
        errors.push(format!(
            "Failed to remove deployment transaction marker {}: {err}",
            marker.display()
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn is_absent_target_backup(backup: &Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(backup)
        .map_err(|err| format!("Failed to inspect backup {}: {err}", backup.display()))?;
    if !metadata.file_type().is_dir() {
        return Ok(false);
    }

    let mut entries = fs::read_dir(backup)
        .map_err(|err| format!("Failed to read backup {}: {err}", backup.display()))?;
    let Some(entry) = entries.next() else {
        return Ok(false);
    };
    let entry = entry.map_err(|err| format!("Failed to read backup entry: {err}"))?;
    if entry.file_name() != OsStr::new(ABSENT_TARGET_SENTINEL) {
        return Ok(false);
    }
    let file_type = entry.file_type().map_err(|err| {
        format!(
            "Failed to inspect absent-target sentinel {}: {err}",
            entry.path().display()
        )
    })?;
    if !file_type.is_file() {
        return Ok(false);
    }

    match entries.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(err)) => Err(format!("Failed to read backup entry: {err}")),
    }
}

fn with_rollback_error(primary: String, targets: &[PathBuf], marker: &Path) -> String {
    match rollback_transaction(targets, marker) {
        Ok(()) => primary,
        Err(rollback) => format!("{primary}; rollback failed: {rollback}"),
    }
}

fn list_temporary_directories(parent: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(format!(
                "Failed to inspect deployment parent {} for stale staging directories: {err}",
                parent.display()
            ));
        }
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| format!("Failed to read deployment parent entry: {err}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(STAGING_PREFIX) || name.starts_with(ABSENT_BACKUP_PREFIX) {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

fn cleanup_stale_temporary_directories(parent: &Path) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in list_temporary_directories(parent)? {
        if let Err(err) = remove_path(&path) {
            errors.push(format!(
                "Failed to remove stale deployment path {}: {err}",
                path.display()
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn cleanup_backups_best_effort(targets: &[PathBuf]) {
    for target in targets {
        match backup_path(target) {
            Ok(backup) => {
                if let Err(err) = remove_path(&backup) {
                    log::warn!(
                        "Deployment committed but backup cleanup failed for {}: {err}",
                        backup.display()
                    );
                }
            }
            Err(err) => log::warn!("Deployment committed but backup path was invalid: {err}"),
        }
    }
}

fn cleanup_snapshots_best_effort(marker: &Path) {
    match snapshot_directory(marker) {
        Ok(directory) => {
            if let Err(err) = remove_path(&directory) {
                log::warn!(
                    "Deployment committed but snapshot cleanup failed for {}: {err}",
                    directory.display()
                );
            }
        }
        Err(err) => log::warn!("Deployment committed but snapshot path was invalid: {err}"),
    }
}

fn snapshot_directory(marker: &Path) -> Result<PathBuf, String> {
    marker
        .parent()
        .map(|parent| parent.join(SNAPSHOT_DIRECTORY))
        .ok_or_else(|| {
            format!(
                "Cannot determine snapshot directory for marker {}",
                marker.display()
            )
        })
}

fn backup_path(target: &Path) -> Result<PathBuf, String> {
    let file_name = target.file_name().ok_or_else(|| {
        format!(
            "Cannot determine file name for deployment target {}",
            target.display()
        )
    })?;
    let mut backup_name = OsString::from(".");
    backup_name.push(file_name);
    backup_name.push(BACKUP_SUFFIX);
    Ok(target.with_file_name(backup_name))
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

    remove_path_with_metadata(path, &metadata)
}

fn remove_existing_path(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    remove_path_with_metadata(path, &metadata)
}

fn remove_path_with_metadata(path: &Path, metadata: &fs::Metadata) -> std::io::Result<()> {
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

    fn find_quarantine(parent: &Path) -> Option<PathBuf> {
        fs::read_dir(parent)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(QUARANTINE_PREFIX))
            })
    }

    struct Interrupted {
        _dir: TempDir,
        parent: PathBuf,
        target: PathBuf,
        config: PathBuf,
        marker: PathBuf,
    }

    /// Set up a transaction that published a new tree and rewrote config, then
    /// "crashed" before commit — the marker, backup, and snapshots all remain.
    fn stage_interrupted_transaction() -> Interrupted {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();
        let target = parent.join("scripts");
        let config = parent.join("config.toml");

        write_file(&target.join("old.txt"), b"old");
        write_file(&config, b"original config");

        let snapshots =
            FileSnapshots::capture(std::slice::from_ref(&target), std::slice::from_ref(&config))
                .unwrap();
        let marker = parent.join(TRANSACTION_MARKER);
        assert!(marker.exists());

        write_file(&config, b"new config");
        let staged = StagedDirectory::new(target.clone()).unwrap();
        write_file(&staged.path().join("new.txt"), b"new");
        let published = publish_staged_batch(vec![staged], snapshots).unwrap();
        drop(published); // crash before commit/rollback: marker + backup persist

        Interrupted {
            _dir: dir,
            parent,
            target,
            config,
            marker,
        }
    }

    #[test]
    fn regular_file_capture_restore_roundtrips_contents_and_permissions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        write_file(&path, b"original");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        }

        let snapshot = capture_file_snapshot(&path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_file(&path, b"modified");

        restore_file_snapshot(snapshot).unwrap();

        assert_eq!(read_file(&path), "original");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o400, "read-only permission must be restored");
        }
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_restore_preserves_directory_at_referent() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("link");
        let referent = dir.path().join("referent");
        symlink("referent", &link).unwrap();

        let snapshot = capture_file_snapshot(&link).unwrap();

        // Days after the crash, the user populates the referent with real content.
        write_file(&referent.join("user_data.txt"), b"precious");

        restore_file_snapshot(snapshot).unwrap();

        assert!(referent.is_dir(), "directory at the referent must survive");
        assert_eq!(read_file(&referent.join("user_data.txt")), "precious");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_restore_removes_regular_file_at_referent() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("link");
        let referent = dir.path().join("referent");
        symlink("referent", &link).unwrap();

        let snapshot = capture_file_snapshot(&link).unwrap();

        // A regular file is the exact shape an install writes through the link.
        write_file(&referent, b"written-by-install");

        restore_file_snapshot(snapshot).unwrap();

        assert!(
            !referent.exists(),
            "regular file the install wrote is removed"
        );
    }

    #[test]
    fn marker_recovery_rolls_back_and_is_idempotent() {
        let harness = stage_interrupted_transaction();

        recover_staged_batch(std::slice::from_ref(&harness.target)).unwrap();

        assert_eq!(read_file(&harness.target.join("old.txt")), "old");
        assert!(!harness.target.join("new.txt").exists());
        assert_eq!(read_file(&harness.config), "original config");
        assert!(!harness.marker.exists());
        assert!(!backup_path(&harness.target).unwrap().exists());
        assert!(!harness.parent.join(SNAPSHOT_DIRECTORY).exists());
        assert!(find_quarantine(&harness.parent).is_none());

        // Running recovery again converges to a clean no-op.
        recover_staged_batch(std::slice::from_ref(&harness.target)).unwrap();
        assert_eq!(read_file(&harness.target.join("old.txt")), "old");
        assert_eq!(read_file(&harness.config), "original config");
        assert!(find_quarantine(&harness.parent).is_none());
    }

    #[test]
    fn failed_rollback_quarantines_instead_of_deleting() {
        let harness = stage_interrupted_transaction();

        // Induce a persistent restore failure: corrupt the snapshot manifest so
        // the config restore cannot parse and rollback cannot complete.
        let manifest = harness
            .parent
            .join(SNAPSHOT_DIRECTORY)
            .join(SNAPSHOT_MANIFEST);
        write_file(&manifest, b"{ not valid json");

        // Recovery converges to Ok despite the unrollbackable transaction.
        recover_staged_batch(std::slice::from_ref(&harness.target)).unwrap();

        let quarantine =
            find_quarantine(&harness.parent).expect("transaction state must be quarantined");
        assert!(
            quarantine
                .join(SNAPSHOT_DIRECTORY)
                .join(SNAPSHOT_MANIFEST)
                .exists(),
            "the snapshot backup data must survive inside the quarantine"
        );
        assert!(quarantine.join(TRANSACTION_MARKER).exists());
        // Nothing was deleted at the batch parent; the state moved wholesale.
        assert!(!harness.marker.exists());
        assert!(!harness.parent.join(SNAPSHOT_DIRECTORY).exists());

        // A subsequent recovery returns Ok (the wedge is gone).
        recover_staged_batch(std::slice::from_ref(&harness.target)).unwrap();
    }

    #[test]
    fn orphaned_backups_are_quarantined_not_deleted() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();
        let target = parent.join("scripts");

        // Leftover backup + snapshots with no marker — indistinguishable from a
        // user who deleted the marker, so they must be preserved.
        let backup = backup_path(&target).unwrap();
        write_file(&backup.join("old.txt"), b"precious");
        let snapshot_dir = parent.join(SNAPSHOT_DIRECTORY);
        write_file(&snapshot_dir.join(SNAPSHOT_MANIFEST), b"{}");
        let staging = parent.join(format!("{STAGING_PREFIX}abcd"));
        write_file(&staging.join("scratch"), b"scratch");

        recover_staged_batch(std::slice::from_ref(&target)).unwrap();

        assert!(!staging.exists(), "stale staging is deleted");
        assert!(!backup.exists(), "backup is moved out of the batch parent");
        assert!(
            !snapshot_dir.exists(),
            "snapshots are moved out of the parent"
        );

        let quarantine = find_quarantine(&parent).expect("leftovers must be quarantined");
        assert_eq!(
            read_file(&quarantine.join(".scripts.quill-backup").join("old.txt")),
            "precious"
        );
        assert!(
            quarantine
                .join(SNAPSHOT_DIRECTORY)
                .join(SNAPSHOT_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn stale_staging_directories_are_removed_without_quarantine() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();
        let target = parent.join("scripts");
        write_file(&target.join("live.txt"), b"live");

        let staging = parent.join(format!("{STAGING_PREFIX}xyz"));
        write_file(&staging.join("scratch"), b"scratch");
        let absent = parent.join(format!("{ABSENT_BACKUP_PREFIX}xyz"));
        write_file(&absent.join("scratch"), b"scratch");

        recover_staged_batch(std::slice::from_ref(&target)).unwrap();

        assert!(!staging.exists());
        assert!(!absent.exists());
        // No user-data-bearing leftovers, so no quarantine is created.
        assert!(find_quarantine(&parent).is_none());
        assert_eq!(read_file(&target.join("live.txt")), "live");
    }

    #[test]
    fn absent_target_backup_is_discriminated_from_real_backups() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();

        let sentinel_backup = parent.join(".scripts.quill-backup");
        create_absent_target_backup(&sentinel_backup, &parent).unwrap();
        assert!(is_absent_target_backup(&sentinel_backup).unwrap());

        let real_backup = parent.join(".templates.quill-backup");
        write_file(&real_backup.join("content.txt"), b"data");
        assert!(!is_absent_target_backup(&real_backup).unwrap());

        let decoy = parent.join(".decoy.quill-backup");
        write_file(&decoy.join(ABSENT_TARGET_SENTINEL), b"x");
        write_file(&decoy.join("extra.txt"), b"y");
        assert!(!is_absent_target_backup(&decoy).unwrap());
    }

    #[test]
    fn absent_target_publish_rolls_back_to_absent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().to_path_buf();
        let target = parent.join("scripts");
        assert!(!target.exists());

        let snapshots = FileSnapshots::capture(std::slice::from_ref(&target), &[]).unwrap();
        let staged = StagedDirectory::new(target.clone()).unwrap();
        write_file(&staged.path().join("new.txt"), b"new");
        let published = publish_staged_batch(vec![staged], snapshots).unwrap();

        assert!(target.join("new.txt").exists());
        assert!(is_absent_target_backup(&backup_path(&target).unwrap()).unwrap());

        published.rollback().unwrap();

        assert!(!target.exists(), "absent state must be restored");
        assert!(!backup_path(&target).unwrap().exists());
        assert!(!parent.join(TRANSACTION_MARKER).exists());
        assert!(!parent.join(SNAPSHOT_DIRECTORY).exists());
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
