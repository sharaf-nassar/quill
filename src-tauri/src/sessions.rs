use std::collections::{HashMap, HashSet};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tantivy::collector::{Count, FacetCollector, TopDocs};
use tantivy::query::{BooleanQuery, Occur, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::*;
use tantivy::snippet::SnippetGenerator;
use tantivy::{DateTime, Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::integrations::IntegrationProvider;
use crate::transcript_identity::{JsonlRecord, parse_jsonl_records, resolve_codex_native_identity};

const CLAUDE_SOURCE_ROOT_KEY: &str = "claude:projects";
const CODEX_SOURCE_ROOT_KEY: &str = "codex:sessions";
const ROOT_DIAGNOSTIC_MAX_CHARS: usize = 240;

/// One provider-owned filesystem root that may contain retained transcripts.
///
/// `resolved_root_path` records the path selected by `data_paths`, including
/// demo overrides. Existing roots also carry their canonical filesystem path.
#[allow(dead_code)] // Consumed by model analytics in upcoming tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderSourceRoot {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_root_key: &'static str,
    pub(crate) resolved_root_path: PathBuf,
    pub(crate) canonical_root_path: Option<PathBuf>,
    pub(crate) outcome: ProviderRootEnumerationOutcome,
    pub(crate) sources: Vec<DiscoveredRetainedJsonlSource>,
}

/// Whether a provider root was enumerated completely.
///
/// A failed root may still contain sources found before the filesystem error.
/// Diagnostics are bounded and intentionally omit raw paths and OS messages.
#[allow(dead_code)] // Consumed by model analytics in upcoming tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderRootEnumerationOutcome {
    Complete,
    Failed { diagnostic: String },
}

/// A retained JSONL source discovered from a provider-owned filesystem root.
///
/// The source key is derived from the provider-qualified root key and the
/// canonical path, preventing Claude and Codex paths from colliding.
#[allow(dead_code)] // Consumed by model analytics in upcoming tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiscoveredRetainedJsonlSource {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_root_key: &'static str,
    pub(crate) source_key: String,
    pub(crate) filesystem_path: PathBuf,
    pub(crate) canonical_path: PathBuf,
    pub(crate) layout_hint: RetainedJsonlSourceLayoutHint,
}

/// Filesystem-layout facts available without reading transcript contents.
#[allow(dead_code)] // Consumed by model analytics in upcoming tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RetainedJsonlSourceLayoutHint {
    ClaudeParent { default_project: String },
    ClaudeSubagent { default_project: String },
    CodexTranscript,
}

/// Bounded failure returned while validating one hook-notified transcript.
///
/// Invalid candidates are caller errors. Unavailable validation means local
/// filesystem state prevented Quill from proving source ownership safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedNotifySourceValidationError {
    Invalid(&'static str),
    Unavailable(&'static str),
}

#[derive(Debug)]
struct RetainedJsonlCandidate {
    filesystem_path: PathBuf,
}

/// Validate one notified path without walking either provider transcript tree.
///
/// Claude and Codex source identity comes only from the configured provider
/// root, canonical containment, and the same layout classifier used by full
/// inventory. MiniMax has no model adapter and retains its legacy search-only
/// behavior: the target must remain locally resolvable, but has no provider
/// root or layout requirement.
pub(crate) fn validate_retained_notify_source(
    provider: IntegrationProvider,
    candidate_path: &Path,
) -> Result<Option<DiscoveredRetainedJsonlSource>, RetainedNotifySourceValidationError> {
    if provider == IntegrationProvider::MiniMax {
        let canonical_path = canonicalize_notify_candidate(provider, candidate_path)?;
        std::fs::metadata(&canonical_path).map_err(|error| {
            log_notify_validation_io_error(
                provider,
                "inspect legacy search-only notify target",
                &canonical_path,
                &error,
            );
            match error.kind() {
                std::io::ErrorKind::NotFound => {
                    RetainedNotifySourceValidationError::Invalid("jsonl_path does not exist")
                }
                _ => RetainedNotifySourceValidationError::Unavailable(
                    "Session transcript validation is temporarily unavailable",
                ),
            }
        })?;
        return Ok(None);
    }

    if candidate_path.extension() != Some(std::ffi::OsStr::new("jsonl")) {
        return Err(RetainedNotifySourceValidationError::Invalid(
            "jsonl_path must identify a retained JSONL transcript",
        ));
    }

    let (source_root_key, resolved_root_path) = match provider {
        IntegrationProvider::Claude => (
            CLAUDE_SOURCE_ROOT_KEY,
            crate::data_paths::resolve_claude_projects_dir(),
        ),
        IntegrationProvider::Codex => (
            CODEX_SOURCE_ROOT_KEY,
            crate::data_paths::resolve_codex_sessions_dir(),
        ),
        IntegrationProvider::MiniMax => unreachable!("MiniMax returned above"),
    };

    match std::fs::symlink_metadata(&resolved_root_path) {
        Ok(_) => {}
        Err(error) => {
            log_notify_validation_io_error(
                provider,
                "inspect configured transcript root",
                &resolved_root_path,
                &error,
            );
            return Err(if error.kind() == std::io::ErrorKind::NotFound {
                RetainedNotifySourceValidationError::Invalid(
                    "Configured retained transcript root does not exist",
                )
            } else {
                RetainedNotifySourceValidationError::Unavailable(
                    "Retained transcript validation is temporarily unavailable",
                )
            });
        }
    }

    let root_metadata = std::fs::metadata(&resolved_root_path).map_err(|error| {
        log_notify_validation_io_error(
            provider,
            "follow configured transcript root",
            &resolved_root_path,
            &error,
        );
        RetainedNotifySourceValidationError::Unavailable(
            "Retained transcript validation is temporarily unavailable",
        )
    })?;
    if !root_metadata.is_dir() {
        return Err(RetainedNotifySourceValidationError::Unavailable(
            "Configured retained transcript root is unavailable",
        ));
    }

    let canonical_root_path = std::fs::canonicalize(&resolved_root_path).map_err(|error| {
        log_notify_validation_io_error(
            provider,
            "canonicalize configured transcript root",
            &resolved_root_path,
            &error,
        );
        RetainedNotifySourceValidationError::Unavailable(
            "Retained transcript validation is temporarily unavailable",
        )
    })?;
    let canonical_path = canonicalize_notify_candidate(provider, candidate_path)?;
    let source_metadata = std::fs::metadata(&canonical_path).map_err(|error| {
        log_notify_validation_io_error(
            provider,
            "inspect notified transcript",
            &canonical_path,
            &error,
        );
        match error.kind() {
            std::io::ErrorKind::NotFound => RetainedNotifySourceValidationError::Invalid(
                "jsonl_path does not identify a retained transcript",
            ),
            _ => RetainedNotifySourceValidationError::Unavailable(
                "Retained transcript validation is temporarily unavailable",
            ),
        }
    })?;
    if !source_metadata.is_file() {
        return Err(RetainedNotifySourceValidationError::Invalid(
            "jsonl_path must identify a retained transcript file",
        ));
    }
    if !canonical_path.starts_with(&canonical_root_path) {
        return Err(RetainedNotifySourceValidationError::Invalid(
            "jsonl_path is outside the configured retained transcript root",
        ));
    }

    let layout_hint =
        retained_jsonl_source_layout_hint(provider, &canonical_root_path, &canonical_path).ok_or(
            RetainedNotifySourceValidationError::Invalid(
                "jsonl_path is outside the configured retained transcript layout",
            ),
        )?;

    Ok(Some(DiscoveredRetainedJsonlSource {
        provider,
        source_root_key,
        source_key: canonical_source_key(source_root_key, &canonical_path),
        filesystem_path: canonical_path.clone(),
        canonical_path,
        layout_hint,
    }))
}

fn canonicalize_notify_candidate(
    provider: IntegrationProvider,
    candidate_path: &Path,
) -> Result<PathBuf, RetainedNotifySourceValidationError> {
    std::fs::canonicalize(candidate_path).map_err(|error| {
        log_notify_validation_io_error(
            provider,
            "canonicalize notified transcript",
            candidate_path,
            &error,
        );
        match error.kind() {
            std::io::ErrorKind::NotFound => {
                RetainedNotifySourceValidationError::Invalid("jsonl_path does not exist")
            }
            _ => RetainedNotifySourceValidationError::Unavailable(
                "Session transcript validation is temporarily unavailable",
            ),
        }
    })
}

fn log_notify_validation_io_error(
    provider: IntegrationProvider,
    operation: &str,
    path: &Path,
    error: &std::io::Error,
) {
    log::warn!(
        "Session notify validation filesystem error: provider={} operation={operation} path={} kind={:?} error={error}",
        provider.as_str(),
        path.display(),
        error.kind(),
    );
}

fn retained_jsonl_source_layout_hint(
    provider: IntegrationProvider,
    canonical_root_path: &Path,
    canonical_path: &Path,
) -> Option<RetainedJsonlSourceLayoutHint> {
    if canonical_path.extension() != Some(std::ffi::OsStr::new("jsonl")) {
        return None;
    }

    let relative = canonical_path.strip_prefix(canonical_root_path).ok()?;
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    match provider {
        IntegrationProvider::Claude => match components.as_slice() {
            [project, _transcript] => Some(RetainedJsonlSourceLayoutHint::ClaudeParent {
                default_project: SessionIndex::project_display_name(
                    project.to_str().unwrap_or("unknown"),
                ),
            }),
            [project, _session, subagents, _transcript]
                if *subagents == std::ffi::OsStr::new("subagents") =>
            {
                Some(RetainedJsonlSourceLayoutHint::ClaudeSubagent {
                    default_project: SessionIndex::project_display_name(
                        project.to_str().unwrap_or("unknown"),
                    ),
                })
            }
            _ => None,
        },
        IntegrationProvider::Codex if !components.is_empty() => {
            Some(RetainedJsonlSourceLayoutHint::CodexTranscript)
        }
        IntegrationProvider::Codex | IntegrationProvider::MiniMax => None,
    }
}

/// Enumerate Claude and Codex transcript roots independently.
///
/// This inventory is intentionally separate from Session Search discovery so
/// one unreadable provider root cannot suppress the other provider and cannot
/// change existing indexing behavior.
#[allow(dead_code)] // Consumed by model analytics in upcoming tasks.
pub(crate) fn enumerate_retained_jsonl_source_roots() -> Vec<ProviderSourceRoot> {
    vec![
        enumerate_claude_retained_jsonl_source_root(),
        enumerate_codex_retained_jsonl_source_root(),
    ]
}

/// Stable identities for every configured retained transcript root.
///
/// Callers that serialize whole-root work use these identities before walking
/// the filesystem, so live reconciliation cannot interleave with inventory.
pub(crate) fn retained_jsonl_source_root_identities() -> [(IntegrationProvider, &'static str); 2] {
    [
        (IntegrationProvider::Claude, CLAUDE_SOURCE_ROOT_KEY),
        (IntegrationProvider::Codex, CODEX_SOURCE_ROOT_KEY),
    ]
}

/// Enumerate the configured Claude transcript root as one independent outcome.
pub(crate) fn enumerate_claude_retained_jsonl_source_root() -> ProviderSourceRoot {
    enumerate_provider_source_root(
        IntegrationProvider::Claude,
        CLAUDE_SOURCE_ROOT_KEY,
        crate::data_paths::resolve_claude_projects_dir(),
        collect_claude_jsonl_candidates,
    )
}

/// Enumerate the configured Codex transcript root as one independent outcome.
pub(crate) fn enumerate_codex_retained_jsonl_source_root() -> ProviderSourceRoot {
    enumerate_provider_source_root(
        IntegrationProvider::Codex,
        CODEX_SOURCE_ROOT_KEY,
        crate::data_paths::resolve_codex_sessions_dir(),
        collect_codex_jsonl_candidates,
    )
}

/// Admit every retained transcript discovered during a Session Search scan to
/// the independent model-source reconciliation queue.
///
/// Enumeration and admission deliberately happen outside `IndexState` so an
/// unchanged search mtime, empty search extraction, or later scan failure
/// cannot suppress provider-qualified model evidence. The queue performs the
/// blocking fingerprint/read work and preserves each source's owning root.
fn enqueue_startup_model_source_reconciliation(app_handle: &tauri::AppHandle) {
    for root in enumerate_retained_jsonl_source_roots() {
        if let ProviderRootEnumerationOutcome::Failed { diagnostic } = &root.outcome {
            log::warn!(
                "Startup model source inventory incomplete: provider={} root={} diagnostic={diagnostic}",
                root.provider.as_str(),
                root.source_root_key,
            );
        }

        for source in root.sources {
            let provider = source.provider;
            let source_root_key = source.source_root_key;
            if let Err(error) = crate::enqueue_model_usage_live_source(app_handle, source) {
                log::warn!(
                    "Failed to enqueue startup model source reconciliation: provider={} root={} error={error}",
                    provider.as_str(),
                    source_root_key,
                );
            }
        }
    }
}

fn enumerate_provider_source_root(
    provider: IntegrationProvider,
    source_root_key: &'static str,
    resolved_root_path: PathBuf,
    collect_candidates: fn(
        &Path,
        IntegrationProvider,
        &mut Option<String>,
    ) -> Vec<RetainedJsonlCandidate>,
) -> ProviderSourceRoot {
    let mut diagnostic = None;

    match std::fs::symlink_metadata(&resolved_root_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log_inventory_io_error(
                provider,
                "inspect root directory entry",
                &resolved_root_path,
                &error,
            );
            record_root_failure(&mut diagnostic, provider, "configured root does not exist.");
            return finish_provider_source_root(
                provider,
                source_root_key,
                resolved_root_path,
                None,
                diagnostic,
                Vec::new(),
            );
        }
        Err(error) => {
            log_inventory_io_error(
                provider,
                "inspect root directory entry",
                &resolved_root_path,
                &error,
            );
            record_root_failure(&mut diagnostic, provider, "root could not be inspected.");
            return finish_provider_source_root(
                provider,
                source_root_key,
                resolved_root_path,
                None,
                diagnostic,
                Vec::new(),
            );
        }
    }

    let root_metadata = match std::fs::metadata(&resolved_root_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log_inventory_io_error(provider, "follow root target", &resolved_root_path, &error);
            record_root_failure(
                &mut diagnostic,
                provider,
                "root target could not be inspected.",
            );
            return finish_provider_source_root(
                provider,
                source_root_key,
                resolved_root_path,
                None,
                diagnostic,
                Vec::new(),
            );
        }
    };

    if !root_metadata.is_dir() {
        record_root_failure(&mut diagnostic, provider, "root is not a directory.");
        return finish_provider_source_root(
            provider,
            source_root_key,
            resolved_root_path,
            None,
            diagnostic,
            Vec::new(),
        );
    }

    let canonical_root_path = match std::fs::canonicalize(&resolved_root_path) {
        Ok(path) => path,
        Err(error) => {
            log_inventory_io_error(provider, "canonicalize root", &resolved_root_path, &error);
            record_root_failure(
                &mut diagnostic,
                provider,
                "root could not be canonicalized.",
            );
            return finish_provider_source_root(
                provider,
                source_root_key,
                resolved_root_path,
                None,
                diagnostic,
                Vec::new(),
            );
        }
    };

    let candidates = collect_candidates(&resolved_root_path, provider, &mut diagnostic);
    let mut sources = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let canonical_path = match std::fs::canonicalize(&candidate.filesystem_path) {
            Ok(path) => path,
            Err(error) => {
                log_inventory_io_error(
                    provider,
                    "canonicalize JSONL source",
                    &candidate.filesystem_path,
                    &error,
                );
                record_root_failure(
                    &mut diagnostic,
                    provider,
                    "could not canonicalize all JSONL sources.",
                );
                continue;
            }
        };

        match std::fs::metadata(&canonical_path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => continue,
            Err(error) => {
                log_inventory_io_error(
                    provider,
                    "inspect canonical JSONL source",
                    &canonical_path,
                    &error,
                );
                record_root_failure(
                    &mut diagnostic,
                    provider,
                    "could not inspect all canonical JSONL sources.",
                );
                continue;
            }
        }

        if !canonical_path.starts_with(&canonical_root_path) {
            log::warn!(
                "Transcript inventory skipped source outside canonical root: provider={} root={} source={}",
                provider.as_str(),
                canonical_root_path.display(),
                canonical_path.display(),
            );
            continue;
        }
        let Some(layout_hint) =
            retained_jsonl_source_layout_hint(provider, &canonical_root_path, &canonical_path)
        else {
            continue;
        };

        sources.push(DiscoveredRetainedJsonlSource {
            provider,
            source_root_key,
            source_key: canonical_source_key(source_root_key, &canonical_path),
            filesystem_path: candidate.filesystem_path,
            canonical_path,
            layout_hint,
        });
    }

    sources.sort_by(|left, right| {
        left.source_key
            .cmp(&right.source_key)
            .then_with(|| left.filesystem_path.cmp(&right.filesystem_path))
    });
    sources.dedup_by(|left, right| left.source_key == right.source_key);

    finish_provider_source_root(
        provider,
        source_root_key,
        resolved_root_path,
        Some(canonical_root_path),
        diagnostic,
        sources,
    )
}

fn finish_provider_source_root(
    provider: IntegrationProvider,
    source_root_key: &'static str,
    resolved_root_path: PathBuf,
    canonical_root_path: Option<PathBuf>,
    diagnostic: Option<String>,
    sources: Vec<DiscoveredRetainedJsonlSource>,
) -> ProviderSourceRoot {
    let outcome = diagnostic.map_or(ProviderRootEnumerationOutcome::Complete, |diagnostic| {
        ProviderRootEnumerationOutcome::Failed { diagnostic }
    });

    ProviderSourceRoot {
        provider,
        source_root_key,
        resolved_root_path,
        canonical_root_path,
        outcome,
        sources,
    }
}

fn collect_claude_jsonl_candidates(
    projects_dir: &Path,
    provider: IntegrationProvider,
    diagnostic: &mut Option<String>,
) -> Vec<RetainedJsonlCandidate> {
    let mut candidates = Vec::new();

    for project_entry in read_directory_entries(projects_dir, provider, diagnostic) {
        let project_dir = project_entry.path();
        if !path_is_directory(&project_dir, provider, diagnostic, true) {
            continue;
        }

        for entry in read_directory_entries(&project_dir, provider, diagnostic) {
            let path = entry.path();

            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                candidates.push(RetainedJsonlCandidate {
                    filesystem_path: path.clone(),
                });
            }

            if !path_is_directory(&path, provider, diagnostic, true) {
                continue;
            }

            let subagents_dir = path.join("subagents");
            if !path_is_directory(&subagents_dir, provider, diagnostic, false) {
                continue;
            }

            for subagent_entry in read_directory_entries(&subagents_dir, provider, diagnostic) {
                let subagent_path = subagent_entry.path();
                if subagent_path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
                {
                    candidates.push(RetainedJsonlCandidate {
                        filesystem_path: subagent_path,
                    });
                }
            }
        }
    }

    candidates
}

fn collect_codex_jsonl_candidates(
    sessions_dir: &Path,
    provider: IntegrationProvider,
    diagnostic: &mut Option<String>,
) -> Vec<RetainedJsonlCandidate> {
    let mut candidates = Vec::new();

    for entry in walkdir::WalkDir::new(sessions_dir)
        .sort_by_file_name()
        .into_iter()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log_inventory_walkdir_error(provider, sessions_dir, &error);
                record_root_failure(
                    diagnostic,
                    provider,
                    "could not read all filesystem entries.",
                );
                continue;
            }
        };

        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            candidates.push(RetainedJsonlCandidate {
                filesystem_path: entry.into_path(),
            });
        }
    }

    candidates
}

fn read_directory_entries(
    directory: &Path,
    provider: IntegrationProvider,
    diagnostic: &mut Option<String>,
) -> Vec<std::fs::DirEntry> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            log_inventory_io_error(provider, "read transcript directory", directory, &error);
            record_root_failure(
                diagnostic,
                provider,
                "could not read all transcript directories.",
            );
            return Vec::new();
        }
    };

    let mut collected = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => collected.push(entry),
            Err(error) => {
                log_inventory_io_error(
                    provider,
                    "read transcript directory entry",
                    directory,
                    &error,
                );
                record_root_failure(
                    diagnostic,
                    provider,
                    "could not read all filesystem entries.",
                );
            }
        }
    }
    collected.sort_by_key(std::fs::DirEntry::file_name);
    collected
}

fn path_is_directory(
    path: &Path,
    provider: IntegrationProvider,
    diagnostic: &mut Option<String>,
    missing_is_failure: bool,
) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !missing_is_failure => {
            return false;
        }
        Err(error) => {
            log_inventory_io_error(provider, "inspect directory entry", path, &error);
            record_root_failure(
                diagnostic,
                provider,
                "could not inspect all filesystem entries.",
            );
            return false;
        }
    }

    match std::fs::metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) => {
            log_inventory_io_error(provider, "follow directory target", path, &error);
            record_root_failure(
                diagnostic,
                provider,
                "could not inspect all filesystem entries.",
            );
            false
        }
    }
}

fn log_inventory_io_error(
    provider: IntegrationProvider,
    operation: &str,
    path: &Path,
    error: &std::io::Error,
) {
    log::warn!(
        "Transcript inventory filesystem error: provider={} operation={operation} path={} kind={:?} error={error}",
        provider.as_str(),
        path.display(),
        error.kind(),
    );
}

fn log_inventory_walkdir_error(provider: IntegrationProvider, root: &Path, error: &walkdir::Error) {
    let path = error.path().unwrap_or(root);
    let error_kind = error.io_error().map(std::io::Error::kind);
    log::warn!(
        "Transcript inventory walk error: provider={} operation=walk transcript root path={} depth={} kind={error_kind:?} error={error}",
        provider.as_str(),
        path.display(),
        error.depth(),
    );
}

fn record_root_failure(
    diagnostic: &mut Option<String>,
    provider: IntegrationProvider,
    detail: &str,
) {
    if diagnostic.is_some() {
        return;
    }

    let message = format!("{provider} transcript inventory {detail}");
    *diagnostic = Some(if message.chars().count() <= ROOT_DIAGNOSTIC_MAX_CHARS {
        message
    } else {
        let mut bounded = message
            .chars()
            .take(ROOT_DIAGNOSTIC_MAX_CHARS - 1)
            .collect::<String>();
        bounded.push('…');
        bounded
    });
}

fn canonical_source_key(source_root_key: &str, canonical_path: &Path) -> String {
    let mut key = String::with_capacity(source_root_key.len() + 4);
    key.push_str(source_root_key);

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        key.push_str(":fs-unix:");
        append_hex_bytes(&mut key, canonical_path.as_os_str().as_bytes());
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        key.push_str(":fs-windows:");
        for unit in canonical_path.as_os_str().encode_wide() {
            append_hex_bytes(&mut key, &unit.to_be_bytes());
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        key.push_str(":fs-other:");
        key.push_str(&canonical_path.to_string_lossy());
    }

    key
}

#[cfg(any(unix, windows))]
fn append_hex_bytes(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}

// ---------------------------------------------------------------------------
// Schema fields wrapper
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SessionSchema {
    pub provider: Field,
    pub message_id: Field,
    pub session_id: Field,
    pub content: Field,
    pub role: Field,
    pub project: Field,
    pub host: Field,
    pub provider_facet: Field,
    pub timestamp: Field,
    pub git_branch: Field,
    pub tools_used: Field,
    pub files_modified: Field,
    pub code_changes: Field,
    pub commands_run: Field,
    pub tool_details: Field,
    pub display_text: Field,
    #[allow(dead_code)]
    pub schema: Schema,
}

// ---------------------------------------------------------------------------
// Index state -- tracks which files have been indexed and their mtimes
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
pub struct IndexState {
    /// Map of JSONL file path -> last-modified epoch seconds
    pub file_mtimes: HashMap<String, u64>,
}

// ---------------------------------------------------------------------------
// SessionIndex -- main struct that owns the tantivy index
// ---------------------------------------------------------------------------

pub struct SessionIndex {
    pub index: Index,
    pub fields: SessionSchema,
    pub writer: Arc<Mutex<IndexWriter>>,
    pub reader: IndexReader,
    pub index_dir: PathBuf,
    pub state: Mutex<IndexState>,
}

impl SessionIndex {
    const SCHEMA_VERSION: u32 = 5;

    /// Open an existing index or create a new one at the given directory.
    pub fn open_or_create(index_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(index_dir)
            .map_err(|e| format!("Failed to create index dir: {e}"))?;

        // Check schema version — rebuild index if schema changed
        let version_path = index_dir.join("schema_version.txt");
        let stored_version: u32 = std::fs::read_to_string(&version_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);

        if stored_version < Self::SCHEMA_VERSION {
            log::info!(
                "Schema version mismatch ({stored_version} < {}), rebuilding index",
                Self::SCHEMA_VERSION
            );
            // Remove entire index directory and recreate it (handles files + subdirectories)
            let _ = std::fs::remove_dir_all(index_dir);
            std::fs::create_dir_all(index_dir)
                .map_err(|e| format!("Failed to recreate index dir: {e}"))?;
        }

        let (schema, fields) = Self::build_schema();

        let dir = tantivy::directory::MmapDirectory::open(index_dir)
            .map_err(|e| format!("Failed to open MmapDirectory: {e}"))?;

        let index = Index::open_or_create(dir, schema)
            .map_err(|e| format!("Failed to open or create index: {e}"))?;

        let writer: IndexWriter = index
            .writer(50_000_000)
            .map_err(|e| format!("Failed to create IndexWriter: {e}"))?;

        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| format!("Failed to create IndexReader: {e}"))?;

        let state = Self::load_state(index_dir);

        let _ = std::fs::write(&version_path, Self::SCHEMA_VERSION.to_string());

        Ok(Self {
            index,
            fields,
            writer: Arc::new(Mutex::new(writer)),
            reader,
            index_dir: index_dir.to_path_buf(),
            state: Mutex::new(state),
        })
    }

    /// Build the tantivy schema with all 10 fields.
    fn build_schema() -> (Schema, SessionSchema) {
        let mut builder = Schema::builder();

        // STRING | STORED fields (untokenized, exact-match, stored)
        let provider = builder.add_text_field("provider", STRING | STORED);
        let message_id = builder.add_text_field("message_id", STRING | STORED);
        let session_id = builder.add_text_field("session_id", STRING | STORED);
        let role = builder.add_text_field("role", STRING | STORED);
        let git_branch = builder.add_text_field("git_branch", STRING | STORED);

        // TEXT | STORED fields (tokenized, full-text searchable, stored)
        let content = builder.add_text_field("content", TEXT | STORED);
        let tools_used = builder.add_text_field("tools_used", TEXT | STORED);
        let files_modified = builder.add_text_field("files_modified", TEXT | STORED);
        let code_changes = builder.add_text_field("code_changes", TEXT | STORED);
        let commands_run = builder.add_text_field("commands_run", TEXT | STORED);
        let tool_details = builder.add_text_field("tool_details", TEXT | STORED);
        let display_text = builder.add_text_field("display_text", TEXT | STORED);

        // Facet fields (hierarchical)
        let project = builder.add_facet_field("project", FacetOptions::default());
        let host = builder.add_facet_field("host", FacetOptions::default());
        let provider_facet = builder.add_facet_field("provider_facet", FacetOptions::default());

        // Date field (indexed, stored, fast)
        let date_opts = DateOptions::from(INDEXED)
            .set_stored()
            .set_fast()
            .set_precision(DateTimePrecision::Seconds);
        let timestamp = builder.add_date_field("timestamp", date_opts);

        let schema = builder.build();

        let fields = SessionSchema {
            provider,
            message_id,
            session_id,
            content,
            role,
            project,
            host,
            provider_facet,
            timestamp,
            git_branch,
            tools_used,
            files_modified,
            code_changes,
            commands_run,
            tool_details,
            display_text,
            schema: schema.clone(),
        };

        (schema, fields)
    }

    /// Load persisted index state from disk (file mtimes tracking).
    fn load_state(index_dir: &Path) -> IndexState {
        let state_path = index_dir.join("index_state.json");
        if state_path.exists() {
            match std::fs::read_to_string(&state_path) {
                Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                Err(_) => IndexState::default(),
            }
        } else {
            IndexState::default()
        }
    }

    /// Save the current index state to disk.
    pub fn save_state(&self) -> Result<(), String> {
        let state_path = self.index_dir.join("index_state.json");
        let state = self.state.lock();
        let json =
            serde_json::to_string_pretty(&*state).map_err(|e| format!("Serialize state: {e}"))?;
        std::fs::write(&state_path, json).map_err(|e| format!("Write state: {e}"))?;
        Ok(())
    }

    /// Get a fresh Searcher from the reader pool.
    pub fn searcher(&self) -> tantivy::Searcher {
        self.reader.searcher()
    }

    /// Extract a human-readable project name from a directory-encoded path.
    ///
    /// Claude Code encodes CWD paths by replacing `/` (and `.`) with `-`, so
    /// `-home-mamba-work-claude-usage` represents `/home/mamba/work/claude-usage`.
    /// The encoding is lossy — a literal hyphen in a directory name is
    /// indistinguishable from a path separator.
    ///
    /// We recover the real path by greedily walking the filesystem: at each
    /// level we try the longest candidate that exists, which correctly
    /// preserves names like `claude-usage` and `nasha-lab`.
    ///
    /// Falls back to the last `-`-delimited segment if the path can't be
    /// resolved (e.g. the directory was deleted).
    pub fn project_display_name(dir_name: &str) -> String {
        // Strip the leading `-` which represents the root `/`
        let remaining = dir_name.strip_prefix('-').unwrap_or(dir_name);
        if remaining.is_empty() {
            return dir_name.to_string();
        }

        let segments: Vec<&str> = remaining.split('-').collect();
        let mut path = std::path::PathBuf::from("/");
        let mut i = 0;

        while i < segments.len() {
            // Greedy: try the longest possible component first
            let mut matched = false;
            for end in (i + 1..=segments.len()).rev() {
                let candidate = segments[i..end].join("-");
                let try_path = path.join(&candidate);
                if try_path.exists() {
                    path = try_path;
                    i = end;
                    matched = true;
                    break;
                }
            }
            if !matched {
                // No filesystem match — append remaining segments as one component
                let rest = segments[i..].join("-");
                path.push(&rest);
                break;
            }
        }

        // Return the last component of the recovered path
        path.file_name()
            .and_then(|n| n.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(dir_name)
            .to_string()
    }

    fn build_index_document(
        &self,
        provider: IntegrationProvider,
        msg: &ExtractedMessage,
        project_facet: &str,
        host_facet: &str,
    ) -> TantivyDocument {
        let mut doc = TantivyDocument::default();

        doc.add_text(self.fields.provider, provider.as_str());
        doc.add_text(self.fields.message_id, &msg.uuid);
        doc.add_text(self.fields.session_id, &msg.session_id);
        doc.add_text(self.fields.content, &msg.content);
        doc.add_text(self.fields.role, &msg.role);
        doc.add_text(self.fields.git_branch, &msg.git_branch);
        doc.add_text(self.fields.tools_used, msg.tools_used.join(" "));
        doc.add_text(self.fields.files_modified, msg.files_modified.join(" "));
        doc.add_text(self.fields.code_changes, msg.code_changes.join("\n"));
        doc.add_text(self.fields.commands_run, msg.commands_run.join("\n"));
        doc.add_text(self.fields.tool_details, msg.tool_details.join("\n"));

        // Compose display_text: text content + tool summaries
        let mut display_parts: Vec<String> = Vec::new();
        if !msg.content.is_empty() {
            display_parts.push(truncate(&msg.content, 500));
        }
        for change in &msg.code_changes {
            display_parts.push(change.clone());
        }
        for cmd in &msg.commands_run {
            display_parts.push(cmd.clone());
        }
        for detail in &msg.tool_details {
            display_parts.push(detail.clone());
        }
        let display_text = truncate(&display_parts.join("\n"), 2000);
        doc.add_text(self.fields.display_text, &display_text);

        doc.add_facet(
            self.fields.project,
            Facet::from(&format!("/{project_facet}")),
        );
        doc.add_facet(self.fields.host, Facet::from(&format!("/{host_facet}")));
        doc.add_facet(
            self.fields.provider_facet,
            Facet::from(&format!("/{}", provider.as_str())),
        );

        // Parse timestamp as RFC3339 -> tantivy DateTime
        let ts = if !msg.timestamp.is_empty() {
            chrono::DateTime::parse_from_rfc3339(&msg.timestamp)
                .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                .unwrap_or(DateTime::from_timestamp_secs(0))
        } else {
            DateTime::from_timestamp_secs(0)
        };
        doc.add_date(self.fields.timestamp, ts);

        doc
    }

    fn add_message_to_writer(
        &self,
        writer: &IndexWriter,
        provider: IntegrationProvider,
        msg: &ExtractedMessage,
        project_facet: &str,
        host_facet: &str,
    ) -> Result<(), String> {
        let doc = self.build_index_document(provider, msg, project_facet, host_facet);
        writer
            .add_document(doc)
            .map_err(|e| format!("Add document: {e}"))?;
        Ok(())
    }

    fn delete_session_docs_with_writer(
        &self,
        writer: &IndexWriter,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let provider_term = Term::from_field_text(self.fields.provider, provider.as_str());
        let session_term = Term::from_field_text(self.fields.session_id, session_id);
        let delete_query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(provider_term, IndexRecordOption::Basic)),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(session_term, IndexRecordOption::Basic)),
            ),
        ]);

        writer
            .delete_query(Box::new(delete_query))
            .map(|_| ())
            .map_err(|e| format!("Delete session docs: {e}"))
    }

    pub(crate) fn replace_session_docs_batch(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        project_facet: &str,
        host_facet: &str,
        messages: &[ExtractedMessage],
    ) -> Result<usize, String> {
        let mut writer = self.writer.lock();
        self.delete_session_docs_with_writer(&writer, provider, session_id)?;
        for msg in messages {
            self.add_message_to_writer(&writer, provider, msg, project_facet, host_facet)?;
        }
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(messages.len())
    }

    pub(crate) fn append_messages_batch(
        &self,
        provider: IntegrationProvider,
        project_facet: &str,
        host_facet: &str,
        messages: &[ExtractedMessage],
    ) -> Result<usize, String> {
        let mut writer = self.writer.lock();
        for msg in messages {
            self.add_message_to_writer(&writer, provider, msg, project_facet, host_facet)?;
        }
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(messages.len())
    }

    pub(crate) fn local_hostname() -> String {
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .or_else(|_| {
                std::fs::read_to_string("/etc/hostname")
                    .map(|s| s.trim().to_string())
                    .or_else(|_| {
                        std::process::Command::new("hostname")
                            .output()
                            .map_err(|e| e.to_string())
                            .and_then(|o| {
                                String::from_utf8(o.stdout)
                                    .map(|s| s.trim().to_string())
                                    .map_err(|e| e.to_string())
                            })
                    })
            })
            .unwrap_or_else(|_| "unknown".to_string())
    }

    fn discover_session_files() -> Result<Vec<DiscoveredSessionFile>, String> {
        let mut files = Self::discover_claude_session_files()?;
        files.extend(Self::discover_codex_session_files()?);
        Ok(files)
    }

    fn discover_claude_session_files() -> Result<Vec<DiscoveredSessionFile>, String> {
        let projects_dir = crate::data_paths::resolve_claude_projects_dir();
        Self::discover_claude_session_files_in(&projects_dir)
    }

    /// Test-friendly variant: enumerate Claude transcripts (parent + sub-agent)
    /// under the supplied `projects_dir`. Two explicit passes per project:
    /// first picks up `<projectSlug>/*.jsonl` (parents), second recurses the
    /// whole `<projectSlug>/<session-uuid>/subagents/` subtree for every
    /// `.jsonl` at any depth — both flat `subagents/agent-*.jsonl` and
    /// Workflow-nested `subagents/workflows/wf_<id>/agent-*.jsonl`. We avoid
    /// walkdir and stay bounded to that subtree so unrelated JSONLs nested
    /// elsewhere never sneak in.
    fn discover_claude_session_files_in(
        projects_dir: &Path,
    ) -> Result<Vec<DiscoveredSessionFile>, String> {
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for project_entry in std::fs::read_dir(projects_dir)
            .map_err(|e| format!("Read projects dir: {e}"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
        {
            let project_dir = project_entry.path();
            let project_dir_name = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let project_name = Self::project_display_name(project_dir_name);

            // (1) Parent transcripts: <projectSlug>/*.jsonl
            for entry in std::fs::read_dir(&project_dir)
                .map_err(|e| format!("Read project dir: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            {
                files.push(DiscoveredSessionFile {
                    provider: IntegrationProvider::Claude,
                    path: entry.path(),
                    default_project: project_name.clone(),
                    is_subagent: false,
                });
            }

            // (2) Sub-agent transcripts live somewhere under
            // <projectSlug>/<session-uuid>/subagents/. They appear flat as
            // `subagents/agent-*.jsonl`, and one level deeper for
            // Workflow-spawned agents as
            // `subagents/workflows/wf_<id>/agent-*.jsonl`. Walk the entire
            // subagents/ subtree with an explicit stack (no walkdir) so every
            // `.jsonl` at any depth is collected, staying bounded to that
            // subtree so unrelated nested JSONLs never sneak in.
            for session_entry in std::fs::read_dir(&project_dir)
                .map_err(|e| format!("Read project dir: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
            {
                let subagents_dir = session_entry.path().join("subagents");
                if !subagents_dir.is_dir() {
                    continue;
                }
                let mut stack = vec![subagents_dir];
                while let Some(dir) = stack.pop() {
                    // Skip an unreadable directory rather than aborting the
                    // whole scan, matching the entry-level filter_map below.
                    let Ok(read_dir) = std::fs::read_dir(&dir) else {
                        continue;
                    };
                    for entry in read_dir.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        if path.is_dir() {
                            stack.push(path);
                        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                            files.push(DiscoveredSessionFile {
                                provider: IntegrationProvider::Claude,
                                path,
                                default_project: project_name.clone(),
                                is_subagent: true,
                            });
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    fn discover_codex_session_files() -> Result<Vec<DiscoveredSessionFile>, String> {
        let sessions_dir = crate::data_paths::resolve_codex_sessions_dir();

        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&sessions_dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        {
            files.push(DiscoveredSessionFile {
                provider: IntegrationProvider::Codex,
                path: entry.path().to_path_buf(),
                default_project: "unknown".to_string(),
                is_subagent: false,
            });
        }

        Ok(files)
    }

    /// Scan Claude and Codex session JSONL files and index new/modified files.
    /// Returns the number of newly indexed messages.
    pub fn startup_scan(
        &self,
        app_handle: &tauri::AppHandle,
        storage: Option<&crate::storage::Storage>,
    ) -> Result<usize, String> {
        use tauri::Emitter;

        // @lat: [[data-flow#Model Observation Reconciliation]]
        // Keep model-source discovery independent from Session Search's mtime
        // cache and extraction outcome. Admission is non-blocking; the shared
        // model runner performs fingerprint reconciliation in the background.
        enqueue_startup_model_source_reconciliation(app_handle);

        let mut total_indexed = 0usize;
        let mut index_changed = false;
        let mut state = self.state.lock();
        let hostname = Self::local_hostname();
        let mut writer = self.writer.lock();

        // Migration 20 backfill hook: when storage signals a pending sub-agent
        // re-ingest, drop the mtime cache so every transcript is re-extracted
        // in this boot. The migration already truncated response_times /
        // tool_actions; this re-scan repopulates them with the new
        // is_sidechain / agent_id / parent_uuid columns set. The pending flag
        // is cleared AFTER the scan loop completes successfully — if the scan
        // errors out partway, the flag stays set so the next boot retries.
        let subagent_reingest_pending = storage
            .and_then(|s| s.get_setting("subagent_reingest_pending").ok().flatten())
            .is_some();
        let skill_usage_reingest_pending = storage
            .and_then(|s| s.get_setting("skill_usage_reingest_pending").ok().flatten())
            .is_some();
        // Feature 008: migration 26 sets `runtime_event_reingest_pending`
        // so the next mtime sweep re-runs `process_discovered_file` for
        // every transcript and populates `session_events`. Same shape as
        // the migration-20 / 21 backfill handlers.
        let runtime_event_reingest_pending = storage
            .and_then(|s| {
                s.get_setting("runtime_event_reingest_pending")
                    .ok()
                    .flatten()
            })
            .is_some();
        // Feature 009: migration 27 sets `hook_invocation_reingest_pending`
        // so the next mtime sweep re-extracts hook fires from every
        // Claude transcript and populates `hook_invocations`. Codex has
        // no historical hook data to backfill — its rows arrive live
        // via `POST /api/v1/hooks/observed` — so the sweep is a no-op
        // for Codex even when the flag is set.
        let hook_invocation_reingest_pending = storage
            .and_then(|s| {
                s.get_setting("hook_invocation_reingest_pending")
                    .ok()
                    .flatten()
            })
            .is_some();
        if subagent_reingest_pending
            || skill_usage_reingest_pending
            || runtime_event_reingest_pending
            || hook_invocation_reingest_pending
        {
            log::info!(
                "Session derived-data migration: clearing mtime cache to force full transcript re-ingest"
            );
            state.file_mtimes.clear();
        }

        for discovered in Self::discover_session_files()? {
            let file_key = discovered.path.to_string_lossy().to_string();
            let mtime = std::fs::metadata(&discovered.path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let known_mtime = state.file_mtimes.get(&file_key).copied();
            if known_mtime == Some(mtime) {
                continue;
            }

            let extracted = extract_messages_from_jsonl(discovered.provider, &discovered.path);
            let project_name = extracted
                .project_name
                .clone()
                .filter(|project| !project.is_empty())
                .unwrap_or_else(|| discovered.default_project.clone());

            // Always delete-then-reinsert per session, even on first sight of a
            // file. Hook-driven /sessions/notify may have already indexed this
            // session before the mtime sweep first sees the file; gating delete
            // on known_mtime would let those docs stack up on top of fresh
            // inserts. delete_query is a no-op when no docs match, so this is
            // safe for genuinely new files.
            if !extracted.session_id.is_empty() {
                if let Err(e) = self.delete_session_docs_with_writer(
                    &writer,
                    discovered.provider,
                    &extracted.session_id,
                ) {
                    log::warn!("Failed to delete old session docs: {e}");
                } else {
                    index_changed = true;
                }
            }

            for msg in &extracted.messages {
                if let Err(e) = self.add_message_to_writer(
                    &writer,
                    discovered.provider,
                    msg,
                    &project_name,
                    &hostname,
                ) {
                    log::warn!("Failed to index message: {e}");
                } else {
                    index_changed = true;
                }
            }

            total_indexed += extracted.messages.len();
            state.file_mtimes.insert(file_key, mtime);
        }

        // Commit all changes
        if index_changed {
            writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        }

        // Scan completed without bubbling an error — safe to clear the
        // migration-20 backfill flag. If we errored above, the flag stays set
        // so the next boot re-runs the full transcript re-ingest.
        if subagent_reingest_pending
            && let Some(storage) = storage
            && let Err(e) = storage.delete_setting("subagent_reingest_pending")
        {
            log::warn!("Failed to clear subagent_reingest_pending flag: {e}");
        }
        if skill_usage_reingest_pending
            && let Some(storage) = storage
            && let Err(e) = storage.delete_setting("skill_usage_reingest_pending")
        {
            log::warn!("Failed to clear skill_usage_reingest_pending flag: {e}");
        }
        // Feature 008: clear the migration-26 backfill flag once the
        // sweep completes successfully. If we bubbled an error above, the
        // flag stays set so the next boot retries the catch-up scan.
        // @lat: [[data-flow#Session Indexing Pipeline]]
        if runtime_event_reingest_pending
            && let Some(storage) = storage
            && let Err(e) = storage.delete_setting("runtime_event_reingest_pending")
        {
            log::warn!("Failed to clear runtime_event_reingest_pending flag: {e}");
        }
        // Feature 009: clear the migration-27 backfill flag once the
        // sweep completes successfully (same semantics as the feature
        // 008 flag above). If the sweep aborted, the flag stays set so
        // the next boot retries the hook-invocation catch-up scan.
        // @lat: [[backend#Database#Schema#Hook Invocations]]
        if hook_invocation_reingest_pending
            && let Some(storage) = storage
            && let Err(e) = storage.delete_setting("hook_invocation_reingest_pending")
        {
            log::warn!("Failed to clear hook_invocation_reingest_pending flag: {e}");
        }

        drop(writer);

        // Must drop state lock before save_state which acquires it
        drop(state);

        self.save_state()?;

        log::info!("Session index scan complete: {total_indexed} messages indexed");
        let _ = app_handle.emit("sessions-index-updated", total_indexed);

        Ok(total_indexed)
    }

    // -------------------------------------------------------------------
    // Search
    // -------------------------------------------------------------------

    /// Search the index with a query string and optional filters.
    pub fn search(
        &self,
        query: &str,
        filters: &SearchFilters,
        sort_by: &str,
        page: usize,
        page_size: usize,
    ) -> Result<SearchResults, String> {
        let start = std::time::Instant::now();
        let searcher = self.searcher();
        let f = &self.fields;

        let mut parser = QueryParser::for_index(
            &self.index,
            vec![
                f.content,
                f.tools_used,
                f.files_modified,
                f.code_changes,
                f.commands_run,
                f.tool_details,
                f.display_text,
            ],
        );
        parser.set_conjunction_by_default();
        // Boost concrete artifact fields so they outrank prose noise; equal
        // weighting plus BM25 length-normalization otherwise lets long fields
        // smother short selective ones. display_text is a derived superset of
        // content+code_changes+commands_run+tool_details — kept in the field
        // set with a tiny boost only so SnippetGenerator (which filters terms
        // by field) can still highlight matches against it.
        parser.set_field_boost(f.files_modified, 4.0);
        parser.set_field_boost(f.code_changes, 2.5);
        parser.set_field_boost(f.commands_run, 2.5);
        parser.set_field_boost(f.tool_details, 1.5);
        parser.set_field_boost(f.content, 1.0);
        parser.set_field_boost(f.tools_used, 0.5);
        parser.set_field_boost(f.display_text, 0.1);

        let text_query: Box<dyn tantivy::query::Query> = if query.trim().is_empty() {
            Box::new(tantivy::query::AllQuery)
        } else {
            let (parsed, errors) = parser.parse_query_lenient(query);
            if !errors.is_empty() {
                log::debug!("Session search parse errors for {query:?}: {errors:?}");
            }
            parsed
        };

        // Combine with filter clauses via BooleanQuery
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> =
            vec![(Occur::Must, text_query)];

        // Project facet filter
        if let Some(ref proj) = filters.project {
            let facet = Facet::from(&format!("/{proj}"));
            let term = Term::from_facet(f.project, &facet);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Host facet filter
        if let Some(ref host) = filters.host {
            let facet = Facet::from(&format!("/{host}"));
            let term = Term::from_facet(f.host, &facet);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Role filter
        if let Some(ref role) = filters.role {
            let term = Term::from_field_text(f.role, role);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        if let Some(provider) = filters.provider {
            let term = Term::from_field_text(f.provider, provider.as_str());
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Git branch filter
        if let Some(ref branch) = filters.git_branch {
            let term = Term::from_field_text(f.git_branch, branch);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Session ID filter
        if let Some(ref sid) = filters.session_id {
            let term = Term::from_field_text(f.session_id, sid);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Date range filter
        if filters.date_from.is_some() || filters.date_to.is_some() {
            let parse_date = |s: &str| -> Option<DateTime> {
                // Try RFC3339 first, then plain date
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                    .ok()
                    .or_else(|| {
                        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                            .map(|d| {
                                DateTime::from_timestamp_secs(
                                    d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
                                )
                            })
                            .ok()
                    })
            };

            let lower = match &filters.date_from {
                Some(from_str) => {
                    let dt = parse_date(from_str).unwrap_or(DateTime::MIN);
                    Bound::Included(Term::from_field_date(f.timestamp, dt))
                }
                None => Bound::Unbounded,
            };
            let upper = match &filters.date_to {
                Some(to_str) => {
                    let dt = parse_date(to_str).unwrap_or(DateTime::MAX);
                    Bound::Included(Term::from_field_date(f.timestamp, dt))
                }
                None => Bound::Unbounded,
            };
            clauses.push((Occur::Must, Box::new(RangeQuery::new(lower, upper))));
        }

        let combined = BooleanQuery::new(clauses);
        let limit = page_size.min(100);
        let offset = page * page_size;

        let (doc_addresses, total_count): (Vec<(f32, tantivy::DocAddress)>, usize) =
            if sort_by == "recency" {
                let (top_docs, count) = searcher
                    .search(
                        &combined,
                        &(
                            TopDocs::with_limit(limit)
                                .and_offset(offset)
                                .order_by_fast_field::<DateTime>("timestamp", tantivy::Order::Desc),
                            Count,
                        ),
                    )
                    .map_err(|e| format!("Search error: {e}"))?;
                let addrs = top_docs
                    .into_iter()
                    .map(|(_, addr)| (0.0f32, addr))
                    .collect();
                (addrs, count)
            } else {
                let (top_docs, count) = searcher
                    .search(
                        &combined,
                        &(TopDocs::with_limit(limit).and_offset(offset), Count),
                    )
                    .map_err(|e| format!("Search error: {e}"))?;
                (top_docs, count)
            };

        // Snippet generator for display_text field
        let snippet_gen = SnippetGenerator::create(&searcher, &combined, f.display_text)
            .map_err(|e| format!("Snippet generator error: {e}"))?;

        let mut hits = Vec::with_capacity(doc_addresses.len());
        for (score, doc_addr) in &doc_addresses {
            let doc: TantivyDocument = searcher
                .doc(*doc_addr)
                .map_err(|e| format!("Doc retrieval: {e}"))?;

            let snippet = snippet_gen.snippet_from_doc(&doc);
            // Convert <b>...</b> to <mark>...</mark>
            let snippet_html = snippet
                .to_html()
                .replace("<b>", "<mark>")
                .replace("</b>", "</mark>");

            let get_text = |field: Field| -> String {
                doc.get_first(field)
                    .and_then(|v| v.as_value().as_str().map(|s| s.to_string()))
                    .unwrap_or_default()
            };

            let get_facet_str = |field: Field| -> String {
                doc.get_first(field)
                    .and_then(|v| {
                        v.as_value().as_facet().map(|f| {
                            // Strip leading "/"
                            f.strip_prefix('/').unwrap_or(f).to_string()
                        })
                    })
                    .unwrap_or_default()
            };

            let timestamp = doc
                .get_first(f.timestamp)
                .and_then(|v| v.as_value().as_datetime())
                .map(|dt| {
                    chrono::DateTime::from_timestamp(dt.into_timestamp_secs(), 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            hits.push(SearchHit {
                provider: get_text(f.provider)
                    .parse()
                    .unwrap_or(IntegrationProvider::Claude),
                message_id: get_text(f.message_id),
                session_id: get_text(f.session_id),
                content: get_text(f.content),
                snippet: snippet_html,
                role: get_text(f.role),
                project: get_facet_str(f.project),
                host: get_facet_str(f.host),
                timestamp,
                git_branch: get_text(f.git_branch),
                tools_used: get_text(f.tools_used),
                files_modified: get_text(f.files_modified),
                code_changes: get_text(f.code_changes),
                commands_run: get_text(f.commands_run),
                tool_details: get_text(f.tool_details),
                score: *score,
            });
        }

        Ok(SearchResults {
            hits,
            total_hits: total_count as u64,
            query_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    // -------------------------------------------------------------------
    // Facets
    // -------------------------------------------------------------------

    /// Collect distinct provider, project, and host facets from the index.
    pub fn get_facets(&self) -> Result<SearchFacets, String> {
        let searcher = self.searcher();

        let mut project_collector = FacetCollector::for_field("project");
        project_collector.add_facet(Facet::root());

        let mut host_collector = FacetCollector::for_field("host");
        host_collector.add_facet(Facet::root());

        let mut provider_collector = FacetCollector::for_field("provider_facet");
        provider_collector.add_facet(Facet::root());

        let (project_counts, host_counts, provider_counts) = searcher
            .search(
                &tantivy::query::AllQuery,
                &(project_collector, host_collector, provider_collector),
            )
            .map_err(|e| format!("Facet collection error: {e}"))?;

        let projects = project_counts
            .get("/")
            .map(|(facet, count)| FacetCount {
                name: facet
                    .to_string()
                    .strip_prefix('/')
                    .unwrap_or(&facet.to_string())
                    .to_string(),
                count,
            })
            .collect();

        let hosts = host_counts
            .get("/")
            .map(|(facet, count)| FacetCount {
                name: facet
                    .to_string()
                    .strip_prefix('/')
                    .unwrap_or(&facet.to_string())
                    .to_string(),
                count,
            })
            .collect();

        let providers = provider_counts
            .get("/")
            .map(|(facet, count)| FacetCount {
                name: facet
                    .to_string()
                    .strip_prefix('/')
                    .unwrap_or(&facet.to_string())
                    .to_string(),
                count,
            })
            .collect();

        Ok(SearchFacets {
            providers,
            projects,
            hosts,
        })
    }

    // -------------------------------------------------------------------
    // Context -- surrounding messages for a search hit
    // -------------------------------------------------------------------

    /// Find the JSONL file for a session and return a window of messages
    /// around the target message.
    pub fn get_context(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        message_id: &str,
        window: usize,
    ) -> Result<SessionContext, String> {
        let path = find_session_path(provider, session_id)?
            .ok_or_else(|| format!("JSONL file not found for session {session_id}"))?;
        let extracted = extract_messages_from_jsonl(provider, &path);
        let project_name = extracted.project_name.unwrap_or_default();
        let messages = extracted.messages;

        // Find the index of the target message
        let target_idx = messages
            .iter()
            .position(|m| m.uuid == message_id)
            .unwrap_or(0);

        let start = target_idx.saturating_sub(window);
        let end = (target_idx + window + 1).min(messages.len());

        let context_messages: Vec<ContextMessage> = messages[start..end]
            .iter()
            .map(|m| {
                let tool_summary = m
                    .code_changes
                    .iter()
                    .chain(m.commands_run.iter())
                    .chain(m.tool_details.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");

                ContextMessage {
                    message_id: m.uuid.clone(),
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_summary,
                    tools_used: m.tools_used.join(" "),
                    timestamp: m.timestamp.clone(),
                    is_match: m.uuid == message_id,
                }
            })
            .collect();

        Ok(SessionContext {
            provider,
            session_id: session_id.to_string(),
            project: project_name,
            messages: context_messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Search result types (serializable for frontend)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct SearchHit {
    pub provider: IntegrationProvider,
    pub message_id: String,
    pub session_id: String,
    pub content: String,
    pub snippet: String,
    pub role: String,
    pub project: String,
    pub host: String,
    pub timestamp: String,
    pub git_branch: String,
    pub tools_used: String,
    pub files_modified: String,
    pub code_changes: String,
    pub commands_run: String,
    pub tool_details: String,
    pub score: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total_hits: u64,
    pub query_time_ms: u64,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct SearchFilters {
    pub provider: Option<IntegrationProvider>,
    pub project: Option<String>,
    pub host: Option<String>,
    pub role: Option<String>,
    pub git_branch: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct FacetCount {
    pub name: String,
    pub count: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchFacets {
    pub providers: Vec<FacetCount>,
    pub projects: Vec<FacetCount>,
    pub hosts: Vec<FacetCount>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ContextMessage {
    pub message_id: String,
    pub role: String,
    pub content: String,
    pub tool_summary: String,
    pub tools_used: String,
    pub timestamp: String,
    pub is_match: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionContext {
    pub provider: IntegrationProvider,
    pub session_id: String,
    pub project: String,
    pub messages: Vec<ContextMessage>,
}

#[derive(Debug)]
struct DiscoveredSessionFile {
    provider: IntegrationProvider,
    path: PathBuf,
    default_project: String,
    /// True when this file lives under `<session>/subagents/agent-*.jsonl`.
    /// Hints the extractor to expect every record to carry isSidechain=true
    /// and lets the indexer treat sub-agent rows as part of the parent
    /// session_id while still tagging them for tree roll-ups. Currently
    /// informational — the per-record `isSidechain` field drives DB writes.
    #[allow(dead_code)]
    is_subagent: bool,
}

// ---------------------------------------------------------------------------
// Extracted message -- intermediate struct from JSONL parsing
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct ToolAction {
    pub tool_use_id: String,
    pub source_ordinal: u64,
    pub block_ordinal: usize,
    pub tool_name: String,
    pub category: String, // "code_change", "command", "tool_detail"
    pub file_path: Option<String>,
    pub summary: String,
    pub full_input: Option<String>,  // JSON string, max 10KB
    pub full_output: Option<String>, // JSON string, max 10KB, set later from tool_result
    pub timestamp: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SkillAccess {
    pub skill_name: String,
    pub skill_path: String,
}

/// Event-kind classification used by the runtime-event pipeline. Five
/// variants matching specs/008-runtime-redesign/contracts/session-events.md
/// (EVT-CL-2..EVT-CL-5 and EVT-CX-1). Used by both extractor and storage
/// ingest paths so the conversion between [`ExtractedEvent`] and
/// [`crate::storage::SessionEventInput`] is a plain field map.
// @lat: [[backend#Database#Schema#Code and Runtime Metrics]]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEventKind {
    UserText,
    UserToolResult,
    AsstText,
    AsstThinking,
    AsstToolUse,
}

impl SessionEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionEventKind::UserText => "user_text",
            SessionEventKind::UserToolResult => "user_tool_result",
            SessionEventKind::AsstText => "asst_text",
            SessionEventKind::AsstThinking => "asst_thinking",
            SessionEventKind::AsstToolUse => "asst_tool_use",
        }
    }
}

/// One non-meta `user` or `assistant` JSONL line lifted into a discrete
/// timestamped event for the active-interval computation in
/// [`crate::storage::Storage::get_llm_runtime_stats`]. Built alongside
/// [`ExtractedMessage`] during the same parse pass — see
/// specs/008-runtime-redesign/contracts/session-events.md (EVT-CL-*).
// @lat: [[data-flow#Session Indexing Pipeline#Dual Emission for Runtime Tracking]]
#[allow(dead_code)] // Source-local identity fields are consumed in Task 3.
pub struct ExtractedEvent {
    pub source_ordinal: u64,
    pub event_ordinal: usize,
    pub timestamp: String,
    pub kind: SessionEventKind,
    pub is_sidechain: bool,
    #[allow(dead_code)] // Native source identity supplies analytics attribution.
    pub agent_id: Option<String>,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
}

pub struct ExtractedMessage {
    pub uuid: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub git_branch: String,
    pub tools_used: Vec<String>,
    pub files_modified: Vec<String>,
    // New fields for tool data summaries
    pub code_changes: Vec<String>,
    pub commands_run: Vec<String>,
    pub tool_details: Vec<String>,
    // Tool actions for SQLite storage
    #[allow(dead_code)]
    pub tool_actions: Vec<ToolAction>,
    /// Claude Code sub-agent attribution. Always false/None for Codex (no
    /// sub-agent concept) and for top-level Claude messages.
    pub is_sidechain: bool,
    #[allow(dead_code)] // Native source identity supplies analytics attribution.
    pub agent_id: Option<String>,
    pub parent_uuid: Option<String>,
    /// Working directory at the time of the message. Claude reads it from the
    /// top-level `cwd` field on each JSONL row; Codex reads it once from
    /// `session_meta.payload.cwd`. None if not present in the transcript.
    pub cwd: Option<String>,
}

pub struct ExtractedSession {
    pub session_id: String,
    pub project_name: Option<String>,
    pub messages: Vec<ExtractedMessage>,
    /// Per-event timeline emitted alongside [`messages`] for the active-
    /// interval runtime pipeline (feature 008). Populated by
    /// [`extract_claude_messages_from_jsonl`] and
    /// [`extract_codex_messages_from_jsonl`] in the same parse pass.
    pub events: Vec<ExtractedEvent>,
    /// Observed lifecycle-hook fires emitted alongside [`messages`] and
    /// [`events`] (feature 009). Populated only by the Claude extractor,
    /// which inspects `type:"attachment"` JSONL records carrying
    /// `hook_*` payloads. Codex transcripts do not record hook
    /// executions, so the Codex extractor always leaves this empty —
    /// Codex hook data arrives live via the
    /// `POST /api/v1/hooks/observed` endpoint instead.
    pub hook_invocations: Vec<HookInvocation>,
}

/// Owned form of a hook fire extracted from a Claude transcript
/// (feature 009). Held by [`ExtractedSession`] until source-owned transcript
/// reconciliation builds the atomic analytics snapshot. See
/// specs/009-hooks-breakdown-tab/contracts/hook-invocations.md.
// @lat: [[backend#Database#Schema#Hook Invocations]]
#[derive(Clone, Debug)]
pub struct HookInvocation {
    #[allow(dead_code)] // Native source identity supplies root and sidechain attribution.
    pub session_id: String,
    #[allow(dead_code)]
    pub agent_id: Option<String>,
    #[allow(dead_code)]
    pub is_sidechain: bool,
    pub timestamp: String,
    pub hook_event: String,
    pub hook_matcher: Option<String>,
    pub tool_name: Option<String>,
    pub hook_identity: String,
    pub script_command_raw: Option<String>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub cwd: Option<String>,
    pub message_id: Option<String>,
}

// ---------------------------------------------------------------------------
// JSONL parsing
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find the last char boundary at or before max_len to avoid panic on multi-byte UTF-8
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i <= max_len)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("{}... [truncated]", &s[..boundary])
    }
}

/// Build a human-readable summary for a Claude tool invocation.
/// Returns (category, summary, file_path).
fn build_claude_tool_summary(
    tool_name: &str,
    input: Option<&serde_json::Value>,
) -> (String, String, Option<String>) {
    let inp = input.and_then(|v| v.as_object());

    let get_str = |key: &str| -> String {
        inp.and_then(|o| o.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    match tool_name {
        "Edit" => {
            let file_path = get_str("file_path");
            let old = truncate(&get_str("old_string"), 80);
            let new = truncate(&get_str("new_string"), 80);
            let summary = format!("Edit {file_path}: \"{old}\" -> \"{new}\"");
            ("code_change".to_string(), summary, Some(file_path))
        }
        "Write" => {
            let file_path = get_str("file_path");
            let content_preview = truncate(&get_str("content"), 120);
            let summary = format!("Write {file_path}: {content_preview}");
            ("code_change".to_string(), summary, Some(file_path))
        }
        "Bash" => {
            let command = get_str("command");
            let summary = format!("$ {command}");
            ("command".to_string(), summary, None)
        }
        "Read" => {
            let file_path = get_str("file_path");
            let summary = format!("Read {file_path}");
            ("tool_detail".to_string(), summary, Some(file_path))
        }
        "Grep" => {
            let pattern = get_str("pattern");
            let path = get_str("path");
            let glob = get_str("glob");
            let target = if !path.is_empty() { path } else { glob };
            let summary = format!("Grep \"{pattern}\" in {target}");
            ("tool_detail".to_string(), summary, None)
        }
        "Glob" => {
            let pattern = get_str("pattern");
            let summary = format!("Glob \"{pattern}\"");
            ("tool_detail".to_string(), summary, None)
        }
        "Agent" => {
            let prompt = truncate(&get_str("prompt"), 120);
            let summary = format!("Agent: {prompt}");
            ("tool_detail".to_string(), summary, None)
        }
        _ => {
            let summary = tool_name.to_string();
            ("tool_detail".to_string(), summary, None)
        }
    }
}

fn build_codex_function_tool_summary(
    tool_name: &str,
    arguments: &str,
) -> (String, String, Option<String>) {
    let parsed: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let input = parsed.as_object();

    let get_str = |key: &str| -> String {
        input
            .and_then(|obj| obj.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };

    match tool_name {
        "exec_command" => {
            let command = get_str("cmd");
            ("command".to_string(), format!("$ {command}"), None)
        }
        "write_stdin" => {
            let chars = truncate(&get_str("chars"), 120);
            ("command".to_string(), format!("stdin {chars}"), None)
        }
        _ if tool_name.starts_with("mcp__") => {
            let detail = ["query", "text", "path", "uri", "url"]
                .into_iter()
                .map(&get_str)
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let summary = if detail.is_empty() {
                tool_name.to_string()
            } else {
                format!("{tool_name}: {}", truncate(&detail, 120))
            };
            let file_path = ["file_path", "path"]
                .into_iter()
                .map(&get_str)
                .find(|value| !value.is_empty());
            ("tool_detail".to_string(), summary, file_path)
        }
        _ => {
            let detail = ["path", "file_path", "workdir", "query", "text"]
                .into_iter()
                .map(&get_str)
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let summary = if detail.is_empty() {
                tool_name.to_string()
            } else {
                format!("{tool_name}: {}", truncate(&detail, 120))
            };
            let file_path = ["file_path", "path"]
                .into_iter()
                .map(get_str)
                .find(|value| !value.is_empty());
            ("tool_detail".to_string(), summary, file_path)
        }
    }
}

fn build_codex_custom_tool_summary(
    tool_name: &str,
    input: &str,
) -> (String, String, Option<String>) {
    match tool_name {
        "apply_patch" => {
            let files = extract_apply_patch_files(input);
            let file_path = files.first().cloned();
            let summary = if files.is_empty() {
                "Patch".to_string()
            } else {
                format!("Patch {}", truncate(&files.join(", "), 160))
            };
            ("code_change".to_string(), summary, file_path)
        }
        _ => (
            "tool_detail".to_string(),
            format!("{tool_name}: {}", truncate(input, 120)),
            None,
        ),
    }
}

fn extract_apply_patch_files(patch: &str) -> Vec<String> {
    let mut files = Vec::new();

    for line in patch.lines() {
        for prefix in ["*** Update File: ", "*** Add File: ", "*** Delete File: "] {
            if let Some(path) = line.strip_prefix(prefix) {
                files.push(path.to_string());
            }
        }
    }

    files
}

pub fn extract_skill_accesses_from_tool_action(action: &ToolAction) -> Vec<SkillAccess> {
    let mut paths = Vec::new();

    match action.tool_name.as_str() {
        "Read" => {
            if let Some(file_path) = action.file_path.as_deref() {
                collect_skill_paths_from_text(file_path, &mut paths);
            }
            if let Some(file_path) =
                extract_tool_input_string(action.full_input.as_deref(), &["file_path", "path"])
            {
                collect_skill_paths_from_text(&file_path, &mut paths);
            }
        }
        "exec_command" => {
            if let Some(command) =
                extract_tool_input_string(action.full_input.as_deref(), &["cmd", "command"])
                && command_reads_skill_file(&command)
            {
                collect_skill_paths_from_text(&command, &mut paths);
            }
        }
        "Skill" => {
            if let Some(raw_skill) =
                extract_tool_input_string(action.full_input.as_deref(), &["skill"])
                && let Some(access) = skill_access_from_skill_tool_input(&raw_skill)
            {
                return vec![access];
            }
            return Vec::new();
        }
        _ => {}
    }

    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter_map(|skill_path| {
            let skill_name = skill_name_from_skill_path(&skill_path)?;
            let access = SkillAccess {
                skill_name,
                skill_path,
            };
            if seen.insert(access.clone()) {
                Some(access)
            } else {
                None
            }
        })
        .collect()
}

/// Canonicalize a hook script command into a stable identity string used
/// to aggregate hook fires in the `hook_invocations` table (feature 009).
/// Rule (per spec FR-003 / research R-D):
///
/// 1. If `command` is `None` (older Claude transcripts), fall back to
///    `hook_name`.
/// 2. Strip a leading interpreter (`node`, `bash`, `sh`, `python`,
///    `python3`) when present and take the first shell-token of the
///    remainder as the executable path.
/// 3. If the executable resolves into Quill-managed script directories
///    (`/.config/quill/scripts/` or `/.config/quill/codex/scripts/`),
///    return `quill:<basename>` so per-machine identity stays stable.
/// 4. If the executable begins with `${CLAUDE_PLUGIN_ROOT}/`, keep it
///    verbatim — the unexpanded env-var prefix is the only stable
///    plugin-scoped identifier the transcript provides.
/// 5. Otherwise, return the basename of the executable.
///
/// See specs/009-hooks-breakdown-tab/contracts/hook-invocations.md
/// (§ Canonicalization rule).
// @lat: [[backend#Database#Schema#Hook Invocations]]
pub fn canonicalize_hook_identity(command: Option<&str>, hook_name: &str) -> String {
    let raw = match command {
        Some(c) => c.trim(),
        None => return hook_name.to_string(),
    };
    if raw.is_empty() {
        return hook_name.to_string();
    }
    let after_interp = strip_interpreter_prefix(raw).unwrap_or(raw);
    let exe = first_shell_token(after_interp);
    if exe.is_empty() {
        return hook_name.to_string();
    }
    if is_quill_managed_path(exe) {
        let bn = basename_of(exe);
        return format!("quill:{}", bn);
    }
    if exe.starts_with("${CLAUDE_PLUGIN_ROOT}/") {
        return exe.to_string();
    }
    basename_of(exe).to_string()
}

fn strip_interpreter_prefix(s: &str) -> Option<&str> {
    for interp in ["node ", "bash ", "sh ", "python ", "python3 "] {
        if let Some(rest) = s.strip_prefix(interp) {
            return Some(rest.trim_start());
        }
    }
    None
}

fn first_shell_token(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return "";
    }
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let quote = bytes[0];
        if let Some(end) = bytes[1..].iter().position(|&b| b == quote) {
            return &s[1..1 + end];
        }
        return &s[1..];
    }
    s.split_whitespace().next().unwrap_or("")
}

fn is_quill_managed_path(path: &str) -> bool {
    path.contains("/.config/quill/scripts/") || path.contains("/.config/quill/codex/scripts/")
}

fn basename_of(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

#[cfg(test)]
mod hook_identity_tests {
    use super::canonicalize_hook_identity;

    #[test]
    fn quill_path_becomes_namespaced_basename() {
        let id = canonicalize_hook_identity(
            Some("node \"/home/me/.config/quill/scripts/context-capture.cjs\""),
            "SessionStart:startup",
        );
        assert_eq!(id, "quill:context-capture.cjs");
    }

    #[test]
    fn codex_quill_path_recognized() {
        let id = canonicalize_hook_identity(
            Some("node \"/home/me/.config/quill/codex/scripts/hook-observe.cjs\""),
            "PreToolUse",
        );
        assert_eq!(id, "quill:hook-observe.cjs");
    }

    #[test]
    fn plugin_root_preserved_verbatim() {
        let id = canonicalize_hook_identity(
            Some("${CLAUDE_PLUGIN_ROOT}/hooks-handlers/session-start.sh"),
            "SessionStart:startup",
        );
        assert_eq!(id, "${CLAUDE_PLUGIN_ROOT}/hooks-handlers/session-start.sh");
    }

    #[test]
    fn plugin_root_quoted_with_args_preserves_path() {
        let id = canonicalize_hook_identity(
            Some("\"${CLAUDE_PLUGIN_ROOT}/hooks/run-hook.cmd\" session-start"),
            "SessionStart",
        );
        assert_eq!(id, "${CLAUDE_PLUGIN_ROOT}/hooks/run-hook.cmd");
    }

    #[test]
    fn personal_script_reduces_to_basename() {
        let id = canonicalize_hook_identity(
            Some("/home/me/.codex/hooks/validate-commit-message.sh"),
            "PreToolUse:Bash",
        );
        assert_eq!(id, "validate-commit-message.sh");
    }

    #[test]
    fn missing_command_falls_back_to_hookname() {
        let id = canonicalize_hook_identity(None, "PreToolUse:Bash");
        assert_eq!(id, "PreToolUse:Bash");
    }

    #[test]
    fn empty_command_falls_back_to_hookname() {
        let id = canonicalize_hook_identity(Some("   "), "Stop");
        assert_eq!(id, "Stop");
    }
}

/// Extract a [`HookInvocation`] from a `type:"attachment"` JSONL record
/// when the attachment carries a `hook_*` payload (`hook_success`,
/// `hook_failure`, `hook_timeout`, `hook_blocked`). Returns `None` for
/// any other attachment subtype. The truncation policy on
/// `script_command_raw` matches the wire-side limit applied by
/// `observe.cjs` (2048 chars). Feature 009.
// @lat: [[backend#Database#Schema#Hook Invocations]]
fn extract_hook_invocation_from_attachment(record: &serde_json::Value) -> Option<HookInvocation> {
    let attachment = record.get("attachment")?.as_object()?;
    let att_type = attachment.get("type").and_then(|v| v.as_str())?;
    if !att_type.starts_with("hook_") {
        return None;
    }

    let hook_event = attachment.get("hookEvent").and_then(|v| v.as_str())?;
    if hook_event.is_empty() {
        return None;
    }
    let hook_name = attachment
        .get("hookName")
        .and_then(|v| v.as_str())
        .unwrap_or(hook_event);
    let command = attachment.get("command").and_then(|v| v.as_str());

    let timestamp = record
        .get("timestamp")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();

    let session_id = record
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let is_sidechain = record
        .get("isSidechain")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let agent_id = record
        .get("agentId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let cwd = record
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let parent_uuid = record
        .get("parentUuid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let hook_matcher = hook_name
        .split_once(':')
        .map(|(_, suffix)| suffix)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let tool_name = attachment
        .get("tool_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            // For PreToolUse / PostToolUse, the hookName matcher is the
            // tool name (e.g., "PreToolUse:Bash" → tool_name = "Bash").
            if hook_event == "PreToolUse" || hook_event == "PostToolUse" {
                hook_matcher.clone()
            } else {
                None
            }
        });

    let identity = canonicalize_hook_identity(command, hook_name);

    let exit_code = attachment.get("exitCode").and_then(|v| v.as_i64());
    let duration_ms = attachment.get("durationMs").and_then(|v| v.as_i64());

    let script_command_raw = command.map(|c| {
        if c.len() > 2048 {
            // Walk back to a UTF-8 char boundary to avoid panic on
            // multi-byte sequences straddling the cut.
            let mut cut = 2048;
            while cut > 0 && !c.is_char_boundary(cut) {
                cut -= 1;
            }
            c[..cut].to_string()
        } else {
            c.to_string()
        }
    });

    Some(HookInvocation {
        session_id,
        agent_id,
        is_sidechain,
        timestamp,
        hook_event: hook_event.to_string(),
        hook_matcher,
        tool_name,
        hook_identity: identity,
        script_command_raw,
        exit_code,
        duration_ms,
        cwd,
        message_id: parent_uuid,
    })
}

#[cfg(test)]
mod hook_attachment_tests {
    use super::extract_hook_invocation_from_attachment;
    use serde_json::json;

    #[test]
    fn extracts_session_start_quill_hook() {
        let record = json!({
            "type": "attachment",
            "timestamp": "2026-05-22T22:09:15.299Z",
            "sessionId": "s1",
            "cwd": "/home/me/work/quill",
            "attachment": {
                "type": "hook_success",
                "hookName": "SessionStart:startup",
                "hookEvent": "SessionStart",
                "command": "node \"/home/me/.config/quill/scripts/context-capture.cjs\"",
                "durationMs": 145,
                "exitCode": 0
            }
        });
        let inv = extract_hook_invocation_from_attachment(&record).expect("Some");
        assert_eq!(inv.hook_event, "SessionStart");
        assert_eq!(inv.hook_matcher.as_deref(), Some("startup"));
        assert_eq!(inv.hook_identity, "quill:context-capture.cjs");
        assert_eq!(inv.duration_ms, Some(145));
        assert_eq!(inv.exit_code, Some(0));
        assert_eq!(inv.cwd.as_deref(), Some("/home/me/work/quill"));
    }

    #[test]
    fn extracts_pretool_bash_with_matcher_as_tool() {
        let record = json!({
            "type": "attachment",
            "timestamp": "2026-05-22T22:10:00Z",
            "sessionId": "s1",
            "attachment": {
                "type": "hook_success",
                "hookName": "PreToolUse:Bash",
                "hookEvent": "PreToolUse",
                "command": "${CLAUDE_PLUGIN_ROOT}/hooks-handlers/bash-guard.sh"
            }
        });
        let inv = extract_hook_invocation_from_attachment(&record).expect("Some");
        assert_eq!(inv.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            inv.hook_identity,
            "${CLAUDE_PLUGIN_ROOT}/hooks-handlers/bash-guard.sh"
        );
    }

    #[test]
    fn skips_non_hook_attachments() {
        let record = json!({
            "type": "attachment",
            "attachment": { "type": "image_paste" }
        });
        assert!(extract_hook_invocation_from_attachment(&record).is_none());
    }

    #[test]
    fn falls_back_to_hookname_when_command_missing() {
        let record = json!({
            "type": "attachment",
            "timestamp": "2026-05-22T22:11:00Z",
            "sessionId": "s1",
            "attachment": {
                "type": "hook_success",
                "hookName": "Stop",
                "hookEvent": "Stop"
            }
        });
        let inv = extract_hook_invocation_from_attachment(&record).expect("Some");
        assert_eq!(inv.hook_identity, "Stop");
        assert!(inv.script_command_raw.is_none());
    }
}

/// Build a [`SkillAccess`] from the `skill` field of a Claude Code `Skill`
/// tool call.
///
/// `raw` is the verbatim identifier the model passed, e.g.
/// `"superpowers:using-superpowers"` (plugin-prefixed) or `"speckit-tasks"`
/// (bare). Codex stores the bare folder name derived from `.../SKILL.md`,
/// so we strip the plugin prefix (everything up to and including the first
/// `':'`) from the Claude name to let analytics merge counts across
/// providers. The raw identifier is preserved in `skill_path` (prefixed
/// with `"skill://"` to stay visually distinct from filesystem paths) so a
/// future drilldown can still answer "which plugin loaded this skill?".
///
/// Returns `None` when the trimmed input or the stripped suffix is empty,
/// so the caller skips junk rows instead of inserting `skill_name = ""`.
fn skill_access_from_skill_tool_input(raw: &str) -> Option<SkillAccess> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bare = match trimmed.split_once(':') {
        Some((_, suffix)) => suffix.trim(),
        None => trimmed,
    };
    if bare.is_empty() {
        return None;
    }
    Some(SkillAccess {
        skill_name: bare.to_string(),
        skill_path: format!("skill://{trimmed}"),
    })
}

fn extract_tool_input_string(input: Option<&str>, keys: &[&str]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input?).ok()?;
    let obj = value.as_object()?;
    keys.iter()
        .filter_map(|key| obj.get(*key).and_then(|value| value.as_str()))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn command_reads_skill_file(command: &str) -> bool {
    if !command.contains("SKILL.md") {
        return false;
    }
    if command_has_stdout_write_redirection(command) {
        return false;
    }

    let Some(command_name) = command
        .split_whitespace()
        .next()
        .and_then(|token| Path::new(token).file_name())
        .and_then(|name| name.to_str())
    else {
        return false;
    };

    if command_name == "sed" && sed_command_is_in_place(command) {
        return false;
    }

    matches!(
        command_name,
        "bat" | "batcat" | "cat" | "head" | "less" | "more" | "nl" | "sed" | "tail"
    )
}

fn command_has_stdout_write_redirection(command: &str) -> bool {
    command.split_whitespace().any(|token| {
        matches!(token, ">" | ">>" | "1>" | "1>>")
            || token.starts_with(">")
            || token.starts_with("1>")
    })
}

fn sed_command_is_in_place(command: &str) -> bool {
    command.split_whitespace().skip(1).any(|token| {
        token == "-i"
            || token.starts_with("-i.")
            || token == "--in-place"
            || token.starts_with("--in-place=")
    })
}

fn collect_skill_paths_from_text(text: &str, paths: &mut Vec<String>) {
    let mut search_start = 0;
    while let Some(offset) = text[search_start..].find("SKILL.md") {
        let skill_start = search_start + offset;
        let skill_end = skill_start + "SKILL.md".len();
        let before = &text[..skill_start];
        let path_start = before
            .rfind(|ch: char| {
                ch.is_whitespace()
                    || matches!(
                        ch,
                        '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ','
                    )
            })
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let candidate = text[path_start..skill_end]
            .trim_matches(|ch: char| {
                ch.is_whitespace()
                    || matches!(
                        ch,
                        '"' | '\''
                            | '`'
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '<'
                            | '>'
                            | ','
                            | ';'
                    )
            })
            .to_string();
        if candidate.ends_with("SKILL.md") {
            paths.push(candidate);
        }
        search_start = skill_end;
    }
}

fn skill_name_from_skill_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let parent_path = normalized.strip_suffix("/SKILL.md")?;
    parent_path
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
}

fn has_nonempty_codex_assistant_output(payload: &serde_json::Value) -> bool {
    payload.get("role").and_then(|value| value.as_str()) == Some("assistant")
        && payload
            .get("content")
            .and_then(|value| value.as_array())
            .is_some_and(|content| {
                content.iter().any(|block| {
                    block.get("type").and_then(|value| value.as_str()) == Some("output_text")
                        && block
                            .get("text")
                            .and_then(|value| value.as_str())
                            .is_some_and(|text| !text.trim().is_empty())
                })
            })
}

fn make_tool_message(
    uuid: String,
    session_id: String,
    git_branch: String,
    action: ToolAction,
    cwd: Option<String>,
) -> ExtractedMessage {
    let mut code_changes = Vec::new();
    let mut commands_run = Vec::new();
    let mut tool_details = Vec::new();

    match action.category.as_str() {
        "code_change" => code_changes.push(action.summary.clone()),
        "command" => commands_run.push(action.summary.clone()),
        _ => tool_details.push(action.summary.clone()),
    }

    let files_modified = action.file_path.clone().into_iter().collect();
    let timestamp = action.timestamp.clone();
    let tool_name = action.tool_name.clone();

    ExtractedMessage {
        uuid,
        session_id: session_id.clone(),
        role: "assistant".to_string(),
        content: String::new(),
        timestamp: timestamp.clone(),
        git_branch,
        tools_used: vec![tool_name.clone()],
        files_modified,
        code_changes,
        commands_run,
        tool_details,
        tool_actions: vec![action],
        // Synthetic Codex tool message — no sub-agent attribution applies.
        is_sidechain: false,
        agent_id: None,
        parent_uuid: None,
        cwd,
    }
}

#[allow(dead_code)]
struct ToolUseEntry {
    tool_name: String,
    category: String,
    file_path: Option<String>,
    summary: String,
    full_input: Option<String>,
    timestamp: String,
    // Index into messages vec where this tool_use appeared
    message_idx: usize,
}

/// Extract indexable messages from a provider session transcript.
pub fn extract_messages_from_jsonl(provider: IntegrationProvider, path: &Path) -> ExtractedSession {
    match provider {
        IntegrationProvider::Claude => extract_claude_messages_from_jsonl(path),
        IntegrationProvider::Codex => extract_codex_messages_from_jsonl(path),
        IntegrationProvider::MiniMax => extract_claude_messages_from_jsonl(path),
    }
}

/// Parse already-read retained JSONL content without touching persistence.
pub(crate) fn extract_messages_from_jsonl_contents(
    provider: IntegrationProvider,
    path: &Path,
    contents: &str,
) -> ExtractedSession {
    let records = parse_jsonl_records(contents);
    extract_messages_from_jsonl_records(provider, path, &records)
}

/// Extract search and analytics rows from one ordinal-preserving decode pass.
pub(crate) fn extract_messages_from_jsonl_records(
    provider: IntegrationProvider,
    path: &Path,
    records: &[JsonlRecord],
) -> ExtractedSession {
    match provider {
        IntegrationProvider::Claude => extract_claude_messages_from_jsonl_records(path, records),
        IntegrationProvider::Codex => extract_codex_messages_from_jsonl_records(records),
        IntegrationProvider::MiniMax => extract_claude_messages_from_jsonl_records(path, records),
    }
}

/// Extract indexable messages from a Claude Code JSONL session file.
/// Only "user" and "assistant" type messages are extracted.
/// isMeta messages and messages with empty content are skipped.
fn extract_claude_messages_from_jsonl(path: &Path) -> ExtractedSession {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read JSONL {}: {e}", path.display());
            return ExtractedSession {
                session_id: path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string(),
                project_name: path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    .map(SessionIndex::project_display_name),
                messages: Vec::new(),
                events: Vec::new(),
                hook_invocations: Vec::new(),
            };
        }
    };

    extract_messages_from_jsonl_contents(IntegrationProvider::Claude, path, &contents)
}

fn extract_claude_messages_from_jsonl_records(
    path: &Path,
    records: &[JsonlRecord],
) -> ExtractedSession {
    let mut messages: Vec<ExtractedMessage> = Vec::new();
    let mut events: Vec<ExtractedEvent> = Vec::new();
    // Feature 009: collect hook fires from `type:"attachment"` records
    // carrying a `hook_*` payload (hook_success, hook_failure,
    // hook_timeout, hook_blocked). Populated inline alongside messages
    // and events so a single transcript walk feeds all three sibling
    // ingestion pipelines.
    let mut hook_invocations: Vec<HookInvocation> = Vec::new();
    // Maps tool_use block id -> entry for cross-message correlation
    let mut tool_use_map: HashMap<String, ToolUseEntry> = HashMap::new();

    for record in records {
        let line_index = record.ordinal;
        let obj = &record.value;

        let msg_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Feature 009: peel off hook-attachment records before the
        // user/assistant filter. Hook fires live in their own JSONL
        // row shape and don't carry the message envelope the rest of
        // this loop expects. See contracts/hook-invocations.md.
        if msg_type == "attachment"
            && let Some(invocation) = extract_hook_invocation_from_attachment(obj)
        {
            hook_invocations.push(invocation);
            continue;
        }

        if msg_type != "user" && msg_type != "assistant" {
            continue;
        }

        // Skip isMeta messages
        if obj.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }

        let uuid = obj
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let session_id = obj
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let git_branch = obj
            .get("gitBranch")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Sub-agent attribution. isSidechain is the record's own assertion;
        // parent transcripts emit `false` (or omit it), subagents/*.jsonl rows
        // emit `true`. agentId only appears on sidechain rows. parentUuid is
        // the prior message uuid in the same chain (NULL for the first
        // message of a sub-agent transcript).
        let is_sidechain = obj
            .get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let agent_id = obj
            .get("agentId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let parent_uuid = obj
            .get("parentUuid")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let cwd = obj
            .get("cwd")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let message = match obj.get("message") {
            Some(m) => m,
            None => continue,
        };

        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(msg_type)
            .to_string();
        let content_val = message.get("content");

        let mut text_parts: Vec<String> = Vec::new();
        let mut tools_used: Vec<String> = Vec::new();
        let mut files_modified: Vec<String> = Vec::new();
        let mut code_changes: Vec<String> = Vec::new();
        let mut commands_run: Vec<String> = Vec::new();
        let mut tool_details_vec: Vec<String> = Vec::new();
        let mut tool_actions: Vec<ToolAction> = Vec::new();
        // Feature 008: track content-block shape so we can classify the
        // line into one of five SessionEventKind variants per
        // specs/008-runtime-redesign/contracts/session-events.md
        // (EVT-CL-2..EVT-CL-5). content_is_string captures the
        // plain-string case; the boolean flags below capture array-block
        // presence after non-empty text filtering.
        let mut content_is_string: bool = false;
        let mut has_nonempty_text_block: bool = false;
        let mut has_tool_use_block: bool = false;
        let mut has_tool_result_block: bool = false;
        let mut has_thinking_block: bool = false;

        match content_val {
            // Content is a plain string
            Some(serde_json::Value::String(s)) => {
                content_is_string = true;
                text_parts.push(s.clone());
            }
            // Content is an array of blocks
            Some(serde_json::Value::Array(blocks)) => {
                for (block_ordinal, block) in blocks.iter().enumerate() {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                if !text.trim().is_empty() {
                                    has_nonempty_text_block = true;
                                }
                                text_parts.push(text.to_string());
                            }
                        }
                        "tool_use" => {
                            has_tool_use_block = true;
                            let tool_id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let input = block.get("input");

                            if !name.is_empty() {
                                tools_used.push(name.clone());
                            }

                            // Extract file paths from input
                            if let Some(inp) = input.and_then(|v| v.as_object()) {
                                for key in ["file_path", "path", "pattern"] {
                                    if let Some(val) = inp.get(key).and_then(|v| v.as_str())
                                        && !val.is_empty()
                                    {
                                        files_modified.push(val.to_string());
                                    }
                                }
                            }

                            // Build summary and categorize
                            let (category, summary, file_path) =
                                build_claude_tool_summary(&name, input);

                            match category.as_str() {
                                "code_change" => code_changes.push(summary.clone()),
                                "command" => commands_run.push(summary.clone()),
                                "tool_detail" => tool_details_vec.push(summary.clone()),
                                _ => {}
                            }

                            // Serialize full input (capped at 10KB)
                            let full_input = input.map(|v| {
                                let s = v.to_string();
                                truncate(&s, 10240)
                            });

                            // Store in map for later correlation with tool_result
                            let action = ToolAction {
                                tool_use_id: tool_id.clone(),
                                source_ordinal: line_index,
                                block_ordinal,
                                tool_name: name.clone(),
                                category: category.clone(),
                                file_path: file_path.clone(),
                                summary: summary.clone(),
                                full_input: full_input.clone(),
                                full_output: None,
                                timestamp: timestamp.clone(),
                            };
                            tool_actions.push(action);

                            if !tool_id.is_empty() {
                                tool_use_map.insert(
                                    tool_id,
                                    ToolUseEntry {
                                        tool_name: name,
                                        category,
                                        file_path,
                                        summary,
                                        full_input,
                                        timestamp: timestamp.clone(),
                                        message_idx: messages.len(),
                                    },
                                );
                            }
                        }
                        "tool_result" => {
                            has_tool_result_block = true;
                            let tool_use_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            // Capture output content
                            let output_content = block.get("content").map(|v| {
                                let s = match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    serde_json::Value::Array(arr) => arr
                                        .iter()
                                        .filter_map(|item| {
                                            if item.get("type").and_then(|t| t.as_str())
                                                == Some("text")
                                            {
                                                item.get("text")
                                                    .and_then(|t| t.as_str())
                                                    .map(|s| s.to_string())
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    _ => v.to_string(),
                                };
                                truncate(&s, 10240)
                            });

                            // Correlate with the original tool_use
                            if let Some(entry) = tool_use_map.get_mut(&tool_use_id) {
                                // Update the ToolAction in the original message
                                if entry.message_idx < messages.len()
                                    && let Some(action) = messages[entry.message_idx]
                                        .tool_actions
                                        .iter_mut()
                                        .find(|a| a.tool_use_id == tool_use_id)
                                {
                                    action.full_output = output_content.clone();
                                }

                                // For Bash commands, append truncated output to commands_run summary
                                if entry.tool_name == "Bash"
                                    && let Some(ref output) = output_content
                                {
                                    let output_preview = truncate(output, 300);
                                    let enhanced = format!("{}\n{}", entry.summary, output_preview);
                                    // Update the summary in the original message's commands_run
                                    if entry.message_idx < messages.len()
                                        && let Some(cmd) = messages[entry.message_idx]
                                            .commands_run
                                            .iter_mut()
                                            .find(|c: &&mut String| c.starts_with(&entry.summary))
                                    {
                                        *cmd = enhanced;
                                    }
                                }
                            }
                        }
                        // Track thinking blocks for runtime-event classification
                        // (EVT-CL-5). Image blocks are ignored for both messages
                        // and events.
                        "thinking" => {
                            has_thinking_block = true;
                        }
                        "image" => {}
                        _ => {}
                    }
                }
            }
            _ => continue,
        }

        // Emit every runtime-relevant role present in the record. Ordering is
        // deliberate: a user tool result comes first so the preceding
        // assistant tool-use gap keeps its longer runtime allowance, while an
        // assistant tool use comes last so the next record sees that marker.
        // Event ordinals make every sibling identity stable.
        if !timestamp.is_empty() {
            let mut event_kinds = Vec::with_capacity(3);
            if msg_type == "user" {
                if has_tool_result_block {
                    event_kinds.push(SessionEventKind::UserToolResult);
                }
                if content_is_string || has_nonempty_text_block {
                    event_kinds.push(SessionEventKind::UserText);
                }
            } else {
                // msg_type == "assistant"
                if has_thinking_block {
                    event_kinds.push(SessionEventKind::AsstThinking);
                }
                if content_is_string || has_nonempty_text_block {
                    event_kinds.push(SessionEventKind::AsstText);
                }
                if has_tool_use_block {
                    event_kinds.push(SessionEventKind::AsstToolUse);
                }
            }
            for (event_ordinal, kind) in event_kinds.into_iter().enumerate() {
                events.push(ExtractedEvent {
                    source_ordinal: line_index,
                    event_ordinal,
                    timestamp: timestamp.clone(),
                    kind,
                    is_sidechain,
                    agent_id: agent_id.clone(),
                    uuid: if uuid.is_empty() {
                        None
                    } else {
                        Some(uuid.clone())
                    },
                    parent_uuid: parent_uuid.clone(),
                });
            }
        }

        let content = text_parts.join("\n");
        if content.trim().is_empty() && tools_used.is_empty() {
            continue;
        }

        messages.push(ExtractedMessage {
            uuid,
            session_id: session_id.clone(),
            role,
            content,
            timestamp,
            git_branch,
            tools_used,
            files_modified,
            code_changes,
            commands_run,
            tool_details: tool_details_vec,
            tool_actions,
            is_sidechain,
            agent_id,
            parent_uuid,
            cwd: cwd.clone(),
        });
    }

    ExtractedSession {
        session_id: messages
            .first()
            .map(|message| message.session_id.clone())
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(|stem| stem.to_string())
            })
            .unwrap_or_default(),
        project_name: path
            .parent()
            // For sub-agent transcripts the file lives under a
            // <session-uuid>/subagents/ directory — flat, or nested one level
            // deeper for Workflow agents (subagents/workflows/wf_<id>/). Find
            // the `subagents` ancestor at any depth and step up to
            // <projectSlug> so project_display_name yields the correct label.
            .and_then(|parent| {
                let mut ancestor = Some(parent);
                while let Some(dir) = ancestor {
                    if dir.file_name().and_then(|n| n.to_str()) == Some("subagents") {
                        // dir == <session-uuid>/subagents; two hops up to
                        // <projectSlug>.
                        return dir.parent().and_then(|grand| grand.parent());
                    }
                    ancestor = dir.parent();
                }
                Some(parent)
            })
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .map(SessionIndex::project_display_name),
        messages,
        events,
        hook_invocations,
    }
}

fn extract_codex_messages_from_jsonl(path: &Path) -> ExtractedSession {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read JSONL {}: {e}", path.display());
            return ExtractedSession {
                session_id: String::new(),
                project_name: None,
                messages: Vec::new(),
                events: Vec::new(),
                hook_invocations: Vec::new(),
            };
        }
    };

    extract_messages_from_jsonl_contents(IntegrationProvider::Codex, path, &contents)
}

fn extract_codex_messages_from_jsonl_records(records: &[JsonlRecord]) -> ExtractedSession {
    let mut messages: Vec<ExtractedMessage> = Vec::new();
    let mut events: Vec<ExtractedEvent> = Vec::new();
    let mut tool_use_map: HashMap<String, ToolUseEntry> = HashMap::new();
    let native_identity = match resolve_codex_native_identity(records) {
        Ok(identity) => identity,
        Err(error) => {
            log::warn!("Cannot resolve Codex transcript identity: {error}");
            return ExtractedSession {
                session_id: String::new(),
                project_name: None,
                messages,
                events,
                hook_invocations: Vec::new(),
            };
        }
    };
    let session_id = native_identity.chain_id.clone();
    let cwd = native_identity
        .cwd
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    let mut git_branch = String::new();

    for record in records {
        let line_idx = record.ordinal;
        let obj = &record.value;

        let timestamp = obj
            .get("timestamp")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();

        match obj
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
        {
            "session_meta" => {
                let Some(payload) = obj.get("payload") else {
                    continue;
                };
                if payload.get("id").and_then(|value| value.as_str())
                    == Some(native_identity.source_session_id.as_str())
                {
                    git_branch = payload
                        .get("git")
                        .and_then(|value| value.get("branch"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
            "event_msg" => {
                let Some(payload) = obj.get("payload") else {
                    continue;
                };
                let event_type = payload
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let role = match event_type {
                    "user_message" => "user",
                    "agent_message" => "assistant",
                    _ => continue,
                };
                let content = payload
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                // Emit text events before the search-message empty-content
                // filter. Response-item reasoning and tool-loop records add
                // the other runtime event kinds in the sibling branch below.
                if !timestamp.is_empty() {
                    let kind = match role {
                        "user" => SessionEventKind::UserText,
                        _ => SessionEventKind::AsstText,
                    };
                    events.push(ExtractedEvent {
                        source_ordinal: line_idx,
                        event_ordinal: 0,
                        timestamp: timestamp.clone(),
                        kind,
                        is_sidechain: native_identity.is_sidechain,
                        agent_id: None,
                        uuid: payload
                            .get("id")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned),
                        parent_uuid: None,
                    });
                }
                if content.trim().is_empty() {
                    continue;
                }
                messages.push(ExtractedMessage {
                    uuid: format!("{session_id}:event:{line_idx}"),
                    session_id: session_id.clone(),
                    role: role.to_string(),
                    content,
                    timestamp,
                    git_branch: git_branch.clone(),
                    tools_used: Vec::new(),
                    files_modified: Vec::new(),
                    code_changes: Vec::new(),
                    commands_run: Vec::new(),
                    tool_details: Vec::new(),
                    tool_actions: Vec::new(),
                    is_sidechain: native_identity.is_sidechain,
                    agent_id: None,
                    parent_uuid: None,
                    cwd: cwd.clone(),
                });
            }
            "response_item" => {
                let Some(payload) = obj.get("payload") else {
                    continue;
                };
                match payload
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                {
                    "function_call" => {
                        let name = payload
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = payload
                            .get("arguments")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let call_id = payload
                            .get("call_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if name.is_empty() {
                            continue;
                        }

                        let (category, summary, file_path) =
                            build_codex_function_tool_summary(&name, &arguments);
                        let action = ToolAction {
                            tool_use_id: call_id.clone(),
                            source_ordinal: line_idx,
                            block_ordinal: 0,
                            tool_name: name.clone(),
                            category: category.clone(),
                            file_path: file_path.clone(),
                            summary: summary.clone(),
                            full_input: Some(truncate(&arguments, 10240)),
                            full_output: None,
                            timestamp: timestamp.clone(),
                        };
                        let message_idx = messages.len();
                        messages.push(make_tool_message(
                            format!("{session_id}:tool:{line_idx}"),
                            session_id.clone(),
                            git_branch.clone(),
                            action,
                            cwd.clone(),
                        ));
                        if !timestamp.is_empty() {
                            events.push(ExtractedEvent {
                                source_ordinal: line_idx,
                                event_ordinal: 0,
                                timestamp: timestamp.clone(),
                                kind: SessionEventKind::AsstToolUse,
                                is_sidechain: native_identity.is_sidechain,
                                agent_id: None,
                                uuid: (!call_id.is_empty()).then(|| format!("call:{call_id}")),
                                parent_uuid: None,
                            });
                        }
                        if !call_id.is_empty() {
                            tool_use_map.insert(
                                call_id,
                                ToolUseEntry {
                                    tool_name: name,
                                    category,
                                    file_path,
                                    summary,
                                    full_input: Some(truncate(&arguments, 10240)),
                                    timestamp,
                                    message_idx,
                                },
                            );
                        }
                    }
                    "custom_tool_call" => {
                        let name = payload
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input = payload
                            .get("input")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let call_id = payload
                            .get("call_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if name.is_empty() {
                            continue;
                        }

                        let (category, summary, file_path) =
                            build_codex_custom_tool_summary(&name, &input);
                        let action = ToolAction {
                            tool_use_id: call_id.clone(),
                            source_ordinal: line_idx,
                            block_ordinal: 0,
                            tool_name: name.clone(),
                            category: category.clone(),
                            file_path: file_path.clone(),
                            summary: summary.clone(),
                            full_input: Some(truncate(&input, 10240)),
                            full_output: None,
                            timestamp: timestamp.clone(),
                        };
                        let message_idx = messages.len();
                        messages.push(make_tool_message(
                            format!("{session_id}:tool:{line_idx}"),
                            session_id.clone(),
                            git_branch.clone(),
                            action,
                            cwd.clone(),
                        ));
                        if !timestamp.is_empty() {
                            events.push(ExtractedEvent {
                                source_ordinal: line_idx,
                                event_ordinal: 0,
                                timestamp: timestamp.clone(),
                                kind: SessionEventKind::AsstToolUse,
                                is_sidechain: native_identity.is_sidechain,
                                agent_id: None,
                                uuid: (!call_id.is_empty()).then(|| format!("call:{call_id}")),
                                parent_uuid: None,
                            });
                        }
                        if !call_id.is_empty() {
                            tool_use_map.insert(
                                call_id,
                                ToolUseEntry {
                                    tool_name: name,
                                    category,
                                    file_path,
                                    summary,
                                    full_input: Some(truncate(&input, 10240)),
                                    timestamp,
                                    message_idx,
                                },
                            );
                        }
                    }
                    "function_call_output" | "custom_tool_call_output" => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !timestamp.is_empty() {
                            events.push(ExtractedEvent {
                                source_ordinal: line_idx,
                                event_ordinal: 0,
                                timestamp: timestamp.clone(),
                                kind: SessionEventKind::UserToolResult,
                                is_sidechain: native_identity.is_sidechain,
                                agent_id: None,
                                uuid: (!call_id.is_empty()).then(|| format!("output:{call_id}")),
                                parent_uuid: None,
                            });
                        }
                        if call_id.is_empty() {
                            continue;
                        }
                        let output = payload.get("output").map(|value| {
                            value
                                .as_str()
                                .map(|text| truncate(text, 10240))
                                .unwrap_or_else(|| truncate(&value.to_string(), 10240))
                        });
                        if let Some(entry) = tool_use_map.get(&call_id)
                            && let Some(message) = messages.get_mut(entry.message_idx)
                        {
                            message.timestamp = timestamp.clone();
                            if let Some(action) = message.tool_actions.first_mut() {
                                action.full_output = output.clone();
                            }
                            if entry.category == "command"
                                && let Some(ref output_text) = output
                            {
                                let preview = truncate(output_text, 300);
                                if let Some(command) = message.commands_run.first_mut() {
                                    *command = format!("{command}\n{preview}");
                                }
                            }
                        }
                    }
                    "reasoning" if !timestamp.is_empty() => {
                        events.push(ExtractedEvent {
                            source_ordinal: line_idx,
                            event_ordinal: 0,
                            timestamp,
                            kind: SessionEventKind::AsstThinking,
                            is_sidechain: native_identity.is_sidechain,
                            agent_id: None,
                            uuid: payload
                                .get("id")
                                .and_then(|value| value.as_str())
                                .filter(|value| !value.is_empty())
                                .map(str::to_owned),
                            parent_uuid: None,
                        });
                    }
                    "message"
                        if !timestamp.is_empty()
                            && has_nonempty_codex_assistant_output(payload) =>
                    {
                        let uuid = payload
                            .get("id")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned);
                        events.push(ExtractedEvent {
                            source_ordinal: line_idx,
                            event_ordinal: 0,
                            timestamp: timestamp.clone(),
                            kind: SessionEventKind::AsstText,
                            is_sidechain: native_identity.is_sidechain,
                            agent_id: None,
                            uuid: uuid.clone(),
                            parent_uuid: None,
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    for message in &mut messages {
        message.session_id.clone_from(&native_identity.chain_id);
        message.is_sidechain = native_identity.is_sidechain;
    }

    ExtractedSession {
        session_id,
        project_name: cwd.as_deref().and_then(project_name_from_cwd),
        messages,
        events,
        hook_invocations: Vec::new(),
    }
}

// `pub(crate)` so the learning pipeline's Stream C can resolve a
// session's parent transcript path (sub-agent transcripts live under a
// separate `<session>/subagents/` dir and are never matched here).
pub(crate) fn find_session_path(
    provider: IntegrationProvider,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    match provider {
        IntegrationProvider::Claude => {
            let projects_dir = crate::data_paths::resolve_claude_projects_dir();

            if !projects_dir.exists() {
                return Ok(None);
            }

            for project_entry in std::fs::read_dir(&projects_dir)
                .map_err(|e| format!("Read projects: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
            {
                let candidate = project_entry.path().join(format!("{session_id}.jsonl"));
                if candidate.exists() {
                    return Ok(Some(candidate));
                }
            }

            Ok(None)
        }
        IntegrationProvider::Codex => {
            let sessions_dir = dirs::home_dir()
                .ok_or("Cannot determine home directory")?
                .join(".codex")
                .join("sessions");

            if !sessions_dir.exists() {
                return Ok(None);
            }

            let expected_suffix = format!("{session_id}.jsonl");
            for entry in walkdir::WalkDir::new(&sessions_dir)
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            {
                let file_name = entry.file_name().to_string_lossy();
                if file_name.ends_with(&expected_suffix) {
                    return Ok(Some(entry.path().to_path_buf()));
                }
            }

            Ok(None)
        }
        IntegrationProvider::MiniMax => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Tauri state wrapper and commands
// ---------------------------------------------------------------------------

/// Wrapper for managed Tauri state.
pub struct SessionIndexState(pub Arc<SessionIndex>);

#[tauri::command]
pub async fn search_sessions(
    query: String,
    filters: SearchFilters,
    sort_by: Option<String>,
    page: usize,
    page_size: usize,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SearchResults, String> {
    let idx = state.0.clone();
    let sort = sort_by.unwrap_or_else(|| "relevance".to_string());
    crate::run_blocking(move || idx.search(&query, &filters, &sort, page, page_size))
}

#[tauri::command]
pub async fn get_session_context(
    provider: IntegrationProvider,
    session_id: String,
    around_message_id: String,
    window: Option<u32>,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SessionContext, String> {
    let idx = state.0.clone();
    let w = window.unwrap_or(5) as usize;
    crate::run_blocking(move || idx.get_context(provider, &session_id, &around_message_id, w))
}

#[tauri::command]
pub async fn get_search_facets(
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SearchFacets, String> {
    let idx = state.0.clone();
    crate::run_blocking(move || idx.get_facets())
}

#[tauri::command]
pub async fn sync_search_index(
    app: tauri::AppHandle,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<usize, String> {
    let idx = state.0.clone();
    let storage = crate::STORAGE.get();
    crate::run_blocking(move || idx.startup_scan(&app, storage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a synthetic projects directory that mirrors Claude Code 2.x's
    /// per-session layout: one parent transcript and one sub-agent transcript
    /// living under `<session>/subagents/`.
    fn make_fixture() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        let project_dir = tmp.path().join("-home-test-proj");
        let session_id = "11111111-2222-3333-4444-555555555555";
        let subagents_dir = project_dir.join(session_id).join("subagents");
        fs::create_dir_all(&subagents_dir).expect("mkdir subagents");

        // Parent transcript record (no sidechain flag).
        let parent_record = r#"{"type":"user","uuid":"p1","parentUuid":null,"sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-05-09T10:00:00Z","gitBranch":"main","message":{"role":"user","content":"hello"}}
{"type":"assistant","uuid":"p2","parentUuid":"p1","sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-05-09T10:00:05Z","gitBranch":"main","message":{"role":"assistant","content":[{"type":"text","text":"hi back"}]}}
"#;
        fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            parent_record,
        )
        .expect("write parent jsonl");

        // Sub-agent transcript records (isSidechain=true everywhere, agentId
        // matches filename stem suffix).
        let agent_id = "aaaabbbbccccdddd";
        let subagent_record = format!(
            "{{\"type\":\"user\",\"isSidechain\":true,\"agentId\":\"{agent_id}\",\"parentUuid\":null,\"uuid\":\"s1\",\"sessionId\":\"{session_id}\",\"timestamp\":\"2026-05-09T10:00:10Z\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"do task\"}}}}
{{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"{agent_id}\",\"parentUuid\":\"s1\",\"uuid\":\"s2\",\"sessionId\":\"{session_id}\",\"timestamp\":\"2026-05-09T10:00:15Z\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"done\"}}]}}}}
"
        );
        fs::write(
            subagents_dir.join(format!("agent-{agent_id}.jsonl")),
            subagent_record,
        )
        .expect("write subagent jsonl");

        // Sibling meta.json must be ignored by the .jsonl filter.
        fs::write(
            subagents_dir.join(format!("agent-{agent_id}.meta.json")),
            r#"{"agentType":"general-purpose","description":"test"}"#,
        )
        .expect("write meta");

        tmp
    }

    #[test]
    fn discover_finds_parent_and_subagent_jsonls() {
        let fixture = make_fixture();
        let files =
            SessionIndex::discover_claude_session_files_in(fixture.path()).expect("discover ok");

        assert_eq!(
            files.len(),
            2,
            "expected one parent transcript and one sub-agent transcript, got {files:?}"
        );

        let parent = files
            .iter()
            .find(|f| !f.is_subagent)
            .expect("missing parent entry");
        let subagent = files
            .iter()
            .find(|f| f.is_subagent)
            .expect("missing sub-agent entry");

        assert_eq!(parent.provider, IntegrationProvider::Claude);
        assert!(parent.path.to_string_lossy().ends_with(".jsonl"));
        assert!(
            !parent
                .path
                .components()
                .any(|c| c.as_os_str() == "subagents"),
            "parent path should not traverse subagents/"
        );

        assert_eq!(subagent.provider, IntegrationProvider::Claude);
        assert!(
            subagent
                .path
                .components()
                .any(|c| c.as_os_str() == "subagents"),
            "sub-agent path must traverse subagents/"
        );
        assert!(
            !subagent.path.to_string_lossy().ends_with("meta.json"),
            ".meta.json sidecars must be filtered out"
        );
        assert_eq!(
            parent.default_project, subagent.default_project,
            "sub-agent inherits its parent transcript's project name"
        );
    }

    #[test]
    fn extraction_tags_subagent_records_with_attribution() {
        let fixture = make_fixture();
        let files =
            SessionIndex::discover_claude_session_files_in(fixture.path()).expect("discover ok");

        let subagent_file = files
            .iter()
            .find(|f| f.is_subagent)
            .expect("sub-agent file");
        let extracted = extract_messages_from_jsonl(subagent_file.provider, &subagent_file.path);

        assert!(
            !extracted.messages.is_empty(),
            "expected at least one extracted sub-agent message"
        );
        for msg in &extracted.messages {
            assert!(
                msg.is_sidechain,
                "every record in a subagents/ transcript carries isSidechain=true"
            );
            assert_eq!(
                msg.agent_id.as_deref(),
                Some("aaaabbbbccccdddd"),
                "agent_id must round-trip through extraction"
            );
            assert_eq!(
                msg.session_id, "11111111-2222-3333-4444-555555555555",
                "sub-agent session_id must equal the parent transcript's session_id"
            );
        }
        // First message in the sub-agent chain has parentUuid=null; the second
        // points at the first by uuid.
        assert_eq!(extracted.messages[0].parent_uuid, None);
        assert_eq!(
            extracted.messages[1].parent_uuid.as_deref(),
            Some("s1"),
            "parent_uuid links sub-agent messages into a single chain"
        );

        // Sanity: parent transcript records remain top-level.
        let parent_file = files.iter().find(|f| !f.is_subagent).expect("parent file");
        let parent_extracted = extract_messages_from_jsonl(parent_file.provider, &parent_file.path);
        for msg in &parent_extracted.messages {
            assert!(
                !msg.is_sidechain,
                "parent transcript records must not be tagged sidechain"
            );
            assert!(msg.agent_id.is_none());
        }
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Workflow-Nested Sub-Agent Discovery]]
    #[test]
    fn discover_finds_flat_and_workflow_nested_subagents() {
        let tmp = TempDir::new().expect("tempdir");
        let session_id = "72acc77e-e91c-451f-80b2-748e85fffa1f";
        let project_dir = tmp.path().join("-home-mamba-work-cue-terraform");
        let subagents_dir = project_dir.join(session_id).join("subagents");
        let workflow_dir = subagents_dir.join("workflows").join("wf_b857afa7-e8e");
        fs::create_dir_all(&workflow_dir).expect("mkdir workflow subtree");

        // Parent transcript: <projectSlug>/<uuid>.jsonl.
        fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"uuid\":\"p1\",\"parentUuid\":null,\"sessionId\":\"{session_id}\",\"timestamp\":\"2026-07-10T05:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n"
            ),
        )
        .expect("write parent jsonl");

        // Flat sub-agent: <uuid>/subagents/agent-a.jsonl.
        fs::write(
            subagents_dir.join("agent-a.jsonl"),
            format!(
                "{{\"type\":\"user\",\"isSidechain\":true,\"agentId\":\"a\",\"parentUuid\":null,\"uuid\":\"s1\",\"sessionId\":\"{session_id}\",\"timestamp\":\"2026-07-10T05:01:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"task a\"}}}}\n"
            ),
        )
        .expect("write flat subagent jsonl");

        // Workflow-nested sub-agent with the leaner first-record shape (no
        // cwd/entrypoint/gitBranch/promptId/version):
        // <uuid>/subagents/workflows/wf_<id>/agent-b.jsonl.
        fs::write(
            workflow_dir.join("agent-b.jsonl"),
            format!(
                "{{\"type\":\"user\",\"isSidechain\":true,\"agentId\":\"b\",\"parentUuid\":null,\"uuid\":\"s2\",\"sessionId\":\"{session_id}\",\"timestamp\":\"2026-07-10T05:02:00Z\"}}\n"
            ),
        )
        .expect("write workflow subagent jsonl");

        // Decoy non-jsonl file anywhere in the subtree must be ignored.
        fs::write(
            workflow_dir.join("agent-b.meta.json"),
            r#"{"agentType":"x"}"#,
        )
        .expect("write decoy");

        let files =
            SessionIndex::discover_claude_session_files_in(tmp.path()).expect("discover ok");

        assert_eq!(
            files.len(),
            3,
            "expected parent + flat + workflow-nested transcripts, got {files:?}"
        );

        let parent = files.iter().filter(|f| !f.is_subagent).collect::<Vec<_>>();
        assert_eq!(parent.len(), 1, "exactly one parent transcript");
        assert!(
            parent[0]
                .path
                .to_string_lossy()
                .ends_with(&format!("{session_id}.jsonl"))
        );

        let flat = files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with("agent-a.jsonl"))
            .expect("flat sub-agent discovered");
        assert!(flat.is_subagent, "flat agent must be tagged is_subagent");

        let nested = files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with("agent-b.jsonl"))
            .expect("workflow-nested sub-agent discovered");
        assert!(
            nested.is_subagent,
            "workflow-nested agent must be tagged is_subagent"
        );
        assert!(
            nested
                .path
                .components()
                .any(|c| c.as_os_str() == "workflows"),
            "nested path must traverse subagents/workflows/"
        );

        assert!(
            !files
                .iter()
                .any(|f| f.path.to_string_lossy().ends_with("meta.json")),
            "non-jsonl decoy must be filtered out at any depth"
        );
    }
}
