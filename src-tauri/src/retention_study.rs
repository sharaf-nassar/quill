//! Private, maintainer-only retention corpus study protocol.
//!
//! This module deliberately has no Tauri command, IPC type, product setting,
//! discovery, or network path.  It accepts only explicit local paths, opens
//! the source read-only, proves it did not change, and mutates fresh page
//! backups inside a private workspace.

use crate::retention_engine::{self, RetentionDeleteControls, RetentionDeleteRequest};
use crate::storage::{MAX_SUPPORTED_SCHEMA_VERSION, Storage};
use chrono::{DateTime, Utc};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MANIFEST_VERSION: u32 = 1;
const PAGE_BATCH: i32 = 128;
const PRIVATE_MANIFEST: &str = "private-manifest.json";

/// A stable, display-safe failure code for the internal study protocol.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StudyErrorCode {
    ApprovalIncomplete,
    ApprovalRevoked,
    ApprovalExpired,
    SourceInvalid,
    SourceSchemaUnrecognized,
    SourceSchemaTooNew,
    SourceChanged,
    SourceAlias,
    OutputCollision,
    UnsafePermissions,
    DiskPreflight,
    Cancelled,
    ManifestInvalid,
    PrivacySignoffRequired,
    MeasurementUnavailable,
}

/// Expected study failures retain internal detail without exposing it in a
/// scrubbed report. Callers can publish only [`StudyError::code`].
#[derive(Debug)]
pub struct StudyError {
    pub code: StudyErrorCode,
    detail: String,
}

impl StudyError {
    fn new(code: StudyErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for StudyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for StudyError {}

/// Explicit human authorization captured before a source can be opened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub corpus_label: String,
    pub owner_or_custodian: String,
    pub authorized_operator: String,
    pub reviewers: Vec<String>,
    pub approved_by: String,
    pub approved_at: String,
    pub expires_at: String,
    pub revoked: bool,
    pub allow_scratch_replay: bool,
    pub allow_aggregate_publication: bool,
    pub cleanup_deadline: String,
    pub independence_rationale: String,
    pub fixed_utc_anchor: String,
    pub retention_window_days: i64,
    pub quill_stopped_acknowledged: bool,
    pub concurrent_load_policy: String,
    pub prior_prune_state: String,
    pub prior_rollup_state: String,
    pub prior_archive_state: String,
}

impl ApprovalRecord {
    fn validate(&self) -> Result<(), StudyError> {
        let required = [
            ("corpus_label", &self.corpus_label),
            ("owner_or_custodian", &self.owner_or_custodian),
            ("authorized_operator", &self.authorized_operator),
            ("approved_by", &self.approved_by),
            ("approved_at", &self.approved_at),
            ("expires_at", &self.expires_at),
            ("cleanup_deadline", &self.cleanup_deadline),
            ("independence_rationale", &self.independence_rationale),
            ("fixed_utc_anchor", &self.fixed_utc_anchor),
            ("concurrent_load_policy", &self.concurrent_load_policy),
            ("prior_prune_state", &self.prior_prune_state),
            ("prior_rollup_state", &self.prior_rollup_state),
            ("prior_archive_state", &self.prior_archive_state),
        ];
        if required.iter().any(|(_, value)| value.trim().is_empty())
            || self.reviewers.is_empty()
            || self.retention_window_days <= 0
            || !self.quill_stopped_acknowledged
        {
            return Err(StudyError::new(
                StudyErrorCode::ApprovalIncomplete,
                "required approval field is absent",
            ));
        }
        if self.revoked {
            return Err(StudyError::new(
                StudyErrorCode::ApprovalRevoked,
                "approval record is revoked",
            ));
        }
        let expiry = DateTime::parse_from_rfc3339(&self.expires_at).map_err(|_| {
            StudyError::new(
                StudyErrorCode::ApprovalIncomplete,
                "expires_at is not RFC3339",
            )
        })?;
        if expiry.with_timezone(&Utc) <= Utc::now() {
            return Err(StudyError::new(
                StudyErrorCode::ApprovalExpired,
                "approval record has expired",
            ));
        }
        DateTime::parse_from_rfc3339(&self.approved_at).map_err(|_| {
            StudyError::new(
                StudyErrorCode::ApprovalIncomplete,
                "approved_at is not RFC3339",
            )
        })?;
        DateTime::parse_from_rfc3339(&self.fixed_utc_anchor).map_err(|_| {
            StudyError::new(
                StudyErrorCode::ApprovalIncomplete,
                "fixed_utc_anchor is not RFC3339",
            )
        })?;
        Ok(())
    }
}

/// Every source and sidecar is hashed before and after source reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileInventory {
    pub label: String,
    pub present: bool,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
    pub identity: Option<String>,
}

/// Source facts proved without migration or any source write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInventory {
    pub source_path: String,
    pub schema_version: i32,
    pub page_size: u64,
    pub page_count: u64,
    pub files: Vec<FileInventory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRecord {
    pub scratch_path: String,
    pub pages: u64,
    pub page_size: u64,
    pub elapsed_ms: u64,
    pub integrity_check: String,
}

/// Private on-disk evidence. It is intentionally not a report format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateManifest {
    pub schema_version: u32,
    pub lifecycle: String,
    pub created_at: String,
    pub approval: ApprovalRecord,
    pub source: SourceInventory,
    pub source_after_reads: SourceInventory,
    pub profile_backup: BackupRecord,
    pub cleanup_disposition: String,
    pub matrix: Vec<ReplayObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayObservation {
    pub archive_mode: String,
    pub cache_state: String,
    pub status: String,
    pub duration_ms: u64,
    pub reason_code: Option<String>,
}

/// A cooperative cancellation marker, polled between bounded SQLite work.
#[derive(Clone)]
pub struct StudyCancellation(Arc<AtomicBool>);

impl StudyCancellation {
    pub fn from_marker(marker: &Path) -> Self {
        let marker = marker.to_path_buf();
        let cancelled = Arc::new(AtomicBool::new(marker.exists()));
        let watcher = Arc::clone(&cancelled);
        thread::spawn(move || {
            while !watcher.load(Ordering::Relaxed) {
                if marker.exists() {
                    watcher.store(true, Ordering::Relaxed);
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        Self(cancelled)
    }

    pub fn check(&self) -> Result<(), StudyError> {
        if self.0.load(Ordering::Relaxed) {
            Err(StudyError::new(
                StudyErrorCode::Cancelled,
                "cancellation marker observed",
            ))
        } else {
            Ok(())
        }
    }
}

pub struct ProfileRequest<'a> {
    pub approval: ApprovalRecord,
    pub source: &'a Path,
    pub workspace: &'a Path,
    pub cancellation: StudyCancellation,
}

pub struct ReplayRequest<'a> {
    pub manifest: &'a Path,
    pub workspace: &'a Path,
    pub cancellation: StudyCancellation,
}

pub struct ReportRequest<'a> {
    pub manifest: &'a Path,
    pub output: &'a Path,
    pub privacy_signoff: bool,
}

pub struct SyntheticSmokeRequest<'a> {
    pub workspace: &'a Path,
}

pub struct DbstatRequest<'a> {
    pub manifest: &'a Path,
    pub scratch: &'a Path,
    pub cancellation: StudyCancellation,
}

fn io_error(code: StudyErrorCode, action: &str, error: std::io::Error) -> StudyError {
    StudyError::new(code, format!("{action}: {error}"))
}

fn canonical_existing(path: &Path, code: StudyErrorCode) -> Result<PathBuf, StudyError> {
    fs::canonicalize(path).map_err(|error| io_error(code, "canonicalize path", error))
}

fn identity(path: &Path) -> Result<String, StudyError> {
    let metadata = fs::metadata(path)
        .map_err(|error| io_error(StudyErrorCode::SourceInvalid, "stat path", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Ok(format!(
            "fallback:{}:{}",
            metadata.len(),
            metadata
                .modified()
                .map_err(|error| io_error(
                    StudyErrorCode::SourceInvalid,
                    "read modification time",
                    error
                ))?
                .elapsed()
                .map_err(|error| StudyError::new(StudyErrorCode::SourceInvalid, error.to_string()))?
                .as_nanos()
        ))
    }
}

fn reject_unsafe_permissions(path: &Path) -> Result<(), StudyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|error| io_error(StudyErrorCode::SourceInvalid, "stat permissions", error))?
            .permissions()
            .mode();
        if mode & 0o022 != 0 {
            return Err(StudyError::new(
                StudyErrorCode::UnsafePermissions,
                "group or world writable path",
            ));
        }
    }
    Ok(())
}

fn set_private_permissions(path: &Path, directory: bool) -> Result<(), StudyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .map_err(|error| {
            io_error(
                StudyErrorCode::UnsafePermissions,
                "set private permissions",
                error,
            )
        })?;
        let mode = fs::metadata(path)
            .map_err(|error| {
                io_error(
                    StudyErrorCode::UnsafePermissions,
                    "verify private permissions",
                    error,
                )
            })?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(StudyError::new(
                StudyErrorCode::UnsafePermissions,
                "private permission verification failed",
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, directory);
        return Err(StudyError::new(
            StudyErrorCode::UnsafePermissions,
            "owner-only ACL verification is unavailable on this platform",
        ));
    }
    Ok(())
}

fn prepare_workspace(workspace: &Path) -> Result<PathBuf, StudyError> {
    if workspace.exists() {
        if !workspace.is_dir() {
            return Err(StudyError::new(
                StudyErrorCode::OutputCollision,
                "workspace is not a directory",
            ));
        }
    } else {
        fs::create_dir_all(workspace).map_err(|error| {
            io_error(StudyErrorCode::OutputCollision, "create workspace", error)
        })?;
    }
    let canonical = canonical_existing(workspace, StudyErrorCode::OutputCollision)?;
    set_private_permissions(&canonical, true)?;
    Ok(canonical)
}

fn sidecar_paths(source: &Path) -> [PathBuf; 3] {
    [
        source.to_path_buf(),
        PathBuf::from(format!("{}-wal", source.display())),
        PathBuf::from(format!("{}-shm", source.display())),
    ]
}

fn fingerprint(path: &Path, label: &str) -> Result<FileInventory, StudyError> {
    if !path.exists() {
        return Ok(FileInventory {
            label: label.into(),
            present: false,
            bytes: None,
            sha256: None,
            identity: None,
        });
    }
    reject_unsafe_permissions(path)?;
    let mut file = File::open(path).map_err(|error| {
        io_error(
            StudyErrorCode::SourceInvalid,
            "open source component",
            error,
        )
    })?;
    let mut hash = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|error| {
            io_error(
                StudyErrorCode::SourceInvalid,
                "hash source component",
                error,
            )
        })?;
        if read == 0 {
            break;
        }
        hash.update(&buf[..read]);
    }
    let metadata = fs::metadata(path).map_err(|error| {
        io_error(
            StudyErrorCode::SourceInvalid,
            "stat source component",
            error,
        )
    })?;
    Ok(FileInventory {
        label: label.into(),
        present: true,
        bytes: Some(metadata.len()),
        sha256: Some(hex::encode(hash.finalize())),
        identity: Some(identity(path)?),
    })
}

fn source_inventory(source: &Path) -> Result<SourceInventory, StudyError> {
    let source = canonical_existing(source, StudyErrorCode::SourceInvalid)?;
    let paths = sidecar_paths(&source);
    let files = vec![
        fingerprint(&paths[0], "usage.db")?,
        fingerprint(&paths[1], "usage.db-wal")?,
        fingerprint(&paths[2], "usage.db-shm")?,
    ];
    let conn = open_source_read_only(&source, files[1].present, files[2].present)?;
    let recognized: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('settings', 'schema_version')", [], |row| row.get(0))
        .map_err(|error| StudyError::new(StudyErrorCode::SourceInvalid, format!("inspect SQLite schema: {error}")))?;
    if recognized != 2 {
        return Err(StudyError::new(
            StudyErrorCode::SourceSchemaUnrecognized,
            "source is not a recognizable Quill database",
        ));
    }
    let version: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            StudyError::new(
                StudyErrorCode::SourceSchemaUnrecognized,
                format!("read source schema version: {error}"),
            )
        })?;
    if version > MAX_SUPPORTED_SCHEMA_VERSION {
        return Err(StudyError::new(
            StudyErrorCode::SourceSchemaTooNew,
            format!("source schema version {version} exceeds {MAX_SUPPORTED_SCHEMA_VERSION}"),
        ));
    }
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| StudyError::new(StudyErrorCode::SourceInvalid, error.to_string()))?;
    let page_count: u64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|error| StudyError::new(StudyErrorCode::SourceInvalid, error.to_string()))?;
    Ok(SourceInventory {
        source_path: source.display().to_string(),
        schema_version: version,
        page_size,
        page_count,
        files,
    })
}

/// Open without allowing SQLite to create WAL/SHM sidecars. A stopped source
/// without a WAL can safely use immutable mode; a WAL source must already have
/// its SHM sidecar or it is treated as unsafe rather than repaired in place.
fn open_source_read_only(
    source: &Path,
    wal_present: bool,
    shm_present: bool,
) -> Result<Connection, StudyError> {
    if wal_present && !shm_present {
        return Err(StudyError::new(
            StudyErrorCode::SourceInvalid,
            "WAL source has no SHM sidecar; refusing to create one",
        ));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let uri = if wal_present {
        format!("file:{}?mode=ro", sqlite_uri_path(source))
    } else {
        format!("file:{}?mode=ro&immutable=1", sqlite_uri_path(source))
    };
    Connection::open_with_flags(uri, flags).map_err(|error| {
        StudyError::new(
            StudyErrorCode::SourceInvalid,
            format!("open read-only SQLite source: {error}"),
        )
    })
}

fn sqlite_uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%25")
        .replace('#', "%23")
        .replace('?', "%3F")
        .replace(' ', "%20")
}

fn ensure_source_unchanged(before: &SourceInventory) -> Result<SourceInventory, StudyError> {
    let after = source_inventory(Path::new(&before.source_path))?;
    if before.files != after.files {
        return Err(StudyError::new(
            StudyErrorCode::SourceChanged,
            format!(
                "source or WAL/SHM sidecar changed during source reads: {:?} -> {:?}",
                before.files, after.files
            ),
        ));
    }
    Ok(after)
}

fn reject_aliases(source: &Path, outputs: &[&Path]) -> Result<(), StudyError> {
    let source_identity = identity(source)?;
    for output in outputs {
        if output.exists() && identity(output)? == source_identity {
            return Err(StudyError::new(
                StudyErrorCode::SourceAlias,
                "source and output refer to the same filesystem object",
            ));
        }
    }
    Ok(())
}

fn create_empty_private(path: &Path) -> Result<File, StudyError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            io_error(
                StudyErrorCode::OutputCollision,
                "create private output",
                error,
            )
        })?;
    set_private_permissions(path, false)?;
    Ok(file)
}

fn page_backup(
    source: &Path,
    scratch: &Path,
    cancellation: &StudyCancellation,
) -> Result<BackupRecord, StudyError> {
    cancellation.check()?;
    create_empty_private(scratch)?;
    let source_conn = open_source_read_only(
        source,
        PathBuf::from(format!("{}-wal", source.display())).exists(),
        PathBuf::from(format!("{}-shm", source.display())).exists(),
    )?;
    let mut scratch_conn = Connection::open(scratch).map_err(|error| {
        StudyError::new(
            StudyErrorCode::OutputCollision,
            format!("open scratch backup: {error}"),
        )
    })?;
    let began = Instant::now();
    let backup = Backup::new(&source_conn, &mut scratch_conn).map_err(|error| {
        StudyError::new(
            StudyErrorCode::SourceInvalid,
            format!("start SQLite page backup: {error}"),
        )
    })?;
    loop {
        cancellation.check()?;
        match backup.step(PAGE_BATCH).map_err(|error| {
            StudyError::new(
                StudyErrorCode::SourceInvalid,
                format!("step SQLite page backup: {error}"),
            )
        })? {
            StepResult::Done => break,
            StepResult::More => {}
            StepResult::Busy | StepResult::Locked => thread::sleep(Duration::from_millis(25)),
            _ => {
                return Err(StudyError::new(
                    StudyErrorCode::SourceInvalid,
                    "unknown SQLite backup outcome",
                ));
            }
        }
    }
    drop(backup);
    let integrity: String = scratch_conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| {
            StudyError::new(
                StudyErrorCode::SourceInvalid,
                format!("check scratch integrity: {error}"),
            )
        })?;
    if integrity != "ok" {
        return Err(StudyError::new(
            StudyErrorCode::SourceInvalid,
            "scratch integrity check failed",
        ));
    }
    let page_size: u64 = scratch_conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| StudyError::new(StudyErrorCode::SourceInvalid, error.to_string()))?;
    let pages: u64 = scratch_conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|error| StudyError::new(StudyErrorCode::SourceInvalid, error.to_string()))?;
    Ok(BackupRecord {
        scratch_path: scratch.display().to_string(),
        pages,
        page_size,
        elapsed_ms: began.elapsed().as_millis() as u64,
        integrity_check: integrity,
    })
}

fn write_manifest(path: &Path, manifest: &PrivateManifest) -> Result<(), StudyError> {
    let mut output = create_empty_private(path)?;
    serde_json::to_writer_pretty(&mut output, manifest).map_err(|error| {
        StudyError::new(
            StudyErrorCode::ManifestInvalid,
            format!("serialize private manifest: {error}"),
        )
    })?;
    output.write_all(b"\n").map_err(|error| {
        io_error(
            StudyErrorCode::ManifestInvalid,
            "write private manifest",
            error,
        )
    })
}

fn read_manifest(path: &Path) -> Result<PrivateManifest, StudyError> {
    let file = File::open(path).map_err(|error| {
        io_error(
            StudyErrorCode::ManifestInvalid,
            "open private manifest",
            error,
        )
    })?;
    let manifest: PrivateManifest = serde_json::from_reader(file).map_err(|error| {
        StudyError::new(
            StudyErrorCode::ManifestInvalid,
            format!("parse private manifest: {error}"),
        )
    })?;
    if manifest.schema_version != MANIFEST_VERSION || manifest.lifecycle.is_empty() {
        return Err(StudyError::new(
            StudyErrorCode::ManifestInvalid,
            "unsupported or incomplete private manifest",
        ));
    }
    manifest.approval.validate()?;
    Ok(manifest)
}

/// Profile a source with a read-only inventory and a page-preserving scratch
/// backup. The scratch copy is migrated only to validate the scratch route and
/// is removed before returning; the private manifest remains for replay.
pub fn profile_source(request: ProfileRequest<'_>) -> Result<PathBuf, StudyError> {
    request.approval.validate()?;
    request.cancellation.check()?;
    let workspace = prepare_workspace(request.workspace)?;
    let source = canonical_existing(request.source, StudyErrorCode::SourceInvalid)?;
    let manifest_path = workspace.join(PRIVATE_MANIFEST);
    let scratch = workspace.join("profile-scratch.db");
    if manifest_path.exists() || scratch.exists() {
        return Err(StudyError::new(
            StudyErrorCode::OutputCollision,
            "profile output already exists",
        ));
    }
    reject_aliases(&source, &[&manifest_path, &scratch])?;
    let before = source_inventory(&source)?;
    let estimated = before
        .page_size
        .checked_mul(before.page_count)
        .ok_or_else(|| {
            StudyError::new(
                StudyErrorCode::DiskPreflight,
                "backup page arithmetic overflow",
            )
        })?;
    let available = available_space(&workspace)?;
    if available < estimated.saturating_add(16 * 1024 * 1024) {
        return Err(StudyError::new(
            StudyErrorCode::DiskPreflight,
            "insufficient scratch space for page backup",
        ));
    }
    let backup = page_backup(&source, &scratch, &request.cancellation)?;
    request.cancellation.check()?;
    let storage = Storage::init_study_scratch(&scratch).map_err(|error| {
        StudyError::new(
            StudyErrorCode::SourceInvalid,
            format!("migrate scratch database: {error}"),
        )
    })?;
    drop(storage);
    let after = ensure_source_unchanged(&before)?;
    let manifest = PrivateManifest {
        schema_version: MANIFEST_VERSION,
        lifecycle: "profiled".into(),
        created_at: Utc::now().to_rfc3339(),
        approval: request.approval,
        source: before,
        source_after_reads: after,
        profile_backup: backup,
        cleanup_disposition: "profile scratch removed by default".into(),
        matrix: Vec::new(),
    };
    write_manifest(&manifest_path, &manifest)?;
    remove_scratch(&scratch);
    Ok(manifest_path)
}

fn remove_scratch(scratch: &Path) {
    for path in sidecar_paths(scratch) {
        let _ = fs::remove_file(path);
    }
}

/// Run one archive-off and one archive-on scratch replay. It is deliberately
/// not a measurement classifier: callers need three approved warm runs later.
pub fn run_replay_matrix(request: ReplayRequest<'_>) -> Result<PrivateManifest, StudyError> {
    let workspace = prepare_workspace(request.workspace)?;
    let mut manifest = read_manifest(request.manifest)?;
    if !manifest.approval.allow_scratch_replay {
        return Err(StudyError::new(
            StudyErrorCode::ApprovalIncomplete,
            "approval does not allow scratch replay",
        ));
    }
    let source = canonical_existing(
        Path::new(&manifest.source.source_path),
        StudyErrorCode::SourceInvalid,
    )?;
    let before = source_inventory(&source)?;
    if before.files != manifest.source.files {
        return Err(StudyError::new(
            StudyErrorCode::SourceChanged,
            "profiled source no longer matches manifest",
        ));
    }
    let cutoff = DateTime::parse_from_rfc3339(&manifest.approval.fixed_utc_anchor)
        .map_err(|_| StudyError::new(StudyErrorCode::ManifestInvalid, "invalid fixed UTC anchor"))?
        .with_timezone(&Utc)
        - chrono::TimeDelta::days(manifest.approval.retention_window_days);
    for archive_on in [false, true] {
        request.cancellation.check()?;
        let mode = if archive_on {
            "archive_on"
        } else {
            "archive_off"
        };
        let scratch = workspace.join(format!("replay-{mode}.db"));
        let archive = workspace.join(format!("archive-{mode}"));
        if scratch.exists() || archive.exists() {
            return Err(StudyError::new(
                StudyErrorCode::OutputCollision,
                format!("{mode} output already exists"),
            ));
        }
        reject_aliases(&source, &[&scratch, &archive])?;
        let started = Instant::now();
        let result = (|| -> Result<(), StudyError> {
            page_backup(&source, &scratch, &request.cancellation)?;
            let storage = Storage::init_study_scratch(&scratch).map_err(|error| {
                StudyError::new(
                    StudyErrorCode::SourceInvalid,
                    format!("migrate replay scratch: {error}"),
                )
            })?;
            let delete_request = RetentionDeleteRequest {
                cutoff: cutoff.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                window_days: manifest.approval.retention_window_days,
                bytes_before: fs::metadata(&scratch)
                    .map_err(|error| {
                        io_error(StudyErrorCode::SourceInvalid, "stat scratch", error)
                    })?
                    .len(),
                ran_at: Utc::now(),
            };
            let controls = RetentionDeleteControls {
                archive_directory: archive_on.then_some(archive.as_path()),
                ..RetentionDeleteControls::default()
            };
            retention_engine::run_retention_delete_phase(&storage, &delete_request, &controls)
                .map_err(|error| {
                    StudyError::new(
                        StudyErrorCode::MeasurementUnavailable,
                        format!("run scratch retention replay: {error}"),
                    )
                })?;
            request.cancellation.check()?;
            let bytes = storage.preflight_database_compaction().map_err(|result| {
                StudyError::new(
                    StudyErrorCode::DiskPreflight,
                    format!("scratch compaction preflight: {:?}", result.reason),
                )
            })?;
            let compacted = storage.vacuum_database(bytes);
            if compacted.status != "completed" {
                return Err(StudyError::new(
                    StudyErrorCode::MeasurementUnavailable,
                    "scratch compaction skipped",
                ));
            }
            Ok(())
        })();
        let observation = match result {
            Ok(()) => ReplayObservation {
                archive_mode: mode.into(),
                cache_state: "unclassified".into(),
                status: "completed".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                reason_code: None,
            },
            Err(error) => ReplayObservation {
                archive_mode: mode.into(),
                cache_state: "unclassified".into(),
                status: "failed".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                reason_code: Some(format!("{:?}", error.code).to_ascii_lowercase()),
            },
        };
        manifest.matrix.push(observation);
        remove_scratch(&scratch);
        let _ = fs::remove_dir_all(&archive);
    }
    let after = ensure_source_unchanged(&before)?;
    manifest.source_after_reads = after;
    manifest.lifecycle = "replay_complete".into();
    // Existing private manifest is updated in place only inside its owner-only
    // workspace; reports are still exclusively-created separate files.
    let serialized = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| StudyError::new(StudyErrorCode::ManifestInvalid, error.to_string()))?;
    fs::write(request.manifest, serialized).map_err(|error| {
        io_error(
            StudyErrorCode::ManifestInvalid,
            "update private manifest",
            error,
        )
    })?;
    set_private_permissions(request.manifest, false)?;
    Ok(manifest)
}

/// Render a new scrubbed report. It refuses publication intent without a
/// separate privacy signoff and never mutates the exact private manifest.
pub fn render_scrubbed_report(request: ReportRequest<'_>) -> Result<(), StudyError> {
    if !request.privacy_signoff {
        return Err(StudyError::new(
            StudyErrorCode::PrivacySignoffRequired,
            "human privacy signoff is required",
        ));
    }
    let manifest = read_manifest(request.manifest)?;
    if !manifest.approval.allow_aggregate_publication {
        return Err(StudyError::new(
            StudyErrorCode::PrivacySignoffRequired,
            "approval does not permit aggregate publication",
        ));
    }
    if request.output.exists() {
        return Err(StudyError::new(
            StudyErrorCode::OutputCollision,
            "report output already exists",
        ));
    }
    let mut report = create_empty_private(request.output)?;
    writeln!(report, "# Retention Corpus Study Evidence (Scrubbed)")
        .map_err(|error| io_error(StudyErrorCode::ManifestInvalid, "write report", error))?;
    writeln!(report, "\nStatus: {}", manifest.lifecycle)
        .map_err(|error| io_error(StudyErrorCode::ManifestInvalid, "write report", error))?;
    writeln!(
        report,
        "\nThis report contains no source paths, identifiers, payloads, or raw errors."
    )
    .map_err(|error| io_error(StudyErrorCode::ManifestInvalid, "write report", error))?;
    writeln!(report, "\n## Replay matrix")
        .map_err(|error| io_error(StudyErrorCode::ManifestInvalid, "write report", error))?;
    for observation in &manifest.matrix {
        writeln!(
            report,
            "- {}: {} ({} ms{})",
            observation.archive_mode,
            observation.status,
            observation.duration_ms,
            observation
                .reason_code
                .as_deref()
                .map(|code| format!(", code {code}"))
                .unwrap_or_default()
        )
        .map_err(|error| io_error(StudyErrorCode::ManifestInvalid, "write report", error))?;
    }
    writeln!(report, "\nClassification: insufficient evidence (three controlled warm runs per mode are required).\n").map_err(|error| io_error(StudyErrorCode::ManifestInvalid, "write report", error))?;
    Ok(())
}

/// Verify the existing fixture and protocol mechanics without treating its
/// timing as corpus evidence. The emitted checklist contains no private data.
pub fn run_synthetic_smoke(
    request: SyntheticSmokeRequest<'_>,
) -> Result<Vec<&'static str>, StudyError> {
    let workspace = prepare_workspace(request.workspace)?;
    let fixture = crate::retention_fixture::build_retention_fixture(
        &crate::retention_fixture::RetentionFixtureSpec::default(),
    )
    .map_err(|error| {
        StudyError::new(
            StudyErrorCode::MeasurementUnavailable,
            format!("build synthetic fixture: {error}"),
        )
    })?;
    let approval = synthetic_approval();
    let marker = workspace.join("synthetic-cancel-marker");
    let manifest = profile_source(ProfileRequest {
        approval,
        source: fixture.db_path(),
        workspace: &workspace,
        cancellation: StudyCancellation::from_marker(&marker),
    })?;
    let _ = run_replay_matrix(ReplayRequest {
        manifest: &manifest,
        workspace: &workspace,
        cancellation: StudyCancellation::from_marker(&marker),
    })?;
    Ok(vec![
        "explicit approval validated",
        "source remained unchanged",
        "page backup integrity checked",
        "scratch-only migration completed",
        "archive-off replay completed",
        "archive-on replay completed",
        "no synthetic timing classified as corpus evidence",
    ])
}

/// Walk `dbstat` only on an identity-verified scratch copy. This capability
/// reports private facts; timing disposition is intentionally deferred.
pub fn measure_dbstat(request: DbstatRequest<'_>) -> Result<Vec<(String, i64)>, StudyError> {
    let manifest = read_manifest(request.manifest)?;
    let scratch = canonical_existing(request.scratch, StudyErrorCode::SourceInvalid)?;
    if identity(&scratch)? == identity(Path::new(&manifest.source.source_path))? {
        return Err(StudyError::new(
            StudyErrorCode::SourceAlias,
            "dbstat scratch aliases the source",
        ));
    }
    request.cancellation.check()?;
    let conn = Connection::open_with_flags(
        &scratch,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        StudyError::new(
            StudyErrorCode::SourceInvalid,
            format!("open dbstat scratch: {error}"),
        )
    })?;
    let mut stmt = conn
        .prepare("SELECT name, SUM(pgsize) FROM dbstat GROUP BY name ORDER BY name")
        .map_err(|error| {
            StudyError::new(
                StudyErrorCode::MeasurementUnavailable,
                format!("open dbstat virtual table: {error}"),
            )
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| {
            StudyError::new(
                StudyErrorCode::MeasurementUnavailable,
                format!("query dbstat: {error}"),
            )
        })?;
    let mut values = Vec::new();
    for row in rows {
        request.cancellation.check()?;
        values.push(row.map_err(|error| {
            StudyError::new(StudyErrorCode::MeasurementUnavailable, error.to_string())
        })?);
    }
    Ok(values)
}

fn synthetic_approval() -> ApprovalRecord {
    ApprovalRecord {
        corpus_label: "synthetic-fixture".into(),
        owner_or_custodian: "maintainer".into(),
        authorized_operator: "maintainer".into(),
        reviewers: vec!["maintainer".into()],
        approved_by: "synthetic-smoke".into(),
        approved_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        revoked: false,
        allow_scratch_replay: true,
        allow_aggregate_publication: false,
        cleanup_deadline: "2099-01-02T00:00:00Z".into(),
        independence_rationale: "synthetic fixture, never corpus evidence".into(),
        fixed_utc_anchor: "2026-07-24T00:00:00Z".into(),
        retention_window_days: 90,
        quill_stopped_acknowledged: true,
        concurrent_load_policy: "single synthetic process".into(),
        prior_prune_state: "not_applicable".into(),
        prior_rollup_state: "not_applicable".into(),
        prior_archive_state: "not_applicable".into(),
    }
}

#[cfg(unix)]
fn available_space(path: &Path) -> Result<u64, StudyError> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| StudyError::new(StudyErrorCode::DiskPreflight, "workspace contains NUL"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(StudyError::new(
            StudyErrorCode::DiskPreflight,
            "read free scratch space failed",
        ));
    }
    let stats = unsafe { stats.assume_init() };
    stats.f_bavail.checked_mul(stats.f_frsize).ok_or_else(|| {
        StudyError::new(
            StudyErrorCode::DiskPreflight,
            "free-space arithmetic overflow",
        )
    })
}

#[cfg(not(unix))]
fn available_space(_path: &Path) -> Result<u64, StudyError> {
    Err(StudyError::new(
        StudyErrorCode::DiskPreflight,
        "free-space preflight unavailable on this platform",
    ))
}
