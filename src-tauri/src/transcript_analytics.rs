//! Complete in-memory analytics snapshots parsed from retained transcripts.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::Metadata;
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use crate::integrations::IntegrationProvider;
use crate::sessions::{
    DiscoveredRetainedJsonlSource, ExtractedMessage, ProviderRootEnumerationOutcome,
    ProviderSourceRoot, RetainedJsonlSourceLayoutHint, SessionEventKind,
    enumerate_retained_jsonl_source_roots, extract_messages_from_jsonl_records,
    extract_skill_accesses_from_tool_action, retained_jsonl_source_root_identities,
};
use crate::storage::{
    Storage, StoredTranscriptAnalyticsSource, TranscriptAnalyticsReplacement,
    UnchangedTranscriptAnalyticsSource, model_source_content_sha256,
};
use crate::transcript_identity::{
    IdentityError, JsonlRecord, NativeChainIdentity, RETAINED_TRANSCRIPT_MAX_BYTES,
    SourceRootGraph, parse_jsonl_records, resolve_codex_native_identity,
};
use chrono::DateTime;
use parking_lot::{Condvar, Mutex};
use serde_json::Value;

const STABLE_READ_MAX_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TranscriptReconciliationRootKey {
    provider: &'static str,
    source_root_key: String,
}

static ACTIVE_TRANSCRIPT_RECONCILIATION_ROOTS: LazyLock<(
    Mutex<HashSet<TranscriptReconciliationRootKey>>,
    Condvar,
)> = LazyLock::new(|| (Mutex::new(HashSet::new()), Condvar::new()));

struct TranscriptReconciliationPermit {
    roots: Vec<TranscriptReconciliationRootKey>,
}

impl Drop for TranscriptReconciliationPermit {
    fn drop(&mut self) {
        let (active, wake) = &*ACTIVE_TRANSCRIPT_RECONCILIATION_ROOTS;
        let mut active = active.lock();
        for root in &self.roots {
            active.remove(root);
        }
        wake.notify_all();
    }
}

fn acquire_transcript_reconciliation(
    roots: impl IntoIterator<Item = (IntegrationProvider, String)>,
) -> Result<TranscriptReconciliationPermit, String> {
    let mut roots = roots
        .into_iter()
        .map(
            |(provider, source_root_key)| TranscriptReconciliationRootKey {
                provider: provider.as_str(),
                source_root_key,
            },
        )
        .collect::<Vec<_>>();
    roots.sort_by(|left, right| {
        left.provider
            .cmp(right.provider)
            .then_with(|| left.source_root_key.cmp(&right.source_root_key))
    });
    roots.dedup();
    if roots.is_empty() || roots.iter().any(|root| root.source_root_key.is_empty()) {
        return Err("Transcript reconciliation root identity is incomplete".into());
    }

    let (active, wake) = &*ACTIVE_TRANSCRIPT_RECONCILIATION_ROOTS;
    let mut active = active.lock();
    while roots.iter().any(|root| active.contains(root)) {
        wake.wait(&mut active);
    }
    active.extend(roots.iter().cloned());
    Ok(TranscriptReconciliationPermit { roots })
}

#[cfg(unix)]
type SourceFileIdentity = (u64, u64);
#[cfg(not(unix))]
type SourceFileIdentity = Option<std::time::SystemTime>;

#[cfg(unix)]
fn source_file_identity(metadata: &Metadata) -> SourceFileIdentity {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

// Windows' `volume_serial_number`/`file_index` are gated behind the unstable
// `windows_by_handle` feature, so stable release builds fall back to creation
// time. This identity only guards intra-read swap detection alongside the
// separately tracked mtime and size; durable cross-rename identity comes from
// the JSONL native chain, not the file system.
#[cfg(not(unix))]
fn source_file_identity(metadata: &Metadata) -> SourceFileIdentity {
    metadata.created().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StableSourceStat {
    identity: SourceFileIdentity,
    mtime_ns: i64,
    size_bytes: i64,
}

fn stable_source_stat(metadata: &Metadata) -> Result<StableSourceStat, TranscriptAnalyticsError> {
    let modified = metadata
        .modified()
        .map_err(TranscriptAnalyticsError::Read)?;
    let elapsed = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TranscriptAnalyticsError::InvalidSourceMetadata)?;
    Ok(StableSourceStat {
        identity: source_file_identity(metadata),
        mtime_ns: i64::try_from(elapsed.as_nanos())
            .map_err(|_| TranscriptAnalyticsError::InvalidSourceMetadata)?,
        size_bytes: i64::try_from(metadata.len())
            .map_err(|_| TranscriptAnalyticsError::InvalidSourceMetadata)?,
    })
}

fn read_stable_transcript(
    path: &std::path::Path,
) -> Result<(Vec<u8>, StableSourceStat), TranscriptAnalyticsError> {
    for _ in 0..STABLE_READ_MAX_ATTEMPTS {
        let before =
            stable_source_stat(&std::fs::metadata(path).map_err(TranscriptAnalyticsError::Read)?)?;
        if u64::try_from(before.size_bytes).is_ok_and(|size| size > RETAINED_TRANSCRIPT_MAX_BYTES) {
            return Err(TranscriptAnalyticsError::SourceTooLarge);
        }

        let mut file = std::fs::File::open(path).map_err(TranscriptAnalyticsError::Read)?;
        let opened_before =
            stable_source_stat(&file.metadata().map_err(TranscriptAnalyticsError::Read)?)?;
        if opened_before != before {
            continue;
        }

        let mut bytes = Vec::new();
        file.by_ref()
            .take(RETAINED_TRANSCRIPT_MAX_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(TranscriptAnalyticsError::Read)?;
        if bytes.len() as u64 > RETAINED_TRANSCRIPT_MAX_BYTES {
            return Err(TranscriptAnalyticsError::SourceTooLarge);
        }

        let opened_after =
            stable_source_stat(&file.metadata().map_err(TranscriptAnalyticsError::Read)?)?;
        let after =
            stable_source_stat(&std::fs::metadata(path).map_err(TranscriptAnalyticsError::Read)?)?;
        let read_size_matches = usize::try_from(after.size_bytes).ok() == Some(bytes.len());
        if before == opened_after && before == after && read_size_matches {
            return Ok((bytes, after));
        }
    }
    Err(TranscriptAnalyticsError::UnstableSource)
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptAnalyticsSourceState {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_root_key: String,
    pub(crate) source_key: String,
    pub(crate) source_path: PathBuf,
    pub(crate) source_session_id: String,
    pub(crate) analytics_session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) is_sidechain: bool,
    pub(crate) agent_id: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) hostname: String,
    pub(crate) mtime_ns: i64,
    pub(crate) size_bytes: i64,
    pub(crate) content_sha256: String,
    pub(crate) seen_generation: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnedSessionEvent {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: String,
    pub(crate) event_key: String,
    pub(crate) session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) is_sidechain: bool,
    pub(crate) timestamp: String,
    pub(crate) kind: SessionEventKind,
    pub(crate) uuid: Option<String>,
    pub(crate) parent_uuid: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnedResponseTime {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: String,
    pub(crate) session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) timestamp: String,
    pub(crate) response_secs: Option<f64>,
    pub(crate) idle_secs: Option<f64>,
    pub(crate) is_sidechain: bool,
    pub(crate) agent_id: Option<String>,
    pub(crate) parent_uuid: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnedToolAction {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: String,
    pub(crate) action_key: String,
    pub(crate) message_id: String,
    pub(crate) session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) tool_name: String,
    pub(crate) category: String,
    pub(crate) file_path: Option<String>,
    pub(crate) summary: String,
    pub(crate) full_input: Option<String>,
    pub(crate) full_output: Option<String>,
    pub(crate) timestamp: String,
    pub(crate) is_sidechain: bool,
    pub(crate) agent_id: Option<String>,
    pub(crate) parent_uuid: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnedSkillUsage {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: String,
    pub(crate) session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) message_id: String,
    pub(crate) skill_name: String,
    pub(crate) skill_path: String,
    pub(crate) timestamp: String,
    pub(crate) tool_name: String,
    pub(crate) cwd: Option<String>,
    pub(crate) hostname: String,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnedHookInvocation {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: String,
    pub(crate) session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) is_sidechain: bool,
    pub(crate) timestamp: String,
    pub(crate) hook_event: String,
    pub(crate) hook_matcher: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) hook_identity: String,
    pub(crate) script_command_raw: Option<String>,
    pub(crate) exit_code: Option<i64>,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) cwd: Option<String>,
    pub(crate) hostname: String,
    pub(crate) message_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptAnalyticsSnapshot {
    pub(crate) source: TranscriptAnalyticsSourceState,
    pub(crate) session_events: Vec<OwnedSessionEvent>,
    pub(crate) response_times: Vec<OwnedResponseTime>,
    pub(crate) tool_actions: Vec<OwnedToolAction>,
    pub(crate) skill_usages: Vec<OwnedSkillUsage>,
    pub(crate) hook_invocations: Vec<OwnedHookInvocation>,
}

/// Bounded per-source diagnostics for record anomalies that are skipped
/// instead of rejecting an otherwise usable source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptRecordDiagnostics {
    /// Records whose native identity disagreed with the accepted identity.
    pub(crate) conflicting_identity_records: usize,
    /// Sources whose retained layout hint disagreed with the native identity.
    pub(crate) layout_hint_conflicts: usize,
    /// Source ordinal of the first skipped record, kept for triage.
    pub(crate) first_conflict_ordinal: Option<u64>,
}

impl TranscriptRecordDiagnostics {
    fn skipped_records(&self) -> usize {
        self.conflicting_identity_records
            .saturating_add(self.layout_hint_conflicts)
    }

    fn is_empty(&self) -> bool {
        self.skipped_records() == 0
    }
}

fn log_record_diagnostics(
    source: &DiscoveredRetainedJsonlSource,
    diagnostics: &TranscriptRecordDiagnostics,
) {
    if diagnostics.is_empty() {
        return;
    }
    log::warn!(
        "Retained transcript analytics source skipped anomalous records: provider={} source={} conflicting_identity_records={} layout_hint_conflicts={} first_conflict_ordinal={:?}",
        source.provider.as_str(),
        source.source_key,
        diagnostics.conflicting_identity_records,
        diagnostics.layout_hint_conflicts,
        diagnostics.first_conflict_ordinal,
    );
}

pub(crate) struct ParsedTranscriptAnalyticsSource {
    pub(crate) native_identity: NativeChainIdentity,
    pub(crate) diagnostics: TranscriptRecordDiagnostics,
    snapshot: TranscriptAnalyticsSnapshot,
}

pub(crate) struct CompletedTranscriptSourceRoot {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_root_key: String,
    pub(crate) generation: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptSourceResult {
    Replaced,
    SuppressedUnchanged,
    StaleGeneration,
}

fn stored_native_identity(source: &StoredTranscriptAnalyticsSource) -> Option<NativeChainIdentity> {
    Some(NativeChainIdentity {
        provider: source.provider,
        source_session_id: source.source_session_id.clone()?,
        chain_id: source.chain_id.clone()?,
        parent_chain_id: source.parent_chain_id.clone(),
        is_sidechain: source.is_sidechain,
        agent_id: source.agent_id.clone(),
        cwd: source.cwd.clone(),
    })
}

/// One inventory refresh owed by a source that did not change, held until the
/// caller decides when to persist it.
///
/// Classification never writes it itself: the startup walk accumulates a whole
/// root's worth and commits them in a single transaction, while the live path
/// applies its single refresh immediately. Fields are owned because the batch
/// outlives the borrowed classification inputs.
struct PendingUnchangedRefresh {
    provider: IntegrationProvider,
    source_key: String,
    source_root_key: &'static str,
    source_path: PathBuf,
    generation: i64,
    mtime_ns: i64,
    size_bytes: i64,
    content_sha256: Option<String>,
    /// Reported when the row does not update because the generation advanced.
    stale_generation_error: &'static str,
}

impl PendingUnchangedRefresh {
    fn descriptor(&self) -> UnchangedTranscriptAnalyticsSource<'_> {
        UnchangedTranscriptAnalyticsSource {
            provider: self.provider,
            source_key: &self.source_key,
            source_root_key: self.source_root_key,
            source_path: &self.source_path,
            generation: self.generation,
            mtime_ns: self.mtime_ns,
            size_bytes: self.size_bytes,
            content_sha256: self.content_sha256.as_deref(),
        }
    }

    fn stale_generation_failure(&self) -> String {
        TranscriptAnalyticsError::Storage(self.stale_generation_error.into()).to_string()
    }
}

struct UnchangedTranscriptSource {
    discovered: DiscoveredRetainedJsonlSource,
    native_identity: Option<NativeChainIdentity>,
    previous_root: Option<String>,
    suppressed: bool,
    refresh: PendingUnchangedRefresh,
}

enum ClassifiedTranscriptSource {
    Parsed(Box<ParsedTranscriptAnalyticsSource>),
    Unchanged,
}

/// Raw bytes of a source that must be re-parsed, read exactly once.
struct ChangedTranscriptSourceBytes {
    bytes: Vec<u8>,
    stable_stat: StableSourceStat,
    content_sha256: String,
}

enum TranscriptSourceFreshness {
    Changed(Box<ChangedTranscriptSourceBytes>),
    Unchanged(Box<UnchangedTranscriptSource>),
}

/// Decide whether a discovered source still matches its last-good registry
/// state without extracting any analytics rows.
///
/// `force_full_reparse` reflects the durable reingest marker: while it is set
/// the mtime and content-digest short-circuits are bypassed so an interrupted
/// rebuild replays every retained source instead of trusting stale fingerprints.
///
/// An unchanged verdict carries its owed inventory refresh instead of writing
/// it, so callers choose the transaction granularity.
fn classify_transcript_source_freshness(
    source: &DiscoveredRetainedJsonlSource,
    existing: Option<&StoredTranscriptAnalyticsSource>,
    generation: i64,
    force_full_reparse: bool,
) -> Result<TranscriptSourceFreshness, TranscriptAnalyticsError> {
    let Some(existing) = existing.filter(|stored| {
        stored.provider == source.provider
            && stored.source_root_key == source.source_root_key
            && stored.source_path == source.canonical_path
    }) else {
        return read_changed_transcript_source(source).map(TranscriptSourceFreshness::Changed);
    };

    let metadata =
        std::fs::metadata(&source.canonical_path).map_err(TranscriptAnalyticsError::Read)?;
    let stat = stable_source_stat(&metadata)?;
    let suppressed =
        existing.processing_status == "suppressed" || existing.suppressed_sha256.is_some();
    let has_last_good_identity = stored_native_identity(existing).is_some();
    let fast_unchanged = !force_full_reparse
        && existing.mtime_ns == Some(stat.mtime_ns)
        && existing.size_bytes == Some(stat.size_bytes)
        && existing.content_sha256.is_some();

    if suppressed
        || (existing.processing_status == "ok" && has_last_good_identity && fast_unchanged)
    {
        return Ok(TranscriptSourceFreshness::Unchanged(Box::new(
            UnchangedTranscriptSource {
                discovered: source.clone(),
                native_identity: stored_native_identity(existing),
                previous_root: existing.analytics_session_id.clone(),
                suppressed,
                refresh: PendingUnchangedRefresh {
                    provider: source.provider,
                    source_key: source.source_key.clone(),
                    source_root_key: source.source_root_key,
                    source_path: source.canonical_path.clone(),
                    generation,
                    mtime_ns: stat.mtime_ns,
                    size_bytes: stat.size_bytes,
                    content_sha256: None,
                    stale_generation_error: "unchanged transcript generation advanced during refresh",
                },
            },
        )));
    }

    let (bytes, stable_stat) = read_stable_transcript(&source.canonical_path)?;
    let content_sha256 = model_source_content_sha256(&bytes);
    if !force_full_reparse
        && existing.processing_status == "ok"
        && has_last_good_identity
        && existing.content_sha256.as_deref() == Some(content_sha256.as_str())
    {
        return Ok(TranscriptSourceFreshness::Unchanged(Box::new(
            UnchangedTranscriptSource {
                discovered: source.clone(),
                native_identity: stored_native_identity(existing),
                previous_root: existing.analytics_session_id.clone(),
                suppressed: false,
                refresh: PendingUnchangedRefresh {
                    provider: source.provider,
                    source_key: source.source_key.clone(),
                    source_root_key: source.source_root_key,
                    source_path: source.canonical_path.clone(),
                    generation,
                    mtime_ns: stable_stat.mtime_ns,
                    size_bytes: stable_stat.size_bytes,
                    content_sha256: Some(content_sha256),
                    stale_generation_error: "content-unchanged transcript generation advanced during refresh",
                },
            },
        )));
    }

    Ok(TranscriptSourceFreshness::Changed(Box::new(
        ChangedTranscriptSourceBytes {
            bytes,
            stable_stat,
            content_sha256,
        },
    )))
}

fn read_changed_transcript_source(
    source: &DiscoveredRetainedJsonlSource,
) -> Result<Box<ChangedTranscriptSourceBytes>, TranscriptAnalyticsError> {
    let (bytes, stable_stat) = read_stable_transcript(&source.canonical_path)?;
    let content_sha256 = model_source_content_sha256(&bytes);
    Ok(Box::new(ChangedTranscriptSourceBytes {
        bytes,
        stable_stat,
        content_sha256,
    }))
}

/// Classify one source and fully parse it when its content changed.
///
/// The live notify path uses this so a changed transcript is read exactly once.
fn classify_transcript_analytics_source(
    storage: &Storage,
    source: &DiscoveredRetainedJsonlSource,
    existing: Option<&StoredTranscriptAnalyticsSource>,
    hostname: &str,
    generation: i64,
    force_full_reparse: bool,
) -> Result<ClassifiedTranscriptSource, TranscriptAnalyticsError> {
    match classify_transcript_source_freshness(source, existing, generation, force_full_reparse)? {
        TranscriptSourceFreshness::Unchanged(unchanged) => {
            // Exactly one source, so the batched refresh is already one
            // transaction; no accumulation buys anything here.
            let refreshed = storage
                .refresh_unchanged_transcript_analytics_source(unchanged.refresh.descriptor())
                .map_err(TranscriptAnalyticsError::Storage)?;
            if !refreshed {
                return Err(TranscriptAnalyticsError::Storage(
                    unchanged.refresh.stale_generation_error.into(),
                ));
            }
            Ok(ClassifiedTranscriptSource::Unchanged)
        }
        TranscriptSourceFreshness::Changed(changed) => parse_transcript_analytics_source_bytes(
            source,
            hostname,
            changed.bytes,
            changed.stable_stat,
            changed.content_sha256,
        )
        .map(Box::new)
        .map(ClassifiedTranscriptSource::Parsed),
    }
}

/// Identity-only view of one source used by the startup inventory phase.
///
/// Holding this instead of a parsed snapshot bounds startup memory to a single
/// source: root stamping needs cross-source identity, never the extracted rows.
struct TranscriptSourceIdentity {
    discovered: DiscoveredRetainedJsonlSource,
    native_identity: Option<NativeChainIdentity>,
    previous_root: Option<String>,
    suppressed: bool,
    /// The source content changed, so it must be re-parsed and committed.
    changed: bool,
    /// Inventory refresh owed by an unchanged source, flushed once per root.
    pending_refresh: Option<PendingUnchangedRefresh>,
}

fn classify_transcript_source_identity(
    source: &DiscoveredRetainedJsonlSource,
    existing: Option<&StoredTranscriptAnalyticsSource>,
    generation: i64,
    force_full_reparse: bool,
) -> Result<TranscriptSourceIdentity, TranscriptAnalyticsError> {
    match classify_transcript_source_freshness(source, existing, generation, force_full_reparse)? {
        TranscriptSourceFreshness::Unchanged(unchanged) => Ok(TranscriptSourceIdentity {
            discovered: unchanged.discovered,
            native_identity: unchanged.native_identity,
            previous_root: unchanged.previous_root,
            suppressed: unchanged.suppressed,
            changed: false,
            pending_refresh: Some(unchanged.refresh),
        }),
        TranscriptSourceFreshness::Changed(changed) => {
            let contents = std::str::from_utf8(&changed.bytes)
                .map_err(|_| TranscriptAnalyticsError::InvalidUtf8)?;
            let records = parse_jsonl_records(contents);
            let (native_identity, diagnostics) = resolve_native_identity(source, &records)?;
            drop(records);
            drop(changed);
            log_record_diagnostics(source, &diagnostics);
            Ok(TranscriptSourceIdentity {
                discovered: source.clone(),
                native_identity: Some(native_identity),
                previous_root: existing.and_then(|stored| stored.analytics_session_id.clone()),
                suppressed: false,
                changed: true,
                pending_refresh: None,
            })
        }
    }
}

/// A failure that could not be attributed to a single source.
enum RootReconciliationFault {
    /// This provider root cannot be reconciled now; other roots are unaffected
    /// and this root simply produces no prune proof.
    RootUnavailable(String),
    /// The database refused even a bounded diagnostic write. Nothing else in
    /// this run can retain last-known-good state, so the run is abandoned
    /// before any prune proof is consulted.
    Database(String),
}

/// Persist a bounded per-source diagnostic.
///
/// A storage failure here is the deliberate signal that the database itself is
/// unusable: the write is a single bounded upsert, and without it the failed
/// source keeps a stale `seen_generation` and would be pruned as if deleted.
fn record_source_failure(
    storage: &Storage,
    source: &DiscoveredRetainedJsonlSource,
    generation: i64,
    error: &str,
) -> Result<(), RootReconciliationFault> {
    log::warn!(
        "Retained transcript analytics source failed: provider={} source={} error={error}",
        source.provider.as_str(),
        source.source_key,
    );
    storage
        .record_transcript_analytics_source_failure(
            source.provider,
            &source.source_key,
            source.source_root_key,
            &source.canonical_path,
            generation,
            error,
        )
        .map_err(|storage_error| {
            RootReconciliationFault::Database(format!(
                "cannot persist transcript analytics failure for source {}: {storage_error}",
                source.source_key
            ))
        })
}

/// Best-effort diagnostic used by the live path, which reports its own error.
fn record_live_source_failure(
    storage: &Storage,
    source: &DiscoveredRetainedJsonlSource,
    generation: i64,
    error: &str,
) {
    if let Err(
        RootReconciliationFault::Database(message)
        | RootReconciliationFault::RootUnavailable(message),
    ) = record_source_failure(storage, source, generation, error)
    {
        log::warn!("Could not persist transcript analytics failure: {message}");
    }
}

fn resolved_snapshot(
    parsed: ParsedTranscriptAnalyticsSource,
    graph: &SourceRootGraph,
    generation: i64,
) -> Result<TranscriptAnalyticsSnapshot, String> {
    let root = graph
        .resolve(
            parsed.native_identity.provider,
            &parsed.native_identity.chain_id,
        )
        .map_err(|error| error.to_string())?;
    stamp_analytics_root(parsed, &root, generation).map_err(|error| error.to_string())
}

fn commit_transcript_snapshot(
    storage: &Storage,
    snapshot: &TranscriptAnalyticsSnapshot,
) -> Result<TranscriptSourceResult, String> {
    Ok(
        match storage.replace_transcript_analytics_snapshot(snapshot)? {
            TranscriptAnalyticsReplacement::Replaced => TranscriptSourceResult::Replaced,
            TranscriptAnalyticsReplacement::SuppressedUnchanged => {
                TranscriptSourceResult::SuppressedUnchanged
            }
            TranscriptAnalyticsReplacement::StaleGeneration => {
                TranscriptSourceResult::StaleGeneration
            }
        },
    )
}

const TRANSCRIPT_ANALYTICS_REINGEST_MARKER: &str = "transcript_analytics_reingest_pending";

/// Read the durable migration-30 reingest marker once per reconciliation run.
fn transcript_analytics_reingest_pending(storage: &Storage) -> bool {
    match storage.get_setting(TRANSCRIPT_ANALYTICS_REINGEST_MARKER) {
        Ok(value) => value.is_some(),
        Err(error) => {
            log::warn!("Could not read transcript analytics reingest marker: {error}");
            false
        }
    }
}

/// Per-root reconciliation counters returned to the startup driver.
#[derive(Default)]
struct RootReconciliationOutcome {
    replaced_sources: usize,
    failed_sources: usize,
    skipped_records: usize,
    proof: Option<CompletedTranscriptSourceRoot>,
}

struct CommittedTranscriptSource {
    result: TranscriptSourceResult,
    skipped_records: usize,
}

/// Compare the identity fields that decide cross-source root membership.
///
/// `cwd` is deliberately excluded: it is descriptive origin, and a last-good
/// registry row can legitimately carry a different one than a fresh parse.
fn native_identity_matches(left: &NativeChainIdentity, right: &NativeChainIdentity) -> bool {
    left.provider == right.provider
        && left.source_session_id == right.source_session_id
        && left.chain_id == right.chain_id
        && left.parent_chain_id == right.parent_chain_id
        && left.is_sidechain == right.is_sidechain
        && left.agent_id == right.agent_id
}

/// Parse, stamp, and commit exactly one source, then drop its snapshot.
fn commit_reconciled_source(
    storage: &Storage,
    discovered: &DiscoveredRetainedJsonlSource,
    inventoried: &NativeChainIdentity,
    resolved_root: &str,
    hostname: &str,
    generation: i64,
) -> Result<CommittedTranscriptSource, String> {
    let parsed =
        parse_transcript_analytics_source(discovered, hostname).map_err(|e| e.to_string())?;
    // The file can change between inventory and commit. Stamping a root that
    // was resolved from a different identity would silently reparent rows, so
    // drift is a source failure that retains last-known-good data instead.
    if !native_identity_matches(&parsed.native_identity, inventoried) {
        return Err(TranscriptAnalyticsError::SourceIdentityDrift.to_string());
    }
    let skipped_records = parsed.diagnostics.skipped_records();
    log_record_diagnostics(discovered, &parsed.diagnostics);
    let snapshot =
        stamp_analytics_root(parsed, resolved_root, generation).map_err(|e| e.to_string())?;
    let result = commit_transcript_snapshot(storage, &snapshot)?;
    drop(snapshot);
    Ok(CommittedTranscriptSource {
        result,
        skipped_records,
    })
}

/// Persist every deferred unchanged-source refresh for one root in a single
/// transaction and report the sources the caller must still fail.
///
/// The batched write reports which rows did not update, so a source whose
/// generation advanced under a concurrent run keeps the same per-source
/// treatment it had when each refresh owned its own transaction. A batch-level
/// storage error is attributed to every source in the batch for the same
/// reason: none of their rows advanced.
fn flush_unchanged_refreshes(
    storage: &Storage,
    identities: &[TranscriptSourceIdentity],
) -> HashMap<String, String> {
    let pending = identities
        .iter()
        .filter_map(|identity| identity.pending_refresh.as_ref())
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return HashMap::new();
    }
    let descriptors = pending
        .iter()
        .map(|refresh| refresh.descriptor())
        .collect::<Vec<_>>();
    match storage.refresh_unchanged_transcript_analytics_sources(&descriptors) {
        Ok(stale_keys) => {
            let stale_keys = stale_keys
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            pending
                .iter()
                .filter(|refresh| stale_keys.contains(refresh.source_key.as_str()))
                .map(|refresh| {
                    (
                        refresh.source_key.clone(),
                        refresh.stale_generation_failure(),
                    )
                })
                .collect()
        }
        Err(error) => pending
            .iter()
            .map(|refresh| {
                (
                    refresh.source_key.clone(),
                    TranscriptAnalyticsError::Storage(error.clone()).to_string(),
                )
            })
            .collect(),
    }
}

/// Reconcile one provider root in two phases.
///
/// Phase one resolves cross-source native identity without extracting any rows,
/// so the whole-root graph is known before the first commit. Phase two parses,
/// stamps, commits, and drops one snapshot at a time. Peak memory is therefore
/// one source rather than the entire retained corpus, at the cost of a second
/// read of each source that actually needs committing.
///
/// A single failed source never cancels the rest of the root: its bounded
/// diagnostic refreshes `seen_generation`, which keeps it out of the prune set,
/// so enumeration completeness alone decides whether the root can be pruned.
// @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots]]
fn reconcile_transcript_source_root(
    storage: &Storage,
    root: &ProviderSourceRoot,
    hostname: &str,
    force_full_reparse: bool,
) -> Result<RootReconciliationOutcome, RootReconciliationFault> {
    let generation = storage
        .begin_transcript_analytics_generation(root.provider, root.source_root_key)
        .map_err(RootReconciliationFault::RootUnavailable)?;
    let persisted = storage
        .list_transcript_analytics_sources_for_root(root.provider, root.source_root_key)
        .map_err(RootReconciliationFault::RootUnavailable)?;
    let persisted_by_key = persisted
        .iter()
        .map(|source| (source.source_key.as_str(), source))
        .collect::<HashMap<_, _>>();
    let discovered_keys = root
        .sources
        .iter()
        .map(|source| source.source_key.as_str())
        .collect::<HashSet<_>>();
    let enumeration_complete = matches!(root.outcome, ProviderRootEnumerationOutcome::Complete);

    let mut outcome = RootReconciliationOutcome::default();
    let mut identities = Vec::with_capacity(root.sources.len());
    let mut handled_keys = HashSet::new();
    for source in &root.sources {
        match classify_transcript_source_identity(
            source,
            persisted_by_key.get(source.source_key.as_str()).copied(),
            generation,
            force_full_reparse,
        ) {
            Ok(identity) => {
                handled_keys.insert(source.source_key.as_str());
                identities.push(identity);
            }
            Err(error) => {
                outcome.failed_sources = outcome.failed_sources.saturating_add(1);
                record_source_failure(storage, source, generation, &error.to_string())?;
            }
        }
    }

    // Every unchanged source owes only a `seen_generation` bump, so the whole
    // root advances in one transaction instead of one per source.
    let refresh_failures = flush_unchanged_refreshes(storage, &identities);
    let (refresh_failed, identities): (Vec<_>, Vec<_>) = identities
        .into_iter()
        .partition(|identity| refresh_failures.contains_key(&identity.discovered.source_key));
    for identity in refresh_failed {
        handled_keys.remove(identity.discovered.source_key.as_str());
        outcome.failed_sources = outcome.failed_sources.saturating_add(1);
        let error = refresh_failures
            .get(&identity.discovered.source_key)
            .map(String::as_str)
            .unwrap_or("unchanged transcript refresh failed");
        record_source_failure(storage, &identity.discovered, generation, error)?;
    }

    let mut graph_metadata = identities
        .iter()
        .filter_map(|identity| identity.native_identity.clone())
        .collect::<Vec<_>>();
    for source in &persisted {
        let present_but_failed = discovered_keys.contains(source.source_key.as_str())
            && !handled_keys.contains(source.source_key.as_str());
        let retained_from_incomplete_root =
            !enumeration_complete && !discovered_keys.contains(source.source_key.as_str());
        if (present_but_failed || retained_from_incomplete_root)
            && let Some(native) = stored_native_identity(source)
        {
            graph_metadata.push(native);
        }
    }
    let graph = SourceRootGraph::from_metadata(graph_metadata);

    for identity in identities {
        if identity.suppressed {
            continue;
        }
        let Some(native) = identity.native_identity else {
            outcome.failed_sources = outcome.failed_sources.saturating_add(1);
            record_source_failure(
                storage,
                &identity.discovered,
                generation,
                "unchanged transcript has no last-good native identity",
            )?;
            continue;
        };
        let resolved_root = match graph.resolve(native.provider, &native.chain_id) {
            Ok(resolved_root) => resolved_root,
            Err(error) => {
                outcome.failed_sources = outcome.failed_sources.saturating_add(1);
                record_source_failure(
                    storage,
                    &identity.discovered,
                    generation,
                    &error.to_string(),
                )?;
                continue;
            }
        };
        if !identity.changed && identity.previous_root.as_deref() == Some(resolved_root.as_str()) {
            continue;
        }
        match commit_reconciled_source(
            storage,
            &identity.discovered,
            &native,
            &resolved_root,
            hostname,
            generation,
        ) {
            Ok(committed) => {
                if matches!(committed.result, TranscriptSourceResult::Replaced) {
                    outcome.replaced_sources = outcome.replaced_sources.saturating_add(1);
                }
                outcome.skipped_records = outcome
                    .skipped_records
                    .saturating_add(committed.skipped_records);
            }
            Err(error) => {
                outcome.failed_sources = outcome.failed_sources.saturating_add(1);
                record_source_failure(storage, &identity.discovered, generation, &error)?;
            }
        }
    }

    if enumeration_complete {
        outcome.proof = Some(CompletedTranscriptSourceRoot {
            provider: root.provider,
            source_root_key: root.source_root_key.to_owned(),
            generation,
        });
    }
    Ok(outcome)
}

/// Reconcile one validated retained source for live notifications.
// @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots]]
pub(crate) fn reconcile_live_transcript_source(
    storage: &Storage,
    source: &DiscoveredRetainedJsonlSource,
    hostname: &str,
) -> Result<TranscriptSourceResult, String> {
    let _permit =
        acquire_transcript_reconciliation([(source.provider, source.source_root_key.to_owned())])?;
    let generation =
        storage.begin_transcript_analytics_generation(source.provider, source.source_root_key)?;
    let persisted = storage
        .list_transcript_analytics_sources_for_root(source.provider, source.source_root_key)?;
    let existing = persisted
        .iter()
        .find(|stored| stored.source_key == source.source_key);
    let initial = match classify_transcript_analytics_source(
        storage,
        source,
        existing,
        hostname,
        generation,
        transcript_analytics_reingest_pending(storage),
    ) {
        Ok(ClassifiedTranscriptSource::Parsed(parsed)) => *parsed,
        Ok(ClassifiedTranscriptSource::Unchanged) => {
            return Ok(TranscriptSourceResult::SuppressedUnchanged);
        }
        Err(error) => {
            record_live_source_failure(storage, source, generation, &error.to_string());
            return Err(error.to_string());
        }
    };
    let mut staged = vec![(source.clone(), initial)];
    let mut staged_keys = HashSet::from([source.source_key.clone()]);

    // A changed ancestor can move persisted descendants to a new root. Grow
    // the staged set only with those descendants, then resolve once the graph
    // converges. Unrelated sessions under the same provider root stay untouched.
    loop {
        let mut metadata = staged
            .iter()
            .map(|(_, parsed)| parsed.native_identity.clone())
            .collect::<Vec<_>>();
        metadata.extend(persisted.iter().filter_map(|stored| {
            (!staged_keys.contains(&stored.source_key))
                .then(|| stored_native_identity(stored))
                .flatten()
        }));
        let graph = SourceRootGraph::from_metadata(metadata);
        for (_, parsed) in &staged {
            graph
                .resolve(
                    parsed.native_identity.provider,
                    &parsed.native_identity.chain_id,
                )
                .map_err(|error| error.to_string())?;
        }

        let affected = persisted.iter().find(|stored| {
            if staged_keys.contains(&stored.source_key) || stored.suppressed_sha256.is_some() {
                return false;
            }
            let (Some(chain_id), Some(previous_root)) =
                (&stored.chain_id, &stored.analytics_session_id)
            else {
                return false;
            };
            graph
                .resolve(stored.provider, chain_id)
                .is_ok_and(|resolved| resolved != *previous_root)
        });
        let Some(affected) = affected else {
            let mut replaced = false;
            let mut stale = false;
            // Each snapshot is stamped, committed, and dropped in turn so the
            // staged set never holds more than one materialized row set.
            for (discovered, parsed) in staged {
                log_record_diagnostics(&discovered, &parsed.diagnostics);
                let snapshot = resolved_snapshot(parsed, &graph, generation)?;
                let committed = commit_transcript_snapshot(storage, &snapshot)?;
                drop(snapshot);
                replaced |= matches!(committed, TranscriptSourceResult::Replaced);
                stale |= matches!(committed, TranscriptSourceResult::StaleGeneration);
            }
            return Ok(if replaced {
                TranscriptSourceResult::Replaced
            } else if stale {
                TranscriptSourceResult::StaleGeneration
            } else {
                TranscriptSourceResult::SuppressedUnchanged
            });
        };

        let discovered = match crate::sessions::validate_retained_notify_source(
            affected.provider,
            &affected.source_path,
        ) {
            Ok(Some(discovered)) if discovered.source_key == affected.source_key => discovered,
            Ok(Some(_)) => return Err("Affected transcript descendant changed identity".into()),
            Ok(None) => return Err("Affected transcript descendant is not retained".into()),
            Err(error) => {
                return Err(format!(
                    "Affected transcript descendant cannot be validated: {error:?}"
                ));
            }
        };
        let parsed = match parse_transcript_analytics_source(&discovered, hostname) {
            Ok(parsed) => parsed,
            Err(error) => {
                record_live_source_failure(storage, &discovered, generation, &error.to_string());
                return Err(error.to_string());
            }
        };
        staged_keys.insert(discovered.source_key.clone());
        staged.push((discovered, parsed));
        if staged.len() > persisted.len().saturating_add(1) {
            return Err("Transcript analytics root graph did not converge".into());
        }
    }
}

pub(crate) fn prune_completed_transcript_root(
    storage: &Storage,
    proof: &CompletedTranscriptSourceRoot,
) -> Result<usize, String> {
    storage.prune_transcript_analytics_sources_for_root(proof)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptAnalyticsReconciliationSummary {
    pub(crate) replaced_sources: usize,
    pub(crate) pruned_sources: usize,
    pub(crate) failed_sources: usize,
    pub(crate) skipped_records: usize,
    pub(crate) completed_all_roots: bool,
    pub(crate) failure: Option<String>,
}

/// Keep the first cause so a later symptom cannot mask the original failure.
fn record_summary_failure(summary: &mut TranscriptAnalyticsReconciliationSummary, failure: String) {
    log::warn!("{failure}");
    if summary.failure.is_none() {
        summary.failure = Some(failure);
    }
}

/// Run the durable whole-root reconciliation independently of Session Search.
/// The root permit remains owned from inventory through final pruning.
///
/// Roots are reconciled and pruned one at a time. A source-level or root-level
/// failure never cancels the remaining roots; only a database that refuses a
/// bounded diagnostic write abandons the run, because nothing after that point
/// could retain last-known-good state.
pub(crate) fn run_startup_transcript_analytics_reconciliation(
    storage: &Storage,
    hostname: &str,
) -> Result<TranscriptAnalyticsReconciliationSummary, String> {
    let _permit = acquire_transcript_reconciliation(
        retained_jsonl_source_root_identities()
            .into_iter()
            .map(|(provider, source_root_key)| (provider, source_root_key.to_owned())),
    )?;
    let force_full_reparse = transcript_analytics_reingest_pending(storage);
    let roots = enumerate_retained_jsonl_source_roots();
    let mut summary = TranscriptAnalyticsReconciliationSummary::default();
    let mut completed_roots = 0usize;
    for root in &roots {
        let outcome = match reconcile_transcript_source_root(
            storage,
            root,
            hostname,
            force_full_reparse,
        ) {
            Ok(outcome) => outcome,
            Err(RootReconciliationFault::RootUnavailable(error)) => {
                record_summary_failure(
                    &mut summary,
                    format!(
                        "retained transcript analytics root unavailable: provider={} error={error}",
                        root.provider.as_str()
                    ),
                );
                continue;
            }
            Err(RootReconciliationFault::Database(error)) => {
                return Err(format!(
                    "retained transcript analytics database unusable: {error}"
                ));
            }
        };
        summary.replaced_sources = summary
            .replaced_sources
            .saturating_add(outcome.replaced_sources);
        summary.failed_sources = summary
            .failed_sources
            .saturating_add(outcome.failed_sources);
        summary.skipped_records = summary
            .skipped_records
            .saturating_add(outcome.skipped_records);
        if outcome.failed_sources > 0 {
            record_summary_failure(
                &mut summary,
                format!(
                    "retained transcript analytics sources failed: provider={} count={}",
                    root.provider.as_str(),
                    outcome.failed_sources
                ),
            );
        }
        let Some(proof) = outcome.proof else {
            continue;
        };
        match prune_completed_transcript_root(storage, &proof) {
            Ok(pruned) => {
                summary.pruned_sources = summary.pruned_sources.saturating_add(pruned);
                completed_roots = completed_roots.saturating_add(1);
            }
            Err(error) => record_summary_failure(
                &mut summary,
                format!("retained transcript analytics prune failed: {error}"),
            ),
        }
    }
    summary.completed_all_roots = completed_roots == roots.len();
    if summary.completed_all_roots
        && force_full_reparse
        && let Err(error) = storage.delete_setting(TRANSCRIPT_ANALYTICS_REINGEST_MARKER)
    {
        summary.completed_all_roots = false;
        record_summary_failure(
            &mut summary,
            format!("retained transcript analytics marker clear failed: {error}"),
        );
    }
    log::info!(
        "Retained transcript analytics reconciliation: replaced={} pruned={} failed_sources={} skipped_records={} roots_complete={}",
        summary.replaced_sources,
        summary.pruned_sources,
        summary.failed_sources,
        summary.skipped_records,
        summary.completed_all_roots,
    );
    Ok(summary)
}

#[derive(Debug)]
pub(crate) enum TranscriptAnalyticsError {
    Read(std::io::Error),
    Storage(String),
    InvalidUtf8,
    InvalidSourceMetadata,
    SourceTooLarge,
    UnstableSource,
    Identity(IdentityError),
    UnsupportedProvider,
    SourceIdentityDrift,
    EmptyResolvedRoot,
    InconsistentSnapshot,
}

impl fmt::Display for TranscriptAnalyticsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "cannot read retained transcript: {error}"),
            Self::Storage(error) => write!(formatter, "cannot persist transcript state: {error}"),
            Self::InvalidUtf8 => formatter.write_str("retained transcript is not valid UTF-8"),
            Self::InvalidSourceMetadata => {
                formatter.write_str("retained transcript metadata is invalid")
            }
            Self::SourceTooLarge => formatter.write_str("retained transcript exceeds 256 MiB"),
            Self::UnstableSource => {
                formatter.write_str("retained transcript changed during bounded read retries")
            }
            Self::Identity(error) => {
                write!(formatter, "cannot resolve transcript identity: {error}")
            }
            Self::UnsupportedProvider => {
                formatter.write_str("provider does not own retained transcript analytics")
            }
            Self::SourceIdentityDrift => formatter
                .write_str("retained transcript identity changed between inventory and commit"),
            Self::EmptyResolvedRoot => formatter.write_str("resolved analytics root is empty"),
            Self::InconsistentSnapshot => {
                formatter.write_str("snapshot rows do not share native source identity")
            }
        }
    }
}

impl std::error::Error for TranscriptAnalyticsError {}

impl From<IdentityError> for TranscriptAnalyticsError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

struct ClaudeNativeIdentityResolution {
    identity: NativeChainIdentity,
    diagnostics: TranscriptRecordDiagnostics,
}

/// Resolve the Claude source identity from the first conforming record.
///
/// A stray record — a sidechain record restated inside a parent file, or a
/// record copied across a fork with its prior `sessionId` — is skipped and
/// counted instead of rejecting the whole source. Only a source with no valid
/// identity at all fails.
fn resolve_claude_native_identity(
    records: &[JsonlRecord],
) -> Result<ClaudeNativeIdentityResolution, IdentityError> {
    let mut native: Option<NativeChainIdentity> = None;
    let mut diagnostics = TranscriptRecordDiagnostics::default();
    for record in records {
        let Some(object) = record.value.as_object() else {
            continue;
        };
        if !matches!(
            object.get("type").and_then(Value::as_str),
            Some("user" | "assistant" | "attachment")
        ) {
            continue;
        }
        let Some(source_session_id) = nonempty_string(object.get("sessionId")) else {
            continue;
        };
        let native_sidechain = object.get("isSidechain").and_then(Value::as_bool) == Some(true);
        let agent_id = nonempty_string(object.get("agentId"));
        let (chain_id, parent_chain_id, agent_id) = if native_sidechain {
            let Some(agent_id) = agent_id else {
                continue;
            };
            (
                agent_id.clone(),
                Some(source_session_id.clone()),
                Some(agent_id),
            )
        } else {
            (source_session_id.clone(), None, None)
        };
        let candidate = NativeChainIdentity {
            provider: IntegrationProvider::Claude,
            source_session_id,
            chain_id,
            parent_chain_id,
            is_sidechain: native_sidechain,
            agent_id,
            cwd: nonempty_string(object.get("cwd")).map(PathBuf::from),
        };
        let Some(current) = &mut native else {
            native = Some(candidate);
            continue;
        };
        if !native_identity_matches(current, &candidate) {
            diagnostics.conflicting_identity_records =
                diagnostics.conflicting_identity_records.saturating_add(1);
            diagnostics
                .first_conflict_ordinal
                .get_or_insert(record.ordinal);
            continue;
        }
        if current.cwd.is_none() {
            current.cwd = candidate.cwd;
        }
    }
    native
        .map(|identity| ClaudeNativeIdentityResolution {
            identity,
            diagnostics,
        })
        .ok_or(IdentityError::MissingNativeIdentity)
}

/// Resolve provider-native identity and collect skipped-record diagnostics.
fn resolve_native_identity(
    source: &DiscoveredRetainedJsonlSource,
    records: &[JsonlRecord],
) -> Result<(NativeChainIdentity, TranscriptRecordDiagnostics), TranscriptAnalyticsError> {
    let (native_identity, mut diagnostics) = match source.provider {
        IntegrationProvider::Claude => {
            let resolved = resolve_claude_native_identity(records)?;
            (resolved.identity, resolved.diagnostics)
        }
        IntegrationProvider::Codex => (
            resolve_codex_native_identity(records)?,
            TranscriptRecordDiagnostics::default(),
        ),
        IntegrationProvider::MiniMax => {
            return Err(TranscriptAnalyticsError::UnsupportedProvider);
        }
    };
    // A retained-layout disagreement is one anomalous fact about an otherwise
    // usable source, so it is counted rather than discarding every row.
    if source.provider == IntegrationProvider::Claude {
        let layout_is_sidechain = matches!(
            source.layout_hint,
            RetainedJsonlSourceLayoutHint::ClaudeSubagent { .. }
        );
        if layout_is_sidechain != native_identity.is_sidechain {
            diagnostics.layout_hint_conflicts = diagnostics.layout_hint_conflicts.saturating_add(1);
        }
    }
    Ok((native_identity, diagnostics))
}

fn response_seconds(later: &str, earlier: &str) -> Option<f64> {
    let later = DateTime::parse_from_rfc3339(later).ok()?;
    let earlier = DateTime::parse_from_rfc3339(earlier).ok()?;
    Some((later - earlier).num_milliseconds() as f64 / 1000.0)
}

fn source_local_response_times(
    provider: IntegrationProvider,
    source_key: &str,
    native: &NativeChainIdentity,
    messages: &[ExtractedMessage],
) -> Vec<OwnedResponseTime> {
    let mut sorted = messages
        .iter()
        .filter(|message| !message.timestamp.is_empty())
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    let mut rows = Vec::new();
    let mut previous_assistant: Option<String> = None;

    if provider == IntegrationProvider::Codex {
        let mut pending_user: Option<&ExtractedMessage> = None;
        let mut pending_assistant: Option<&ExtractedMessage> = None;
        let mut finish_turn = |user: &ExtractedMessage,
                               assistant: &ExtractedMessage,
                               previous: &mut Option<String>| {
            push_response_time(
                provider,
                source_key,
                native,
                user,
                assistant,
                previous.as_deref(),
                &mut rows,
            );
            *previous = Some(assistant.timestamp.clone());
        };
        for message in sorted {
            match message.role.as_str() {
                "user" => {
                    if let (Some(user), Some(assistant)) =
                        (pending_user.take(), pending_assistant.take())
                    {
                        finish_turn(user, assistant, &mut previous_assistant);
                    }
                    pending_user = Some(message);
                }
                "assistant" if pending_user.is_some() => pending_assistant = Some(message),
                "assistant" => previous_assistant = Some(message.timestamp.clone()),
                _ => {}
            }
        }
        if let (Some(user), Some(assistant)) = (pending_user, pending_assistant) {
            finish_turn(user, assistant, &mut previous_assistant);
        }
    } else {
        let mut pending_user: Option<&ExtractedMessage> = None;
        for message in sorted {
            match message.role.as_str() {
                "user" => pending_user = Some(message),
                "assistant" => {
                    if let Some(user) = pending_user.take() {
                        push_response_time(
                            provider,
                            source_key,
                            native,
                            user,
                            message,
                            previous_assistant.as_deref(),
                            &mut rows,
                        );
                    }
                    previous_assistant = Some(message.timestamp.clone());
                }
                _ => {}
            }
        }
    }
    rows
}

fn push_response_time(
    provider: IntegrationProvider,
    source_key: &str,
    native: &NativeChainIdentity,
    user: &ExtractedMessage,
    assistant: &ExtractedMessage,
    previous_assistant: Option<&str>,
    rows: &mut Vec<OwnedResponseTime>,
) {
    let response_limit = if provider == IntegrationProvider::Codex {
        6.0 * 60.0 * 60.0
    } else {
        600.0
    };
    let response_secs = response_seconds(&assistant.timestamp, &user.timestamp)
        .filter(|seconds| *seconds > 0.0 && *seconds <= response_limit);
    let idle_secs = previous_assistant
        .and_then(|previous| response_seconds(&user.timestamp, previous))
        .filter(|seconds| *seconds > 0.0 && *seconds <= 600.0);
    if response_secs.is_none() && idle_secs.is_none() {
        return;
    }
    rows.push(OwnedResponseTime {
        provider,
        source_key: source_key.to_owned(),
        session_id: native.chain_id.clone(),
        chain_id: native.chain_id.clone(),
        parent_chain_id: native.parent_chain_id.clone(),
        timestamp: assistant.timestamp.clone(),
        response_secs,
        idle_secs,
        is_sidechain: native.is_sidechain,
        agent_id: native.agent_id.clone(),
        parent_uuid: assistant.parent_uuid.clone(),
    });
}

/// Parse all transcript-derived analytics without mutating storage.
// @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots]]
pub(crate) fn parse_transcript_analytics_source(
    source: &DiscoveredRetainedJsonlSource,
    hostname: &str,
) -> Result<ParsedTranscriptAnalyticsSource, TranscriptAnalyticsError> {
    let (bytes, stable_stat) = read_stable_transcript(&source.canonical_path)?;
    let content_sha256 = model_source_content_sha256(&bytes);
    parse_transcript_analytics_source_bytes(source, hostname, bytes, stable_stat, content_sha256)
}

fn parse_transcript_analytics_source_bytes(
    source: &DiscoveredRetainedJsonlSource,
    hostname: &str,
    bytes: Vec<u8>,
    stable_stat: StableSourceStat,
    content_sha256: String,
) -> Result<ParsedTranscriptAnalyticsSource, TranscriptAnalyticsError> {
    let contents =
        std::str::from_utf8(&bytes).map_err(|_| TranscriptAnalyticsError::InvalidUtf8)?;
    let records = parse_jsonl_records(contents);
    drop(bytes);
    let (native_identity, diagnostics) = resolve_native_identity(source, &records)?;

    let extracted =
        extract_messages_from_jsonl_records(source.provider, &source.canonical_path, &records);
    let source_key = source.source_key.clone();
    let mut native_event_ordinals = HashMap::<String, usize>::new();
    let session_events = extracted
        .events
        .iter()
        .filter(|event| DateTime::parse_from_rfc3339(&event.timestamp).is_ok())
        .map(|event| {
            let event_key =
                if let Some(uuid) = event.uuid.as_deref().filter(|uuid| !uuid.is_empty()) {
                    let ordinal = native_event_ordinals.entry(uuid.to_owned()).or_default();
                    let key = format!("native:{uuid}:{ordinal}");
                    *ordinal += 1;
                    key
                } else {
                    format!("{}:{}", event.source_ordinal, event.event_ordinal)
                };
            OwnedSessionEvent {
                provider: source.provider,
                source_key: source_key.clone(),
                event_key,
                session_id: native_identity.chain_id.clone(),
                chain_id: native_identity.chain_id.clone(),
                parent_chain_id: native_identity.parent_chain_id.clone(),
                agent_id: native_identity.agent_id.clone(),
                is_sidechain: native_identity.is_sidechain,
                timestamp: event.timestamp.clone(),
                kind: event.kind,
                uuid: event.uuid.clone(),
                parent_uuid: event.parent_uuid.clone(),
            }
        })
        .collect();

    let response_times = source_local_response_times(
        source.provider,
        &source_key,
        &native_identity,
        &extracted.messages,
    );
    let mut tool_actions = Vec::new();
    let mut skill_usages = Vec::new();
    for message in &extracted.messages {
        for action in &message.tool_actions {
            let action_key = if action.tool_use_id.is_empty() {
                if message.uuid.is_empty() {
                    format!("record:{}:{}", action.source_ordinal, action.block_ordinal)
                } else {
                    format!("{}:{}", message.uuid, action.block_ordinal)
                }
            } else {
                action.tool_use_id.clone()
            };
            tool_actions.push(OwnedToolAction {
                provider: source.provider,
                source_key: source_key.clone(),
                action_key,
                message_id: message.uuid.clone(),
                session_id: native_identity.chain_id.clone(),
                chain_id: native_identity.chain_id.clone(),
                parent_chain_id: native_identity.parent_chain_id.clone(),
                tool_name: action.tool_name.clone(),
                category: action.category.clone(),
                file_path: action.file_path.clone(),
                summary: action.summary.clone(),
                full_input: action.full_input.clone(),
                full_output: action.full_output.clone(),
                timestamp: action.timestamp.clone(),
                is_sidechain: native_identity.is_sidechain,
                agent_id: native_identity.agent_id.clone(),
                parent_uuid: message.parent_uuid.clone(),
            });
            for access in extract_skill_accesses_from_tool_action(action) {
                skill_usages.push(OwnedSkillUsage {
                    provider: source.provider,
                    source_key: source_key.clone(),
                    session_id: native_identity.chain_id.clone(),
                    chain_id: native_identity.chain_id.clone(),
                    parent_chain_id: native_identity.parent_chain_id.clone(),
                    message_id: message.uuid.clone(),
                    skill_name: access.skill_name,
                    skill_path: access.skill_path,
                    timestamp: action.timestamp.clone(),
                    tool_name: action.tool_name.clone(),
                    cwd: message.cwd.clone(),
                    hostname: hostname.to_owned(),
                });
            }
        }
    }
    let hook_invocations = extracted
        .hook_invocations
        .iter()
        .map(|hook| OwnedHookInvocation {
            provider: source.provider,
            source_key: source_key.clone(),
            session_id: native_identity.chain_id.clone(),
            chain_id: native_identity.chain_id.clone(),
            parent_chain_id: native_identity.parent_chain_id.clone(),
            agent_id: native_identity.agent_id.clone(),
            is_sidechain: native_identity.is_sidechain,
            timestamp: hook.timestamp.clone(),
            hook_event: hook.hook_event.clone(),
            hook_matcher: hook.hook_matcher.clone(),
            tool_name: hook.tool_name.clone(),
            hook_identity: hook.hook_identity.clone(),
            script_command_raw: hook.script_command_raw.clone(),
            exit_code: hook.exit_code,
            duration_ms: hook.duration_ms,
            cwd: hook.cwd.clone(),
            hostname: hostname.to_owned(),
            message_id: hook.message_id.clone(),
        })
        .collect();

    let project = extracted.project_name;
    let cwd = native_identity.cwd.clone().or_else(|| {
        extracted
            .messages
            .iter()
            .find_map(|message| message.cwd.as_deref().map(PathBuf::from))
    });
    let snapshot = TranscriptAnalyticsSnapshot {
        source: TranscriptAnalyticsSourceState {
            provider: source.provider,
            source_root_key: source.source_root_key.to_owned(),
            source_key,
            source_path: source.canonical_path.clone(),
            source_session_id: native_identity.source_session_id.clone(),
            analytics_session_id: native_identity.chain_id.clone(),
            chain_id: native_identity.chain_id.clone(),
            parent_chain_id: native_identity.parent_chain_id.clone(),
            is_sidechain: native_identity.is_sidechain,
            agent_id: native_identity.agent_id.clone(),
            project,
            cwd,
            hostname: hostname.to_owned(),
            mtime_ns: stable_stat.mtime_ns,
            size_bytes: stable_stat.size_bytes,
            content_sha256,
            seen_generation: 0,
        },
        session_events,
        response_times,
        tool_actions,
        skill_usages,
        hook_invocations,
    };
    Ok(ParsedTranscriptAnalyticsSource {
        native_identity,
        diagnostics,
        snapshot,
    })
}

/// Validate one native snapshot and stamp the coordinator-resolved root.
pub(crate) fn stamp_analytics_root(
    mut parsed: ParsedTranscriptAnalyticsSource,
    root_session_id: &str,
    generation: i64,
) -> Result<TranscriptAnalyticsSnapshot, TranscriptAnalyticsError> {
    if root_session_id.trim().is_empty() {
        return Err(TranscriptAnalyticsError::EmptyResolvedRoot);
    }
    let native = &parsed.native_identity;
    if parsed.snapshot.source.provider != native.provider
        || parsed.snapshot.source.chain_id != native.chain_id
        || parsed.snapshot.source.parent_chain_id != native.parent_chain_id
    {
        return Err(TranscriptAnalyticsError::InconsistentSnapshot);
    }
    let source_key = parsed.snapshot.source.source_key.as_str();
    let rows_match = parsed.snapshot.session_events.iter().all(|row| {
        row.provider == native.provider
            && row.source_key == source_key
            && row.chain_id == native.chain_id
    }) && parsed.snapshot.response_times.iter().all(|row| {
        row.provider == native.provider
            && row.source_key == source_key
            && row.chain_id == native.chain_id
    }) && parsed.snapshot.tool_actions.iter().all(|row| {
        row.provider == native.provider
            && row.source_key == source_key
            && row.chain_id == native.chain_id
    }) && parsed.snapshot.skill_usages.iter().all(|row| {
        row.provider == native.provider
            && row.source_key == source_key
            && row.chain_id == native.chain_id
    }) && parsed.snapshot.hook_invocations.iter().all(|row| {
        row.provider == native.provider
            && row.source_key == source_key
            && row.chain_id == native.chain_id
    });
    if !rows_match {
        return Err(TranscriptAnalyticsError::InconsistentSnapshot);
    }

    parsed.snapshot.source.analytics_session_id = root_session_id.to_owned();
    parsed.snapshot.source.seen_generation = generation;
    for row in &mut parsed.snapshot.session_events {
        row.session_id = root_session_id.to_owned();
    }
    for row in &mut parsed.snapshot.response_times {
        row.session_id = root_session_id.to_owned();
    }
    for row in &mut parsed.snapshot.tool_actions {
        row.session_id = root_session_id.to_owned();
    }
    for row in &mut parsed.snapshot.skill_usages {
        row.session_id = root_session_id.to_owned();
    }
    for row in &mut parsed.snapshot.hook_invocations {
        row.session_id = root_session_id.to_owned();
    }
    Ok(parsed.snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;
    use std::fs::{File, FileTimes};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    const TEST_HOSTNAME: &str = "host-a";
    const TEST_TIMESTAMP: &str = "2026-01-01T00:00:00.000Z";
    /// Explicit mtime so fingerprint tests never depend on the wall clock or on
    /// filesystem timestamp granularity.
    const FIXED_MTIME_NS: i64 = 1_700_000_000_123_456_789;
    const CLASSIFY_GENERATION: i64 = 9;

    fn source_root_key(provider: IntegrationProvider) -> &'static str {
        retained_jsonl_source_root_identities()
            .into_iter()
            .find(|(candidate, _)| *candidate == provider)
            .map(|(_, key)| key)
            .expect("provider owns a retained source root")
    }

    fn claude_parent_hint() -> RetainedJsonlSourceLayoutHint {
        RetainedJsonlSourceLayoutHint::ClaudeParent {
            default_project: "proj".to_owned(),
        }
    }

    fn claude_subagent_hint() -> RetainedJsonlSourceLayoutHint {
        RetainedJsonlSourceLayoutHint::ClaudeSubagent {
            default_project: "proj".to_owned(),
        }
    }

    /// Describe a discovered source without touching the filesystem.
    fn discovered_source(
        provider: IntegrationProvider,
        path: PathBuf,
        layout_hint: RetainedJsonlSourceLayoutHint,
    ) -> DiscoveredRetainedJsonlSource {
        let source_key = format!(
            "{}:{}",
            provider.as_str(),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        DiscoveredRetainedJsonlSource {
            provider,
            source_root_key: source_root_key(provider),
            source_key,
            filesystem_path: path.clone(),
            canonical_path: path,
            layout_hint,
        }
    }

    fn jsonl_body(lines: &[String]) -> String {
        let mut body = lines.join("\n");
        body.push('\n');
        body
    }

    /// Write one JSONL transcript with a pinned mtime and describe it.
    fn write_jsonl_source(
        dir: &Path,
        name: &str,
        provider: IntegrationProvider,
        layout_hint: RetainedJsonlSourceLayoutHint,
        lines: &[String],
    ) -> DiscoveredRetainedJsonlSource {
        let path = dir.join(name);
        std::fs::write(&path, jsonl_body(lines)).expect("write transcript");
        set_mtime_ns(&path, FIXED_MTIME_NS);
        discovered_source(provider, path, layout_hint)
    }

    fn set_mtime_ns(path: &Path, mtime_ns: i64) {
        let file = File::options()
            .write(true)
            .open(path)
            .expect("open transcript for timestamps");
        let modified =
            UNIX_EPOCH + Duration::from_nanos(u64::try_from(mtime_ns).expect("positive mtime"));
        file.set_times(FileTimes::new().set_modified(modified))
            .expect("pin transcript mtime");
    }

    fn stat_of(path: &Path) -> (i64, i64) {
        let metadata = std::fs::metadata(path).expect("transcript metadata");
        let stat = stable_source_stat(&metadata).expect("stable transcript stat");
        (stat.mtime_ns, stat.size_bytes)
    }

    fn content_digest_of(path: &Path) -> String {
        model_source_content_sha256(&std::fs::read(path).expect("read transcript"))
    }

    fn claude_line(record_type: &str, session_id: &str, uuid: &str, cwd: Option<&str>) -> String {
        let role = if record_type == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let mut record = serde_json::Map::new();
        record.insert("type".to_owned(), json!(record_type));
        record.insert("sessionId".to_owned(), json!(session_id));
        record.insert("uuid".to_owned(), json!(uuid));
        record.insert("timestamp".to_owned(), json!(TEST_TIMESTAMP));
        record.insert("isSidechain".to_owned(), json!(false));
        record.insert(
            "message".to_owned(),
            json!({ "role": role, "content": "hello" }),
        );
        if let Some(cwd) = cwd {
            record.insert("cwd".to_owned(), json!(cwd));
        }
        Value::Object(record).to_string()
    }

    fn claude_sidechain_line(session_id: &str, agent_id: &str, uuid: &str) -> String {
        json!({
            "type": "user",
            "sessionId": session_id,
            "agentId": agent_id,
            "uuid": uuid,
            "timestamp": TEST_TIMESTAMP,
            "isSidechain": true,
            "cwd": "/work/a",
            "message": { "role": "user", "content": "hello" }
        })
        .to_string()
    }

    fn stored_source(
        discovered: &DiscoveredRetainedJsonlSource,
        chain_id: &str,
        mtime_ns: i64,
        size_bytes: i64,
        content_sha256: &str,
    ) -> StoredTranscriptAnalyticsSource {
        StoredTranscriptAnalyticsSource {
            provider: discovered.provider,
            source_key: discovered.source_key.clone(),
            source_root_key: discovered.source_root_key.to_owned(),
            source_path: discovered.canonical_path.clone(),
            source_session_id: Some(chain_id.to_owned()),
            analytics_session_id: Some(chain_id.to_owned()),
            chain_id: Some(chain_id.to_owned()),
            parent_chain_id: None,
            agent_id: None,
            is_sidechain: false,
            project: None,
            cwd: None,
            hostname: Some(TEST_HOSTNAME.to_owned()),
            mtime_ns: Some(mtime_ns),
            size_bytes: Some(size_bytes),
            content_sha256: Some(content_sha256.to_owned()),
            seen_generation: 1,
            processing_status: "ok".to_owned(),
            last_attempt_at_ms: None,
            last_success_at_ms: None,
            last_error: None,
            suppressed_sha256: None,
            suppressed_at_ms: None,
        }
    }

    /// Mutation applied to the persisted row a scenario classifies against.
    type StoredTweak = fn(&mut StoredTranscriptAnalyticsSource);
    /// Case name, optional in-place rewrite, stored-row tweak, expected verdict.
    type FreshnessCase = (
        &'static str,
        Option<&'static str>,
        StoredTweak,
        FreshnessVerdict,
    );
    /// Case name, stored-row tweak, expected verdict.
    type ForcedFreshnessCase = (&'static str, StoredTweak, FreshnessVerdict);

    #[derive(Debug, PartialEq, Eq)]
    enum FreshnessVerdict {
        /// Short-circuited on mtime and size alone; no content digest computed.
        UnchangedFastPath,
        /// Short-circuited on a recomputed sha256 after the fingerprint missed.
        UnchangedDigest,
        /// Suppressed sources never reparse regardless of any fingerprint.
        UnchangedSuppressed,
        Changed,
    }

    fn verdict(freshness: &TranscriptSourceFreshness) -> FreshnessVerdict {
        match freshness {
            TranscriptSourceFreshness::Changed(_) => FreshnessVerdict::Changed,
            TranscriptSourceFreshness::Unchanged(unchanged) if unchanged.suppressed => {
                FreshnessVerdict::UnchangedSuppressed
            }
            TranscriptSourceFreshness::Unchanged(unchanged) => {
                if unchanged.refresh.content_sha256.is_some() {
                    FreshnessVerdict::UnchangedDigest
                } else {
                    FreshnessVerdict::UnchangedFastPath
                }
            }
        }
    }

    /// Classify one transcript after optionally rewriting its bytes in place
    /// with the original mtime pinned back, so mtime and size stay identical
    /// while the content differs.
    fn classify_scenario(
        rewrite_session_id: Option<&str>,
        tweak_stored: StoredTweak,
        force_full_reparse: bool,
    ) -> Result<TranscriptSourceFreshness, TranscriptAnalyticsError> {
        let dir = TempDir::new().expect("tempdir");
        let discovered = write_jsonl_source(
            dir.path(),
            "sess-a.jsonl",
            IntegrationProvider::Claude,
            claude_parent_hint(),
            &[claude_line("user", "sess-a", "uuid-1", Some("/work/a"))],
        );
        let (mtime_ns, size_bytes) = stat_of(&discovered.canonical_path);
        let digest = content_digest_of(&discovered.canonical_path);
        let mut stored = stored_source(&discovered, "sess-a", mtime_ns, size_bytes, &digest);
        tweak_stored(&mut stored);
        if let Some(rewrite_session_id) = rewrite_session_id {
            let rewritten = jsonl_body(&[claude_line(
                "user",
                rewrite_session_id,
                "uuid-1",
                Some("/work/a"),
            )]);
            std::fs::write(&discovered.canonical_path, &rewritten).expect("rewrite transcript");
            let (_, rewritten_size) = stat_of(&discovered.canonical_path);
            assert_eq!(
                rewritten_size, size_bytes,
                "rewrite must preserve the source size"
            );
            set_mtime_ns(&discovered.canonical_path, mtime_ns);
            assert_eq!(
                stat_of(&discovered.canonical_path),
                (mtime_ns, size_bytes),
                "rewrite must preserve the source fingerprint"
            );
        }
        classify_transcript_source_freshness(
            &discovered,
            Some(&stored),
            CLASSIFY_GENERATION,
            force_full_reparse,
        )
    }

    /// A sparse file stands in for a 256 MiB transcript: the cap is enforced
    /// from `metadata().len()`, so an apparent length is all the guard sees and
    /// no test needs to materialize a quarter gigabyte.
    fn write_oversized_sparse_source(dir: &Path, name: &str) -> DiscoveredRetainedJsonlSource {
        let path = dir.join(name);
        let file = File::create(&path).expect("create sparse transcript");
        file.set_len(RETAINED_TRANSCRIPT_MAX_BYTES.saturating_add(1))
            .expect("extend sparse transcript");
        drop(file);
        set_mtime_ns(&path, FIXED_MTIME_NS);
        discovered_source(IntegrationProvider::Claude, path, claude_parent_hint())
    }

    fn init_storage_in(dir: &TempDir) -> Storage {
        // SAFETY: env mutation; tests are serialized via `#[serial]`.
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", dir.path());
        }
        Storage::init().expect("init storage")
    }

    fn clear_env() {
        // SAFETY: env mutation; tests are serialized via `#[serial]`.
        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_DATA_DIR");
        }
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Claude Identity Anomaly Skipping]]
    #[test]
    fn claude_identity_skips_anomalous_records_instead_of_failing_the_source() {
        let lines = [
            // Leading record owns the identity but declares no cwd.
            claude_line("user", "sess-a", "uuid-1", None),
            json!({ "type": "summary", "summary": "ignored" }).to_string(),
            // Stray record copied across a fork: counted, never adopted.
            claude_line("assistant", "sess-b", "uuid-2", Some("/work/b")),
            claude_line("assistant", "sess-a", "uuid-3", Some("/work/a")),
        ];
        let records = parse_jsonl_records(&jsonl_body(&lines));
        let resolved = resolve_claude_native_identity(&records).expect("usable identity");

        assert_eq!(resolved.identity.source_session_id, "sess-a");
        assert_eq!(resolved.identity.chain_id, "sess-a");
        assert!(!resolved.identity.is_sidechain);
        assert_eq!(
            resolved.identity.cwd,
            Some(PathBuf::from("/work/a")),
            "a later conforming record still backfills a missing cwd"
        );
        assert_eq!(resolved.diagnostics.conflicting_identity_records, 1);
        assert_eq!(
            resolved.diagnostics.first_conflict_ordinal,
            Some(2),
            "the ordinal must point at the stray line, not the record index"
        );
        assert_eq!(resolved.diagnostics.layout_hint_conflicts, 0);
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Claude Layout Hint Mismatch]]
    #[test]
    fn claude_layout_hint_mismatch_is_a_diagnostic_not_an_error() {
        let parent_records = parse_jsonl_records(&jsonl_body(&[claude_line(
            "user",
            "sess-a",
            "uuid-1",
            Some("/work/a"),
        )]));
        let mismatched = discovered_source(
            IntegrationProvider::Claude,
            PathBuf::from("/unread/sess-a.jsonl"),
            claude_subagent_hint(),
        );
        let (identity, diagnostics) =
            resolve_native_identity(&mismatched, &parent_records).expect("usable identity");
        assert_eq!(identity.chain_id, "sess-a");
        assert!(!identity.is_sidechain);
        assert_eq!(diagnostics.layout_hint_conflicts, 1);
        assert_eq!(diagnostics.conflicting_identity_records, 0);

        let agreeing = discovered_source(
            IntegrationProvider::Claude,
            PathBuf::from("/unread/sess-a.jsonl"),
            claude_parent_hint(),
        );
        let (_, agreeing_diagnostics) =
            resolve_native_identity(&agreeing, &parent_records).expect("usable identity");
        assert_eq!(agreeing_diagnostics.layout_hint_conflicts, 0);

        let sidechain_records = parse_jsonl_records(&jsonl_body(&[claude_sidechain_line(
            "sess-a", "agent-7", "uuid-1",
        )]));
        let subagent = discovered_source(
            IntegrationProvider::Claude,
            PathBuf::from("/unread/agent-7.jsonl"),
            claude_subagent_hint(),
        );
        let (sidechain_identity, sidechain_diagnostics) =
            resolve_native_identity(&subagent, &sidechain_records).expect("usable identity");
        assert_eq!(sidechain_identity.chain_id, "agent-7");
        assert_eq!(
            sidechain_identity.parent_chain_id.as_deref(),
            Some("sess-a")
        );
        assert!(sidechain_identity.is_sidechain);
        assert_eq!(sidechain_diagnostics.layout_hint_conflicts, 0);
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Claude Source Without Identity]]
    #[test]
    fn claude_source_without_any_valid_identity_still_fails() {
        let lines = [
            json!({ "type": "summary", "summary": "ignored" }).to_string(),
            json!({ "type": "user", "uuid": "uuid-1", "timestamp": TEST_TIMESTAMP }).to_string(),
            json!({
                "type": "assistant",
                "sessionId": "sess-a",
                "isSidechain": true,
                "uuid": "uuid-2",
                "timestamp": TEST_TIMESTAMP
            })
            .to_string(),
        ];
        let records = parse_jsonl_records(&jsonl_body(&lines));
        assert_eq!(
            resolve_claude_native_identity(&records).err(),
            Some(IdentityError::MissingNativeIdentity)
        );

        let source = discovered_source(
            IntegrationProvider::Claude,
            PathBuf::from("/unread/sess-a.jsonl"),
            claude_parent_hint(),
        );
        assert!(matches!(
            resolve_native_identity(&source, &records),
            Err(TranscriptAnalyticsError::Identity(
                IdentityError::MissingNativeIdentity
            ))
        ));
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Freshness Fingerprint Short-Circuits]]
    #[test]
    fn classify_transcript_source_freshness_short_circuits_on_fingerprints() {
        let cases: [FreshnessCase; 8] = [
            (
                "identical mtime and size skip the digest entirely",
                None,
                |_| {},
                FreshnessVerdict::UnchangedFastPath,
            ),
            (
                "an in-place rewrite under an identical fingerprint stays trusted",
                Some("sess-x"),
                |_| {},
                FreshnessVerdict::UnchangedFastPath,
            ),
            (
                "mtime drift falls through to a matching digest",
                None,
                |stored| stored.mtime_ns = stored.mtime_ns.map(|value| value - 1_000_000),
                FreshnessVerdict::UnchangedDigest,
            ),
            (
                "mtime drift with changed content reparses",
                Some("sess-x"),
                |stored| stored.mtime_ns = stored.mtime_ns.map(|value| value - 1_000_000),
                FreshnessVerdict::Changed,
            ),
            (
                "a missing stored digest forces the read path",
                Some("sess-x"),
                |stored| stored.content_sha256 = None,
                FreshnessVerdict::Changed,
            ),
            (
                "a failed status is never fast-pathed",
                None,
                |stored| stored.processing_status = "failed".to_owned(),
                FreshnessVerdict::Changed,
            ),
            (
                "a row without last-good identity is never fast-pathed",
                None,
                |stored| stored.chain_id = None,
                FreshnessVerdict::Changed,
            ),
            (
                "a row recorded for another path is ignored",
                None,
                |stored| stored.source_path = PathBuf::from("/elsewhere/other.jsonl"),
                FreshnessVerdict::Changed,
            ),
        ];

        for (name, rewrite, tweak, expected) in cases {
            let freshness = classify_scenario(rewrite, tweak, false)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(verdict(&freshness), expected, "{name}");
            if let TranscriptSourceFreshness::Unchanged(unchanged) = &freshness {
                assert_eq!(
                    unchanged.refresh.generation, CLASSIFY_GENERATION,
                    "{name}: the owed refresh carries this run's generation"
                );
                assert!(!unchanged.suppressed, "{name}");
            }
        }
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Fast Path Avoids Source Reads]]
    #[test]
    fn unchanged_fast_path_returns_without_reading_source_contents() {
        let dir = TempDir::new().expect("tempdir");
        let discovered = write_oversized_sparse_source(dir.path(), "sess-huge.jsonl");
        let (mtime_ns, size_bytes) = stat_of(&discovered.canonical_path);
        assert!(
            u64::try_from(size_bytes).expect("positive size") > RETAINED_TRANSCRIPT_MAX_BYTES,
            "the fixture must be unreadable under the retained cap"
        );
        let stored = stored_source(
            &discovered,
            "sess-huge",
            mtime_ns,
            size_bytes,
            "sha-of-record",
        );

        // Any read of this source raises SourceTooLarge, so an Unchanged
        // verdict is proof the contents were never opened.
        let freshness = classify_transcript_source_freshness(
            &discovered,
            Some(&stored),
            CLASSIFY_GENERATION,
            false,
        )
        .expect("fingerprint short-circuit");
        assert_eq!(verdict(&freshness), FreshnessVerdict::UnchangedFastPath);
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Forced Reparse Bypasses Short-Circuits]]
    #[test]
    fn force_full_reparse_bypasses_both_short_circuits() {
        let cases: [ForcedFreshnessCase; 4] = [
            (
                "force bypasses the mtime and size fast path",
                |_| {},
                FreshnessVerdict::Changed,
            ),
            (
                "force bypasses a matching content digest",
                |stored| stored.mtime_ns = stored.mtime_ns.map(|value| value - 1_000_000),
                FreshnessVerdict::Changed,
            ),
            (
                "a suppressed status is honoured under force",
                |stored| stored.processing_status = "suppressed".to_owned(),
                FreshnessVerdict::UnchangedSuppressed,
            ),
            (
                "a suppressed digest marker is honoured under force",
                |stored| stored.suppressed_sha256 = Some("sha-of-suppressed".to_owned()),
                FreshnessVerdict::UnchangedSuppressed,
            ),
        ];

        for (name, tweak, expected) in cases {
            let forced = classify_scenario(None, tweak, true)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(verdict(&forced), expected, "{name}");
            let unforced = classify_scenario(None, tweak, false)
                .unwrap_or_else(|error| panic!("{name} (unforced): {error}"));
            if expected == FreshnessVerdict::Changed {
                assert_ne!(
                    verdict(&unforced),
                    FreshnessVerdict::Changed,
                    "{name}: the unforced run must short-circuit, or force proves nothing"
                );
            } else {
                assert_eq!(verdict(&unforced), expected, "{name}");
            }
        }
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Forced Reparse Reads The Source]]
    #[test]
    fn force_full_reparse_reads_a_source_the_fast_path_would_have_skipped() {
        let dir = TempDir::new().expect("tempdir");
        let discovered = write_oversized_sparse_source(dir.path(), "sess-huge.jsonl");
        let (mtime_ns, size_bytes) = stat_of(&discovered.canonical_path);
        let stored = stored_source(
            &discovered,
            "sess-huge",
            mtime_ns,
            size_bytes,
            "sha-of-record",
        );

        // The fingerprint matches, so only an actual read can raise this.
        assert!(matches!(
            classify_transcript_source_freshness(
                &discovered,
                Some(&stored),
                CLASSIFY_GENERATION,
                true,
            ),
            Err(TranscriptAnalyticsError::SourceTooLarge)
        ));
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Retained Transcript Size Cap]]
    #[test]
    fn read_stable_transcript_rejects_sources_over_the_retained_cap() {
        let dir = TempDir::new().expect("tempdir");
        let discovered = write_oversized_sparse_source(dir.path(), "sess-huge.jsonl");
        assert!(matches!(
            read_stable_transcript(&discovered.canonical_path),
            Err(TranscriptAnalyticsError::SourceTooLarge)
        ));
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Identity Comparison Excludes Cwd]]
    #[test]
    fn native_identity_matches_ignores_cwd_but_not_chain_membership() {
        let base = NativeChainIdentity {
            provider: IntegrationProvider::Claude,
            source_session_id: "sess-a".to_owned(),
            chain_id: "sess-a".to_owned(),
            parent_chain_id: None,
            is_sidechain: false,
            agent_id: None,
            cwd: Some(PathBuf::from("/work/a")),
        };
        let relocated = NativeChainIdentity {
            cwd: Some(PathBuf::from("/work/elsewhere")),
            ..base.clone()
        };
        assert!(native_identity_matches(&base, &relocated));
        assert!(native_identity_matches(
            &base,
            &NativeChainIdentity {
                cwd: None,
                ..base.clone()
            }
        ));

        let mutations: [(&str, NativeChainIdentity); 4] = [
            (
                "chain id",
                NativeChainIdentity {
                    chain_id: "sess-b".to_owned(),
                    ..base.clone()
                },
            ),
            (
                "source session id",
                NativeChainIdentity {
                    source_session_id: "sess-b".to_owned(),
                    ..base.clone()
                },
            ),
            (
                "parent chain id",
                NativeChainIdentity {
                    parent_chain_id: Some("sess-root".to_owned()),
                    ..base.clone()
                },
            ),
            (
                "agent id",
                NativeChainIdentity {
                    agent_id: Some("agent-7".to_owned()),
                    ..base.clone()
                },
            ),
        ];
        for (name, mutated) in mutations {
            assert!(
                !native_identity_matches(&base, &mutated),
                "{name} must decide chain membership"
            );
        }
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Commit-Time Identity Drift]]
    #[test]
    #[serial]
    fn commit_reconciled_source_rejects_identity_drift_but_not_a_moved_cwd() {
        clear_env();
        let data_dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&data_dir);
        let transcripts = TempDir::new().expect("tempdir");
        let discovered = write_jsonl_source(
            transcripts.path(),
            "sess-a.jsonl",
            IntegrationProvider::Claude,
            claude_parent_hint(),
            &[
                claude_line("user", "sess-a", "uuid-1", Some("/work/a")),
                claude_line("assistant", "sess-a", "uuid-2", Some("/work/a")),
            ],
        );
        let generation = storage
            .begin_transcript_analytics_generation(
                IntegrationProvider::Claude,
                discovered.source_root_key,
            )
            .expect("begin generation");
        let inventoried = NativeChainIdentity {
            provider: IntegrationProvider::Claude,
            source_session_id: "sess-a".to_owned(),
            chain_id: "sess-a".to_owned(),
            parent_chain_id: None,
            is_sidechain: false,
            agent_id: None,
            cwd: Some(PathBuf::from("/work/a")),
        };

        // Phase one saw a different session than phase two is about to commit.
        let drifted = NativeChainIdentity {
            source_session_id: "sess-moved".to_owned(),
            chain_id: "sess-moved".to_owned(),
            ..inventoried.clone()
        };
        let error = match commit_reconciled_source(
            &storage,
            &discovered,
            &drifted,
            "sess-moved",
            TEST_HOSTNAME,
            generation,
        ) {
            Ok(_) => panic!("drifted identity must not be stamped with the stale root"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            TranscriptAnalyticsError::SourceIdentityDrift.to_string()
        );

        // `cwd` is descriptive origin, so a moved checkout still commits.
        let relocated = NativeChainIdentity {
            cwd: Some(PathBuf::from("/work/elsewhere")),
            ..inventoried
        };
        let committed = commit_reconciled_source(
            &storage,
            &discovered,
            &relocated,
            "sess-a",
            TEST_HOSTNAME,
            generation,
        )
        .expect("a differing cwd alone is not drift");
        assert_eq!(committed.result, TranscriptSourceResult::Replaced);
        assert_eq!(committed.skipped_records, 0);
        clear_env();
    }
}
