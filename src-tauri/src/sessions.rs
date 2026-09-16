use std::collections::{HashMap, HashSet};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tantivy::collector::{Count, FacetCollector, TopDocs};
use tantivy::query::{BooleanQuery, Occur, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::*;
use tantivy::snippet::SnippetGenerator;
use tantivy::{DateTime, Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::integrations::IntegrationProvider;
use crate::transcript_identity::{
    JsonlRecord, ModelSourceFastFingerprint, model_source_fast_fingerprint, parse_jsonl_records,
    resolve_codex_native_identity,
};

const CLAUDE_SOURCE_ROOT_KEY: &str = "claude:projects";
const CODEX_SOURCE_ROOT_KEY: &str = "codex:sessions";
const PI_SOURCE_ROOT_KEY: &str = "pi:sessions";
const ROOT_DIAGNOSTIC_MAX_CHARS: usize = 240;
/// Search role for Pi `custom_message` entries: extension-injected context
/// that shaped the conversation without being a turn of it.
const PI_CUSTOM_MESSAGE_ROLE: &str = "custom_message";
pub const COMPACT_SEARCH_MAX_BYTES: usize = 32 * 1024;

/// One provider-owned filesystem root that may contain retained transcripts.
///
/// `resolved_root_path` records the path selected by `data_paths`, including
/// demo overrides. Existing roots also carry their canonical filesystem path.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderRootEnumerationOutcome {
    Complete,
    Failed { diagnostic: String },
}

/// A retained JSONL source discovered from a provider-owned filesystem root.
///
/// The source key is derived from provider-native identity. Claude and Codex
/// use provider-qualified canonical paths; Pi uses its header plus local host.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RetainedJsonlSourceLayoutHint {
    ClaudeParent { default_project: String },
    ClaudeSubagent { default_project: String },
    CodexTranscript,
    PiTranscript,
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
        IntegrationProvider::Pi => (
            PI_SOURCE_ROOT_KEY,
            crate::data_paths::resolve_pi_sessions_dir().map_err(|error| {
                log::warn!("Failed to resolve Pi transcript root: {error}");
                RetainedNotifySourceValidationError::Unavailable(
                    "Pi transcript validation is temporarily unavailable",
                )
            })?,
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
    if provider == IntegrationProvider::Pi
        && crate::pi_session::read_pi_session_header(&canonical_path).is_none()
    {
        return Err(RetainedNotifySourceValidationError::Invalid(
            "jsonl_path does not contain a supported Pi session header",
        ));
    }

    let layout_hint =
        retained_jsonl_source_layout_hint(provider, &canonical_root_path, &canonical_path).ok_or(
            RetainedNotifySourceValidationError::Invalid(
                "jsonl_path is outside the configured retained transcript layout",
            ),
        )?;

    let source_key = retained_source_key(
        provider,
        source_root_key,
        &canonical_path,
        (provider == IntegrationProvider::Pi)
            .then(SessionIndex::local_hostname)
            .as_deref(),
    )
    .ok_or(RetainedNotifySourceValidationError::Invalid(
        "jsonl_path does not contain a supported retained source identity",
    ))?;

    Ok(Some(DiscoveredRetainedJsonlSource {
        provider,
        source_root_key,
        source_key,
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
            [project, _session, subagents, ..]
                if *subagents == std::ffi::OsStr::new("subagents")
                    && is_claude_subagent_transcript(canonical_path) =>
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
        IntegrationProvider::Pi if !components.is_empty() => {
            Some(RetainedJsonlSourceLayoutHint::PiTranscript)
        }
        IntegrationProvider::Codex | IntegrationProvider::Pi | IntegrationProvider::MiniMax => None,
    }
}

/// Claude workflows store bookkeeping JSONL beside actual `agent-*.jsonl`
/// transcripts. Only the latter use the Claude transcript schema.
fn is_claude_subagent_transcript(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("agent-") && name.ends_with(".jsonl"))
}

/// Enumerate retained transcript roots independently.
///
/// This inventory is intentionally separate from Session Search discovery so
/// one unreadable provider root cannot suppress another provider.
pub(crate) fn enumerate_retained_jsonl_source_roots() -> Vec<ProviderSourceRoot> {
    vec![
        enumerate_claude_retained_jsonl_source_root(),
        enumerate_codex_retained_jsonl_source_root(),
        enumerate_pi_retained_jsonl_source_root(),
    ]
}

/// Stable identities for every configured retained transcript root.
///
/// Callers that serialize whole-root work use these identities before walking
/// the filesystem, so live reconciliation cannot interleave with inventory.
pub(crate) fn retained_jsonl_source_root_identities() -> Vec<(IntegrationProvider, &'static str)> {
    vec![
        (IntegrationProvider::Claude, CLAUDE_SOURCE_ROOT_KEY),
        (IntegrationProvider::Codex, CODEX_SOURCE_ROOT_KEY),
        (IntegrationProvider::Pi, PI_SOURCE_ROOT_KEY),
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

pub(crate) fn enumerate_pi_retained_jsonl_source_root() -> ProviderSourceRoot {
    match crate::data_paths::resolve_pi_sessions_dir() {
        Ok(path) => enumerate_provider_source_root(
            IntegrationProvider::Pi,
            PI_SOURCE_ROOT_KEY,
            path,
            collect_pi_jsonl_candidates,
        ),
        Err(error) => finish_provider_source_root(
            IntegrationProvider::Pi,
            PI_SOURCE_ROOT_KEY,
            PathBuf::new(),
            None,
            Some(format!("Pi transcript inventory unavailable: {error}")),
            Vec::new(),
        ),
    }
}

/// Admit every retained Claude/Codex transcript to the model-source queue.
///
/// App startup and Session Search scans both use this boundary so a completed
/// one-time backfill does not leave later retained files dependent on opening
/// Search. Enumeration and admission happen outside `IndexState`; the queue
/// performs blocking fingerprint/read work and preserves source identity.
pub(crate) fn enqueue_startup_model_source_reconciliation(app_handle: &tauri::AppHandle) {
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
            if provider == IntegrationProvider::Pi {
                continue;
            }
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
    ) -> Vec<DiscoveredSessionFile>,
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
    let pi_hostname = (provider == IntegrationProvider::Pi).then(SessionIndex::local_hostname);
    let mut sources = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let canonical_path = match std::fs::canonicalize(&candidate.path) {
            Ok(path) => path,
            Err(error) => {
                log_inventory_io_error(
                    provider,
                    "canonicalize JSONL source",
                    &candidate.path,
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

        let Some(source_key) = retained_source_key(
            provider,
            source_root_key,
            &canonical_path,
            pi_hostname.as_deref(),
        ) else {
            continue;
        };
        sources.push(DiscoveredRetainedJsonlSource {
            provider,
            source_root_key,
            source_key,
            filesystem_path: candidate.path,
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
) -> Vec<DiscoveredSessionFile> {
    let mut candidates = Vec::new();
    let Ok(project_entries) = read_directory_entries(projects_dir, provider, diagnostic) else {
        return candidates;
    };

    for project_entry in project_entries {
        let project_dir = project_entry.path();
        if !path_is_directory(&project_dir, provider, diagnostic, true) {
            continue;
        }
        let Ok(project_entries) = read_directory_entries(&project_dir, provider, diagnostic) else {
            continue;
        };

        for entry in project_entries {
            let path = entry.path();

            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                candidates.push(DiscoveredSessionFile {
                    path: path.clone(),
                    is_subagent: false,
                });
            }

            if !path_is_directory(&path, provider, diagnostic, true) {
                continue;
            }

            let subagents_dir = path.join("subagents");
            if !path_is_directory(&subagents_dir, provider, diagnostic, false) {
                continue;
            }

            let mut stack = vec![subagents_dir];
            let mut visited = HashSet::new();
            while let Some(directory) = stack.pop() {
                if let Ok(canonical) = std::fs::canonicalize(&directory)
                    && !visited.insert(canonical)
                {
                    continue;
                }
                let entries = match read_directory_entries(&directory, provider, diagnostic) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for entry in entries {
                    let subagent_path = entry.path();
                    if path_is_directory(&subagent_path, provider, diagnostic, true) {
                        stack.push(subagent_path);
                    } else if is_claude_subagent_transcript(&subagent_path) {
                        candidates.push(DiscoveredSessionFile {
                            path: subagent_path,
                            is_subagent: true,
                        });
                    }
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
) -> Vec<DiscoveredSessionFile> {
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
            candidates.push(DiscoveredSessionFile {
                path: entry.into_path(),
                is_subagent: false,
            });
        }
    }

    candidates
}

fn collect_pi_jsonl_candidates(
    sessions_dir: &Path,
    provider: IntegrationProvider,
    diagnostic: &mut Option<String>,
) -> Vec<DiscoveredSessionFile> {
    collect_codex_jsonl_candidates(sessions_dir, provider, diagnostic)
}

/// Enumerate every Claude transcript under `projects_dir`, flagging sub-agent
/// files.
///
/// Live snapshot scanning shares this walker with retained inventory so the
/// project tree is only ever traversed one way, and both see the same flat
/// parents plus the whole `subagents/` subtree.
pub(crate) fn discover_claude_transcripts_in(projects_dir: &Path) -> Vec<(PathBuf, bool)> {
    if !projects_dir.exists() {
        return Vec::new();
    }
    let mut diagnostic = None;
    collect_claude_jsonl_candidates(projects_dir, IntegrationProvider::Claude, &mut diagnostic)
        .into_iter()
        .map(|candidate| (candidate.path, candidate.is_subagent))
        .collect()
}

/// Enumerate every Codex rollout under `sessions_dir`.
///
/// Live snapshot scanning shares this walker with retained inventory so the
/// date-partitioned session tree is only ever traversed one way.
pub(crate) fn discover_codex_transcripts_in(sessions_dir: &Path) -> Vec<PathBuf> {
    if !sessions_dir.exists() {
        return Vec::new();
    }
    let mut diagnostic = None;
    collect_codex_jsonl_candidates(sessions_dir, IntegrationProvider::Codex, &mut diagnostic)
        .into_iter()
        .map(|candidate| candidate.path)
        .collect()
}

/// Enumerate every Pi session file under `sessions_dir`.
///
/// Live snapshot scanning shares this walker with retained inventory so the
/// per-cwd tree and the nested child runs under it are only ever traversed one
/// way.
pub(crate) fn discover_pi_transcripts_in(sessions_dir: &Path) -> Vec<PathBuf> {
    if !sessions_dir.exists() {
        return Vec::new();
    }
    let mut diagnostic = None;
    collect_pi_jsonl_candidates(sessions_dir, IntegrationProvider::Pi, &mut diagnostic)
        .into_iter()
        .map(|candidate| candidate.path)
        .collect()
}

/// A Codex rollout filename ends with the 36-character thread uuid.
const CODEX_THREAD_ID_LEN: usize = 36;

/// Root session id for a Claude transcript: the file stem for a parent, and the
/// directory holding `subagents/` for a sub-agent at any depth.
///
/// Retained inventory and live snapshot scanning both need this rule, so it
/// lives beside the walker that classifies the layout rather than being
/// restated by each consumer.
pub(crate) fn claude_root_session_id(path: &Path, is_subagent: bool) -> Option<String> {
    let name = if is_subagent {
        path.ancestors()
            .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "subagents"))?
            .parent()?
            .file_name()?
    } else {
        path.file_stem()?
    };
    name.to_str().map(str::to_owned)
}

/// Thread id from a `rollout-<timestamp>-<thread id>.jsonl` filename.
///
/// Taking the trailing uuid rather than skipping a fixed timestamp width means a
/// malformed name is rejected instead of yielding a truncated id. The id is only
/// a locator: identity always comes from the rollout's own `session_meta`.
pub(crate) fn codex_thread_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let rest = stem.strip_prefix("rollout-")?;
    let thread_id = rest.get(rest.len().checked_sub(CODEX_THREAD_ID_LEN)?..)?;
    thread_id
        .chars()
        .all(|character| character.is_ascii_hexdigit() || character == '-')
        .then(|| thread_id.to_owned())
}

fn read_directory_entries(
    directory: &Path,
    provider: IntegrationProvider,
    diagnostic: &mut Option<String>,
) -> Result<Vec<std::fs::DirEntry>, std::io::Error> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            log_inventory_io_error(provider, "read transcript directory", directory, &error);
            record_root_failure(
                diagnostic,
                provider,
                "could not read all transcript directories.",
            );
            return Err(error);
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
    Ok(collected)
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

fn retained_source_key(
    provider: IntegrationProvider,
    source_root_key: &str,
    canonical_path: &Path,
    pi_hostname: Option<&str>,
) -> Option<String> {
    if provider == IntegrationProvider::Pi {
        let header = crate::pi_session::read_pi_session_header(canonical_path)?;
        return crate::storage::pi_source_key(pi_hostname?, &header.id).ok();
    }
    Some(canonical_source_key(source_root_key, canonical_path))
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
    output.push_str(&crate::hex_encode(bytes));
}

// ---------------------------------------------------------------------------
// Schema fields wrapper
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SessionSchema {
    pub provider: Field,
    pub message_id: Field,
    pub session_id: Field,
    pub parent_session_id: Field,
    pub content: Field,
    pub role: Field,
    pub project: Field,
    pub project_path: Field,
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
    pub custom_type: Field,
}

// ---------------------------------------------------------------------------
// Index state -- tracks which sources have been indexed and their fingerprints
// ---------------------------------------------------------------------------

/// Notify-only display evidence; absence of the entire hint set means watcher
/// admission and preserves the last applied hints. A supplied None parent clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SearchSourceHints {
    pub(crate) parent_session_id: Option<String>,
    pub(crate) git_branch: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) host: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct SourceSearch<'a> {
    pub(crate) index: &'a SessionIndex,
    pub(crate) hints: Option<&'a SearchSourceHints>,
    pub(crate) indexed: Option<&'a std::cell::Cell<usize>>,
}

/// What the sweep remembers about one indexed source.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct IndexedSource {
    /// Nanosecond mtime and size at the time of the last extraction.
    #[serde(flatten)]
    pub(crate) fingerprint: ModelSourceFastFingerprint,
    /// Provider-native session id the last extraction produced; empty when
    /// identity never resolved, in which case there is nothing to prune.
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) hints: Option<SearchSourceHints>,
    #[serde(default)]
    pub(crate) canonical_path: Option<PathBuf>,
}

/// Persisted beside the Tantivy segments as `index_state.json`.
///
/// Keyed by canonical retained source key, the same identity analytics and
/// the live coordinator use, so a provider is recoverable from the key prefix
/// and a moved or re-mounted root does not orphan every entry. The index
/// directory is rebuilt whenever `SCHEMA_VERSION` moves, so this shape never
/// needs to read an older layout.
#[derive(Serialize, Deserialize, Default)]
pub struct IndexState {
    #[serde(default)]
    pub(crate) sources: HashMap<String, IndexedSource>,
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
    const SCHEMA_VERSION: u32 = 9;
    const PRODUCTION_WRITER_HEAP_BYTES: usize = 50_000_000;
    #[cfg(test)]
    const TEST_WRITER_HEAP_BYTES: usize = 15_000_000;

    /// Open an existing index or create a new one at the given directory.
    pub fn open_or_create(index_dir: &Path) -> Result<Self, String> {
        Self::open_or_create_with_writer_heap(index_dir, Self::PRODUCTION_WRITER_HEAP_BYTES)
    }

    #[cfg(test)]
    pub(crate) fn open_or_create_for_tests(index_dir: &Path) -> Result<Self, String> {
        // Tantivy selects one worker at its 15 MB minimum, so parallel index
        // tests do not multiply production's three-worker writer footprint.
        Self::open_or_create_with_writer_heap(index_dir, Self::TEST_WRITER_HEAP_BYTES)
    }

    fn open_or_create_with_writer_heap(
        index_dir: &Path,
        writer_heap_bytes: usize,
    ) -> Result<Self, String> {
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
            .writer(writer_heap_bytes)
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

    /// Build the Tantivy schema.
    fn build_schema() -> (Schema, SessionSchema) {
        let mut builder = Schema::builder();

        // STRING | STORED fields (untokenized, exact-match, stored)
        let provider = builder.add_text_field("provider", STRING | STORED);
        let message_id = builder.add_text_field("message_id", STRING | STORED);
        let session_id = builder.add_text_field("session_id", STRING | STORED);
        let parent_session_id = builder.add_text_field("parent_session_id", STRING | STORED);
        let role = builder.add_text_field("role", STRING | STORED);
        let git_branch = builder.add_text_field("git_branch", STRING | STORED);
        let custom_type = builder.add_text_field("custom_type", STRING | STORED);

        // TEXT | STORED fields (tokenized, full-text searchable, stored)
        let content = builder.add_text_field("content", TEXT | STORED);
        let tools_used = builder.add_text_field("tools_used", TEXT | STORED);
        let files_modified = builder.add_text_field("files_modified", TEXT | STORED);
        let code_changes = builder.add_text_field("code_changes", TEXT | STORED);
        let commands_run = builder.add_text_field("commands_run", TEXT | STORED);
        let tool_details = builder.add_text_field("tool_details", TEXT | STORED);
        let display_text = builder.add_text_field("display_text", TEXT | STORED);

        // Facet fields (hierarchical)
        let project = builder.add_facet_field("project", FacetOptions::default().set_stored());
        let project_path = builder.add_text_field("project_path", STRING);
        let host = builder.add_facet_field("host", FacetOptions::default().set_stored());
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
            parent_session_id,
            content,
            role,
            project,
            project_path,
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
            custom_type,
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

    /// Persist committed checkpoints once at the owning batch boundary.
    /// Keep serialization and replacement under the same lock so an older
    /// concurrent flush cannot overwrite newer checkpoints.
    pub fn save_state(&self) -> Result<(), String> {
        use std::io::Write;

        let state_path = self.index_dir.join("index_state.json");
        let state = self.state.lock().unwrap();
        let mut file = tempfile::NamedTempFile::new_in(&self.index_dir)
            .map_err(|e| format!("Stage index state: {e}"))?;
        {
            let mut output = std::io::BufWriter::new(file.as_file_mut());
            serde_json::to_writer_pretty(&mut output, &*state)
                .map_err(|e| format!("Serialize state: {e}"))?;
            output.flush().map_err(|e| format!("Write state: {e}"))?;
        }
        file.as_file()
            .sync_all()
            .map_err(|e| format!("Flush state: {e}"))?;
        file.persist(state_path)
            .map_err(|e| format!("Replace state: {e}"))?;
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
        if let Some(parent_session_id) = &msg.parent_session_id {
            doc.add_text(self.fields.parent_session_id, parent_session_id);
        }
        doc.add_text(self.fields.content, &msg.content);
        doc.add_text(self.fields.role, &msg.role);
        if let Some(custom_type) = msg.custom_type.as_deref().filter(|value| !value.is_empty()) {
            doc.add_text(self.fields.custom_type, custom_type);
        }
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
        if let Some(cwd) = msg.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
            doc.add_text(self.fields.project_path, cwd);
        }
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
        let mut writer = self.writer.lock().unwrap();
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
        let mut writer = self.writer.lock().unwrap();
        for msg in messages {
            self.add_message_to_writer(&writer, provider, msg, project_facet, host_facet)?;
        }
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(messages.len())
    }

    pub(crate) fn local_hostname() -> String {
        fn short(hostname: String) -> Option<String> {
            hostname
                .trim()
                .split('.')
                .next()
                .filter(|hostname| !hostname.is_empty())
                .map(str::to_owned)
        }

        std::env::var("HOSTNAME")
            .ok()
            .and_then(short)
            .or_else(|| std::env::var("COMPUTERNAME").ok().and_then(short))
            .or_else(|| {
                std::fs::read_to_string("/etc/hostname")
                    .ok()
                    .and_then(short)
            })
            .or_else(|| {
                crate::config::external_command("hostname")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .and_then(short)
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Sync the index with every retained Claude, Codex, and Pi transcript.
    ///
    /// Runs on every watcher pass and on the `sync_search_index` command, not
    /// only at startup. Returns the number of newly indexed messages.
    pub fn sync(&self, app_handle: &tauri::AppHandle) -> Result<usize, String> {
        let roots = enumerate_retained_jsonl_source_roots();
        self.sync_with_roots(app_handle, &roots)
    }

    pub(crate) fn sync_with_roots(
        &self,
        app_handle: &tauri::AppHandle,
        roots: &[ProviderSourceRoot],
    ) -> Result<usize, String> {
        use tauri::Emitter;

        let (mut total_indexed, analytics_changed) = crate::get_storage()
            .map(|storage| self.sync_retained_sources(storage, roots))
            .unwrap_or_default();
        let search = self.sync_inner(roots);
        // Search or checkpoint failure must not hide an analytics commit.
        if analytics_changed {
            let _ = app_handle.emit(crate::TRANSCRIPT_ANALYTICS_UPDATED_EVENT, ());
        }
        // A prune error must not discard checkpoints already committed by
        // sync_retained_sources earlier in this pass.
        if search.is_err() {
            self.save_state()?;
        }
        total_indexed += search?;
        let _ = app_handle.emit("sessions-index-updated", total_indexed);
        Ok(total_indexed)
    }

    fn sync_retained_sources(
        &self,
        storage: &crate::storage::Storage,
        roots: &[ProviderSourceRoot],
    ) -> (usize, bool) {
        let mut total_indexed = 0;
        let mut analytics_changed = false;
        for source in roots.iter().flat_map(|root| &root.sources) {
            // Recovery owns analytics-only retries. An unchanged Search
            // sweep must not issue a live registry refresh for every source.
            if std::fs::metadata(&source.canonical_path)
                .ok()
                .and_then(|metadata| model_source_fast_fingerprint(&metadata).ok())
                .is_some_and(|fingerprint| self.source_is_current(source, fingerprint))
            {
                continue;
            }
            let (analytics, search) = crate::transcript_analytics::reconcile_retained_source(
                storage,
                source,
                &Self::local_hostname(),
                Some(self),
            );
            match analytics {
                Ok(crate::transcript_analytics::TranscriptSourceResult::Replaced) => {
                    analytics_changed = true
                }
                Ok(_) => {}
                Err(error) => log::warn!("Retained source analytics retry: {error}"),
            }
            match search {
                Ok(count) => total_indexed += count,
                Err(error) => log::warn!("Retained source Search retry: {error}"),
            }
        }
        (total_indexed, analytics_changed)
    }

    #[cfg(test)]
    pub(crate) fn sync_without_emit(&self) -> Result<usize, String> {
        let roots = enumerate_retained_jsonl_source_roots();
        self.sync_inner(&roots)
    }

    /// One pass over the retained inventory: prune documents of sources that
    /// vanished from a completely enumerated root, then re-extract every
    /// source whose mtime or size moved since it was last indexed.
    ///
    /// Derived analytics rows are owned by retained reconciliation, so this
    /// sweep never consults migration re-ingest markers: a schema bump on the
    /// index directory is the one way to force a full re-extract.
    // @lat: [[data-flow#Session Indexing Pipeline]]
    fn sync_inner(&self, roots: &[ProviderSourceRoot]) -> Result<usize, String> {
        // Pruning uses only a complete inventory, never a parse failure.
        {
            let mut state = self.state.lock().unwrap();
            let discovered = roots
                .iter()
                .flat_map(|root| &root.sources)
                .map(|source| source.source_key.as_str())
                .collect::<HashSet<_>>();
            let vanished = state
                .sources
                .iter()
                .filter_map(|(key, indexed)| {
                    roots
                        .iter()
                        .find(|root| {
                            matches!(root.outcome, ProviderRootEnumerationOutcome::Complete)
                                && key.starts_with(root.source_root_key)
                                && !discovered.contains(key.as_str())
                                // A live commit may postdate this inventory. Only
                                // proven absence can prune its new checkpoint.
                                && !indexed.canonical_path.as_ref().is_some_and(|path| {
                                    path.starts_with(root.canonical_root_path.as_ref().unwrap_or(&root.resolved_root_path))
                                        && !matches!(path.try_exists(), Ok(false))
                                        && (root.provider != IntegrationProvider::Pi
                                            || crate::pi_session::read_pi_session_header(path)
                                                .is_none_or(|header| header.id == indexed.session_id))
                                })
                        })
                        .map(|root| (key.clone(), root.provider, indexed.session_id.clone()))
                })
                .collect::<Vec<_>>();
            if !vanished.is_empty() {
                let mut writer = self.writer.lock().unwrap();
                let result = (|| {
                    for (_, provider, session_id) in &vanished {
                        self.delete_session_docs_with_writer(&writer, *provider, session_id)?;
                    }
                    writer
                        .commit()
                        .map_err(|error| format!("Commit index prune: {error}"))?;
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    writer
                        .rollback()
                        .map_err(|rollback| format!("{error}; rollback: {rollback}"))?;
                    return Err(error);
                }
                for (key, _, _) in vanished {
                    state.sources.remove(&key);
                }
            }
        }
        let mut total_indexed = 0;
        for source in roots.iter().flat_map(|root| &root.sources) {
            match self.sync_source(source, &Self::local_hostname()) {
                Ok(count) => total_indexed += count,
                Err(error) => log::warn!("Search source retained last-good documents: {error}"),
            }
        }
        self.save_state()?;
        Ok(total_indexed)
    }

    pub(crate) fn source_is_current(
        &self,
        source: &DiscoveredRetainedJsonlSource,
        fingerprint: ModelSourceFastFingerprint,
    ) -> bool {
        self.state
            .lock()
            .unwrap()
            .sources
            .get(&source.source_key)
            .is_some_and(|indexed| {
                indexed.fingerprint == fingerprint
                    && indexed.canonical_path.as_ref() == Some(&source.canonical_path)
            })
    }

    pub(crate) fn sync_source(
        &self,
        source: &DiscoveredRetainedJsonlSource,
        hostname: &str,
    ) -> Result<usize, String> {
        self.sync_source_with_hints(source, hostname, None)
    }

    pub(crate) fn sync_source_with_hints(
        &self,
        source: &DiscoveredRetainedJsonlSource,
        hostname: &str,
        hints: Option<&SearchSourceHints>,
    ) -> Result<usize, String> {
        crate::transcript_work::with_source(|| {
            let fingerprint = std::fs::metadata(&source.canonical_path)
                .map_err(|error| error.to_string())
                .and_then(|metadata| {
                    model_source_fast_fingerprint(&metadata).map_err(|error| error.to_string())
                })?;
            if self.source_is_current(source, fingerprint)
                && hints.is_none_or(|hints| {
                    self.state
                        .lock()
                        .unwrap()
                        .sources
                        .get(&source.source_key)
                        .is_some_and(|indexed| indexed.hints.as_ref() == Some(hints))
                })
            {
                return Ok(0);
            }
            let (mut extracted, fingerprint) =
                read_extracted_session(source.provider, &source.canonical_path)?;
            self.replace_retained_source(source, fingerprint, &mut extracted, hostname, hints)
        })
    }

    /// The in-memory checkpoint advances only after a committed stable replacement.
    /// Owning sweep/drain batches persist it with save_state, including partial
    /// successes. A crash before that flush safely replays idempotent replacements.
    pub(crate) fn replace_retained_source(
        &self,
        source: &DiscoveredRetainedJsonlSource,
        fingerprint: ModelSourceFastFingerprint,
        extracted: &mut ExtractedSession,
        hostname: &str,
        hints: Option<&SearchSourceHints>,
    ) -> Result<usize, String> {
        let observed = std::fs::metadata(&source.canonical_path)
            .map_err(|error| error.to_string())
            .and_then(|metadata| {
                model_source_fast_fingerprint(&metadata).map_err(|error| error.to_string())
            })?;
        if observed != fingerprint {
            return Err("Search source changed before replacement".into());
        }
        let mut state = self.state.lock().unwrap();
        if extracted.session_id.is_empty() {
            if state
                .sources
                .get(&source.source_key)
                .is_some_and(|previous| !previous.session_id.is_empty())
            {
                return Err("Search source lost its native identity".into());
            }
            state.sources.insert(
                source.source_key.clone(),
                IndexedSource {
                    fingerprint,
                    session_id: String::new(),
                    hints: hints.cloned(),
                    canonical_path: Some(source.canonical_path.clone()),
                },
            );
            return Ok(0);
        }
        let hints = hints.cloned().or_else(|| {
            state
                .sources
                .get(&source.source_key)
                .and_then(|indexed| indexed.hints.clone())
        });
        if state
            .sources
            .get(&source.source_key)
            .is_some_and(|indexed| {
                indexed.fingerprint == fingerprint
                    && indexed.hints == hints
                    && indexed.canonical_path.as_ref() == Some(&source.canonical_path)
            })
        {
            return Ok(0);
        }
        if let Some(hints) = &hints {
            for message in &mut extracted.messages {
                if source.provider == IntegrationProvider::Pi {
                    message
                        .parent_session_id
                        .clone_from(&hints.parent_session_id);
                }
                if message.git_branch.is_empty()
                    && let Some(branch) = &hints.git_branch
                {
                    message.git_branch.clone_from(branch);
                }
            }
        }
        let hostname = hints
            .as_ref()
            .and_then(|hints| hints.host.as_deref())
            .filter(|host| !host.is_empty())
            .unwrap_or(hostname);
        let project = hints
            .as_ref()
            .and_then(|hints| hints.project.as_deref())
            .or(extracted.project_name.as_deref())
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown");
        let mut writer = self.writer.lock().unwrap();
        let result = (|| {
            if let Some(previous) = state.sources.get(&source.source_key)
                && previous.session_id != extracted.session_id
            {
                self.delete_session_docs_with_writer(
                    &writer,
                    source.provider,
                    &previous.session_id,
                )?;
            }
            self.delete_session_docs_with_writer(&writer, source.provider, &extracted.session_id)?;
            for message in &extracted.messages {
                self.add_message_to_writer(&writer, source.provider, message, project, hostname)?;
            }
            writer
                .commit()
                .map_err(|error| format!("Commit index: {error}"))?;
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            writer
                .rollback()
                .map_err(|rollback| format!("{error}; rollback: {rollback}"))?;
            return Err(error);
        }
        state.sources.insert(
            source.source_key.clone(),
            IndexedSource {
                fingerprint,
                session_id: extracted.session_id.clone(),
                hints,
                canonical_path: Some(source.canonical_path.clone()),
            },
        );
        Ok(extracted.messages.len())
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
        let non_pi = BooleanQuery::new(vec![
            (Occur::Must, Box::new(tantivy::query::AllQuery)),
            (
                Occur::MustNot,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.provider, IntegrationProvider::Pi.as_str()),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        clauses.push((
            Occur::Must,
            Box::new(BooleanQuery::new(vec![
                (Occur::Should, Box::new(non_pi)),
                (
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_text(f.role, "user"),
                        IndexRecordOption::Basic,
                    )),
                ),
                (
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_text(f.role, "assistant"),
                        IndexRecordOption::Basic,
                    )),
                ),
                (
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_text(f.role, PI_CUSTOM_MESSAGE_ROLE),
                        IndexRecordOption::Basic,
                    )),
                ),
            ])),
        ));

        // Project facet filter
        if let Some(ref proj) = filters.project {
            let facet = Facet::from(&format!("/{proj}"));
            let facet_query = Box::new(TermQuery::new(
                Term::from_facet(f.project, &facet),
                IndexRecordOption::Basic,
            ));
            if Path::new(proj).is_absolute() {
                let path_query = Box::new(TermQuery::new(
                    Term::from_field_text(f.project_path, proj),
                    IndexRecordOption::Basic,
                ));
                clauses.push((
                    Occur::Must,
                    Box::new(BooleanQuery::new(vec![
                        (Occur::Should, path_query),
                        (Occur::Should, facet_query),
                    ])),
                ));
            } else {
                clauses.push((Occur::Must, facet_query));
            }
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
                parent_session_id: doc
                    .get_first(f.parent_session_id)
                    .and_then(|value| value.as_value().as_str().map(str::to_owned)),
                content: get_text(f.content),
                snippet: snippet_html,
                role: get_text(f.role),
                project: get_facet_str(f.project),
                session_name: None,
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
        crate::transcript_work::with_source(|| {
            let path = find_session_path(provider, session_id)?
                .ok_or_else(|| format!("JSONL file not found for session {session_id}"))?;
            let (extracted, _) = read_extracted_session(provider, &path)?;
            let project_name = extracted.project_name.unwrap_or_default();
            let messages = extracted.messages;

            // Find the index of the target message
            let target_idx = messages
                .iter()
                .position(|m| m.uuid == message_id)
                .unwrap_or(0);

            let start = target_idx.saturating_sub(window);
            let end = target_idx
                .saturating_add(window)
                .saturating_add(1)
                .min(messages.len());

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

            // Transfer the explicitly requested wire response, not the raw
            // transcript/other rows. Response byte limits remain an API concern.
            Ok(SessionContext {
                provider,
                session_id: session_id.to_string(),
                project: project_name,
                session_name: None,
                messages: context_messages,
            })
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
    pub parent_session_id: Option<String>,
    pub content: String,
    pub snippet: String,
    pub role: String,
    pub project: String,
    /// Display name the provider gave this session, when one is persisted.
    /// Filled from the analytics registry after the index query returns.
    pub session_name: Option<String>,
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

impl SearchResults {
    pub fn compact_for_ai(&self, max_bytes: usize) -> serde_json::Value {
        let mut hits = Vec::new();
        let mut truncated_response = false;

        // ponytail: page size is capped at 100; use incremental accounting if it grows.
        for hit in &self.hits {
            hits.push(serde_json::json!({
                "provider": hit.provider,
                "message_id": truncate(&hit.message_id, 512),
                "session_id": truncate(&hit.session_id, 512),
                "parent_session_id": hit
                    .parent_session_id
                    .as_deref()
                    .map(|value| truncate(value, 512)),
                "snippet": truncate(&hit.snippet, 2_048),
                "role": truncate(&hit.role, 32),
                "project": truncate(&hit.project, 512),
                "session_name": hit
                    .session_name
                    .as_deref()
                    .map(|name| truncate(name, 512)),
                "host": truncate(&hit.host, 512),
                "timestamp": truncate(&hit.timestamp, 64),
                "git_branch": truncate(&hit.git_branch, 512),
                "score": hit.score,
            }));
            let candidate = serde_json::json!({
                "hits": &hits,
                "total_hits": self.total_hits,
                "query_time_ms": self.query_time_ms,
                "truncated": false,
            });
            if serde_json::to_vec(&candidate).is_ok_and(|encoded| encoded.len() > max_bytes) {
                hits.pop();
                truncated_response = true;
                break;
            }
        }

        if hits.len() < self.hits.len() {
            truncated_response = true;
        }
        serde_json::json!({
            "hits": hits,
            "total_hits": self.total_hits,
            "query_time_ms": self.query_time_ms,
            "truncated": truncated_response,
        })
    }
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
    /// Display name the provider gave this session, when one is persisted.
    pub session_name: Option<String>,
    pub messages: Vec<ContextMessage>,
}

#[derive(Debug)]
struct DiscoveredSessionFile {
    path: PathBuf,
    /// True when this file lives under `<session>/subagents/agent-*.jsonl`.
    /// Hints the extractor to expect every record to carry isSidechain=true
    /// and lets the indexer treat sub-agent rows as part of the parent
    /// session_id while still tagging them for tree roll-ups. The per-record
    /// `isSidechain` field drives DB writes; live transcript scanning uses this
    /// flag to tell a root transcript from an agent one.
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
    pub is_error: Option<bool>,
    pub details_json: Option<String>,
    pub result_image_count: Option<i64>,
    /// Wall-clock span from a persisted Pi `tool_span` receipt; NULL elsewhere.
    pub duration_ms: Option<i64>,
    // Lines added/removed for `code_change` actions, computed at ingest from the
    // FULL (untruncated) tool input before `full_input` is capped at 10KB. NULL
    // for non-code-change actions and legacy rows ingested before migration 33.
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
    pub timestamp: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SkillAccess {
    pub skill_name: String,
    pub skill_path: String,
}

/// Event-kind classification used by the runtime-event pipeline. Five
/// variants matching specs/008-runtime-redesign/contracts/session-events.md
/// (EVT-CL-2..EVT-CL-5 and EVT-CX-1).
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
pub struct ExtractedEvent {
    pub source_ordinal: u64,
    pub event_ordinal: usize,
    pub timestamp: String,
    pub kind: SessionEventKind,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
}

pub struct ExtractedMessage {
    pub uuid: String,
    pub session_id: String,
    pub parent_session_id: Option<String>,
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
    pub parent_uuid: Option<String>,
    /// Working directory at the time of the message. Claude reads it from the
    /// top-level `cwd` field on each JSONL row; Codex reads it once from
    /// `session_meta.payload.cwd`. None if not present in the transcript.
    pub cwd: Option<String>,
    /// Pi's `customType` for injected-context messages (role
    /// `custom_message`). None for every conversation message.
    pub custom_type: Option<String>,
}

#[derive(Default)]
pub struct ExtractedSession {
    /// Provider-native session id; empty when identity could not be resolved
    /// from the file, in which case `messages` is empty too.
    pub session_id: String,
    pub project_name: Option<String>,
    pub messages: Vec<ExtractedMessage>,
    /// Per-event timeline emitted alongside [`messages`] for the active-
    /// interval runtime pipeline (feature 008). Populated by
    /// [`extract_claude_messages_from_jsonl_records`] and
    /// [`extract_codex_messages_from_jsonl_records`] in the same parse pass.
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

const TOOL_RESULT_DETAILS_MAX_BYTES: usize = 10_240;

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

fn pi_tool_result_details(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_object)
        .and_then(|details| serde_json::to_string(details).ok())
        .filter(|details| details.len() <= TOOL_RESULT_DETAILS_MAX_BYTES)
}

fn pi_tool_result_image_count(content: Option<&serde_json::Value>) -> i64 {
    content
        .and_then(serde_json::Value::as_array)
        .map_or(0, |blocks| {
            blocks
                .iter()
                .filter(|block| {
                    block.get("type").and_then(serde_json::Value::as_str) == Some("image")
                })
                .count() as i64
        })
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
        "MultiEdit" => {
            let file_path = get_str("file_path");
            let edit_count = inp
                .and_then(|o| o.get("edits"))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let summary = format!("MultiEdit {file_path}: {edit_count} edits");
            ("code_change".to_string(), summary, Some(file_path))
        }
        "NotebookEdit" => {
            let notebook_path = get_str("notebook_path");
            let mode = get_str("edit_mode");
            let mode = if mode.is_empty() {
                "replace".to_string()
            } else {
                mode
            };
            let summary = format!("NotebookEdit {notebook_path} ({mode})");
            ("code_change".to_string(), summary, Some(notebook_path))
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
        _ if tool_name.starts_with("mcp__")
            && inp.is_some_and(|o| mcp_file_write_lines(o).is_some()) =>
        {
            let file_path = ["file_path", "path"]
                .into_iter()
                .map(get_str)
                .find(|value| !value.is_empty());
            let summary = match &file_path {
                Some(path) => format!("{tool_name}: {path}"),
                None => tool_name.to_string(),
            };
            ("code_change".to_string(), summary, file_path)
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
        "apply_patch" => {
            // Codex can surface `apply_patch` as a function call whose patch body
            // rides in the arguments JSON; classify it like the custom-tool-call
            // form so its lines are counted.
            let patch = extract_function_apply_patch_text(arguments);
            let files = extract_apply_patch_files(&patch);
            let file_path = files.first().cloned();
            let summary = if files.is_empty() {
                "Patch".to_string()
            } else {
                format!("Patch {}", truncate(&files.join(", "), 160))
            };
            ("code_change".to_string(), summary, file_path)
        }
        "exec_command" => {
            let command = get_str("cmd");
            ("command".to_string(), format!("$ {command}"), None)
        }
        "write_stdin" => {
            let chars = truncate(&get_str("chars"), 120);
            ("command".to_string(), format!("stdin {chars}"), None)
        }
        _ if tool_name.starts_with("mcp__") => {
            let is_file_write = input.is_some_and(|o| mcp_file_write_lines(o).is_some());
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
            let category = if is_file_write {
                "code_change"
            } else {
                "tool_detail"
            };
            (category.to_string(), summary, file_path)
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

/// Count (added, removed) lines from an MCP tool input that carries a
/// recognizable file-write shape. Returns `None` when the shape is not clearly
/// a file write/edit, so callers stay conservative about classifying MCP tools
/// as code changes. Shared by both classifier and line-count paths so the two
/// stay in agreement.
pub(crate) fn mcp_file_write_lines(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<(i64, i64)> {
    let as_str = |key: &str| obj.get(key).and_then(|v| v.as_str());

    // old/new string edit pair (Edit-like MCP tools).
    if let (Some(old), Some(new)) = (as_str("old_string"), as_str("new_string")) {
        return Some((new.lines().count() as i64, old.lines().count() as i64));
    }

    // path + content write (Write-like MCP tools). Only when a non-empty path
    // accompanies a content field, to avoid counting unrelated payloads.
    let has_path = as_str("file_path").is_some_and(|s| !s.is_empty())
        || as_str("path").is_some_and(|s| !s.is_empty());
    if has_path && let Some(content) = as_str("content") {
        return Some((content.lines().count() as i64, 0));
    }

    None
}

/// Count (added, removed) lines for a structured code-change tool input,
/// operating on the FULL untruncated value. Returns `None` for shapes that are
/// not code changes. Kept in sync with [`crate::storage::parse_code_change`],
/// which performs the same counting for legacy rows from `full_input`.
pub(crate) fn count_code_change_lines(
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<(i64, i64)> {
    let obj = input.as_object()?;
    let as_str = |key: &str| obj.get(key).and_then(|v| v.as_str());

    match tool_name {
        "Edit" => {
            let old = as_str("old_string")?;
            let new = as_str("new_string")?;
            Some((new.lines().count() as i64, old.lines().count() as i64))
        }
        "Write" => {
            let content = as_str("content")?;
            Some((content.lines().count() as i64, 0))
        }
        "MultiEdit" => {
            let edits = obj.get("edits").and_then(|v| v.as_array())?;
            let mut added = 0i64;
            let mut removed = 0i64;
            for edit in edits {
                let Some(edit_obj) = edit.as_object() else {
                    continue;
                };
                if let Some(new) = edit_obj.get("new_string").and_then(|v| v.as_str()) {
                    added += new.lines().count() as i64;
                }
                if let Some(old) = edit_obj.get("old_string").and_then(|v| v.as_str()) {
                    removed += old.lines().count() as i64;
                }
            }
            Some((added, removed))
        }
        "NotebookEdit" => {
            let new_source = as_str("new_source").unwrap_or("");
            let lines = new_source.lines().count() as i64;
            match as_str("edit_mode").unwrap_or("replace") {
                // Deleting a cell removes its content; without the old source we
                // conservatively count the removed lines and add nothing.
                "delete" => Some((0, lines)),
                _ => Some((lines, 0)),
            }
        }
        _ if tool_name.starts_with("mcp__") => mcp_file_write_lines(obj),
        _ => None,
    }
}

/// Count (added, removed) lines from an `apply_patch` patch body. Mirrors
/// [`crate::storage::parse_code_change`]'s apply-patch counting so ingest-time
/// and legacy-fallback totals match.
pub(crate) fn count_apply_patch_lines(patch: &str) -> Option<(i64, i64)> {
    let mut added = 0i64;
    let mut removed = 0i64;
    let mut mode = "";
    let mut saw_file = false;

    for line in patch.lines() {
        if line.strip_prefix("*** Add File: ").is_some() {
            mode = "add";
            saw_file = true;
            continue;
        }
        if line.strip_prefix("*** Update File: ").is_some() {
            mode = "update";
            saw_file = true;
            continue;
        }
        if line.strip_prefix("*** Delete File: ").is_some() {
            mode = "delete";
            saw_file = true;
            continue;
        }
        if line.starts_with("*** ") {
            mode = "";
            continue;
        }
        match mode {
            "add" if line.starts_with('+') => added += 1,
            "update" if line.starts_with('+') => added += 1,
            "update" if line.starts_with('-') => removed += 1,
            _ => {}
        }
    }

    if !saw_file && added == 0 && removed == 0 {
        return None;
    }
    Some((added, removed))
}

/// Extract the patch body from a Codex `apply_patch` function-call `arguments`
/// JSON. Codex sends the patch under `input` (mirroring the custom-tool-call
/// shape) or `patch`; if neither is present but the raw arguments already look
/// like a patch, fall back to the raw string.
fn extract_function_apply_patch_text(arguments: &str) -> String {
    if let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(arguments)
    {
        for key in ["input", "patch"] {
            if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
                return text.to_string();
            }
        }
    }
    arguments.to_string()
}

/// Count (added, removed) lines for a Codex function-call code change from its
/// full `arguments` string: `apply_patch` patch bodies and structured MCP
/// file-write inputs.
fn codex_function_change_lines(tool_name: &str, arguments: &str) -> Option<(i64, i64)> {
    if tool_name == "apply_patch" {
        let patch = extract_function_apply_patch_text(arguments);
        return count_apply_patch_lines(&patch);
    }
    let parsed: serde_json::Value = serde_json::from_str(arguments).ok()?;
    count_code_change_lines(tool_name, &parsed)
}

/// Count (added, removed) lines for a Codex custom-tool-call code change. The
/// only code-change custom tool is `apply_patch`, whose raw patch body is the
/// `input` string.
fn codex_custom_change_lines(tool_name: &str, input: &str) -> Option<(i64, i64)> {
    if tool_name == "apply_patch" {
        return count_apply_patch_lines(input);
    }
    None
}

pub fn extract_skill_accesses_from_tool_action(action: &ToolAction) -> Vec<SkillAccess> {
    let mut paths = Vec::new();

    // Pi names the same two tools in lowercase (`read`, `bash`) and has no
    // `Skill` tool at all — a Pi skill is loaded by reading its SKILL.md, so
    // the read arm is the only one that can ever attribute one.
    match action.tool_name.as_str() {
        "Read" | "read" => {
            if let Some(file_path) = action.file_path.as_deref() {
                collect_skill_paths_from_text(file_path, &mut paths);
            }
            if let Some(file_path) =
                extract_tool_input_string(action.full_input.as_deref(), &["file_path", "path"])
            {
                collect_skill_paths_from_text(&file_path, &mut paths);
            }
        }
        "exec_command" | "bash" => {
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

/// Build Pi's owned `tool_actions` and `skill_usages` rows from one notify
/// parse.
///
/// Retained reconciliation reaches the identical Pi row shape through
/// `transcript_analytics::parse_transcript_analytics_source`. Both paths use
/// the same builder and canonical source key, so action identity and skill
/// fan-out cannot drift. Pi chain identity is flat: every row is the session's
/// own.
// @lat: [[data-flow#Session Indexing Pipeline#Enrichment]]
#[cfg(test)]
pub(crate) fn pi_transcript_tool_rows(
    session_id: &str,
    hostname: &str,
    messages: &[ExtractedMessage],
) -> (
    Vec<crate::transcript_analytics::OwnedToolAction>,
    Vec<crate::transcript_analytics::OwnedSkillUsage>,
) {
    let source_key = crate::storage::pi_source_key(hostname, session_id)
        .expect("validated Pi transcript identity");
    crate::transcript_analytics::owned_tool_rows(
        &crate::transcript_analytics::OwnedToolRowIdentity {
            provider: IntegrationProvider::Pi,
            source_key: &source_key,
            session_id,
            chain_id: session_id,
            parent_chain_id: None,
            agent_id: None,
            is_sidechain: false,
            hostname,
        },
        messages,
    )
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
            Some("node \"/home/me/.config/quill/scripts/session-sync.cjs\""),
            "SessionStart:startup",
        );
        assert_eq!(id, "quill:session-sync.cjs");
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
                "command": "node \"/home/me/.config/quill/scripts/session-sync.cjs\"",
                "durationMs": 145,
                "exitCode": 0
            }
        });
        let inv = extract_hook_invocation_from_attachment(&record).expect("Some");
        assert_eq!(inv.hook_event, "SessionStart");
        assert_eq!(inv.hook_matcher.as_deref(), Some("startup"));
        assert_eq!(inv.hook_identity, "quill:session-sync.cjs");
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

pub(crate) fn codex_text_blocks<'a>(
    payload: &'a serde_json::Value,
    block_type: &'a str,
) -> impl Iterator<Item = &'a str> {
    payload
        .get("content")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(move |block| block.get("type").and_then(|value| value.as_str()) == Some(block_type))
        .filter_map(|block| block.get("text").and_then(|value| value.as_str()))
        .filter(|text| !text.trim().is_empty())
}

pub(crate) fn has_nonempty_codex_assistant_output(payload: &serde_json::Value) -> bool {
    payload.get("role").and_then(|value| value.as_str()) == Some("assistant")
        && codex_text_blocks(payload, "output_text").next().is_some()
}

fn codex_response_message(payload: &serde_json::Value) -> Option<(&str, String)> {
    let role = payload.get("role").and_then(|value| value.as_str())?;
    let block_type = match role {
        "user" => "input_text",
        "assistant" => "output_text",
        _ => return None,
    };
    let content = codex_text_blocks(payload, block_type)
        .collect::<Vec<_>>()
        .join("\n");
    (!content.is_empty()).then_some((role, content))
}

fn duplicate_codex_legacy_message_ordinals(records: &[JsonlRecord]) -> HashSet<u64> {
    let mut duplicates = HashSet::new();
    let mut previous: Option<(bool, &str, String, &str, u64)> = None;

    for record in records {
        let object = &record.value;
        let Some(payload) = object.get("payload") else {
            continue;
        };
        let candidate = match object.get("type").and_then(|value| value.as_str()) {
            Some("event_msg") => {
                let role = match payload.get("type").and_then(|value| value.as_str()) {
                    Some("user_message") => "user",
                    Some("agent_message") => "assistant",
                    _ => continue,
                };
                let Some(content) = payload
                    .get("message")
                    .and_then(|value| value.as_str())
                    .filter(|content| !content.trim().is_empty())
                else {
                    continue;
                };
                (false, role, content.to_owned())
            }
            Some("response_item")
                if payload.get("type").and_then(|value| value.as_str()) == Some("message") =>
            {
                let Some((role, content)) = codex_response_message(payload) else {
                    continue;
                };
                (true, role, content)
            }
            _ => continue,
        };
        let timestamp = object
            .get("timestamp")
            .and_then(|value| value.as_str())
            .unwrap_or("");

        if let Some((
            previous_native,
            previous_role,
            previous_content,
            previous_timestamp,
            previous_ordinal,
        )) = previous.as_ref()
            && *previous_native != candidate.0
            && *previous_role == candidate.1
            && *previous_content == candidate.2
            && record.ordinal == previous_ordinal.saturating_add(1)
            && codex_message_timestamps_within_10ms(previous_timestamp, timestamp)
        {
            if candidate.0 {
                duplicates.insert(*previous_ordinal);
                previous = Some((true, candidate.1, candidate.2, timestamp, record.ordinal));
            } else {
                duplicates.insert(record.ordinal);
            }
        } else {
            previous = Some((
                candidate.0,
                candidate.1,
                candidate.2,
                timestamp,
                record.ordinal,
            ));
        }
    }

    duplicates
}

fn codex_message_timestamps_within_10ms(left: &str, right: &str) -> bool {
    let Ok(left) = chrono::DateTime::parse_from_rfc3339(left) else {
        return false;
    };
    let Ok(right) = chrono::DateTime::parse_from_rfc3339(right) else {
        return false;
    };
    (left - right).num_milliseconds().abs() <= 10
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
        parent_session_id: None,
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
        parent_uuid: None,
        cwd,
        custom_type: None,
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
///
/// Production callers must hold `transcript_work::with_source` through both
/// extraction and consumption/drop of the returned rows. Decoder serialization
/// alone does not bound outputs retained by callers.
pub fn extract_messages_from_jsonl(provider: IntegrationProvider, path: &Path) -> ExtractedSession {
    match read_extracted_session(provider, path) {
        Ok((extracted, _)) => extracted,
        Err(error) => {
            log::warn!("Failed to extract JSONL {}: {error}", path.display());
            unsupported_extracted_session()
        }
    }
}

/// Bounded stable reads fail explicitly so writers retain last-known-good docs.
fn read_extracted_session(
    provider: IntegrationProvider,
    path: &Path,
) -> Result<(ExtractedSession, ModelSourceFastFingerprint), String> {
    let path = path.to_path_buf();
    crate::transcript_work::decode(move || {
        let (bytes, fingerprint) = crate::transcript_identity::read_stable_transcript(&path)
            .map_err(|error| error.to_string())?;
        let contents = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
        let extracted = if provider == IntegrationProvider::Pi {
            let session = crate::pi_session::parse_pi_session_jsonl(contents)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Missing supported Pi session header".to_owned())?;
            extract_pi_session(&path, session)
        } else {
            extract_messages_from_jsonl_contents(provider, &path, contents)
        };
        Ok((extracted, fingerprint))
    })
}

/// Parse already-read retained JSONL content without touching persistence.
pub(crate) fn extract_messages_from_jsonl_contents(
    provider: IntegrationProvider,
    path: &Path,
    contents: &str,
) -> ExtractedSession {
    if provider == IntegrationProvider::Pi {
        return extract_pi_messages(path, crate::pi_session::parse_pi_session_jsonl(contents));
    }
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
        IntegrationProvider::Pi => {
            unreachable!("Pi is decoded directly into owned PiSession entries")
        }
        IntegrationProvider::MiniMax => unreachable!("MiniMax has no transcript source"),
    }
}

fn extract_pi_messages(
    path: &Path,
    parsed: Result<Option<crate::pi_session::PiSession>, crate::pi_session::PiSessionParseError>,
) -> ExtractedSession {
    match parsed {
        Ok(Some(session)) => extract_pi_session(path, session),
        Ok(None) => unsupported_extracted_session(),
        Err(error) => {
            log::warn!("Failed to parse Pi JSONL {}: {error}", path.display());
            unsupported_extracted_session()
        }
    }
}

pub(crate) fn extract_pi_session(
    _path: &Path,
    session: crate::pi_session::PiSession,
) -> ExtractedSession {
    let session_id = session.header.id;
    let cwd = session.header.cwd;
    let project_name = Path::new(&cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    let mut seen = HashSet::new();
    let mut messages: Vec<ExtractedMessage> = Vec::new();
    let mut events = Vec::new();
    let mut tool_use_map: HashMap<String, ToolUseEntry> = HashMap::new();
    // Injected context is its own entry type, so it is merged back by source
    // ordinal to keep search documents and context windows in file order.
    let mut custom_messages = session.custom_messages.into_iter().peekable();

    for entry in session.entries {
        while custom_messages
            .peek()
            .is_some_and(|custom| custom.source_ordinal < entry.source_ordinal)
        {
            let custom = custom_messages.next().expect("peeked custom message");
            if let Some(message) = pi_custom_message(custom, &session_id, &cwd, &mut seen) {
                messages.push(message);
            }
        }
        if !seen.insert(entry.base.id.clone()) {
            continue;
        }
        let Some(role) = entry.message.get("role").and_then(|value| value.as_str()) else {
            continue;
        };

        let event_kinds = match role {
            "user" => vec![SessionEventKind::UserText],
            "toolResult" => vec![SessionEventKind::UserToolResult],
            "assistant" => {
                let content = entry.message.get("content");
                let has_text = content
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
                    || content
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block.get("type").and_then(serde_json::Value::as_str)
                                    == Some("text")
                                    && block
                                        .get("text")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|text| !text.trim().is_empty())
                            })
                        });
                let has_thinking =
                    content
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block.get("type").and_then(serde_json::Value::as_str)
                                    == Some("thinking")
                            })
                        });
                let has_tool =
                    content
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block.get("type").and_then(serde_json::Value::as_str)
                                    == Some("toolCall")
                            })
                        });
                [
                    has_thinking.then_some(SessionEventKind::AsstThinking),
                    has_text.then_some(SessionEventKind::AsstText),
                    has_tool.then_some(SessionEventKind::AsstToolUse),
                ]
                .into_iter()
                .flatten()
                .collect()
            }
            _ => Vec::new(),
        };
        for (event_ordinal, kind) in event_kinds.into_iter().enumerate() {
            events.push(ExtractedEvent {
                source_ordinal: entry.source_ordinal,
                event_ordinal,
                timestamp: entry.base.timestamp.clone(),
                kind,
                uuid: Some(entry.base.id.clone()),
                parent_uuid: entry.base.parent_id.clone(),
            });
        }

        if role == "toolResult" {
            let Some(tool_use_id) = entry
                .message
                .get("toolCallId")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let output = entry
                .message
                .get("content")
                .map(pi_message_text)
                .map(|value| truncate(&value, 10_240));
            let is_error = Some(
                entry
                    .message
                    .get("isError")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            );
            let details_json = pi_tool_result_details(entry.message.get("details"));
            let result_image_count = Some(pi_tool_result_image_count(entry.message.get("content")));
            if let Some(tool_entry) = tool_use_map.get(tool_use_id)
                && let Some(message) = messages.get_mut(tool_entry.message_idx)
            {
                if let Some(action) = message
                    .tool_actions
                    .iter_mut()
                    .find(|action| action.tool_use_id == tool_use_id)
                {
                    action.full_output = output.clone();
                    action.is_error = is_error;
                    action.details_json = details_json;
                    action.result_image_count = result_image_count;
                }
                if tool_entry.category == "command"
                    && let Some(output) = &output
                    && let Some(command) = message
                        .commands_run
                        .iter_mut()
                        .find(|command| command.starts_with(&tool_entry.summary))
                {
                    *command = format!("{}\n{}", tool_entry.summary, truncate(output, 300));
                }
            }
            continue;
        }
        if !matches!(role, "user" | "assistant") {
            continue;
        }

        let content = entry
            .message
            .get("content")
            .map(pi_message_text)
            .unwrap_or_default();
        let mut tools_used = Vec::new();
        let mut files_modified = Vec::new();
        let mut code_changes = Vec::new();
        let mut commands_run = Vec::new();
        let mut tool_details = Vec::new();
        let mut tool_actions = Vec::new();

        if role == "assistant"
            && let Some(blocks) = entry
                .message
                .get("content")
                .and_then(serde_json::Value::as_array)
        {
            for (block_ordinal, block) in blocks.iter().enumerate() {
                if block.get("type").and_then(|value| value.as_str()) != Some("toolCall") {
                    continue;
                }
                let Some(tool_name) = block
                    .get("name")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let tool_use_id = block
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let input = block.get("arguments");
                let (category, summary, file_path) = build_pi_tool_summary(tool_name, input);
                let full_input = input.map(|value| truncate(&value.to_string(), 10_240));
                let (lines_added, lines_removed) = pi_code_change_lines(tool_name, input)
                    .map(|(added, removed)| (Some(added), Some(removed)))
                    .unwrap_or((None, None));

                tools_used.push(tool_name.to_string());
                match category.as_str() {
                    "code_change" => {
                        code_changes.push(summary.clone());
                        if let Some(path) = &file_path
                            && !path.is_empty()
                        {
                            files_modified.push(path.clone());
                        }
                    }
                    "command" => commands_run.push(summary.clone()),
                    _ => tool_details.push(summary.clone()),
                }
                tool_actions.push(ToolAction {
                    tool_use_id: tool_use_id.clone(),
                    source_ordinal: entry.source_ordinal,
                    block_ordinal,
                    tool_name: tool_name.to_string(),
                    category: category.clone(),
                    file_path: file_path.clone(),
                    summary: summary.clone(),
                    full_input: full_input.clone(),
                    full_output: None,
                    is_error: None,
                    details_json: None,
                    result_image_count: None,
                    duration_ms: None,
                    lines_added,
                    lines_removed,
                    timestamp: entry.base.timestamp.clone(),
                });
                if !tool_use_id.is_empty() {
                    tool_use_map.insert(
                        tool_use_id,
                        ToolUseEntry {
                            tool_name: tool_name.to_string(),
                            category,
                            file_path,
                            summary,
                            full_input,
                            timestamp: entry.base.timestamp.clone(),
                            message_idx: messages.len(),
                        },
                    );
                }
            }
        }

        if content.trim().is_empty() && tool_actions.is_empty() {
            continue;
        }
        messages.push(ExtractedMessage {
            uuid: entry.base.id,
            session_id: session_id.clone(),
            parent_session_id: None,
            role: role.to_string(),
            content,
            timestamp: entry.base.timestamp,
            git_branch: String::new(),
            tools_used,
            files_modified,
            code_changes,
            commands_run,
            tool_details,
            tool_actions,
            parent_uuid: entry.base.parent_id,
            cwd: Some(cwd.clone()),
            custom_type: None,
        });
    }
    for custom in custom_messages {
        if let Some(message) = pi_custom_message(custom, &session_id, &cwd, &mut seen) {
            messages.push(message);
        }
    }

    ExtractedSession {
        session_id,
        project_name,
        messages,
        events,
        hook_invocations: Vec::new(),
    }
}

/// Turn one Pi `custom_message` entry into a search document. It carries no
/// tool metadata and emits no runtime event, so injected context stays out of
/// turn, response-time, and tool analytics. `display` is Pi's TUI concern and
/// never gates indexing.
fn pi_custom_message(
    entry: crate::pi_session::PiCustomMessageEntry,
    session_id: &str,
    cwd: &str,
    seen: &mut HashSet<String>,
) -> Option<ExtractedMessage> {
    if !seen.insert(entry.base.id.clone()) || entry.content.trim().is_empty() {
        return None;
    }
    Some(ExtractedMessage {
        uuid: entry.base.id,
        session_id: session_id.to_owned(),
        parent_session_id: None,
        role: PI_CUSTOM_MESSAGE_ROLE.to_string(),
        content: truncate(&entry.content, 10_240),
        timestamp: entry.base.timestamp,
        git_branch: String::new(),
        tools_used: Vec::new(),
        files_modified: Vec::new(),
        code_changes: Vec::new(),
        commands_run: Vec::new(),
        tool_details: Vec::new(),
        tool_actions: Vec::new(),
        parent_uuid: entry.base.parent_id,
        cwd: Some(cwd.to_owned()),
        custom_type: Some(entry.custom_type),
    })
}

fn build_pi_tool_summary(
    tool_name: &str,
    input: Option<&serde_json::Value>,
) -> (String, String, Option<String>) {
    let object = input.and_then(serde_json::Value::as_object);
    let get_str = |key: &str| {
        object
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let path = || {
        ["path", "file_path"]
            .into_iter()
            .map(get_str)
            .find(|value| !value.is_empty())
            .unwrap_or_default()
    };

    match tool_name.to_ascii_lowercase().as_str() {
        "bash" => (
            "command".to_string(),
            format!("$ {}", get_str("command")),
            None,
        ),
        "write" => {
            let path = path();
            (
                "code_change".to_string(),
                format!("Write {path}: {}", truncate(&get_str("content"), 120)),
                Some(path),
            )
        }
        "edit" => {
            let path = path();
            let edit_count = object
                .and_then(|value| value.get("edits"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(1);
            (
                "code_change".to_string(),
                format!("Edit {path}: {edit_count} edits"),
                Some(path),
            )
        }
        "read" => {
            let path = path();
            (
                "tool_detail".to_string(),
                format!("Read {path}"),
                Some(path),
            )
        }
        "grep" => {
            let target = path();
            (
                "tool_detail".to_string(),
                format!("Grep \"{}\" in {target}", get_str("pattern")),
                None,
            )
        }
        "find" => (
            "tool_detail".to_string(),
            format!("Find \"{}\" in {}", get_str("pattern"), path()),
            None,
        ),
        "ls" => ("tool_detail".to_string(), format!("List {}", path()), None),
        _ => ("tool_detail".to_string(), tool_name.to_string(), None),
    }
}

fn pi_code_change_lines(tool_name: &str, input: Option<&serde_json::Value>) -> Option<(i64, i64)> {
    let object = input?.as_object()?;
    match tool_name.to_ascii_lowercase().as_str() {
        "write" => object
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(|content| (content.lines().count() as i64, 0)),
        "edit" => {
            let edits = object.get("edits")?.as_array()?;
            let (mut added, mut removed) = (0, 0);
            for edit in edits {
                if let Some(value) = edit.get("newText").and_then(serde_json::Value::as_str) {
                    added += value.lines().count() as i64;
                }
                if let Some(value) = edit.get("oldText").and_then(serde_json::Value::as_str) {
                    removed += value.lines().count() as i64;
                }
            }
            Some((added, removed))
        }
        _ => None,
    }
}

fn pi_message_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(|value| value.as_str()) == Some("text"))
            .filter_map(|block| block.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn unsupported_extracted_session() -> ExtractedSession {
    ExtractedSession {
        session_id: String::new(),
        project_name: None,
        messages: Vec::new(),
        events: Vec::new(),
        hook_invocations: Vec::new(),
    }
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
        let source_session_id = obj
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
        let session_id = if is_sidechain {
            let Some(agent_id) = agent_id.clone() else {
                continue;
            };
            agent_id
        } else {
            source_session_id
        };
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

                            // Compute line counts from the FULL input before it
                            // is truncated for storage below.
                            let (lines_added, lines_removed) = if category == "code_change" {
                                match input.and_then(|v| count_code_change_lines(&name, v)) {
                                    Some((a, r)) => (Some(a), Some(r)),
                                    None => (None, None),
                                }
                            } else {
                                (None, None)
                            };

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
                                is_error: None,
                                details_json: None,
                                result_image_count: None,
                                duration_ms: None,
                                lines_added,
                                lines_removed,
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
            parent_session_id: None,
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
            parent_uuid,
            cwd: cwd.clone(),
            custom_type: None,
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

fn extract_codex_messages_from_jsonl_records(records: &[JsonlRecord]) -> ExtractedSession {
    let mut messages: Vec<ExtractedMessage> = Vec::new();
    let mut events: Vec<ExtractedEvent> = Vec::new();
    let mut tool_use_map: HashMap<String, ToolUseEntry> = HashMap::new();
    let duplicate_legacy_messages = duplicate_codex_legacy_message_ordinals(records);
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
                if duplicate_legacy_messages.contains(&line_idx) {
                    continue;
                }
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
                    parent_session_id: None,
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
                    parent_uuid: None,
                    cwd: cwd.clone(),
                    custom_type: None,
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
                    "message" => {
                        let Some((role, content)) = codex_response_message(payload) else {
                            continue;
                        };
                        let uuid = payload
                            .get("id")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned);
                        if !timestamp.is_empty() {
                            events.push(ExtractedEvent {
                                source_ordinal: line_idx,
                                event_ordinal: 0,
                                timestamp: timestamp.clone(),
                                kind: if role == "user" {
                                    SessionEventKind::UserText
                                } else {
                                    SessionEventKind::AsstText
                                },
                                uuid: uuid.clone(),
                                parent_uuid: None,
                            });
                        }
                        messages.push(ExtractedMessage {
                            uuid: uuid.unwrap_or_else(|| format!("{session_id}:event:{line_idx}")),
                            session_id: session_id.clone(),
                            parent_session_id: None,
                            role: role.to_owned(),
                            content,
                            timestamp,
                            git_branch: git_branch.clone(),
                            tools_used: Vec::new(),
                            files_modified: Vec::new(),
                            code_changes: Vec::new(),
                            commands_run: Vec::new(),
                            tool_details: Vec::new(),
                            tool_actions: Vec::new(),
                            parent_uuid: None,
                            cwd: cwd.clone(),
                            custom_type: None,
                        });
                    }
                    "agent_message" => {
                        let content = codex_text_blocks(payload, "input_text")
                            .collect::<Vec<_>>()
                            .join("\n");
                        if content.is_empty() {
                            continue;
                        }
                        let author = payload
                            .get("author")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let recipient = payload
                            .get("recipient")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let mut content_parts = Vec::with_capacity(3);
                        if !author.is_empty() {
                            content_parts.push(format!("From: {author}"));
                        }
                        if !recipient.is_empty() {
                            content_parts.push(format!("To: {recipient}"));
                        }
                        content_parts.push(content);
                        messages.push(ExtractedMessage {
                            uuid: format!("{session_id}:event:{line_idx}"),
                            session_id: session_id.clone(),
                            parent_session_id: None,
                            role: author.to_string(),
                            content: content_parts.join("\n"),
                            timestamp,
                            git_branch: git_branch.clone(),
                            tools_used: Vec::new(),
                            files_modified: Vec::new(),
                            code_changes: Vec::new(),
                            commands_run: Vec::new(),
                            tool_details: Vec::new(),
                            tool_actions: Vec::new(),
                            parent_uuid: None,
                            cwd: cwd.clone(),
                            custom_type: None,
                        });
                    }
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
                        // Count lines from the FULL arguments before truncation.
                        let (lines_added, lines_removed) = if category == "code_change" {
                            match codex_function_change_lines(&name, &arguments) {
                                Some((a, r)) => (Some(a), Some(r)),
                                None => (None, None),
                            }
                        } else {
                            (None, None)
                        };
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
                            is_error: None,
                            details_json: None,
                            result_image_count: None,
                            duration_ms: None,
                            lines_added,
                            lines_removed,
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
                        // Count lines from the FULL patch/input before truncation.
                        let (lines_added, lines_removed) = if category == "code_change" {
                            match codex_custom_change_lines(&name, &input) {
                                Some((a, r)) => (Some(a), Some(r)),
                                None => (None, None),
                            }
                        } else {
                            (None, None)
                        };
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
                            is_error: None,
                            details_json: None,
                            result_image_count: None,
                            duration_ms: None,
                            lines_added,
                            lines_removed,
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
                            uuid: payload
                                .get("id")
                                .and_then(|value| value.as_str())
                                .filter(|value| !value.is_empty())
                                .map(str::to_owned),
                            parent_uuid: None,
                        });
                    }
                    // Other response items are either non-search reasoning or
                    // tool data already represented by the branches above.
                    _ => {}
                }
            }
            // Context snapshots, compaction state, and communication metadata
            // carry no user-facing search message. Model analytics parses its
            // own turn_context evidence from the same retained records.
            _ => {}
        }
    }

    for message in &mut messages {
        message.session_id.clone_from(&native_identity.chain_id);
    }

    ExtractedSession {
        session_id,
        project_name: cwd.as_deref().and_then(project_name_from_cwd),
        messages,
        events,
        hook_invocations: Vec::new(),
    }
}

fn unique_session_path(
    provider: IntegrationProvider,
    session_id: &str,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<PathBuf>, String> {
    let mut paths = paths.into_iter().collect::<HashSet<_>>();
    if paths.len() > 1 {
        return Err(format!(
            "Multiple {provider} transcripts match session {session_id}"
        ));
    }
    Ok(paths.drain().next())
}

/// Resolve a provider-native session or chain id to its retained transcript
/// through the analytics source registry, then re-validate that path against
/// the configured root so a stale registry row cannot open an arbitrary file.
///
/// An exact `chain_id` / `source_session_id` match wins over a root match, so
/// a sub-agent chain opens its own transcript rather than its parent's. The
/// registry is the one place every provider's identity already lives, which
/// is what keeps this a single indexed query instead of a corpus parse.
fn registered_session_path(
    storage: &crate::storage::Storage,
    provider: IntegrationProvider,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    let Some((_, source_root_key)) = retained_jsonl_source_root_identities()
        .into_iter()
        .find(|(root_provider, _)| *root_provider == provider)
    else {
        return Ok(None);
    };
    let sources = storage.list_transcript_analytics_sources_for_root(provider, source_root_key)?;
    let mut exact = Vec::new();
    let mut rooted = Vec::new();
    for source in sources {
        if source.processing_status == "suppressed" || source.suppressed_sha256.is_some() {
            continue;
        }
        if source.chain_id.as_deref() == Some(session_id)
            || source.source_session_id.as_deref() == Some(session_id)
        {
            exact.push(source.source_path);
        } else if source.analytics_session_id.as_deref() == Some(session_id) {
            rooted.push(source.source_path);
        }
    }
    let path = if exact.is_empty() {
        unique_session_path(provider, session_id, rooted)?
    } else {
        unique_session_path(provider, session_id, exact)?
    };
    let Some(path) = path else {
        return Ok(None);
    };
    match validate_retained_notify_source(provider, &path) {
        Ok(Some(source)) if source.provider == provider => Ok(Some(source.canonical_path)),
        Ok(_) | Err(_) => Ok(None),
    }
}

/// A Claude parent transcript is named by its session id, so a session that
/// predates the analytics registry still resolves without walking the tree.
fn claude_parent_session_path(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let canonical_root = std::fs::canonicalize(projects_dir).ok()?;
    let file_name = format!("{session_id}.jsonl");
    let mut matches = std::fs::read_dir(projects_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|project| project.path().join(&file_name))
        .filter(|path| {
            std::fs::canonicalize(path).is_ok_and(|canonical| {
                canonical.starts_with(&canonical_root) && canonical.is_file()
            })
        });
    let path = matches.next()?;
    matches.next().is_none().then_some(path)
}

// `pub(crate)` so the learning pipeline's Stream C can resolve parent
// transcripts while Session Search can resolve provider-native child chains.
pub(crate) fn find_session_path(
    provider: IntegrationProvider,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    if provider == IntegrationProvider::MiniMax {
        return Ok(None);
    }
    if let Some(storage) = crate::STORAGE.get()
        && let Some(path) = registered_session_path(storage, provider, session_id)?
    {
        return Ok(Some(path));
    }
    if provider == IntegrationProvider::Claude {
        let projects_dir = crate::data_paths::resolve_claude_projects_dir();
        return Ok(claude_parent_session_path(&projects_dir, session_id));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Tauri state wrapper and commands
// ---------------------------------------------------------------------------

/// Wrapper for managed Tauri state.
pub struct SessionIndexState(pub Arc<SessionIndex>);

/// Attach persisted session display names to a page of hits with one bounded
/// registry read. Sessions without a persisted name stay `None`.
pub(crate) fn attach_session_names(
    storage: Option<&crate::storage::Storage>,
    results: &mut SearchResults,
) {
    let Some(storage) = storage else { return };
    if results.hits.is_empty() {
        return;
    }
    let wanted = results
        .hits
        .iter()
        .map(|hit| (hit.provider, hit.session_id.clone()))
        .collect::<Vec<_>>();
    let names = storage.transcript_session_names(&wanted);
    for hit in &mut results.hits {
        hit.session_name = names.get(&(hit.provider, hit.session_id.clone())).cloned();
    }
}

/// Attach the persisted session display name to one context response.
pub(crate) fn attach_context_session_name(
    storage: Option<&crate::storage::Storage>,
    context: &mut SessionContext,
) {
    let Some(storage) = storage else { return };
    context.session_name = storage
        .transcript_session_names(&[(context.provider, context.session_id.clone())])
        .remove(&(context.provider, context.session_id.clone()));
}

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
    crate::run_blocking(move || {
        let mut results = idx.search(&query, &filters, &sort, page, page_size)?;
        attach_session_names(crate::STORAGE.get(), &mut results);
        Ok(results)
    })
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
    crate::run_blocking(move || {
        let mut context = idx.get_context(provider, &session_id, &around_message_id, w)?;
        attach_context_session_name(crate::STORAGE.get(), &mut context);
        Ok(context)
    })
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
    crate::run_blocking(move || idx.sync(&app))
}

#[cfg(test)]
mod checkpoint_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    // @lat: [[pi-provider-plumbing-tests#Pi Provider Plumbing Test Specs#Transcript Root Coverage]]
    fn retained_analytics_roots_include_persisted_pi() {
        assert_eq!(
            retained_jsonl_source_root_identities(),
            vec![
                (IntegrationProvider::Claude, CLAUDE_SOURCE_ROOT_KEY),
                (IntegrationProvider::Codex, CODEX_SOURCE_ROOT_KEY),
                (IntegrationProvider::Pi, PI_SOURCE_ROOT_KEY),
            ]
        );
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Startup Search Recovery]]
    #[test]
    #[serial]
    fn startup_scan_indexes_persisted_pi_without_notify() {
        let root = TempDir::new().expect("tempdir");
        let claude = root.path().join("claude");
        let codex = root.path().join("codex");
        let pi = root.path().join("pi");
        for directory in [&claude, &codex, &pi] {
            fs::create_dir(directory).expect("create transcript root");
        }
        fs::write(
            pi.join("session.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"pi-startup","timestamp":"2026-08-18T02:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"prompt","parentId":null,"timestamp":"2026-08-18T02:00:01Z","message":{"role":"user","content":"startup-pi-needle"}}"#,
                "\n",
            ),
        )
        .expect("write Pi transcript");
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", root.path());
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", &claude);
            std::env::set_var("QUILL_CODEX_SESSIONS_DIR", &codex);
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &pi);
        }

        let inventory = enumerate_pi_retained_jsonl_source_root();
        assert_eq!(
            inventory.sources.len(),
            1,
            "Pi startup inventory failed: {:?}",
            inventory.outcome
        );
        let notified = validate_retained_notify_source(
            IntegrationProvider::Pi,
            &inventory.sources[0].filesystem_path,
        )
        .expect("validate Pi notify source")
        .expect("Pi retained source");
        assert_eq!(
            notified.source_key, inventory.sources[0].source_key,
            "startup and notify must coalesce on one canonical owner"
        );
        assert_eq!(
            inventory.sources[0].source_key,
            crate::storage::pi_source_key(&SessionIndex::local_hostname(), "pi-startup")
                .expect("canonical local Pi source")
        );
        assert_eq!(
            extract_messages_from_jsonl(
                IntegrationProvider::Pi,
                &inventory.sources[0].filesystem_path,
            )
            .messages
            .len(),
            1
        );
        let index_dir = root.path().join("index");
        let index = SessionIndex::open_or_create_for_tests(&index_dir).expect("open index");
        assert_eq!(
            index.sync_without_emit().expect("scan persisted sources"),
            1
        );
        index.reader.reload().expect("reload startup index");
        let results = index
            .search(
                "startup-pi-needle",
                &SearchFilters {
                    provider: Some(IntegrationProvider::Pi),
                    ..SearchFilters::default()
                },
                "relevance",
                0,
                10,
            )
            .expect("search startup Pi source");
        assert_eq!(results.total_hits, 1);
        assert_eq!(results.hits[0].session_id, "pi-startup");

        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_DATA_DIR");
            std::env::remove_var("QUILL_CLAUDE_PROJECTS_DIR");
            std::env::remove_var("QUILL_CODEX_SESSIONS_DIR");
            std::env::remove_var("QUILL_PI_SESSIONS_DIR");
        }
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Shared Root Session Id]]
    #[test]
    fn root_session_ids_are_derived_by_one_rule_per_provider() {
        let parent = Path::new("/p/-home-proj/9f1c.jsonl");
        assert_eq!(
            claude_root_session_id(parent, false).as_deref(),
            Some("9f1c")
        );

        // A sub-agent names its root by directory, at any nesting depth.
        let flat = Path::new("/p/-home-proj/9f1c/subagents/agent-a1.jsonl");
        let nested = Path::new("/p/-home-proj/9f1c/subagents/workflows/wf_7/agent-a2.jsonl");
        assert_eq!(claude_root_session_id(flat, true).as_deref(), Some("9f1c"));
        assert_eq!(
            claude_root_session_id(nested, true).as_deref(),
            Some("9f1c")
        );
        // Reading a sub-agent path as a parent yields the file stem, not the root.
        assert_eq!(
            claude_root_session_id(flat, false).as_deref(),
            Some("agent-a1")
        );

        let rollout = Path::new(
            "/s/2026/08/07/rollout-2026-08-07T20-22-02-019fdf64-542d-7560-9891-bc73a7097da3.jsonl",
        );
        assert_eq!(
            codex_thread_id(rollout).as_deref(),
            Some("019fdf64-542d-7560-9891-bc73a7097da3"),
            "the trailing uuid is the thread id"
        );

        // Malformed names are rejected rather than yielding a truncated id.
        for bad in [
            "/s/rollout-2026-08-07T20-22-02.jsonl",
            "/s/rollout-.jsonl",
            "/s/2026-08-07T20-22-02-019fdf64-542d-7560-9891-bc73a7097da3.jsonl",
            "/s/rollout-2026-08-07T20-22-02-not*a*uuid*here*aaaaaaaaaaaaaaaaaaaa.jsonl",
        ] {
            assert_eq!(codex_thread_id(Path::new(bad)), None, "rejects {bad}");
        }
    }

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
        let mut diagnostic = None;
        let files = collect_claude_jsonl_candidates(
            fixture.path(),
            IntegrationProvider::Claude,
            &mut diagnostic,
        );
        assert_eq!(diagnostic, None);

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

        assert!(parent.path.to_string_lossy().ends_with(".jsonl"));
        assert!(
            !parent
                .path
                .components()
                .any(|c| c.as_os_str() == "subagents"),
            "parent path should not traverse subagents/"
        );

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
    }

    #[test]
    fn extraction_uses_native_subagent_session_identity() {
        let fixture = make_fixture();
        let files = discover_claude_transcripts_in(fixture.path());

        let (subagent_path, _) = files
            .iter()
            .find(|(_, is_subagent)| *is_subagent)
            .expect("sub-agent file");
        let extracted = extract_messages_from_jsonl(IntegrationProvider::Claude, subagent_path);

        assert!(
            !extracted.messages.is_empty(),
            "expected at least one extracted sub-agent message"
        );
        for msg in &extracted.messages {
            assert_eq!(
                msg.session_id, "aaaabbbbccccdddd",
                "sub-agent session_id must use its native agent chain identity"
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
        let (parent_path, _) = files
            .iter()
            .find(|(_, is_subagent)| !*is_subagent)
            .expect("parent file");
        let parent_extracted =
            extract_messages_from_jsonl(IntegrationProvider::Claude, parent_path);
        for msg in &parent_extracted.messages {
            assert_eq!(msg.session_id, parent_extracted.session_id);
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

        // Workflow bookkeeping must not be mistaken for an agent transcript.
        fs::write(workflow_dir.join("journal.jsonl"), r#"{"entries":[]}"#)
            .expect("write workflow journal");

        let mut diagnostic = None;
        let files = collect_claude_jsonl_candidates(
            tmp.path(),
            IntegrationProvider::Claude,
            &mut diagnostic,
        );
        assert_eq!(diagnostic, None);

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
                .any(|f| f.path.to_string_lossy().ends_with("journal.jsonl")),
            "workflow journals must be filtered out at any depth"
        );

        let canonical_root = fs::canonicalize(tmp.path()).expect("canonical root");
        let canonical_agent =
            fs::canonicalize(workflow_dir.join("agent-b.jsonl")).expect("canonical agent");
        let canonical_journal =
            fs::canonicalize(workflow_dir.join("journal.jsonl")).expect("canonical journal");
        assert!(matches!(
            retained_jsonl_source_layout_hint(
                IntegrationProvider::Claude,
                &canonical_root,
                &canonical_agent,
            ),
            Some(RetainedJsonlSourceLayoutHint::ClaudeSubagent { .. })
        ));
        assert_eq!(
            retained_jsonl_source_layout_hint(
                IntegrationProvider::Claude,
                &canonical_root,
                &canonical_journal,
            ),
            None,
            "retained notify validation must reject workflow journals too"
        );

        let retained = enumerate_provider_source_root(
            IntegrationProvider::Claude,
            CLAUDE_SOURCE_ROOT_KEY,
            tmp.path().to_path_buf(),
            collect_claude_jsonl_candidates,
        );
        assert_eq!(retained.outcome, ProviderRootEnumerationOutcome::Complete);
        assert_eq!(retained.sources.len(), 3);
        assert!(retained.sources.iter().any(|source| {
            source
                .filesystem_path
                .to_string_lossy()
                .ends_with("agent-b.jsonl")
                && matches!(
                    source.layout_hint,
                    RetainedJsonlSourceLayoutHint::ClaudeSubagent { .. }
                )
        }));
    }

    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Search Checkpoint Compatibility]]
    #[test]
    fn old_search_checkpoint_deserializes_without_hints_or_canonical_path() {
        let state: IndexState = serde_json::from_str(r#"{"sources":{"pi:sessions:test":{"mtime_ns":1,"size_bytes":2,"session_id":"native"}}}"#).unwrap();
        let indexed = &state.sources["pi:sessions:test"];
        assert_eq!(indexed.session_id, "native");
        assert!(indexed.hints.is_none());
        assert!(
            indexed.canonical_path.is_none(),
            "old checkpoints revalidate their canonical path once"
        );
    }

    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Search Prune Proof]]
    #[test]
    fn search_prune_does_not_erase_a_live_commit_newer_than_inventory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.jsonl");
        fs::write(&path, r#"{"type":"user","sessionId":"new-session","uuid":"new-message","timestamp":"2026-08-14T08:00:00Z","message":{"role":"user","content":"newneedle"}}
"#).unwrap();
        let source = DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Claude,
            source_root_key: CLAUDE_SOURCE_ROOT_KEY,
            source_key: "claude:projects:new".into(),
            canonical_path: path.clone(),
            filesystem_path: path.clone(),
            layout_hint: RetainedJsonlSourceLayoutHint::ClaudeParent {
                default_project: "test".into(),
            },
        };
        let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
        let inventory = ProviderSourceRoot {
            provider: IntegrationProvider::Claude,
            source_root_key: CLAUDE_SOURCE_ROOT_KEY,
            resolved_root_path: dir.path().to_owned(),
            canonical_root_path: Some(dir.path().to_owned()),
            outcome: ProviderRootEnumerationOutcome::Complete,
            sources: Vec::new(),
        };
        index.sync_source(&source, "host").unwrap();
        index.sync_inner(std::slice::from_ref(&inventory)).unwrap();
        assert!(
            index
                .state
                .lock()
                .unwrap()
                .sources
                .contains_key(&source.source_key)
        );
        fs::remove_file(path).unwrap();
        index.sync_inner(&[inventory]).unwrap();
        assert!(index.state.lock().unwrap().sources.is_empty());
        index.reader.reload().unwrap();
        assert_eq!(
            index
                .search("newneedle", &Default::default(), "relevance", 0, 10)
                .unwrap()
                .total_hits,
            0
        );
    }

    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Pi Header Replacement Prune]]
    #[test]
    fn search_prune_distinguishes_a_live_pi_source_from_replaced_header_identity() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pi.jsonl");
        let body = |id: &str| {
            format!(
                "{}\n",
                serde_json::json!({"type":"session","version":3,"id":id,"cwd":"/work/test","timestamp":"2026-08-14T08:00:00Z"})
            )
        };
        fs::write(&path, body("old-native")).unwrap();
        let source = DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Pi,
            source_root_key: PI_SOURCE_ROOT_KEY,
            source_key: "pi:sessions:old-native".into(),
            canonical_path: path.clone(),
            filesystem_path: path.clone(),
            layout_hint: RetainedJsonlSourceLayoutHint::PiTranscript,
        };
        let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
        let root = ProviderSourceRoot {
            provider: IntegrationProvider::Pi,
            source_root_key: PI_SOURCE_ROOT_KEY,
            resolved_root_path: dir.path().to_owned(),
            canonical_root_path: Some(dir.path().to_owned()),
            outcome: ProviderRootEnumerationOutcome::Complete,
            sources: Vec::new(),
        };
        index.sync_source(&source, "host").unwrap();
        index.sync_inner(std::slice::from_ref(&root)).unwrap();
        assert!(
            index
                .state
                .lock()
                .unwrap()
                .sources
                .contains_key(&source.source_key),
            "an existing matching source survives an older inventory"
        );
        fs::write(path, body("new-native")).unwrap();
        index.sync_inner(&[root]).unwrap();
        assert!(
            index.state.lock().unwrap().sources.is_empty(),
            "a new valid header proves the old native source was replaced even though its path remains"
        );
    }

    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Unchanged Sweep Isolation]]
    #[test]
    fn unchanged_search_sweep_does_not_refresh_analytics_but_recovery_still_runs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("source.jsonl");
        fs::write(&path, r#"{"type":"user","sessionId":"session","uuid":"message","timestamp":"2026-08-14T08:00:00Z","message":{"role":"user","content":"needle"}}
"#).unwrap();
        let source = DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Claude,
            source_root_key: CLAUDE_SOURCE_ROOT_KEY,
            source_key: "claude:projects:source".into(),
            canonical_path: path.clone(),
            filesystem_path: path,
            layout_hint: RetainedJsonlSourceLayoutHint::ClaudeParent {
                default_project: "test".into(),
            },
        };
        let root = ProviderSourceRoot {
            provider: IntegrationProvider::Claude,
            source_root_key: CLAUDE_SOURCE_ROOT_KEY,
            resolved_root_path: dir.path().to_owned(),
            canonical_root_path: Some(dir.path().to_owned()),
            outcome: ProviderRootEnumerationOutcome::Complete,
            sources: vec![source.clone()],
        };
        let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
        let storage = crate::storage::Storage::init_at(dir.path().join("usage.db"), false).unwrap();
        let generation_key = "transcript_analytics_generation:claude:claude:projects";
        storage.set_setting(generation_key, "41").unwrap();
        index.sync_source(&source, "host").unwrap();
        assert_eq!(
            index.sync_retained_sources(&storage, std::slice::from_ref(&root)),
            (0, false)
        );
        assert_eq!(
            storage.get_setting(generation_key).unwrap().as_deref(),
            Some("41"),
            "unchanged sweep never begins a live root generation or registry refresh"
        );
        assert!(
            storage
                .list_transcript_analytics_sources_for_root(
                    IntegrationProvider::Claude,
                    CLAUDE_SOURCE_ROOT_KEY
                )
                .unwrap()
                .is_empty()
        );
        let recovery = crate::transcript_analytics::run_transcript_analytics_reconciliation(
            &storage,
            "host",
            &[root],
        )
        .unwrap();
        assert_eq!(
            recovery.replaced_sources, 1,
            "analytics-only work remains owned by recovery"
        );
    }

    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Context Ownership And Errors]]
    #[test]
    #[serial]
    fn context_extreme_window_does_not_overflow_and_read_failures_are_errors() {
        let fixture = make_fixture();
        let session = "11111111-2222-3333-4444-555555555555";
        let path = fixture
            .path()
            .join("-home-test-proj")
            .join(format!("{session}.jsonl"));
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", fixture.path());
        }
        let index = SessionIndex::open_or_create_for_tests(&fixture.path().join("index")).unwrap();
        let context = index
            .get_context(IntegrationProvider::Claude, session, "p2", usize::MAX)
            .unwrap();
        assert_eq!(context.messages.len(), 2);
        assert_eq!(context.messages[1].content, "hi back");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(crate::transcript_identity::RETAINED_TRANSCRIPT_MAX_BYTES + 1)
            .unwrap();
        assert!(
            index
                .get_context(IntegrationProvider::Claude, session, "p2", 5)
                .unwrap_err()
                .contains("256 MiB")
        );
        fs::write(&path, [0xff]).unwrap();
        assert!(
            index
                .get_context(IntegrationProvider::Claude, session, "p2", 5)
                .is_err()
        );
        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_CLAUDE_PROJECTS_DIR");
        }
    }

    /// Allocation-dense source, three overlapping real owners, fresh retained
    /// caller threads each round. Run alone; XML samples are read-only accounting.
    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Dense Overlapping Owning Benchmark]]
    #[test]
    #[ignore = "allocation-dense overlapping owning-path memory benchmark"]
    #[serial]
    fn dense_pi_transcript_overlapping_owning_paths() {
        use crate::transcript_analytics::{self, TranscriptSourceResult};
        use crate::transcript_work::{DECODE_COUNT, decode};
        use std::io::Write;
        use std::sync::{Arc, Barrier, Condvar, Mutex, mpsc};
        assert!(
            std::env::var_os("MALLOC_ARENA_MAX").is_none(),
            "measure default allocator"
        );
        assert!(
            !std::env::var("GLIBC_TUNABLES")
                .unwrap_or_default()
                .contains("glibc.malloc")
        );
        let mib = std::env::var("QUILL_DENSE_BENCH_MIB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(32);
        let rounds = std::env::var("QUILL_DENSE_BENCH_ROUNDS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(8);
        assert!((1..=244).contains(&mib) && (2..=16).contains(&rounds));
        let dir = TempDir::new().unwrap();
        let artifacts = std::env::var_os("QUILL_DENSE_BENCH_ARTIFACT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.path().join("measurements"));
        fs::create_dir_all(&artifacts).unwrap();
        let path = dir.path().join("dense.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"session","version":3,"id":"dense","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/synthetic"})).unwrap();
        let nested = (0..128).map(|i| serde_json::json!({"i":i,"flags":[true,false],"meta":{"key":"value","tags":["alpha","beta"]},"coords":{"x":i,"y":i+1}})).collect::<Vec<_>>();
        let append = |file: &mut fs::File, i: usize| {
            let base = chrono::DateTime::parse_from_rfc3339("2026-08-14T08:00:00Z").unwrap();
            let time = |offset| {
                (base + chrono::Duration::milliseconds((i * 3 + offset) as i64)).to_rfc3339()
            };
            for row in [
                serde_json::json!({"type":"message","id":format!("user-{i}"),"parentId":null,"timestamp":time(0),"message":{"role":"user","content":"dense request"}}),
                serde_json::json!({"type":"message","id":format!("assistant-{i}"),"parentId":format!("user-{i}"),"timestamp":time(1),"message":{"role":"assistant","provider":"synthetic","model":"model","usage":{"input":20,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":25},"content":[{"type":"text","text":"dense reply"},{"type":"toolCall","id":format!("call-{i}"),"name":"bash","arguments":{"command":"printf dense","nested":nested}}]}}),
                serde_json::json!({"type":"message","id":format!("result-{i}"),"parentId":format!("assistant-{i}"),"timestamp":time(2),"message":{"role":"toolResult","toolCallId":format!("call-{i}"),"toolName":"bash","content":[{"type":"text","text":"dense tool result"}],"details":{"nested":nested},"isError":false}}),
            ] {
                writeln!(file, "{row}").unwrap();
            }
        };
        let mut groups = 0;
        while file.metadata().unwrap().len() < (mib * 1024 * 1024) as u64 {
            append(&mut file, groups);
            groups += 1;
        }
        let storage =
            Arc::new(crate::storage::Storage::init_at(dir.path().join("usage.db"), false).unwrap());
        for marker in [
            "transcript_analytics_reingest_pending",
            "pi_transcript_analytics_reingest_pending",
            "pi_persisted_source_reconciliation_pending",
        ] {
            storage.delete_setting(marker).unwrap();
        }
        let index =
            Arc::new(SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap());
        let hostname = SessionIndex::local_hostname();
        let source = Arc::new(DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Pi,
            source_root_key: PI_SOURCE_ROOT_KEY,
            source_key: crate::storage::pi_source_key(&hostname, "dense").unwrap(),
            canonical_path: path.clone(),
            filesystem_path: path.clone(),
            layout_hint: RetainedJsonlSourceLayoutHint::PiTranscript,
        });
        let roots = Arc::new(vec![ProviderSourceRoot {
            provider: IntegrationProvider::Pi,
            source_root_key: PI_SOURCE_ROOT_KEY,
            resolved_root_path: dir.path().to_owned(),
            canonical_root_path: Some(dir.path().to_owned()),
            outcome: ProviderRootEnumerationOutcome::Complete,
            sources: vec![source.as_ref().clone()],
        }]);
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let mut owners = Vec::new();
        let mut decoder_threads = HashSet::new();
        let mut owner_threads = HashSet::new();
        let started = std::time::Instant::now();
        for round in 0..rounds {
            let changed = round % 2 == 0;
            if changed && round > 0 {
                append(&mut file, groups);
                groups += 1;
            }
            let barrier = Arc::new(Barrier::new(4));
            let (tx, rx) = mpsc::channel();
            for owner in 0..3 {
                let (storage, index, source, roots, hostname, barrier, tx, release) = (
                    Arc::clone(&storage),
                    Arc::clone(&index),
                    Arc::clone(&source),
                    Arc::clone(&roots),
                    hostname.clone(),
                    Arc::clone(&barrier),
                    tx.clone(),
                    Arc::clone(&release),
                );
                owners.push(std::thread::spawn(move || {
                    barrier.wait();
                    let before = DECODE_COUNT.get();
                    match owner {
                        0 => { let (analytics, search) = transcript_analytics::reconcile_retained_source(&storage, &source, &hostname, Some(&index)); analytics.unwrap(); search.unwrap(); }
                        1 => { index.sync_retained_sources(&storage, &roots); index.sync_inner(&roots).unwrap(); }
                        _ => { let summary = transcript_analytics::run_transcript_analytics_reconciliation_with_search(&storage, &hostname, &roots, Some(&index)).unwrap(); assert_eq!(summary.failed_sources, 0); }
                    }
                    let decodes = DECODE_COUNT.get() - before;
                    let decoder_thread = decode(|| std::thread::current().id());
                    tx.send((decodes, decoder_thread, std::thread::current().id())).unwrap();
                    drop(tx);
                    // Keep retired caller arenas alive, like idle pool workers.
                    let mut done = release.0.lock().unwrap();
                    while !*done { done = release.1.wait(done).unwrap(); }
                }));
            }
            drop(tx);
            barrier.wait();
            let mut decodes = 0;
            for (count, decoder, owner) in rx.iter().take(3) {
                decodes += count;
                decoder_threads.insert(decoder);
                owner_threads.insert(owner);
            }
            assert_eq!(owner_threads.len(), (round + 1) * 3);
            assert_eq!(
                decoder_threads.len(),
                1,
                "heavy decoding never rotates onto owners"
            );
            if changed {
                assert!(
                    (1..=2).contains(&decodes),
                    "one full extraction, optionally preceded by root identity inventory: {decodes}"
                );
            } else {
                assert_eq!(decodes, 0, "identical overlapping versions never decode");
            }
            index.reader.reload().unwrap();
            assert_eq!(
                index
                    .search("", &Default::default(), "relevance", 0, 1)
                    .unwrap()
                    .total_hits,
                (groups * 2) as u64
            );
            let conn = rusqlite::Connection::open(dir.path().join("usage.db")).unwrap();
            for (table, expected) in [
                ("session_events", groups * 4),
                ("tool_actions", groups),
                ("model_usage_observations", groups),
            ] {
                assert_eq!(
                    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                        .unwrap(),
                    expected as i64,
                    "{table} complete after round {round}"
                );
            }
            assert_eq!(
                transcript_analytics::reconcile_live_transcript_source(
                    &storage, &source, &hostname
                )
                .unwrap(),
                TranscriptSourceResult::SuppressedUnchanged
            );
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            {
                let xml_path = artifacts.join(format!("malloc-round-{round}.xml"));
                let name = std::ffi::CString::new(xml_path.to_str().unwrap()).unwrap();
                // Read-only malloc_info/mallinfo2; never trim or tune the allocator.
                unsafe {
                    let stream = libc::fopen(name.as_ptr(), c"w".as_ptr());
                    assert!(!stream.is_null());
                    assert_eq!(libc::malloc_info(0, stream), 0);
                    assert_eq!(libc::fclose(stream), 0);
                }
                let xml = fs::read_to_string(xml_path).unwrap();
                let heaps = xml
                    .split("<heap nr=")
                    .skip(1)
                    .map(|heap| {
                        heap.split("</heap>")
                            .next()
                            .unwrap()
                            .split("<system type=\"current\" size=\"")
                            .nth(1)
                            .unwrap()
                            .split('"')
                            .next()
                            .unwrap()
                            .parse::<usize>()
                            .unwrap()
                    })
                    .collect::<Vec<_>>();
                let heap = unsafe { libc::mallinfo2() };
                eprintln!(
                    "dense-heap round={round} arenas={} arenas_ge_64m={} largest_arena={} arena_system={} arena_free={} allocated_estimate={}",
                    heaps.len(),
                    heaps
                        .iter()
                        .filter(|bytes| **bytes >= 64 * 1024 * 1024)
                        .count(),
                    heaps.iter().max().unwrap(),
                    heap.arena,
                    heap.fordblks,
                    heap.uordblks + heap.hblkhd
                );
            }
            eprintln!(
                "dense-bench round={round} bytes={} groups={groups} owners={} decoders={} source_decodes={decodes} elapsed_ms={} {}",
                fs::metadata(&path).unwrap().len(),
                owner_threads.len(),
                decoder_threads.len(),
                started.elapsed().as_millis(),
                fs::read_to_string("/proc/self/status")
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.starts_with("VmRSS:")
                        || line.starts_with("VmHWM:")
                        || line.starts_with("VmSwap:"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        for owner in owners {
            owner.join().unwrap();
        }
    }

    /// Run serially in a fresh process with QUILL_TRANSCRIPT_BENCH_MIB=244.
    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Large Owning Path Benchmark]]
    #[test]
    #[ignore = "large synthetic owning-path memory benchmark"]
    #[serial]
    fn repeated_large_pi_transcript_owning_paths() {
        use std::io::Write;
        let root = TempDir::new().unwrap();
        let pi = root.path().join("pi/--work-quill--");
        fs::create_dir_all(&pi).unwrap();
        let path = pi.join("bench.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"session","version":3,"id":"bench","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"})).unwrap();
        let mib = std::env::var("QUILL_TRANSCRIPT_BENCH_MIB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(32);
        // Native user content exercises JSON ownership, extraction and Tantivy;
        // repeated words keep disk/index size modest without hiding heap bytes.
        let text = "synthetic evidence ".repeat(1024 * 1024 / 19);
        for i in 0..mib {
            writeln!(file, "{}", serde_json::json!({"type":"message","id":format!("entry-{i}"),"parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":text}})).unwrap();
        }
        drop(text);
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_RULES_DIR", root.path().join("rules"));
            std::env::set_var("QUILL_DATA_DIR", root.path());
            std::env::set_var("QUILL_PI_SESSIONS_DIR", root.path().join("pi"));
        }
        let storage = crate::storage::Storage::init().unwrap();
        storage
            .delete_setting("transcript_analytics_reingest_pending")
            .unwrap();
        let index = SessionIndex::open_or_create_for_tests(&root.path().join("index")).unwrap();
        let source = validate_retained_notify_source(IntegrationProvider::Pi, &path)
            .unwrap()
            .unwrap();
        let roots = vec![ProviderSourceRoot {
            provider: IntegrationProvider::Pi,
            source_root_key: source.source_root_key,
            resolved_root_path: root.path().join("pi"),
            canonical_root_path: Some(fs::canonicalize(root.path().join("pi")).unwrap()),
            sources: vec![source.clone()],
            outcome: ProviderRootEnumerationOutcome::Complete,
        }];
        let started = std::time::Instant::now();
        for round in 0..4 {
            if round > 0 {
                writeln!(file, "{}", serde_json::json!({"type":"message","id":format!("append-{round}"),"parentId":null,"timestamp":"2026-08-14T08:00:02Z","message":{"role":"user","content":"appended evidence"}})).unwrap();
            }
            let (analytics, search) = crate::transcript_analytics::reconcile_retained_source(
                &storage,
                &source,
                &SessionIndex::local_hostname(),
                Some(&index),
            );
            assert_eq!(
                analytics.unwrap(),
                crate::transcript_analytics::TranscriptSourceResult::Replaced
            );
            assert_eq!(search.unwrap(), mib + round);
            let decoded = crate::transcript_work::DECODE_COUNT.get();
            assert_eq!(index.sync_inner(&roots).unwrap(), 0);
            assert_eq!(
                crate::transcript_analytics::reconcile_live_transcript_source(
                    &storage,
                    &source,
                    &SessionIndex::local_hostname()
                )
                .unwrap(),
                crate::transcript_analytics::TranscriptSourceResult::SuppressedUnchanged
            );
            assert_eq!(
                crate::transcript_work::DECODE_COUNT.get(),
                decoded,
                "repeat admissions never decode unchanged bytes"
            );
            index.reader.reload().unwrap();
            assert_eq!(
                index
                    .search("", &Default::default(), "relevance", 0, 1)
                    .unwrap()
                    .total_hits,
                (mib + round) as u64
            );
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            {
                // Read-only allocator accounting, not trimming or allocator tuning.
                let heap = unsafe { libc::mallinfo2() };
                eprintln!(
                    "transcript-heap round={round} allocated_estimate={} arena_free={} arena_system={} decodes={decoded}",
                    heap.uordblks + heap.hblkhd,
                    heap.fordblks,
                    heap.arena
                );
            }
            eprintln!(
                "transcript-bench round={round} bytes={} elapsed_ms={} {}",
                fs::metadata(&path).unwrap().len(),
                started.elapsed().as_millis(),
                fs::read_to_string("/proc/self/status")
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.starts_with("VmRSS:")
                        || line.starts_with("VmHWM:")
                        || line.starts_with("VmSwap:"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_RULES_DIR");
            std::env::remove_var("QUILL_DATA_DIR");
            std::env::remove_var("QUILL_PI_SESSIONS_DIR");
        }
    }

    /// A Claude parent transcript is named by its session id, so it resolves
    /// by filename without opening a single transcript; sub-agent chains are
    /// only ever resolved through the analytics registry.
    // @lat: [[session-search-tests#Session Search Test Specs#Registry Backed Context Lookup]]
    #[test]
    fn claude_parent_lookup_resolves_by_filename_without_reading_transcripts() {
        let fixture = make_fixture();
        let session_id = "11111111-2222-3333-4444-555555555555";
        let expected = fixture
            .path()
            .join("-home-test-proj")
            .join(format!("{session_id}.jsonl"));

        assert_eq!(
            claude_parent_session_path(fixture.path(), session_id),
            Some(expected)
        );
        assert_eq!(
            claude_parent_session_path(fixture.path(), "aaaabbbbccccdddd"),
            None,
            "a sub-agent chain id is not a parent filename"
        );
        assert_eq!(claude_parent_session_path(fixture.path(), "missing"), None);
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#No Root Scan]]
    #[test]
    fn pi_session_lookup_does_not_walk_the_transcript_root() {
        let temp = TempDir::new().expect("tempdir");
        let nested = temp.path().join("--work-quill--");
        fs::create_dir_all(&nested).expect("create Pi project directory");
        let transcript = nested.join("different-name.jsonl");
        fs::write(
            &transcript,
            concat!(
                r#"{"type":"session","version":3,"id":"pi-header-id","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"entry","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":"context"}}"#,
                "\n",
            ),
        )
        .expect("write Pi transcript");

        assert_eq!(
            find_session_path(IntegrationProvider::Pi, "pi-header-id"),
            Ok(None)
        );
    }

    /// Result context for a Pi hit or a Claude sub-agent chain resolves the
    /// exact retained transcript through the analytics registry: one indexed
    /// query, no corpus walk, no transcript parse, and never a path outside
    /// the configured root.
    // @lat: [[session-search-tests#Session Search Test Specs#Registry Backed Context Lookup]]
    #[test]
    #[serial]
    fn registry_lookup_resolves_pi_and_subagent_chains_without_walking() {
        let root = TempDir::new().expect("tempdir");
        let claude = root.path().join("claude");
        let codex = root.path().join("codex");
        let pi = root.path().join("pi");
        for directory in [&claude, &codex, &pi] {
            fs::create_dir(directory).expect("create transcript root");
        }
        let session_id = "11111111-2222-3333-4444-555555555555";
        let agent_id = "aaaabbbbccccdddd";
        let subagents = claude
            .join("-home-test-proj")
            .join(session_id)
            .join("subagents");
        fs::create_dir_all(&subagents).expect("mkdir subagents");
        let agent_path = subagents.join(format!("agent-{agent_id}.jsonl"));
        fs::write(
            &agent_path,
            format!(
                "{{\"type\":\"user\",\"isSidechain\":true,\"agentId\":\"{agent_id}\",\"parentUuid\":null,\"uuid\":\"s1\",\"sessionId\":\"{session_id}\",\"timestamp\":\"2026-05-09T10:00:10Z\",\"cwd\":\"/home/test/proj\",\"message\":{{\"role\":\"user\",\"content\":\"do task\"}}}}\n"
            ),
        )
        .expect("write sub-agent transcript");
        let pi_dir = pi.join("--work-quill--");
        fs::create_dir_all(&pi_dir).expect("mkdir Pi project");
        let pi_path = pi_dir.join("different-name.jsonl");
        fs::write(
            &pi_path,
            concat!(
                r#"{"type":"session","version":3,"id":"pi-header-id","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"entry","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":"context"}}"#,
                "\n",
            ),
        )
        .expect("write Pi transcript");

        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", root.path());
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", &claude);
            std::env::set_var("QUILL_CODEX_SESSIONS_DIR", &codex);
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &pi);
        }
        let storage = crate::storage::Storage::init().expect("init storage");
        let hostname = SessionIndex::local_hostname();
        for (provider, path) in [
            (IntegrationProvider::Claude, &agent_path),
            (IntegrationProvider::Pi, &pi_path),
        ] {
            let source = validate_retained_notify_source(provider, path)
                .expect("validate retained source")
                .expect("retained source");
            crate::transcript_analytics::reconcile_live_transcript_source(
                &storage, &source, &hostname,
            )
            .expect("register source");
        }

        assert_eq!(
            registered_session_path(&storage, IntegrationProvider::Pi, "pi-header-id")
                .expect("Pi lookup"),
            Some(fs::canonicalize(&pi_path).expect("canonical Pi path"))
        );
        assert_eq!(
            registered_session_path(&storage, IntegrationProvider::Claude, agent_id)
                .expect("sub-agent lookup"),
            Some(fs::canonicalize(&agent_path).expect("canonical agent path")),
            "a sub-agent chain id opens its own transcript, not the parent's"
        );
        assert_eq!(
            registered_session_path(&storage, IntegrationProvider::Claude, "unknown")
                .expect("unknown lookup"),
            None
        );

        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_DATA_DIR");
            std::env::remove_var("QUILL_CLAUDE_PROJECTS_DIR");
            std::env::remove_var("QUILL_CODEX_SESSIONS_DIR");
            std::env::remove_var("QUILL_PI_SESSIONS_DIR");
        }
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Message Extraction]]
    #[test]
    fn pi_extraction_uses_header_identity_and_deduplicates_entry_ids() {
        let transcript = concat!(
            r#"{"type":"session","version":3,"id":"pi-session","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
            "\n",
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":"find the needle"}}"#,
            "\n",
            r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-08-14T08:00:02Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"skip"},{"type":"text","text":"needle found"}]}}"#,
            "\n",
            r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-08-14T08:00:03Z","message":{"role":"assistant","content":"duplicate"}}"#,
            "\n",
        );

        let extracted = extract_messages_from_jsonl_contents(
            IntegrationProvider::Pi,
            Path::new("not-the-session-id.jsonl"),
            transcript,
        );

        assert_eq!(extracted.session_id, "pi-session");
        assert_eq!(extracted.project_name.as_deref(), Some("quill"));
        assert_eq!(extracted.messages.len(), 2);
        assert_eq!(extracted.messages[0].uuid, "user-1");
        assert_eq!(extracted.messages[1].uuid, "assistant-1");
        assert_eq!(extracted.messages[1].content, "needle found");
        assert_eq!(extracted.messages[1].parent_uuid.as_deref(), Some("user-1"));
        assert!(
            extracted
                .messages
                .iter()
                .all(|message| message.session_id == "pi-session"
                    && message.cwd.as_deref() == Some("/work/quill"))
        );
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Retained Thinking Event Classification]]
    #[test]
    fn pi_retained_thinking_events_are_ordered_and_keep_thinking_only_messages() {
        let transcript = [
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "pi-thinking-events",
                "timestamp": "2026-08-14T08:00:00Z",
                "cwd": "/work/quill"
            }),
            serde_json::json!({
                "type": "message",
                "id": "mixed",
                "parentId": null,
                "timestamp": "2026-08-14T08:00:01Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": ""},
                        {"type": "text", "text": "answer"},
                        {"type": "toolCall", "id": "call", "name": "read", "arguments": {}}
                    ]
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "thinking-only",
                "parentId": "mixed",
                "timestamp": "2026-08-14T08:00:02Z",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "thinking", "thinking": "only"}]
                }
            }),
        ]
        .into_iter()
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        let extracted = extract_messages_from_jsonl_contents(
            IntegrationProvider::Pi,
            Path::new("session.jsonl"),
            &transcript,
        );

        assert_eq!(
            extracted
                .events
                .iter()
                .map(|event| (event.source_ordinal, event.event_ordinal, event.kind))
                .collect::<Vec<_>>(),
            vec![
                (1, 0, SessionEventKind::AsstThinking),
                (1, 1, SessionEventKind::AsstText),
                (1, 2, SessionEventKind::AsstToolUse),
                (2, 0, SessionEventKind::AsstThinking),
            ]
        );
        assert_eq!(
            extracted.messages.len(),
            1,
            "thinking-only entries stay out of search while entering retained events"
        );
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Tool Result Correlation]]
    #[test]
    fn pi_tool_results_attach_to_assistant_actions_without_becoming_messages() {
        let output = "x".repeat(12_000);
        let transcript = [
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "pi-tools",
                "timestamp": "2026-08-14T08:00:00Z",
                "cwd": "/work/quill"
            })
            .to_string(),
            serde_json::json!({
                "type": "message",
                "id": "assistant-1",
                "parentId": null,
                "timestamp": "2026-08-14T08:00:01Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "running"},
                        {
                            "type": "toolCall",
                            "id": "call-1",
                            "name": "bash",
                            "arguments": {"command": "printf hello"}
                        }
                    ]
                }
            })
            .to_string(),
            serde_json::json!({
                "type": "message",
                "id": "result-1",
                "parentId": "assistant-1",
                "timestamp": "2026-08-14T08:00:02Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "call-1",
                    "toolName": "bash",
                    "content": [{"type": "text", "text": output}]
                }
            })
            .to_string(),
            serde_json::json!({
                "type": "message",
                "id": "custom-1",
                "parentId": "result-1",
                "timestamp": "2026-08-14T08:00:03Z",
                "message": {"role": "custom", "content": "do not index"}
            })
            .to_string(),
        ]
        .join("\n");

        let extracted = extract_messages_from_jsonl_contents(
            IntegrationProvider::Pi,
            Path::new("session.jsonl"),
            &transcript,
        );

        assert_eq!(extracted.messages.len(), 1);
        let message = &extracted.messages[0];
        assert_eq!(message.role, "assistant");
        assert_eq!(message.content, "running");
        assert_eq!(message.tools_used, vec!["bash"]);
        assert_eq!(message.commands_run.len(), 1);
        assert!(message.commands_run[0].starts_with("$ printf hello\n"));
        assert!(message.commands_run[0].len() < 400);
        assert_eq!(message.tool_actions.len(), 1);
        let action = &message.tool_actions[0];
        assert_eq!(action.category, "command");
        assert!(
            action
                .full_output
                .as_deref()
                .is_some_and(|value| value.len() <= 10_256 && value.ends_with("... [truncated]"))
        );
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Tool Result Correlation]]
    #[test]
    fn pi_tool_result_evidence_is_bounded_and_last_write_wins() {
        let bounded_details = serde_json::json!({"payload": "x".repeat(10_226)});
        let bounded_details_json =
            serde_json::to_string(&bounded_details).expect("serialize details");
        assert_eq!(bounded_details_json.len(), TOOL_RESULT_DETAILS_MAX_BYTES);
        let oversized_details = serde_json::json!({"diff": "x".repeat(10_230)});
        let transcript = [
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "pi-tool-evidence",
                "timestamp": "2026-08-14T08:00:00Z",
                "cwd": "/work/quill"
            }),
            serde_json::json!({
                "type": "message",
                "id": "assistant-1",
                "parentId": null,
                "timestamp": "2026-08-14T08:00:01Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "toolCall", "id": "command", "name": "bash", "arguments": {"command": "echo hi"}},
                        {"type": "toolCall", "id": "code", "name": "write", "arguments": {"path": "src/main.rs", "content": "one\ntwo"}},
                        {"type": "toolCall", "id": "detail", "name": "read", "arguments": {"path": "README.md"}}
                    ]
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "result-command-first",
                "parentId": "assistant-1",
                "timestamp": "2026-08-14T08:00:02Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "command",
                    "isError": true,
                    "details": {"result": "first"},
                    "content": [{"type": "image"}, {"type": "image"}]
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "result-code",
                "parentId": "assistant-1",
                "timestamp": "2026-08-14T08:00:03Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "code",
                    "isError": true,
                    "details": oversized_details,
                    "content": [{"type": "image"}, {"type": "text", "text": "written"}, {"type": "image"}]
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "result-detail",
                "parentId": "assistant-1",
                "timestamp": "2026-08-14T08:00:04Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "detail",
                    "details": "not-an-object",
                    "content": "read"
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "result-command-last",
                "parentId": "assistant-1",
                "timestamp": "2026-08-14T08:00:05Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "command",
                    "isError": false,
                    "details": bounded_details,
                    "content": [{"type": "text", "text": "done"}, {"type": "image"}]
                }
            }),
        ]
        .into_iter()
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        let extracted = extract_messages_from_jsonl_contents(
            IntegrationProvider::Pi,
            Path::new("session.jsonl"),
            &transcript,
        );
        let actions = &extracted.messages[0].tool_actions;
        let action = |tool_use_id| {
            actions
                .iter()
                .find(|action| action.tool_use_id == tool_use_id)
                .expect("tool action")
        };

        let command = action("command");
        assert_eq!(command.is_error, Some(false));
        assert_eq!(
            command.details_json.as_deref(),
            Some(bounded_details_json.as_str())
        );
        assert_eq!(command.result_image_count, Some(1));

        let code = action("code");
        assert_eq!(code.is_error, Some(true));
        assert_eq!(code.details_json, None);
        assert_eq!(code.result_image_count, Some(2));
        assert_eq!((code.lines_added, code.lines_removed), (Some(2), Some(0)));

        let detail = action("detail");
        assert_eq!(detail.is_error, Some(false));
        assert_eq!(detail.details_json, None);
        assert_eq!(detail.result_image_count, Some(0));
    }

    // @lat: [[session-search-tests#Session Search Test Specs#Index Test Resource Budget]]
    #[test]
    fn production_writer_heap_budget_remains_unchanged() {
        assert_eq!(SessionIndex::PRODUCTION_WRITER_HEAP_BYTES, 50_000_000);
        assert_eq!(SessionIndex::TEST_WRITER_HEAP_BYTES, 15_000_000);
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Provider Safe Search]]
    #[test]
    fn pi_search_hits_and_facets_keep_provider_identity_and_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let index = SessionIndex::open_or_create_for_tests(temp.path()).expect("open index");
        let messages = extract_messages_from_jsonl_contents(
            IntegrationProvider::Pi,
            Path::new("session.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"shared","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"pi-entry","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":"provider-safe-needle"}}"#,
                "\n",
            ),
        )
        .messages;
        index
            .replace_session_docs_batch(
                IntegrationProvider::Pi,
                "shared",
                "quill",
                "host",
                &messages,
            )
            .expect("index Pi message");
        index.reader.reload().expect("reload index");

        let result = index
            .search(
                "provider-safe-needle",
                &SearchFilters {
                    provider: Some(IntegrationProvider::Pi),
                    ..SearchFilters::default()
                },
                "relevance",
                0,
                10,
            )
            .expect("search Pi messages");
        assert_eq!(result.total_hits, 1);
        assert_eq!(result.hits[0].provider, IntegrationProvider::Pi);
        assert_eq!(result.hits[0].message_id, "pi-entry");
        assert_eq!(result.hits[0].project, "quill");
        assert_eq!(result.hits[0].host, "host");
        assert!(
            index
                .get_facets()
                .expect("facets")
                .providers
                .iter()
                .any(|facet| { facet.name == "pi" && facet.count == 1 })
        );
    }

    // @lat: [[session-search-tests#Session Search Test Specs#Conversation Role Guard]]
    #[test]
    fn search_excludes_non_conversation_roles() {
        let temp = TempDir::new().expect("tempdir");
        let index = SessionIndex::open_or_create_for_tests(temp.path()).expect("open index");
        let make_message = |uuid: &str, role: &str, content: &str| ExtractedMessage {
            uuid: uuid.to_string(),
            session_id: "roles".to_string(),
            parent_session_id: None,
            role: role.to_string(),
            content: content.to_string(),
            timestamp: "2026-08-14T08:00:01Z".to_string(),
            git_branch: String::new(),
            tools_used: Vec::new(),
            files_modified: Vec::new(),
            code_changes: Vec::new(),
            commands_run: Vec::new(),
            tool_details: Vec::new(),
            tool_actions: Vec::new(),
            parent_uuid: None,
            cwd: None,
            custom_type: None,
        };
        index
            .replace_session_docs_batch(
                IntegrationProvider::Pi,
                "roles",
                "project",
                "host",
                &[
                    make_message("user", "user", "conversation-needle"),
                    make_message("result", "toolResult", "legacy-tool-result-needle"),
                ],
            )
            .expect("index Pi messages");
        index
            .replace_session_docs_batch(
                IntegrationProvider::Codex,
                "roles",
                "project",
                "host",
                &[make_message("collab", "reviewer", "codex-collab-needle")],
            )
            .expect("index Codex collaboration message");
        index.reader.reload().expect("reload index");

        assert_eq!(
            index
                .search(
                    "legacy-tool-result-needle",
                    &SearchFilters::default(),
                    "relevance",
                    0,
                    10,
                )
                .expect("search legacy role")
                .total_hits,
            0
        );
        assert_eq!(
            index
                .search(
                    "conversation-needle",
                    &SearchFilters::default(),
                    "relevance",
                    0,
                    10,
                )
                .expect("search conversation role")
                .total_hits,
            1
        );
        assert_eq!(
            index
                .search(
                    "codex-collab-needle",
                    &SearchFilters::default(),
                    "relevance",
                    0,
                    10,
                )
                .expect("search Codex collaboration role")
                .total_hits,
            1
        );
    }

    // @lat: [[session-search-tests#Session Search Test Specs#Injected Context Search]]
    #[test]
    fn pi_custom_messages_index_with_their_custom_type_and_emit_no_events() {
        let transcript = [
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "pi-injected",
                "timestamp": "2026-08-14T08:00:00Z",
                "cwd": "/work/quill"
            }),
            serde_json::json!({
                "type": "message",
                "id": "prompt",
                "parentId": null,
                "timestamp": "2026-08-14T08:00:01Z",
                "message": {"role": "user", "content": "conversation-needle"}
            }),
            serde_json::json!({
                "type": "custom_message",
                "id": "hidden-notify",
                "parentId": "prompt",
                "timestamp": "2026-08-14T08:00:02Z",
                "customType": "subagent-notify",
                "content": "hidden-injected-needle",
                "display": false
            }),
            serde_json::json!({
                "type": "custom",
                "id": "non-context",
                "parentId": "hidden-notify",
                "timestamp": "2026-08-14T08:00:03Z",
                "customType": "quill",
                "data": {"note": "excluded-custom-needle"}
            }),
            serde_json::json!({
                "type": "custom",
                "id": "tracking",
                "parentId": "non-context",
                "timestamp": "2026-08-14T08:00:04Z",
                "customType": "quill-tracking",
                "data": {
                    "schema": crate::pi_tracking::PI_PROTOCOL_V2_TRACKING_SCHEMA,
                    "reporter": {
                        "protocol": crate::pi_tracking::PI_PROTOCOL_V2,
                        "version": crate::pi_tracking::PI_PROTOCOL_V2_REPORTER_VERSION,
                        "quill_build": crate::pi_tracking::PI_PROTOCOL_V2_QUILL_BUILD,
                        "capability_digest":
                            crate::pi_tracking::PI_PROTOCOL_V2_CAPABILITY_DIGEST,
                    },
                    "event_uuid": "tracking-excluded-needle",
                    "event": "session_start",
                    "provider": "pi",
                    "normalized_host": "host",
                    "session_id": "pi-injected",
                    "process_instance_id": "process-1",
                    "sequence": 1,
                    "origin_at": "2026-08-14T08:00:04Z",
                    "occurred_at": "2026-08-14T08:00:04Z",
                    "delivery_source": "live",
                    "reason": "startup",
                    "lineage": {"kind": "root"}
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "answer",
                "parentId": "tracking",
                "timestamp": "2026-08-14T08:00:05Z",
                "message": {"role": "assistant", "content": "answer"}
            }),
        ]
        .map(|line| line.to_string())
        .join("\n");

        let extracted = extract_messages_from_jsonl_contents(
            IntegrationProvider::Pi,
            Path::new("session.jsonl"),
            &transcript,
        );
        assert_eq!(
            extracted
                .messages
                .iter()
                .map(|message| (
                    message.uuid.as_str(),
                    message.role.as_str(),
                    message.custom_type.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("prompt", "user", None),
                ("hidden-notify", "custom_message", Some("subagent-notify")),
                ("answer", "assistant", None),
            ],
            "injected context indexes in source order with its custom type"
        );
        assert!(
            extracted
                .events
                .iter()
                .all(|event| event.uuid.as_deref() != Some("hidden-notify")),
            "injected context emits no session or runtime event"
        );

        let temp = TempDir::new().expect("tempdir");
        let index = SessionIndex::open_or_create_for_tests(temp.path()).expect("open index");
        index
            .replace_session_docs_batch(
                IntegrationProvider::Pi,
                "pi-injected",
                "quill",
                "host",
                &extracted.messages,
            )
            .expect("index Pi session");
        index.reader.reload().expect("reload index");

        let hits = |query: &str| {
            index
                .search(query, &SearchFilters::default(), "relevance", 0, 10)
                .expect("search index")
                .hits
        };
        let injected = hits("hidden-injected-needle");
        assert_eq!(injected.len(), 1);
        assert_eq!(injected[0].role, "custom_message");
        assert_eq!(injected[0].message_id, "hidden-notify");
        assert_eq!(
            hits("custom_type:subagent-notify")
                .iter()
                .map(|hit| hit.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["hidden-notify"],
            "customType is searchable metadata"
        );
        assert!(
            hits("excluded-custom-needle").is_empty(),
            "non-context custom entries stay out of search"
        );
        assert!(
            hits("tracking-excluded-needle").is_empty(),
            "quill-tracking entries stay out of search"
        );
        assert_eq!(hits("conversation-needle").len(), 1);
    }

    // @lat: [[session-search-tests#Session Search Test Specs#Compact AI Results]]
    #[test]
    fn compact_search_results_omit_content_and_obey_byte_budget() {
        let hit = SearchHit {
            provider: IntegrationProvider::Pi,
            message_id: "message".to_string(),
            session_id: "session".to_string(),
            parent_session_id: None,
            content: "raw-tool-output".repeat(10_000),
            snippet: "matching snippet ".repeat(1_000),
            role: "assistant".to_string(),
            project: "quill".to_string(),
            session_name: Some("named session".to_string()),
            host: "host".to_string(),
            timestamp: "2026-08-14T08:00:01Z".to_string(),
            git_branch: "main".to_string(),
            tools_used: "bash".to_string(),
            files_modified: String::new(),
            code_changes: String::new(),
            commands_run: String::new(),
            tool_details: String::new(),
            score: 1.0,
        };
        let compact = SearchResults {
            hits: vec![hit; 50],
            total_hits: 50,
            query_time_ms: 2,
        }
        .compact_for_ai(32 * 1024);
        let encoded = serde_json::to_vec(&compact).expect("serialize compact search results");
        let hits = compact["hits"].as_array().expect("compact hits");

        assert!(encoded.len() <= 32 * 1024);
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.get("content").is_none()));
        assert!(
            hits.iter()
                .all(|hit| hit["session_name"] == "named session")
        );
        assert_eq!(compact["truncated"], true);
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Working Directory Filter]]
    #[test]
    fn absolute_project_filter_distinguishes_same_named_directories() {
        let temp = TempDir::new().expect("tempdir");
        let index = SessionIndex::open_or_create_for_tests(temp.path()).expect("open index");

        for (session_id, message_id, cwd) in [
            ("session-a", "message-a", "/work/team-a/quill"),
            ("session-b", "message-b", "/work/team-b/quill"),
        ] {
            let transcript = format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"{session_id}\",\"timestamp\":\"2026-08-14T08:00:00Z\",\"cwd\":\"{cwd}\"}}\n{{\"type\":\"message\",\"id\":\"{message_id}\",\"parentId\":null,\"timestamp\":\"2026-08-14T08:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":\"shared-search-term\"}}}}\n"
            );
            let messages = extract_messages_from_jsonl_contents(
                IntegrationProvider::Pi,
                Path::new("session.jsonl"),
                &transcript,
            )
            .messages;
            index
                .replace_session_docs_batch(
                    IntegrationProvider::Pi,
                    session_id,
                    "quill",
                    "host",
                    &messages,
                )
                .expect("index Pi message");
        }
        index.reader.reload().expect("reload index");

        let result = index
            .search(
                "shared-search-term",
                &SearchFilters {
                    project: Some("/work/team-a/quill".to_string()),
                    ..SearchFilters::default()
                },
                "relevance",
                0,
                10,
            )
            .expect("search one cwd");

        assert_eq!(result.total_hits, 1);
        assert_eq!(result.hits[0].message_id, "message-a");
    }

    // @lat: [[session-search-tests#Session Search Test Specs#Schema Rebuild Measurement]]
    #[test]
    #[ignore = "reproducible index rebuild wall-time measurement"]
    fn measure_session_index_schema_rebuild_on_pinned_corpus() {
        const MANIFEST: &str = "pi-analytics-migration-v1\nsessions=80\nentries=30700\nassistant_messages=12685\ntool_results=16670\n";
        const MANIFEST_SHA256: &str =
            "0489da2b94fe813d785f8b5bc4ed2f871b3f0732cde6aab5334c55788f9f673e";
        const SESSIONS: usize = 80;
        // Every audit-window entry that becomes a search document: assistant
        // messages plus the non-assistant, non-tool-result remainder.
        const DOCUMENTS: usize = 30_700 - 16_670;
        // `custom_message` entries observed in the same window; they carry the
        // schema-8 `custom_type` field this rebuild exists for.
        const INJECTED_CONTEXT: usize = 655;
        const PER_SESSION: usize = DOCUMENTS.div_ceil(SESSIONS);

        use sha2::Digest;
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(MANIFEST)),
            MANIFEST_SHA256
        );
        let temp = TempDir::new().expect("tempdir");
        let corpus = (0..DOCUMENTS)
            .map(|index| {
                let injected = index < INJECTED_CONTEXT;
                ExtractedMessage {
                    uuid: format!("message-{index:05}"),
                    session_id: format!("session-{:03}", index / PER_SESSION),
                    parent_session_id: None,
                    role: if injected {
                        PI_CUSTOM_MESSAGE_ROLE.to_string()
                    } else {
                        "assistant".to_string()
                    },
                    content: format!(
                        "measured rebuild document {index} with enough prose to tokenize \
                         like a real transcript entry rather than a single term"
                    ),
                    timestamp: "2026-08-24T00:00:00Z".to_string(),
                    git_branch: "main".to_string(),
                    tools_used: vec!["bash".to_string()],
                    files_modified: vec![format!("/work/quill/src/file-{index:05}.rs")],
                    code_changes: Vec::new(),
                    commands_run: vec![format!("$ cargo test case-{index:05}")],
                    tool_details: Vec::new(),
                    tool_actions: Vec::new(),
                    parent_uuid: None,
                    cwd: Some("/work/quill".to_string()),
                    custom_type: injected.then(|| "subagent-notify".to_string()),
                }
            })
            .collect::<Vec<_>>();
        // One writer and one commit for the whole sweep, exactly as
        // `startup_scan` reindexes after a schema bump.
        let index_corpus = |index: &SessionIndex| {
            let writer = index.writer.lock().unwrap();
            for message in &corpus {
                index
                    .add_message_to_writer(
                        &writer,
                        IntegrationProvider::Pi,
                        message,
                        "quill",
                        "host",
                    )
                    .expect("index corpus document");
            }
            drop(writer);
            index
                .writer
                .lock()
                .unwrap()
                .commit()
                .expect("commit corpus");
        };
        {
            let index = SessionIndex::open_or_create_for_tests(temp.path()).expect("seed index");
            index_corpus(&index);
        }
        let bytes_before = fs::read_dir(temp.path())
            .expect("read seeded index")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum::<u64>();
        fs::write(
            temp.path().join("schema_version.txt"),
            (SessionIndex::SCHEMA_VERSION - 1).to_string(),
        )
        .expect("pin pre-upgrade schema version");

        let started = std::time::Instant::now();
        let rebuilt = SessionIndex::open_or_create_for_tests(temp.path()).expect("rebuild index");
        index_corpus(&rebuilt);
        rebuilt.reader.reload().expect("reload rebuilt index");
        let wall_time = started.elapsed();

        assert_eq!(
            rebuilt
                .search(
                    "custom_type:subagent-notify",
                    &SearchFilters::default(),
                    "relevance",
                    0,
                    1
                )
                .expect("search rebuilt index")
                .total_hits,
            INJECTED_CONTEXT as u64
        );
        eprintln!(
            "index-rebuild-measurement manifest_sha256={MANIFEST_SHA256} sessions={SESSIONS} documents={DOCUMENTS} injected_context={INJECTED_CONTEXT} bytes_before={bytes_before} wall_time_ms={}",
            wall_time.as_millis()
        );
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Search Schema Rebuild]]
    #[test]
    fn search_schema_change_rebuilds_existing_index() {
        let temp = TempDir::new().expect("tempdir");
        fs::write(
            temp.path().join("schema_version.txt"),
            (SessionIndex::SCHEMA_VERSION - 1).to_string(),
        )
        .expect("write old version");
        let obsolete = temp.path().join("obsolete-index-file");
        fs::write(&obsolete, "old schema").expect("write old index marker");

        let _index = SessionIndex::open_or_create_for_tests(temp.path()).expect("open index");

        assert_eq!(
            fs::read_to_string(temp.path().join("schema_version.txt")).expect("read version"),
            SessionIndex::SCHEMA_VERSION.to_string()
        );
        assert!(!obsolete.exists(), "old schema contents must be removed");
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Provider Safe Cleanup]]
    #[test]
    fn pi_cleanup_does_not_delete_same_id_from_other_providers() {
        let temp = TempDir::new().expect("tempdir");
        let index = SessionIndex::open_or_create_for_tests(temp.path()).expect("open index");
        let make_message = |uuid: &str, content: &str| ExtractedMessage {
            uuid: uuid.to_string(),
            session_id: "shared".to_string(),
            parent_session_id: None,
            role: "user".to_string(),
            content: content.to_string(),
            timestamp: "2026-08-14T08:00:01Z".to_string(),
            git_branch: String::new(),
            tools_used: Vec::new(),
            files_modified: Vec::new(),
            code_changes: Vec::new(),
            commands_run: Vec::new(),
            tool_details: Vec::new(),
            tool_actions: Vec::new(),
            parent_uuid: None,
            cwd: None,
            custom_type: None,
        };
        index
            .replace_session_docs_batch(
                IntegrationProvider::Claude,
                "shared",
                "project",
                "host",
                &[make_message("claude-entry", "keep-me")],
            )
            .expect("index Claude message");
        index
            .replace_session_docs_batch(
                IntegrationProvider::Pi,
                "shared",
                "project",
                "host",
                &[make_message("pi-entry", "remove-me")],
            )
            .expect("index Pi message");
        index
            .replace_session_docs_batch(IntegrationProvider::Pi, "shared", "project", "host", &[])
            .expect("clean Pi session");
        index.reader.reload().expect("reload index");

        assert_eq!(
            index
                .search("keep-me", &SearchFilters::default(), "relevance", 0, 10)
                .expect("search Claude message")
                .total_hits,
            1
        );
        assert_eq!(
            index
                .search("remove-me", &SearchFilters::default(), "relevance", 0, 10)
                .expect("search Pi message")
                .total_hits,
            0
        );
    }

    #[test]
    fn codex_spawn_identity_reaches_messages() {
        let records = parse_jsonl_records(concat!(
            r#"{"type":"session_meta","payload":{"id":"child-1","thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-1"}}}}}"#,
            "\n",
            r#"{"type":"event_msg","timestamp":"2026-08-03T12:00:00Z","payload":{"type":"agent_message","message":"done"}}"#,
        ));
        let extracted = extract_codex_messages_from_jsonl_records(&records);

        assert_eq!(extracted.session_id, "child-1");
        assert_eq!(extracted.messages.len(), 1);
        assert_eq!(extracted.messages[0].session_id, "child-1");
        assert_eq!(extracted.events.len(), 1);
    }

    /// quill-fqwp regression: a Codex rollout with no `session_meta` record
    /// can never resolve identity. It must be fingerprinted like any other
    /// attempted file and skipped on every later sweep, so one poisoned
    /// transcript cannot make the sweep re-extract the corpus each pass. A
    /// source that disappears from a completely enumerated root has its
    /// documents pruned by the session id remembered for it.
    // @lat: [[data-flow#Session Indexing Pipeline]]
    #[test]
    #[serial]
    fn sweep_fingerprints_unresolvable_sources_once_and_prunes_vanished_ones() {
        let root = TempDir::new().expect("tempdir");
        let claude = root.path().join("claude");
        let codex = root.path().join("codex");
        let pi = root.path().join("pi");
        for directory in [&claude, &codex, &pi] {
            fs::create_dir(directory).expect("create transcript root");
        }

        // No `session_meta` record at all -- identity is permanently
        // unresolvable no matter how many times this file is retried.
        fs::write(
            codex.join("rollout-poison.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "event_msg",
                    "timestamp": "2026-08-03T12:00:00Z",
                    "payload": {"type": "agent_message", "message": "orphaned"}
                })
            ),
        )
        .expect("write poisoned Codex transcript");

        for index in 0..3 {
            let lines = [
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": format!("healthy-{index}")}
                })
                .to_string(),
                serde_json::json!({
                    "type": "event_msg",
                    "timestamp": "2026-08-03T12:00:00Z",
                    "payload": {"type": "agent_message", "message": format!("needle-{index}")}
                })
                .to_string(),
            ]
            .join("\n")
                + "\n";
            fs::write(codex.join(format!("rollout-healthy-{index}.jsonl")), lines)
                .expect("write healthy Codex transcript");
        }

        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", root.path());
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", &claude);
            std::env::set_var("QUILL_CODEX_SESSIONS_DIR", &codex);
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &pi);
        }

        let index_dir = root.path().join("index");
        let index = SessionIndex::open_or_create_for_tests(&index_dir).expect("open index");

        assert_eq!(index.sync_without_emit().expect("first sweep"), 3);
        let sources_after_first_sweep = index.state.lock().unwrap().sources.clone();
        assert_eq!(
            sources_after_first_sweep.len(),
            4,
            "every discovered transcript, including the poisoned one, is fingerprinted"
        );
        assert_eq!(
            sources_after_first_sweep
                .values()
                .filter(|indexed| indexed.session_id.is_empty())
                .count(),
            1,
            "the unresolvable source is remembered without a session id"
        );

        assert_eq!(
            index.sync_without_emit().expect("second sweep"),
            0,
            "the second sweep must not re-extract any transcript"
        );
        assert_eq!(
            index.state.lock().unwrap().sources,
            sources_after_first_sweep,
            "second sweep must not disturb the fingerprints"
        );

        fs::remove_file(codex.join("rollout-healthy-1.jsonl")).expect("remove one transcript");
        assert_eq!(index.sync_without_emit().expect("third sweep"), 0);
        assert_eq!(index.state.lock().unwrap().sources.len(), 3);
        index.reader.reload().expect("reload index");
        assert_eq!(
            index
                .search("needle-1", &SearchFilters::default(), "relevance", 0, 10)
                .expect("search pruned session")
                .total_hits,
            0,
            "a vanished source's documents are pruned"
        );
        assert_eq!(
            index
                .search("needle-0", &SearchFilters::default(), "relevance", 0, 10)
                .expect("search retained session")
                .total_hits,
            1
        );

        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_DATA_DIR");
            std::env::remove_var("QUILL_CLAUDE_PROJECTS_DIR");
            std::env::remove_var("QUILL_CODEX_SESSIONS_DIR");
            std::env::remove_var("QUILL_PI_SESSIONS_DIR");
        }
    }
}
