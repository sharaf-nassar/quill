//! Complete in-memory analytics snapshots parsed from retained transcripts.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::LazyLock;

use crate::integrations::IntegrationProvider;
use crate::sessions::{
    DiscoveredRetainedJsonlSource, ExtractedMessage, ProviderRootEnumerationOutcome,
    ProviderSourceRoot, RetainedJsonlSourceLayoutHint, SessionEventKind,
    enumerate_retained_jsonl_source_roots, extract_messages_from_jsonl_records,
    extract_skill_accesses_from_tool_action, retained_jsonl_source_root_identities,
};
use crate::storage::{
    Storage, StoredTranscriptAnalyticsSource, TranscriptAnalyticsReplacement,
    UnchangedTranscriptAnalyticsSource,
};
#[cfg(test)]
use crate::transcript_identity::RETAINED_TRANSCRIPT_MAX_BYTES;
use crate::transcript_identity::{
    IdentityError, JsonlRecord, ModelSourceFastFingerprint, NativeChainIdentity, SourceRootGraph,
    StableTranscriptReadError, model_source_content_sha256, model_source_fast_fingerprint,
    parse_jsonl_records, read_stable_transcript, resolve_codex_native_identity,
    resolve_pi_native_identity,
};
use chrono::DateTime;
use serde_json::Value;
use std::sync::{Condvar, Mutex};

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
        let mut active = active.lock().unwrap();
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
    let mut active = active.lock().unwrap();
    while roots.iter().any(|root| active.contains(root)) {
        active = wake.wait(active).unwrap();
    }
    active.extend(roots.iter().cloned());
    Ok(TranscriptReconciliationPermit { roots })
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
    pub(crate) lines_added: Option<i64>,
    pub(crate) lines_removed: Option<i64>,
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

/// The identity every `tool_actions` and `skill_usages` row of one parse
/// shares, supplied by whichever owner is building them.
///
/// Retained sources resolve it from their native chain identity; Pi's notify
/// path has no sub-agent transcripts and so passes a flat one.
pub(crate) struct OwnedToolRowIdentity<'a> {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) chain_id: &'a str,
    pub(crate) parent_chain_id: Option<&'a str>,
    pub(crate) agent_id: Option<&'a str>,
    pub(crate) is_sidechain: bool,
    pub(crate) hostname: &'a str,
}

/// Build one owner's `tool_actions` and `skill_usages` rows from extracted
/// messages.
///
/// Both owned writers reach their rows through here, so the action-key
/// fallback and the skill fan-out stay one implementation and both paths
/// dedupe against the same partial unique indexes.
pub(crate) fn owned_tool_rows(
    identity: &OwnedToolRowIdentity<'_>,
    messages: &[ExtractedMessage],
) -> (Vec<OwnedToolAction>, Vec<OwnedSkillUsage>) {
    let mut tool_actions = Vec::new();
    let mut skill_usages = Vec::new();
    for message in messages {
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
                provider: identity.provider,
                source_key: identity.source_key.to_owned(),
                action_key,
                message_id: message.uuid.clone(),
                session_id: identity.session_id.to_owned(),
                chain_id: identity.chain_id.to_owned(),
                parent_chain_id: identity.parent_chain_id.map(str::to_owned),
                tool_name: action.tool_name.clone(),
                category: action.category.clone(),
                file_path: action.file_path.clone(),
                summary: action.summary.clone(),
                full_input: action.full_input.clone(),
                full_output: action.full_output.clone(),
                lines_added: action.lines_added,
                lines_removed: action.lines_removed,
                timestamp: action.timestamp.clone(),
                is_sidechain: identity.is_sidechain,
                agent_id: identity.agent_id.map(str::to_owned),
                parent_uuid: message.parent_uuid.clone(),
            });
            for access in extract_skill_accesses_from_tool_action(action) {
                skill_usages.push(OwnedSkillUsage {
                    provider: identity.provider,
                    source_key: identity.source_key.to_owned(),
                    session_id: identity.session_id.to_owned(),
                    chain_id: identity.chain_id.to_owned(),
                    parent_chain_id: identity.parent_chain_id.map(str::to_owned),
                    message_id: message.uuid.clone(),
                    skill_name: access.skill_name,
                    skill_path: access.skill_path,
                    timestamp: action.timestamp.clone(),
                    tool_name: action.tool_name.clone(),
                    cwd: message.cwd.clone(),
                    hostname: identity.hostname.to_owned(),
                });
            }
        }
    }
    (tool_actions, skill_usages)
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
    pub(crate) pi_evidence: Option<PiPersistedEvidence>,
}

#[derive(Clone, Debug)]
pub(crate) struct PiPersistedEvidence {
    pub(crate) lifecycle: Option<PiPersistedLifecycle>,
    pub(crate) receipts: Vec<PiPersistedReceipt>,
    pub(crate) usage: Vec<PiPersistedUsage>,
}

#[derive(Clone, Debug)]
pub(crate) struct PiPersistedLifecycle {
    pub(crate) normalized_hostname: String,
    pub(crate) session_id: String,
    pub(crate) source_key: String,
    pub(crate) origin_at_ms: i64,
    pub(crate) process_instance_id: String,
    pub(crate) current_sequence: i64,
    pub(crate) current_occurrence_id: String,
    pub(crate) occurred_at_ms: i64,
    pub(crate) lifecycle_state: &'static str,
    pub(crate) direct_parent_session_id: Option<String>,
    pub(crate) visible_root_session_id: Option<String>,
    pub(crate) lineage_state: &'static str,
    pub(crate) lineage_reason: Option<String>,
    pub(crate) agent_role: Option<String>,
    pub(crate) reporter_protocol: i64,
    pub(crate) reporter_version: String,
    pub(crate) closed_at_ms: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct PiPersistedReceipt {
    pub(crate) normalized_hostname: String,
    pub(crate) session_id: String,
    pub(crate) event_uuid: String,
    pub(crate) source_key: String,
    pub(crate) entry_id: String,
    pub(crate) process_instance_id: String,
    pub(crate) sequence: i64,
    pub(crate) event_kind: &'static str,
    pub(crate) occurred_at_ms: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct PiPersistedUsage {
    pub(crate) source_record_key: String,
    pub(crate) source_ordinal: i64,
    pub(crate) turn_id: String,
    pub(crate) timestamp: String,
    pub(crate) observed_at_ms: i64,
    pub(crate) model_id: String,
    pub(crate) input_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) cache_creation_tokens: i64,
    pub(crate) cache_read_tokens: i64,
    pub(crate) input_cost: Option<f64>,
    pub(crate) output_cost: Option<f64>,
    pub(crate) cache_read_cost: Option<f64>,
    pub(crate) cache_write_cost: Option<f64>,
    pub(crate) total_cost: Option<f64>,
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
        agent_nickname: None,
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
    stable_stat: ModelSourceFastFingerprint,
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
    let stat = model_source_fast_fingerprint(&metadata).map_err(TranscriptAnalyticsError::from)?;
    let suppressed =
        existing.processing_status == "suppressed" || existing.suppressed_sha256.is_some();
    let has_last_good_identity = stored_native_identity(existing).is_some();
    let fast_unchanged = !force_full_reparse
        && existing.mtime_ns == Some(stat.mtime_ns())
        && existing.size_bytes == Some(stat.size_bytes())
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
                    mtime_ns: stat.mtime_ns(),
                    size_bytes: stat.size_bytes(),
                    content_sha256: None,
                    stale_generation_error: "unchanged transcript generation advanced during refresh",
                },
            },
        )));
    }

    let (bytes, stable_stat) =
        read_stable_transcript(&source.canonical_path).map_err(TranscriptAnalyticsError::from)?;
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
                    mtime_ns: stable_stat.mtime_ns(),
                    size_bytes: stable_stat.size_bytes(),
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
    let (bytes, stable_stat) =
        read_stable_transcript(&source.canonical_path).map_err(TranscriptAnalyticsError::from)?;
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
        })?;
    if source.provider == IntegrationProvider::Pi {
        storage
            .record_pi_source_reconciliation_failure(
                &source.source_key,
                chrono::Utc::now().timestamp_millis(),
            )
            .map_err(|storage_error| {
                RootReconciliationFault::Database(format!(
                    "cannot persist Pi reconciliation health for source {}: {storage_error}",
                    source.source_key
                ))
            })?;
    }
    Ok(())
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
            TranscriptAnalyticsReplacement::Replaced(retention) => {
                // A reinsert the retention watermark changed is the one thing
                // about reconciliation a user cannot otherwise observe, so it
                // is logged rather than left to the row counts.
                if !retention.is_unfiltered() {
                    log::info!(
                        "Retention watermark filtered a transcript replacement: provider={} source={} suppressed={} non_conforming={}",
                        snapshot.source.provider.as_str(),
                        snapshot.source.source_key,
                        retention.suppressed(),
                        retention.non_conforming(),
                    );
                }
                TranscriptSourceResult::Replaced
            }
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
/// `cwd` and `agent_nickname` are deliberately excluded: they are descriptive
/// labels, and a last-good row can legitimately differ from a fresh parse.
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
    let roots = enumerate_retained_jsonl_source_roots();
    run_transcript_analytics_reconciliation(storage, hostname, &roots)
}

pub(crate) fn run_transcript_analytics_reconciliation(
    storage: &Storage,
    hostname: &str,
    roots: &[ProviderSourceRoot],
) -> Result<TranscriptAnalyticsReconciliationSummary, String> {
    let _permit = acquire_transcript_reconciliation(
        retained_jsonl_source_root_identities()
            .into_iter()
            .map(|(provider, source_root_key)| (provider, source_root_key.to_owned())),
    )?;
    let force_full_reparse = transcript_analytics_reingest_pending(storage);
    let mut summary = TranscriptAnalyticsReconciliationSummary::default();
    let mut completed_roots = 0usize;
    for root in roots {
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
    if summary.completed_all_roots && force_full_reparse {
        for marker in [
            TRANSCRIPT_ANALYTICS_REINGEST_MARKER,
            "pi_persisted_source_reconciliation_pending",
        ] {
            if let Err(error) = storage.delete_setting(marker) {
                summary.completed_all_roots = false;
                record_summary_failure(
                    &mut summary,
                    format!("retained transcript analytics marker clear failed: {error}"),
                );
                break;
            }
        }
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
    PiSession(String),
    PiSourceIdentity,
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
            Self::PiSession(error) => {
                write!(formatter, "cannot parse persisted Pi session: {error}")
            }
            Self::PiSourceIdentity => formatter
                .write_str("persisted Pi source identity does not match its tracking entries"),
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

impl From<StableTranscriptReadError> for TranscriptAnalyticsError {
    fn from(error: StableTranscriptReadError) -> Self {
        match error {
            StableTranscriptReadError::Read(error) => Self::Read(error),
            StableTranscriptReadError::InvalidMetadata => Self::InvalidSourceMetadata,
            StableTranscriptReadError::SourceTooLarge => Self::SourceTooLarge,
            StableTranscriptReadError::UnstableSource => Self::UnstableSource,
        }
    }
}

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
            agent_nickname: None,
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
        IntegrationProvider::Pi => {
            let session = crate::pi_session::parse_pi_session_records(
                records
                    .iter()
                    .map(|record| (record.ordinal, record.value.clone())),
            )
            .map_err(|error| TranscriptAnalyticsError::PiSession(error.to_string()))?
            .ok_or_else(|| TranscriptAnalyticsError::PiSession("missing session header".into()))?;
            (
                resolve_pi_native_identity(&session)?,
                TranscriptRecordDiagnostics::default(),
            )
        }
        IntegrationProvider::MiniMax => unreachable!("MiniMax has no retained analytics"),
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

fn pi_timestamp_ms(value: &str) -> Result<i64, TranscriptAnalyticsError> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
        .filter(|timestamp| *timestamp >= 0)
        .ok_or(TranscriptAnalyticsError::PiSourceIdentity)
}

fn pi_usage_dimension(
    usage: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<i64, TranscriptAnalyticsError> {
    let value = usage.get(key).and_then(Value::as_i64).unwrap_or(0);
    (0..=100_000_000)
        .contains(&value)
        .then_some(value)
        .ok_or(TranscriptAnalyticsError::PiSourceIdentity)
}

fn pi_usage_cost(
    usage: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, TranscriptAnalyticsError> {
    let Some(cost) = usage
        .get("cost")
        .and_then(Value::as_object)
        .and_then(|cost| cost.get(key))
    else {
        return Ok(None);
    };
    let value = cost
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or(TranscriptAnalyticsError::PiSourceIdentity)?;
    Ok(Some(value))
}

fn pi_event_kind_name(kind: &crate::models::PiProtocolV2EventKind) -> &'static str {
    match kind {
        crate::models::PiProtocolV2EventKind::SessionStart { .. } => "session_start",
        crate::models::PiProtocolV2EventKind::SessionEnd { .. } => "session_end",
        crate::models::PiProtocolV2EventKind::Lineage { .. } => "lineage",
    }
}

fn pi_lineage_fields(
    lineage: &crate::models::PiProtocolV2Lineage,
    session_id: &str,
) -> (&'static str, Option<String>, Option<String>, Option<String>) {
    match lineage {
        crate::models::PiProtocolV2Lineage::Root => {
            ("root", None, Some(session_id.to_owned()), None)
        }
        crate::models::PiProtocolV2Lineage::Linked { parent_session_id } => {
            ("linked", Some(parent_session_id.clone()), None, None)
        }
        crate::models::PiProtocolV2Lineage::Agent { parent_session_id } => {
            ("agent", Some(parent_session_id.clone()), None, None)
        }
        crate::models::PiProtocolV2Lineage::Unresolved { reason } => {
            ("unresolved", None, None, Some(reason.clone()))
        }
    }
}

fn build_pi_persisted_evidence(
    session: &crate::pi_session::PiSession,
    source_key: &str,
    hostname: &str,
) -> Result<PiPersistedEvidence, TranscriptAnalyticsError> {
    let normalized_hostname = crate::live_tracker::normalize_observed_hostname(hostname)
        .ok_or(TranscriptAnalyticsError::PiSourceIdentity)?;
    if crate::storage::pi_source_key(&normalized_hostname, &session.header.id)
        .ok()
        .as_deref()
        != Some(source_key)
    {
        return Err(TranscriptAnalyticsError::PiSourceIdentity);
    }

    let mut receipts = Vec::with_capacity(session.tracking_entries.len());
    let mut tracking = Vec::with_capacity(session.tracking_entries.len());
    for entry in &session.tracking_entries {
        let event = &entry.tracking.data.event;
        if event.session_id != session.header.id
            || event.normalized_host != normalized_hostname
            || event.provider != crate::models::PiProtocolV2Provider::Pi
        {
            return Err(TranscriptAnalyticsError::PiSourceIdentity);
        }
        let occurred_at_ms = pi_timestamp_ms(&event.occurred_at)?;
        pi_timestamp_ms(&event.origin_at)?;
        let sequence = i64::try_from(event.sequence)
            .map_err(|_| TranscriptAnalyticsError::PiSourceIdentity)?;
        receipts.push(PiPersistedReceipt {
            normalized_hostname: normalized_hostname.clone(),
            session_id: session.header.id.clone(),
            event_uuid: event.event_uuid.clone(),
            source_key: source_key.to_owned(),
            entry_id: entry.base.id.clone(),
            process_instance_id: event.process_instance_id.clone(),
            sequence,
            event_kind: pi_event_kind_name(&event.kind),
            occurred_at_ms,
        });
        tracking.push((entry, occurred_at_ms, sequence));
    }

    let lifecycle = tracking
        .iter()
        .filter(|(entry, _, _)| {
            matches!(
                entry.tracking.data.event.kind,
                crate::models::PiProtocolV2EventKind::SessionStart { .. }
            )
        })
        .max_by_key(|(entry, occurred_at_ms, _)| (*occurred_at_ms, entry.source_ordinal))
        .map(|(start, _, start_sequence)| {
            (
                start.tracking.data.event.process_instance_id.as_str(),
                *start_sequence,
            )
        })
        .map(|(current_process, start_sequence)| {
            let current = tracking
                .iter()
                .filter(|(entry, _, sequence)| {
                    entry.tracking.data.event.process_instance_id == current_process
                        && *sequence >= start_sequence
                })
                .max_by_key(|(entry, occurred_at_ms, sequence)| {
                    (*sequence, *occurred_at_ms, entry.source_ordinal)
                })
                .expect("selected process has a start event");
            let lifecycle_event = tracking
                .iter()
                .filter(|(entry, _, sequence)| {
                    entry.tracking.data.event.process_instance_id == current_process
                        && *sequence >= start_sequence
                        && matches!(
                            entry.tracking.data.event.kind,
                            crate::models::PiProtocolV2EventKind::SessionStart { .. }
                                | crate::models::PiProtocolV2EventKind::SessionEnd { .. }
                        )
                })
                .max_by_key(|(entry, occurred_at_ms, sequence)| {
                    (*sequence, *occurred_at_ms, entry.source_ordinal)
                });
            let lineage_event = tracking
                .iter()
                .filter(|(entry, _, sequence)| {
                    entry.tracking.data.event.process_instance_id == current_process
                        && *sequence >= start_sequence
                        && matches!(
                            entry.tracking.data.event.kind,
                            crate::models::PiProtocolV2EventKind::SessionStart { .. }
                                | crate::models::PiProtocolV2EventKind::Lineage { .. }
                        )
                })
                .max_by_key(|(entry, occurred_at_ms, sequence)| {
                    (*sequence, *occurred_at_ms, entry.source_ordinal)
                });

            let (lineage_state, direct_parent_session_id, visible_root_session_id, lineage_reason) =
                lineage_event
                    .map(|(entry, _, _)| match &entry.tracking.data.event.kind {
                        crate::models::PiProtocolV2EventKind::SessionStart { lineage, .. }
                        | crate::models::PiProtocolV2EventKind::Lineage { lineage, .. } => {
                            pi_lineage_fields(lineage, &session.header.id)
                        }
                        crate::models::PiProtocolV2EventKind::SessionEnd { .. } => unreachable!(),
                    })
                    .unwrap_or(("unresolved", None, None, Some("lineage_missing".into())));
            let agent_role = tracking
                .iter()
                .filter(|(entry, _, sequence)| {
                    entry.tracking.data.event.process_instance_id == current_process
                        && *sequence >= start_sequence
                })
                .filter_map(|(entry, occurred_at_ms, sequence)| {
                    let role = match &entry.tracking.data.event.kind {
                        crate::models::PiProtocolV2EventKind::SessionStart {
                            agent_role, ..
                        }
                        | crate::models::PiProtocolV2EventKind::Lineage { agent_role, .. } => {
                            agent_role.as_ref()
                        }
                        crate::models::PiProtocolV2EventKind::SessionEnd { .. } => None,
                    }?;
                    Some((role, *sequence, *occurred_at_ms, entry.source_ordinal))
                })
                .max_by_key(|(_, sequence, occurred_at_ms, ordinal)| {
                    (*sequence, *occurred_at_ms, *ordinal)
                })
                .map(|(role, _, _, _)| role.clone());
            let current_event = &current.0.tracking.data.event;
            let origin_at_ms = pi_timestamp_ms(&current_event.origin_at)?;
            let lifecycle_state =
                match lifecycle_event.map(|(entry, _, _)| &entry.tracking.data.event.kind) {
                    Some(crate::models::PiProtocolV2EventKind::SessionEnd { .. }) => "closed",
                    _ => "recovering",
                };
            let closed_at_ms = (lifecycle_state == "closed")
                .then(|| lifecycle_event.map(|(_, occurred_at_ms, _)| *occurred_at_ms))
                .flatten();
            Ok::<PiPersistedLifecycle, TranscriptAnalyticsError>(PiPersistedLifecycle {
                normalized_hostname: normalized_hostname.clone(),
                session_id: session.header.id.clone(),
                source_key: source_key.to_owned(),
                origin_at_ms,
                process_instance_id: current_event.process_instance_id.clone(),
                current_sequence: current.2,
                current_occurrence_id: current_event.event_uuid.clone(),
                occurred_at_ms: current.1,
                lifecycle_state,
                direct_parent_session_id,
                visible_root_session_id,
                lineage_state,
                lineage_reason,
                agent_role,
                reporter_protocol: i64::from(current.0.tracking.data.reporter.protocol),
                reporter_version: current.0.tracking.data.reporter.version.clone(),
                closed_at_ms,
            })
        })
        .transpose()?;

    let mut usage = Vec::new();
    let mut seen = HashSet::new();
    for entry in &session.entries {
        if entry.message.get("role").and_then(Value::as_str) != Some("assistant")
            || !seen.insert(entry.base.id.as_str())
        {
            continue;
        }
        let Some(native_usage) = entry.message.get("usage").and_then(Value::as_object) else {
            continue;
        };
        let provider = entry
            .message
            .get("provider")
            .and_then(Value::as_str)
            .ok_or(TranscriptAnalyticsError::PiSourceIdentity)?;
        let model = entry
            .message
            .get("model")
            .and_then(Value::as_str)
            .ok_or(TranscriptAnalyticsError::PiSourceIdentity)?;
        let model_id = crate::model_usage::validate_model_id(&format!("{provider}/{model}"))
            .map_err(|_| TranscriptAnalyticsError::PiSourceIdentity)?;
        usage.push(PiPersistedUsage {
            source_record_key: format!(
                "pi_native_v1:{}:{}",
                session.header.id.len(),
                entry.base.id
            ),
            source_ordinal: i64::try_from(entry.source_ordinal)
                .map_err(|_| TranscriptAnalyticsError::PiSourceIdentity)?,
            turn_id: entry.base.id.clone(),
            timestamp: entry.base.timestamp.clone(),
            observed_at_ms: pi_timestamp_ms(&entry.base.timestamp)?,
            model_id,
            input_tokens: pi_usage_dimension(native_usage, "input")?,
            output_tokens: pi_usage_dimension(native_usage, "output")?,
            cache_creation_tokens: pi_usage_dimension(native_usage, "cacheWrite")?,
            cache_read_tokens: pi_usage_dimension(native_usage, "cacheRead")?,
            input_cost: pi_usage_cost(native_usage, "input")?,
            output_cost: pi_usage_cost(native_usage, "output")?,
            cache_read_cost: pi_usage_cost(native_usage, "cacheRead")?,
            cache_write_cost: pi_usage_cost(native_usage, "cacheWrite")?,
            total_cost: pi_usage_cost(native_usage, "total")?,
        });
    }

    Ok(PiPersistedEvidence {
        lifecycle,
        receipts,
        usage,
    })
}

/// Parse all transcript-derived analytics without mutating storage.
// @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots]]
pub(crate) fn parse_transcript_analytics_source(
    source: &DiscoveredRetainedJsonlSource,
    hostname: &str,
) -> Result<ParsedTranscriptAnalyticsSource, TranscriptAnalyticsError> {
    let (bytes, stable_stat) =
        read_stable_transcript(&source.canonical_path).map_err(TranscriptAnalyticsError::from)?;
    let content_sha256 = model_source_content_sha256(&bytes);
    parse_transcript_analytics_source_bytes(source, hostname, bytes, stable_stat, content_sha256)
}

fn parse_transcript_analytics_source_bytes(
    source: &DiscoveredRetainedJsonlSource,
    hostname: &str,
    bytes: Vec<u8>,
    stable_stat: ModelSourceFastFingerprint,
    content_sha256: String,
) -> Result<ParsedTranscriptAnalyticsSource, TranscriptAnalyticsError> {
    let contents =
        std::str::from_utf8(&bytes).map_err(|_| TranscriptAnalyticsError::InvalidUtf8)?;
    let records = parse_jsonl_records(contents);
    drop(bytes);
    let source_key = source.source_key.clone();
    let (native_identity, diagnostics, extracted, pi_evidence) = if source.provider
        == IntegrationProvider::Pi
    {
        let session = crate::pi_session::parse_pi_session_records(
            records
                .iter()
                .map(|record| (record.ordinal, record.value.clone())),
        )
        .map_err(|error| TranscriptAnalyticsError::PiSession(error.to_string()))?
        .ok_or_else(|| TranscriptAnalyticsError::PiSession("missing session header".into()))?;
        let native_identity = resolve_pi_native_identity(&session)?;
        let pi_evidence = build_pi_persisted_evidence(&session, &source_key, hostname)?;
        let extracted = crate::sessions::extract_pi_session(&source.canonical_path, session);
        (
            native_identity,
            TranscriptRecordDiagnostics::default(),
            extracted,
            Some(pi_evidence),
        )
    } else {
        let (native_identity, diagnostics) = resolve_native_identity(source, &records)?;
        let extracted =
            extract_messages_from_jsonl_records(source.provider, &source.canonical_path, &records);
        (native_identity, diagnostics, extracted, None)
    };
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
    let (tool_actions, skill_usages) = owned_tool_rows(
        &OwnedToolRowIdentity {
            provider: source.provider,
            source_key: &source_key,
            session_id: &native_identity.chain_id,
            chain_id: &native_identity.chain_id,
            parent_chain_id: native_identity.parent_chain_id.as_deref(),
            agent_id: native_identity.agent_id.as_deref(),
            is_sidechain: native_identity.is_sidechain,
            hostname,
        },
        &extracted.messages,
    );
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
            mtime_ns: stable_stat.mtime_ns(),
            size_bytes: stable_stat.size_bytes(),
            content_sha256,
            seen_generation: 0,
        },
        session_events,
        response_times,
        tool_actions,
        skill_usages,
        hook_invocations,
        pi_evidence,
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
    use crate::sessions::ToolAction;
    use chrono::Utc;
    use rusqlite::OptionalExtension;
    use serde_json::json;
    use serial_test::serial;
    use std::fs::{File, FileTimes};
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};
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

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Persisted Turn Recovery]]
    #[test]
    fn persisted_pi_corpus_derives_source_owned_response_times() {
        let dir = TempDir::new().expect("corpus directory");
        let session_id = "01a018c8-2867-71be-a72b-cdf822ddbe75";
        let path = dir.path().join("root.jsonl");
        std::fs::write(&path, include_str!("fixtures/pi-parity-corpus/root.jsonl"))
            .expect("write corpus session");
        let source_key = crate::storage::pi_source_key(TEST_HOSTNAME, session_id)
            .expect("canonical Pi source key");
        let source = DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Pi,
            source_root_key: source_root_key(IntegrationProvider::Pi),
            source_key: source_key.clone(),
            filesystem_path: path.clone(),
            canonical_path: path,
            layout_hint: RetainedJsonlSourceLayoutHint::PiTranscript,
        };

        let parsed = parse_transcript_analytics_source(&source, TEST_HOSTNAME)
            .expect("parse persisted Pi session");
        assert_eq!(parsed.snapshot.response_times.len(), 1);
        assert!(
            parsed
                .snapshot
                .response_times
                .iter()
                .all(|row| row.source_key == source_key)
        );
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
        let stat = model_source_fast_fingerprint(&metadata).expect("stable transcript stat");
        (stat.mtime_ns(), stat.size_bytes())
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

    fn pi_tracking_line(
        id: &str,
        parent_id: Option<&str>,
        timestamp: &str,
        event: Value,
    ) -> String {
        let mut data = event.as_object().expect("tracking event object").clone();
        data.insert(
            "schema".to_owned(),
            json!(crate::pi_tracking::PI_PROTOCOL_V2_TRACKING_SCHEMA),
        );
        data.insert(
            "reporter".to_owned(),
            json!({
                "protocol": crate::pi_tracking::PI_PROTOCOL_V2,
                "version": crate::pi_tracking::PI_PROTOCOL_V2_REPORTER_VERSION,
                "quill_build": crate::pi_tracking::PI_PROTOCOL_V2_QUILL_BUILD,
                "capability_digest": crate::pi_tracking::PI_PROTOCOL_V2_CAPABILITY_DIGEST,
            }),
        );
        json!({
            "type": "custom",
            "id": id,
            "parentId": parent_id,
            "timestamp": timestamp,
            "customType": "quill-tracking",
            "data": Value::Object(data),
        })
        .to_string()
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

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Modern Codex Turn Extraction]]
    #[test]
    fn modern_codex_messages_create_response_rows_without_legacy_duplicates() {
        let dir = TempDir::new().expect("tempdir");
        let source = write_jsonl_source(
            dir.path(),
            "rollout-modern.jsonl",
            IntegrationProvider::Codex,
            RetainedJsonlSourceLayoutHint::CodexTranscript,
            &[
                json!({
                    "type": "session_meta",
                    "timestamp": "2026-01-01T00:00:00.000Z",
                    "payload": { "id": "codex-modern", "cwd": "/work/quill" }
                })
                .to_string(),
                json!({
                    "type": "response_item",
                    "timestamp": "2026-01-01T00:00:01.000Z",
                    "payload": { "type": "message", "role": "user", "content": [
                        { "type": "input_text", "text": "first prompt" }
                    ] }
                })
                .to_string(),
                json!({
                    "type": "response_item",
                    "timestamp": "2026-01-01T00:00:02.000Z",
                    "payload": { "type": "message", "role": "assistant", "content": [
                        { "type": "output_text", "text": "first answer" }
                    ] }
                })
                .to_string(),
                json!({
                    "type": "event_msg",
                    "timestamp": "2026-01-01T00:00:03.000Z",
                    "payload": { "type": "user_message", "message": "second prompt" }
                })
                .to_string(),
                json!({
                    "type": "response_item",
                    "timestamp": "2026-01-01T00:00:03.005Z",
                    "payload": { "type": "message", "role": "user", "content": [
                        { "type": "input_text", "text": "second prompt" }
                    ] }
                })
                .to_string(),
                json!({
                    "type": "response_item",
                    "timestamp": "2026-01-01T00:00:04.000Z",
                    "payload": { "type": "message", "role": "assistant", "content": [
                        { "type": "output_text", "text": "second answer" }
                    ] }
                })
                .to_string(),
                json!({
                    "type": "event_msg",
                    "timestamp": "2026-01-01T00:00:04.005Z",
                    "payload": { "type": "agent_message", "message": "second answer" }
                })
                .to_string(),
                json!({
                    "type": "event_msg",
                    "timestamp": "2026-01-01T00:00:05.000Z",
                    "payload": { "type": "user_message", "message": "legacy-only prompt" }
                })
                .to_string(),
                json!({
                    "type": "event_msg",
                    "timestamp": "2026-01-01T00:00:06.000Z",
                    "payload": { "type": "agent_message", "message": "legacy-only answer" }
                })
                .to_string(),
            ],
        );

        let parsed = parse_transcript_analytics_source(&source, TEST_HOSTNAME)
            .expect("parse modern Codex source");
        assert_eq!(
            parsed.snapshot.response_times.len(),
            3,
            "modern-only and unmatched legacy turns must survive while adjacent copies coalesce"
        );
        assert_eq!(
            parsed
                .snapshot
                .session_events
                .iter()
                .filter(|event| matches!(event.kind, SessionEventKind::UserText))
                .count(),
            3
        );
        assert_eq!(
            parsed
                .snapshot
                .session_events
                .iter()
                .filter(|event| matches!(event.kind, SessionEventKind::AsstText))
                .count(),
            3
        );
    }

    // @lat: [[pi-model-usage-tests#Pi Model Usage Test Specs#Persisted Source Atomic Replacement]]
    #[test]
    #[serial]
    fn persisted_pi_snapshot_replaces_all_owned_evidence_and_retains_last_good() {
        clear_env();
        let data_dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&data_dir);
        let transcripts = TempDir::new().expect("tempdir");
        let source_key = crate::storage::pi_source_key(TEST_HOSTNAME, "session-root")
            .expect("canonical Pi source key");
        let mut lines = vec![
            json!({
                "type": "session",
                "version": 3,
                "id": "session-root",
                "timestamp": "2026-08-18T02:00:00.000Z",
                "cwd": "/work/quill"
            })
            .to_string(),
            pi_tracking_line(
                "track-old-start",
                None,
                "2026-08-18T02:00:01.000Z",
                json!({
                    "event_uuid": "event-old-start",
                    "event": "session_start",
                    "provider": "pi",
                    "normalized_host": TEST_HOSTNAME,
                    "session_id": "session-root",
                    "process_instance_id": "process-old",
                    "sequence": 1,
                    "origin_at": "2026-08-18T02:00:00.000Z",
                    "occurred_at": "2026-08-18T02:00:01.000Z",
                    "delivery_source": "live",
                    "reason": "startup",
                    "lineage": {"kind": "root"}
                }),
            ),
            pi_tracking_line(
                "track-new-start",
                Some("track-old-start"),
                "2026-08-18T02:00:02.000Z",
                json!({
                    "event_uuid": "event-new-start",
                    "event": "session_start",
                    "provider": "pi",
                    "normalized_host": TEST_HOSTNAME,
                    "session_id": "session-root",
                    "process_instance_id": "process-new",
                    "sequence": 1,
                    "origin_at": "2026-08-18T02:00:02.000Z",
                    "occurred_at": "2026-08-18T02:00:02.000Z",
                    "delivery_source": "reconciliation",
                    "reason": "resume",
                    "lineage": {
                        "kind": "agent",
                        "parent_session_id": "session-parent"
                    },
                    "agent_role": "reviewer"
                }),
            ),
            // A later stale end from the superseded process must not close the
            // newer process selected by its own start occurrence.
            pi_tracking_line(
                "track-old-end",
                Some("track-new-start"),
                "2026-08-18T02:00:03.000Z",
                json!({
                    "event_uuid": "event-old-end",
                    "event": "session_end",
                    "provider": "pi",
                    "normalized_host": TEST_HOSTNAME,
                    "session_id": "session-root",
                    "process_instance_id": "process-old",
                    "sequence": 2,
                    "origin_at": "2026-08-18T02:00:00.000Z",
                    "occurred_at": "2026-08-18T02:00:03.000Z",
                    "delivery_source": "reconciliation",
                    "reason": "quit"
                }),
            ),
            json!({
                "type": "message",
                "id": "prompt",
                "parentId": "track-old-end",
                "timestamp": "2026-08-18T02:00:04.000Z",
                "message": {"role": "user", "content": "inspect the skill"}
            })
            .to_string(),
            json!({
                "type": "message",
                "id": "answer",
                "parentId": "prompt",
                "timestamp": "2026-08-18T02:00:05.000Z",
                "message": {
                    "role": "assistant",
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-5",
                    "content": [
                        {"type": "text", "text": "reading"},
                        {
                            "type": "toolCall",
                            "id": "read-skill",
                            "name": "read",
                            "arguments": {
                                "path": "/home/test/.pi/agent/skills/demo/SKILL.md"
                            }
                        }
                    ],
                    "usage": {
                        "input": 11,
                        "output": 7,
                        "cacheWrite": 5,
                        "cacheRead": 3,
                        "cost": {
                            "input": 0.01,
                            "output": 0.02,
                            "cacheWrite": 0.03,
                            "cacheRead": 0.04,
                            "total": 0.10
                        }
                    }
                }
            })
            .to_string(),
            json!({
                "type": "message",
                "id": "result",
                "parentId": "answer",
                "timestamp": "2026-08-18T02:00:06.000Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "read-skill",
                    "toolName": "read",
                    "content": [{"type": "text", "text": "skill body"}]
                }
            })
            .to_string(),
        ];
        let path = transcripts.path().join("session-root.jsonl");
        std::fs::write(&path, jsonl_body(&lines)).expect("write persisted Pi source");
        set_mtime_ns(&path, FIXED_MTIME_NS);
        let source = DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Pi,
            source_root_key: "pi:sessions",
            source_key: source_key.clone(),
            filesystem_path: path.clone(),
            canonical_path: path,
            layout_hint: RetainedJsonlSourceLayoutHint::PiTranscript,
        };
        let generation = storage
            .begin_transcript_analytics_generation(IntegrationProvider::Pi, source.source_root_key)
            .expect("begin Pi generation");
        let snapshot = stamp_analytics_root(
            parse_transcript_analytics_source(&source, TEST_HOSTNAME)
                .expect("parse persisted Pi source"),
            "session-root",
            generation,
        )
        .expect("stamp Pi snapshot");
        assert_eq!(snapshot.session_events.len(), 4);
        assert_eq!(snapshot.response_times.len(), 1);
        assert_eq!(snapshot.tool_actions.len(), 1);
        assert_eq!(snapshot.skill_usages.len(), 1);
        assert_eq!(
            snapshot
                .pi_evidence
                .as_ref()
                .expect("Pi evidence")
                .usage
                .len(),
            1
        );
        let sibling_session = "session-sibling";
        let sibling_key = crate::storage::pi_source_key(TEST_HOSTNAME, sibling_session)
            .expect("canonical sibling Pi source key");
        let mut sibling = snapshot.clone();
        sibling.source.source_key.clone_from(&sibling_key);
        sibling.source.source_path = transcripts.path().join("session-sibling.jsonl");
        sibling.source.source_session_id = sibling_session.to_owned();
        sibling.source.analytics_session_id = sibling_session.to_owned();
        sibling.source.chain_id = sibling_session.to_owned();
        for row in &mut sibling.session_events {
            row.source_key.clone_from(&sibling_key);
            row.session_id = sibling_session.to_owned();
            row.chain_id = sibling_session.to_owned();
        }
        for row in &mut sibling.response_times {
            row.source_key.clone_from(&sibling_key);
            row.session_id = sibling_session.to_owned();
            row.chain_id = sibling_session.to_owned();
        }
        for row in &mut sibling.tool_actions {
            row.source_key.clone_from(&sibling_key);
            row.session_id = sibling_session.to_owned();
            row.chain_id = sibling_session.to_owned();
        }
        for row in &mut sibling.skill_usages {
            row.source_key.clone_from(&sibling_key);
            row.session_id = sibling_session.to_owned();
            row.chain_id = sibling_session.to_owned();
        }
        if let Some(evidence) = &mut sibling.pi_evidence {
            if let Some(lifecycle) = &mut evidence.lifecycle {
                lifecycle.source_key.clone_from(&sibling_key);
                lifecycle.session_id = sibling_session.to_owned();
            }
            for receipt in &mut evidence.receipts {
                receipt.source_key.clone_from(&sibling_key);
                receipt.session_id = sibling_session.to_owned();
            }
        }
        storage
            .replace_transcript_analytics_snapshot(&snapshot)
            .expect("commit persisted Pi snapshot");
        storage
            .replace_transcript_analytics_snapshot(&sibling)
            .expect("commit sibling Pi snapshot");
        let reporter = crate::pi_tracking::PiReporterHealthSubject::new(
            TEST_HOSTNAME,
            "process-new",
            "managed",
            crate::pi_tracking::PI_PROTOCOL_V2,
            crate::pi_tracking::PI_PROTOCOL_V2_REPORTER_VERSION,
            crate::pi_tracking::PI_PROTOCOL_V2_QUILL_BUILD,
            crate::pi_tracking::PI_PROTOCOL_V2_CAPABILITY_DIGEST,
        )
        .expect("Pi reporter health subject");
        storage
            .record_pi_reporter_recovery(
                &reporter,
                crate::pi_tracking::PiReporterHealthDimension::Source,
                Utc::now().timestamp_millis(),
            )
            .expect("seed Pi reporter source health");

        #[derive(Debug, PartialEq)]
        struct LifecycleState {
            process_instance_id: String,
            sequence: i64,
            state: String,
            parent_session_id: Option<String>,
            lineage_state: String,
            agent_role: Option<String>,
        }

        #[derive(Debug, PartialEq)]
        struct PiState {
            evidence_counts: Vec<(&'static str, i64)>,
            model_input_tokens: i64,
            rollup_input_tokens: i64,
            token_rows: (i64, i64),
            lifecycle: Option<LifecycleState>,
            transcript_owner: (i64, i64),
            model_owner: (i64, i64),
            live_owner: i64,
        }

        let persisted_state = |storage: &Storage, key: &str, session_id: &str| {
            let conn = rusqlite::Connection::open(storage.database_path())
                .expect("open persisted Pi test reader");
            let count = |table: &'static str| {
                (
                    table,
                    conn.query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {table}
                             WHERE provider = 'pi' AND source_key = ?1"
                        ),
                        rusqlite::params![key],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("count persisted Pi rows"),
                )
            };
            let sum = |table: &str| {
                conn.query_row(
                    &format!(
                        "SELECT COALESCE(SUM(input_tokens), 0) FROM {table}
                         WHERE provider = 'pi' AND source_key = ?1"
                    ),
                    rusqlite::params![key],
                    |row| row.get::<_, i64>(0),
                )
                .expect("sum persisted Pi input tokens")
            };
            PiState {
                evidence_counts: [
                    "session_events",
                    "response_times",
                    "tool_actions",
                    "skill_usages",
                    "hook_invocations",
                    "runtime_hourly",
                    "runtime_turn_state",
                    "model_usage_observations",
                    "model_usage_hourly",
                    "pi_event_receipts",
                    "pi_session_lifecycle",
                ]
                .map(count)
                .to_vec(),
                model_input_tokens: sum("model_usage_observations"),
                rollup_input_tokens: sum("model_usage_hourly"),
                token_rows: conn
                    .query_row(
                        "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0)
                         FROM token_snapshots
                         WHERE provider = 'pi' AND hostname = ?1 AND session_id = ?2",
                        rusqlite::params![TEST_HOSTNAME, session_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("read persisted Pi token rows"),
                lifecycle: conn
                    .query_row(
                        "SELECT process_instance_id, current_sequence,
                                lifecycle_state, direct_parent_session_id,
                                lineage_state, agent_role
                         FROM pi_session_lifecycle
                         WHERE provider = 'pi' AND source_key = ?1",
                        rusqlite::params![key],
                        |row| {
                            Ok(LifecycleState {
                                process_instance_id: row.get(0)?,
                                sequence: row.get(1)?,
                                state: row.get(2)?,
                                parent_session_id: row.get(3)?,
                                lineage_state: row.get(4)?,
                                agent_role: row.get(5)?,
                            })
                        },
                    )
                    .optional()
                    .expect("read persisted Pi lifecycle"),
                transcript_owner: conn
                    .query_row(
                        "SELECT COUNT(*), COALESCE(MAX(seen_generation), 0)
                         FROM transcript_analytics_sources
                         WHERE provider = 'pi' AND source_key = ?1",
                        rusqlite::params![key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("read transcript owner"),
                model_owner: conn
                    .query_row(
                        "SELECT COUNT(*), COALESCE(MAX(observation_count), 0)
                         FROM model_observation_sources
                         WHERE provider = 'pi' AND source_key = ?1",
                        rusqlite::params![key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("read model owner"),
                live_owner: conn
                    .query_row(
                        "SELECT COUNT(*) FROM live_analytics_sessions
                         WHERE provider = 'pi' AND hostname = ?1 AND session_id = ?2",
                        rusqlite::params![TEST_HOSTNAME, session_id],
                        |row| row.get(0),
                    )
                    .expect("read live owner"),
            }
        };
        let count_of = |state: &PiState, table: &str| {
            state
                .evidence_counts
                .iter()
                .find_map(|(candidate, count)| (*candidate == table).then_some(*count))
                .expect("tracked Pi evidence table")
        };
        let initial = persisted_state(&storage, &source_key, "session-root");
        let sibling_initial = persisted_state(&storage, &sibling_key, sibling_session);
        assert_eq!(count_of(&initial, "session_events"), 4);
        assert_eq!(count_of(&initial, "response_times"), 1);
        assert_eq!(count_of(&initial, "tool_actions"), 1);
        assert_eq!(count_of(&initial, "skill_usages"), 1);
        assert_eq!(count_of(&initial, "model_usage_observations"), 1);
        assert_eq!(count_of(&initial, "pi_event_receipts"), 3);
        assert_eq!(initial.token_rows, (1, 11));
        assert_eq!(initial.transcript_owner, (1, generation));
        assert_eq!(initial.model_owner, (1, 1));
        assert_eq!(initial.live_owner, 1);
        assert_eq!(
            initial.lifecycle,
            Some(LifecycleState {
                process_instance_id: "process-new".to_owned(),
                sequence: 1,
                state: "recovering".to_owned(),
                parent_session_id: Some("session-parent".to_owned()),
                lineage_state: "agent".to_owned(),
                agent_role: Some("reviewer".to_owned()),
            })
        );

        // All deletes and replacement inserts execute before this final
        // registry CHECK fails. Exact state, including the sibling, must roll
        // back rather than exposing the deliberately emptied/changed evidence.
        let mut broken = snapshot.clone();
        broken.session_events.clear();
        broken.response_times.clear();
        broken.tool_actions.clear();
        broken.skill_usages.clear();
        if let Some(evidence) = &mut broken.pi_evidence {
            evidence.receipts.clear();
            evidence.usage[0].input_tokens = 999;
            evidence
                .lifecycle
                .as_mut()
                .expect("broken lifecycle")
                .process_instance_id = "broken-process".to_owned();
        }
        broken.source.mtime_ns = broken.source.mtime_ns.saturating_add(1);
        let trigger_conn = rusqlite::Connection::open(storage.database_path())
            .expect("open Pi failure trigger connection");
        trigger_conn
            .execute_batch(
                "CREATE TRIGGER fail_pi_transcript_registry
                 BEFORE UPDATE ON transcript_analytics_sources
                 WHEN NEW.provider = 'pi'
                 BEGIN
                     SELECT RAISE(ABORT, 'late Pi registry failure');
                 END;",
            )
            .expect("arm final Pi registry failure");
        let error = storage
            .replace_transcript_analytics_snapshot(&broken)
            .expect_err("final registry trigger must fail");
        trigger_conn
            .execute_batch("DROP TRIGGER fail_pi_transcript_registry;")
            .expect("disarm final Pi registry failure");
        assert!(
            error.contains("Upsert transcript source registry"),
            "expected final registry failure, got {error}"
        );
        assert_eq!(
            persisted_state(&storage, &source_key, "session-root"),
            initial
        );
        assert_eq!(
            persisted_state(&storage, &sibling_key, sibling_session),
            sibling_initial
        );

        let drifted = lines[0].replace("session-root", "session-other");
        std::fs::write(
            &source.canonical_path,
            jsonl_body(
                &std::iter::once(drifted)
                    .chain(lines.iter().skip(1).cloned())
                    .collect::<Vec<_>>(),
            ),
        )
        .expect("write drifted Pi source");
        let drift_error = parse_transcript_analytics_source(&source, TEST_HOSTNAME)
            .err()
            .expect("drifted Pi source must fail");
        assert!(
            record_source_failure(&storage, &source, generation, &drift_error.to_string()).is_ok(),
            "record Pi source failure",
        );
        assert_eq!(
            storage
                .pi_reporter_health_summary_at(Utc::now().timestamp_millis())
                .expect("read failed Pi reporter health")
                .expect("Pi reporter health summary")
                .worst_code,
            Some(crate::pi_tracking::PiReporterHealthCode::ReconciliationFailed),
        );
        assert_eq!(
            persisted_state(&storage, &source_key, "session-root"),
            initial
        );

        lines.push(pi_tracking_line(
            "track-new-end",
            Some("result"),
            "2026-08-18T02:00:07.000Z",
            json!({
                "event_uuid": "event-new-end",
                "event": "session_end",
                "provider": "pi",
                "normalized_host": TEST_HOSTNAME,
                "session_id": "session-root",
                "process_instance_id": "process-new",
                "sequence": 2,
                "origin_at": "2026-08-18T02:00:02.000Z",
                "occurred_at": "2026-08-18T02:00:07.000Z",
                "delivery_source": "reconciliation",
                "reason": "quit"
            }),
        ));
        std::fs::write(&source.canonical_path, jsonl_body(&lines))
            .expect("write completed Pi source");
        let next_generation = storage
            .begin_transcript_analytics_generation(IntegrationProvider::Pi, source.source_root_key)
            .expect("advance Pi generation");
        let completed = stamp_analytics_root(
            parse_transcript_analytics_source(&source, TEST_HOSTNAME)
                .expect("parse completed Pi source"),
            "session-root",
            next_generation,
        )
        .expect("stamp completed Pi snapshot");
        storage
            .replace_transcript_analytics_snapshot(&completed)
            .expect("replace completed Pi snapshot");
        let recovered_health = storage
            .pi_reporter_health_summary_at(Utc::now().timestamp_millis())
            .expect("read recovered Pi reporter health")
            .expect("Pi reporter health summary");
        assert_eq!(recovered_health.worst_code, None);
        assert!(recovered_health.recovered_at_ms.is_some());
        let final_state = persisted_state(&storage, &source_key, "session-root");
        assert_eq!(count_of(&final_state, "session_events"), 4);
        assert_eq!(count_of(&final_state, "response_times"), 1);
        assert_eq!(count_of(&final_state, "tool_actions"), 1);
        assert_eq!(count_of(&final_state, "skill_usages"), 1);
        assert_eq!(count_of(&final_state, "model_usage_observations"), 1);
        assert_eq!(count_of(&final_state, "pi_event_receipts"), 4);
        assert_eq!(
            final_state
                .lifecycle
                .as_ref()
                .map(|row| row.process_instance_id.as_str()),
            Some("process-new")
        );
        assert_eq!(
            final_state.lifecycle.as_ref().map(|row| row.sequence),
            Some(2)
        );
        assert_eq!(
            final_state.lifecycle.as_ref().map(|row| row.state.as_str()),
            Some("closed")
        );

        let mut emptied = completed;
        emptied.session_events.clear();
        emptied.response_times.clear();
        emptied.tool_actions.clear();
        emptied.skill_usages.clear();
        emptied.hook_invocations.clear();
        emptied.pi_evidence = Some(PiPersistedEvidence {
            lifecycle: None,
            receipts: Vec::new(),
            usage: Vec::new(),
        });
        storage
            .replace_transcript_analytics_snapshot(&emptied)
            .expect("clear persisted Pi source");
        let empty_state = persisted_state(&storage, &source_key, "session-root");
        assert!(
            empty_state
                .evidence_counts
                .iter()
                .all(|(table, count)| *table == "pi_session_lifecycle" || *count == 0),
            "empty replacement must clear source-owned analytics evidence: {empty_state:?}"
        );
        assert_eq!(count_of(&empty_state, "pi_session_lifecycle"), 1);
        assert_eq!(empty_state.model_input_tokens, 0);
        assert_eq!(empty_state.rollup_input_tokens, 0);
        assert_eq!(empty_state.token_rows, (0, 0));
        assert_eq!(
            empty_state.lifecycle, final_state.lifecycle,
            "missing reconciliation lifecycle cannot delete newer committed state"
        );
        assert_eq!(empty_state.transcript_owner, (1, next_generation));
        assert_eq!(empty_state.model_owner, (1, 0));
        assert_eq!(empty_state.live_owner, 1);
        assert_eq!(
            persisted_state(&storage, &sibling_key, sibling_session),
            sibling_initial,
            "empty target replacement must not reach its sibling"
        );
        clear_env();
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
            Err(StableTranscriptReadError::SourceTooLarge)
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
            agent_nickname: None,
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
                agent_nickname: Some("worker".to_owned()),
                ..base.clone()
            }
        ));
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
            agent_nickname: None,
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

    /// One message carrying one tool action, so a fixture can plant each
    /// action-key fallback in turn.
    fn tool_row_message(
        uuid: &str,
        tool_use_id: &str,
        file_path: Option<&str>,
    ) -> ExtractedMessage {
        ExtractedMessage {
            uuid: uuid.to_owned(),
            session_id: "native-session".to_owned(),
            parent_session_id: None,
            role: "assistant".to_owned(),
            content: String::new(),
            timestamp: TEST_TIMESTAMP.to_owned(),
            git_branch: String::new(),
            tools_used: Vec::new(),
            files_modified: Vec::new(),
            code_changes: Vec::new(),
            commands_run: Vec::new(),
            tool_details: Vec::new(),
            tool_actions: vec![ToolAction {
                tool_use_id: tool_use_id.to_owned(),
                source_ordinal: 7,
                block_ordinal: 2,
                tool_name: "Read".to_owned(),
                category: "tool_detail".to_owned(),
                file_path: file_path.map(str::to_owned),
                summary: "read a file".to_owned(),
                full_input: None,
                full_output: None,
                lines_added: None,
                lines_removed: None,
                timestamp: TEST_TIMESTAMP.to_owned(),
            }],
            is_sidechain: false,
            agent_id: None,
            parent_uuid: Some("parent-uuid".to_owned()),
            cwd: Some("/work/quill".to_owned()),
        }
    }

    /// The retained parser and Pi's notify path build their `tool_actions` and
    /// `skill_usages` through this one builder, so the action-key fallback and
    /// the skill fan-out cannot drift apart per owner: only the identity
    /// columns each owner supplies may differ between the two row sets.
    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Owned Row Builder Shared With Retained Parsing]]
    #[test]
    fn owned_tool_rows_differ_only_by_owner_identity() {
        let messages = [
            tool_row_message("asst-1", "call-read", Some("/skills/unslop/SKILL.md")),
            tool_row_message("asst-2", "", None),
            tool_row_message("", "", None),
        ];

        let (retained_actions, retained_skills) = owned_tool_rows(
            &OwnedToolRowIdentity {
                provider: IntegrationProvider::Claude,
                source_key: "claude:source",
                session_id: "root-chain",
                chain_id: "root-chain",
                parent_chain_id: Some("parent-chain"),
                agent_id: Some("agent-7"),
                is_sidechain: true,
                hostname: TEST_HOSTNAME,
            },
            &messages,
        );
        let pi_source_key = crate::storage::pi_source_key(TEST_HOSTNAME, "pi-session")
            .expect("canonical Pi source key");
        let (pi_actions, pi_skills) = owned_tool_rows(
            &OwnedToolRowIdentity {
                provider: IntegrationProvider::Pi,
                source_key: &pi_source_key,
                session_id: "pi-session",
                chain_id: "pi-session",
                parent_chain_id: None,
                agent_id: None,
                is_sidechain: false,
                hostname: TEST_HOSTNAME,
            },
            &messages,
        );

        let action_keys = |rows: &[OwnedToolAction]| {
            rows.iter()
                .map(|row| row.action_key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            action_keys(&retained_actions),
            vec!["call-read", "asst-2:2", "record:7:2"],
            "a missing tool id falls back to message identity, then to record ordinals"
        );
        assert_eq!(
            action_keys(&pi_actions),
            action_keys(&retained_actions),
            "both owners dedupe on the same action keys"
        );

        let skill_shape = |rows: &[OwnedSkillUsage]| {
            rows.iter()
                .map(|row| {
                    (
                        row.skill_name.clone(),
                        row.skill_path.clone(),
                        row.message_id.clone(),
                        row.tool_name.clone(),
                        row.timestamp.clone(),
                        row.cwd.clone(),
                        row.hostname.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            skill_shape(&pi_skills),
            vec![(
                "unslop".to_owned(),
                "/skills/unslop/SKILL.md".to_owned(),
                "asst-1".to_owned(),
                "Read".to_owned(),
                TEST_TIMESTAMP.to_owned(),
                Some("/work/quill".to_owned()),
                TEST_HOSTNAME.to_owned(),
            )],
            "only the SKILL.md read fans out, carrying the message cwd and host"
        );
        assert_eq!(skill_shape(&retained_skills), skill_shape(&pi_skills));

        let action_owner = |row: &OwnedToolAction| {
            (
                row.provider,
                row.source_key.clone(),
                row.session_id.clone(),
                row.chain_id.clone(),
                row.parent_chain_id.clone(),
                row.agent_id.clone(),
                row.is_sidechain,
            )
        };
        let skill_owner = |row: &OwnedSkillUsage| {
            (
                row.provider,
                row.source_key.clone(),
                row.session_id.clone(),
                row.chain_id.clone(),
                row.parent_chain_id.clone(),
            )
        };
        assert_eq!(
            action_owner(&retained_actions[0]),
            (
                IntegrationProvider::Claude,
                "claude:source".to_owned(),
                "root-chain".to_owned(),
                "root-chain".to_owned(),
                Some("parent-chain".to_owned()),
                Some("agent-7".to_owned()),
                true,
            )
        );
        assert_eq!(
            action_owner(&pi_actions[0]),
            (
                IntegrationProvider::Pi,
                pi_source_key.clone(),
                "pi-session".to_owned(),
                "pi-session".to_owned(),
                None,
                None,
                false,
            ),
            "Pi has no sub-agent transcripts, so its chain identity stays flat"
        );
        assert_eq!(
            skill_owner(&retained_skills[0]),
            (
                IntegrationProvider::Claude,
                "claude:source".to_owned(),
                "root-chain".to_owned(),
                "root-chain".to_owned(),
                Some("parent-chain".to_owned()),
            )
        );
        assert_eq!(
            skill_owner(&pi_skills[0]),
            (
                IntegrationProvider::Pi,
                pi_source_key,
                "pi-session".to_owned(),
                "pi-session".to_owned(),
                None,
            )
        );

        assert_eq!(pi_actions[0].message_id, "asst-1");
        assert_eq!(pi_actions[0].parent_uuid.as_deref(), Some("parent-uuid"));
    }
}
