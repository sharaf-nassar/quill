//! Retention policy primitive (feature 014, Phase 2).
//!
//! Everything durable about retention lives in three rows of the existing
//! `settings` table — there is **no schema migration and no schema-version
//! bump**. This module owns the keys, the value grammars, the two serialized
//! shapes ([`RetentionPolicy`], [`RetentionAuditRecord`]), cutoff derivation,
//! and the monotonic watermark rule. The database-touching half lives on
//! `Storage` (see `storage.rs`, "Retention policy primitive"), which reads and
//! writes through these functions rather than re-deriving their grammars.
//!
//! | Key | Value | Meaning |
//! | --- | --- | --- |
//! | `retention.window_days` | decimal integer as TEXT, or the literal `never`; row absent | Configured window. Absent is the default on every database and means never prune. |
//! | `retention.watermark` | conforming timestamp, exactly 24 chars ending in `Z` | Insert-time cutoff. Absent means no filtering. Advanced monotonically. |
//! | `retention.last_run` | JSON object, [`RetentionAuditRecord`] | Durable audit record of the most recent run. |
//!
//! # Tolerance
//!
//! Every read is tolerant, because these rows survive downgrades, hand edits
//! and interrupted writes, and none of them may be able to block a run:
//!
//! * a `retention.window_days` outside the preset set parses as "never"
//!   rather than as itself — the 30-day floor is a *guarantee*, not a default,
//!   and a hand-edited `7` must not be able to quietly revoke it;
//! * a malformed `retention.watermark` is treated as absent, so a value that
//!   cannot be byte-compared never becomes an insert-time filter;
//! * an unparseable `retention.last_run` parses as [`None`].
//!
//! All three log at `warn` and none of them returns an error, so a corrupted
//! value degrades the record rather than the run.

use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// Settings key holding the configured retention window.
pub const RETENTION_WINDOW_DAYS_KEY: &str = "retention.window_days";

/// Settings key holding the monotonic insert-time watermark.
pub const RETENTION_WATERMARK_KEY: &str = "retention.watermark";

/// Settings key holding the JSON audit record of the most recent run.
pub const RETENTION_LAST_RUN_KEY: &str = "retention.last_run";

/// Literal `retention.window_days` value meaning "never prune".
///
/// An absent row means the same thing; the literal exists so a deliberate
/// "never" is distinguishable from a database that never had the key.
pub const RETENTION_NEVER: &str = "never";

/// The only accepted retention windows, in days.
///
/// The 30-day floor is not cosmetic: `range_to_duration` caps every
/// range-based reader at 30 days, so a floor of 30 is what makes
/// `get_code_stats`, `get_code_stats_history` and `get_llm_runtime_stats`
/// provably unaffected by retention. A shorter window slipping through would
/// silently invalidate that argument, so it is rejected at every boundary —
/// on write, on read, and on cutoff derivation.
pub const RETENTION_WINDOW_PRESETS: [i64; 4] = [30, 90, 180, 365];

/// Discriminator written into every audit record so a later shape can be
/// introduced without a migration: a record carrying an unknown version reads
/// back as [`None`] instead of being coerced into this one.
pub const RETENTION_AUDIT_SCHEMA_VERSION: u32 = 1;

/// Exact character count of a conforming timestamp.
const CONFORMING_TIMESTAMP_LEN: usize = 24;

/// Format producing a conforming timestamp: 24 characters, millisecond
/// precision, trailing `Z`, byte-comparable against stored timestamps.
const CONFORMING_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

/// Faults the retention primitive refuses to paper over.
///
/// Tolerated corruption (an unreadable stored value) is *not* an error — it is
/// a `warn` and a fallback. These variants are the cases where continuing
/// would produce a wrong cutoff or an unreadable audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionPolicyError {
    /// A window outside [`RETENTION_WINDOW_PRESETS`] reached a boundary that
    /// enforces the 30-day floor.
    UnsupportedWindow { window_days: i64 },
    /// Cutoff derivation overflowed the representable instant range.
    CutoffOutOfRange { window_days: i64 },
    /// A cutoff that is not byte-comparable against stored timestamps.
    MalformedCutoff { cutoff: String },
    /// A `"partial"` record without the `error_reason` that gives it meaning.
    PartialWithoutErrorReason,
    /// The audit record could not be serialized to JSON.
    AuditSerialization { reason: String },
}

impl fmt::Display for RetentionPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetentionPolicyError::UnsupportedWindow { window_days } => write!(
                f,
                "Unsupported retention window {window_days}; expected one of {RETENTION_WINDOW_PRESETS:?} or never"
            ),
            RetentionPolicyError::CutoffOutOfRange { window_days } => write!(
                f,
                "Retention cutoff for a {window_days}-day window is outside the representable range"
            ),
            RetentionPolicyError::MalformedCutoff { cutoff } => write!(
                f,
                "Retention cutoff {cutoff:?} is not a conforming timestamp (24 characters ending in Z)"
            ),
            RetentionPolicyError::PartialWithoutErrorReason => {
                write!(f, "A partial retention record requires an error_reason")
            }
            RetentionPolicyError::AuditSerialization { reason } => {
                write!(f, "Retention audit record serialization failed: {reason}")
            }
        }
    }
}

impl std::error::Error for RetentionPolicyError {}

/// Outcome vocabulary of a retention run.
///
/// `Completed` and `Skipped` match `DatabaseCompactionResult`'s vocabulary so
/// the UI can reuse its rendering. `Partial` is retention-specific and means
/// *some chunks committed, then the run stopped*. It is a third status rather
/// than `Completed` plus an `interrupted` flag on purpose: a status that has
/// to be read together with a boolean is a status that will be read wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetentionRunStatus {
    Completed,
    Partial,
    Skipped,
}

impl RetentionRunStatus {
    /// Wire representation, identical to the serialized form.
    pub const fn as_str(self) -> &'static str {
        match self {
            RetentionRunStatus::Completed => "completed",
            RetentionRunStatus::Partial => "partial",
            RetentionRunStatus::Skipped => "skipped",
        }
    }
}

impl fmt::Display for RetentionRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-table counts, carried twice by an audit record: rows deleted, and rows
/// left in place because their timestamp did not conform to the guard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionTableCounts {
    #[serde(default)]
    pub tool_actions: i64,
    #[serde(default)]
    pub session_events: i64,
}

impl RetentionTableCounts {
    /// Counts for both retention target tables.
    pub const fn new(tool_actions: i64, session_events: i64) -> Self {
        Self {
            tool_actions,
            session_events,
        }
    }
}

/// Durable record of the most recent retention run, stored as the JSON value
/// of `retention.last_run`.
///
/// A skipped run is recorded exactly like a completed one, and a partial run
/// is recorded on the error path, because "I tried on this date and nothing
/// happened, because X" is the question the record exists to answer once the
/// toast is gone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionAuditRecord {
    /// Always [`RETENTION_AUDIT_SCHEMA_VERSION`] when written by this build.
    pub schema: u32,
    pub status: RetentionRunStatus,
    /// Skip reason. Stays `None` for a partial, whose explanation lives in
    /// `error_reason`.
    #[serde(default)]
    pub reason: Option<String>,
    /// Populated if and only if `status` is [`RetentionRunStatus::Partial`].
    #[serde(default)]
    pub error_reason: Option<String>,
    #[serde(default)]
    pub window_days: Option<i64>,
    #[serde(default)]
    pub cutoff: Option<String>,
    /// Conforming timestamp of the moment the run finished.
    pub ran_at: String,
    #[serde(default)]
    pub deleted: RetentionTableCounts,
    #[serde(default)]
    pub skipped_nonconforming: RetentionTableCounts,
    #[serde(default)]
    pub bytes_before: u64,
    #[serde(default)]
    pub bytes_after: u64,
}

impl RetentionAuditRecord {
    /// A record with the current schema version, a status and a run instant.
    ///
    /// The `with_*` methods below each return a **new** record rather than
    /// mutating this one, so a caller assembling a result never has a
    /// half-populated record aliased somewhere else.
    pub fn new(status: RetentionRunStatus, ran_at: DateTime<Utc>) -> Self {
        Self {
            schema: RETENTION_AUDIT_SCHEMA_VERSION,
            status,
            reason: None,
            error_reason: None,
            window_days: None,
            cutoff: None,
            ran_at: conforming_timestamp(ran_at),
            deleted: RetentionTableCounts::default(),
            skipped_nonconforming: RetentionTableCounts::default(),
            bytes_before: 0,
            bytes_after: 0,
        }
    }

    /// Copy carrying the skip reason.
    pub fn with_reason(self, reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..self
        }
    }

    /// Copy carrying the partial-run explanation.
    pub fn with_error_reason(self, error_reason: impl Into<String>) -> Self {
        Self {
            error_reason: Some(error_reason.into()),
            ..self
        }
    }

    /// Copy carrying the window and the cutoff the run actually used.
    pub fn with_window(self, window_days: i64, cutoff: impl Into<String>) -> Self {
        Self {
            window_days: Some(window_days),
            cutoff: Some(cutoff.into()),
            ..self
        }
    }

    /// Copy carrying the per-table deletion counts.
    pub fn with_deleted(self, deleted: RetentionTableCounts) -> Self {
        Self { deleted, ..self }
    }

    /// Copy carrying the per-table non-conforming counts.
    pub fn with_skipped_nonconforming(self, skipped_nonconforming: RetentionTableCounts) -> Self {
        Self {
            skipped_nonconforming,
            ..self
        }
    }

    /// Copy carrying the whole-file footprint either side of the run.
    pub fn with_bytes(self, bytes_before: u64, bytes_after: u64) -> Self {
        Self {
            bytes_before,
            bytes_after,
            ..self
        }
    }

    /// Reject a record whose status and explanation disagree.
    ///
    /// A `"partial"` with no `error_reason` is unreadable after the fact —
    /// the whole point of the third status is that it says what went wrong.
    pub fn validate(&self) -> Result<(), RetentionPolicyError> {
        if self.status == RetentionRunStatus::Partial && self.error_reason.is_none() {
            return Err(RetentionPolicyError::PartialWithoutErrorReason);
        }
        Ok(())
    }

    /// Serialize for storage in `retention.last_run`.
    pub fn to_setting_value(&self) -> Result<String, RetentionPolicyError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| RetentionPolicyError::AuditSerialization {
            reason: error.to_string(),
        })
    }
}

/// The three retention settings rows, read together.
///
/// This is the shape `get_retention_policy` returns; every field is
/// independently absent-able because a fresh database has none of the rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RetentionPolicy {
    /// `None` means never prune.
    pub window_days: Option<i64>,
    /// `None` means no insert-time filtering.
    pub watermark: Option<String>,
    /// `None` means no run has been recorded, or the record was unreadable.
    pub last_run: Option<RetentionAuditRecord>,
}

/// Whether a timestamp satisfies the guard retention filters on:
/// `length(timestamp) = 24 AND timestamp LIKE '%Z'`.
///
/// Both halves matter. A 24-character `+0000` form passes the length half and
/// fails the suffix half; a seconds-precision `Z` does the reverse. Only a
/// value that passes both is byte-comparable against the stored corpus.
pub fn is_conforming_timestamp(value: &str) -> bool {
    value.chars().count() == CONFORMING_TIMESTAMP_LEN && value.ends_with('Z')
}

/// Render an instant in the conforming format.
pub fn conforming_timestamp(at: DateTime<Utc>) -> String {
    at.format(CONFORMING_TIMESTAMP_FORMAT).to_string()
}

/// What the insert-time watermark does to one candidate row.
///
/// The three outcomes are exhaustive by construction, which is the point: a
/// row is either inserted normally, suppressed, or inserted *and counted*
/// because its timestamp is not byte-comparable. There is no fourth case where
/// a row silently disappears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionInsertVerdict {
    /// No watermark is set, or the row is at or after it.
    Insert,
    /// Conforming and older than the watermark, so the row is not reinserted.
    Suppress,
    /// Non-conforming, so the row is inserted and counted as a pass-through.
    PassNonConforming,
}

/// Decide the fate of one row at insert time.
///
/// This is [`derive_retention_cutoff`]'s delete guard —
/// `length(timestamp) = 24 AND timestamp LIKE '%Z' AND timestamp < cutoff` —
/// inverted in effect: a row is suppressed exactly when the delete phase would
/// have deleted it. The two guards must agree or a row could be suppressed on
/// reinsert while its original was retained, which is silent data loss with no
/// delete to account for it. A non-conforming timestamp is therefore *always*
/// inserted, never suppressed, and counted so the pass-through is visible.
pub fn retention_insert_verdict(
    watermark: Option<&str>,
    timestamp: &str,
) -> RetentionInsertVerdict {
    let Some(watermark) = watermark else {
        return RetentionInsertVerdict::Insert;
    };
    if !is_conforming_timestamp(timestamp) {
        return RetentionInsertVerdict::PassNonConforming;
    }
    if timestamp < watermark {
        RetentionInsertVerdict::Suppress
    } else {
        RetentionInsertVerdict::Insert
    }
}

/// Accept a window only if it is one of [`RETENTION_WINDOW_PRESETS`].
pub fn validate_window_days(window_days: i64) -> Result<i64, RetentionPolicyError> {
    if RETENTION_WINDOW_PRESETS.contains(&window_days) {
        Ok(window_days)
    } else {
        Err(RetentionPolicyError::UnsupportedWindow { window_days })
    }
}

/// Parse a stored `retention.window_days` value.
///
/// Returns `None` for the literal `never`, and — after a `warn` — for anything
/// unparseable or outside the preset set, so a downgraded or hand-edited value
/// degrades to "never prune" instead of to a window nobody consented to.
pub fn parse_window_days_setting(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case(RETENTION_NEVER) {
        return None;
    }
    let parsed = match trimmed.parse::<i64>() {
        Ok(parsed) => parsed,
        Err(error) => {
            log::warn!(
                "Ignoring unparseable {RETENTION_WINDOW_DAYS_KEY} value {trimmed:?}: {error}"
            );
            return None;
        }
    };
    match validate_window_days(parsed) {
        Ok(window_days) => Some(window_days),
        Err(error) => {
            log::warn!("Ignoring stored retention window: {error}");
            None
        }
    }
}

/// Render a window for storage. "Never" is written by deleting the row rather
/// than by storing a literal, so the default state of every database — old,
/// new, and just-reset — is the same single state: the row is absent.
pub fn window_days_setting_value(window_days: i64) -> Result<String, RetentionPolicyError> {
    Ok(validate_window_days(window_days)?.to_string())
}

/// Parse a stored `retention.watermark` value, treating a non-conforming one
/// as absent so a value that cannot be byte-compared never filters an insert.
pub fn parse_watermark_setting(raw: &str) -> Option<String> {
    if is_conforming_timestamp(raw) {
        return Some(raw.to_string());
    }
    log::warn!("Ignoring non-conforming {RETENTION_WATERMARK_KEY} value {raw:?}");
    None
}

/// Parse a stored `retention.last_run` value tolerantly.
///
/// An unparseable value, or one carrying a schema version this build does not
/// understand, logs at `warn` and reads back as `None`. It never errors and it
/// never blocks the next write.
pub fn parse_audit_record_setting(raw: &str) -> Option<RetentionAuditRecord> {
    match serde_json::from_str::<RetentionAuditRecord>(raw) {
        Ok(record) if record.schema == RETENTION_AUDIT_SCHEMA_VERSION => Some(record),
        Ok(record) => {
            log::warn!(
                "Ignoring {RETENTION_LAST_RUN_KEY} written with unknown schema {}; expected {RETENTION_AUDIT_SCHEMA_VERSION}",
                record.schema
            );
            None
        }
        Err(error) => {
            log::warn!("Ignoring unparseable {RETENTION_LAST_RUN_KEY} value: {error}");
            None
        }
    }
}

/// Derive the cutoff for a window, as of `now`.
///
/// The result is byte-comparable against stored timestamps, which is what lets
/// the scan, the deletes and the insert filter all use plain `<`. The window is
/// re-validated here as well as on write, so a cutoff can never be derived from
/// a window that bypassed the 30-day floor.
///
/// Derivation happens **once**, in `preview_retention`; the run is handed the
/// value back and uses it verbatim. Re-deriving inside the run would place the
/// cutoff later than the one the user approved and delete rows the preview
/// never counted.
pub fn derive_retention_cutoff(
    now: DateTime<Utc>,
    window_days: i64,
) -> Result<String, RetentionPolicyError> {
    let window_days = validate_window_days(window_days)?;
    let span = TimeDelta::try_days(window_days)
        .ok_or(RetentionPolicyError::CutoffOutOfRange { window_days })?;
    let cutoff = now
        .checked_sub_signed(span)
        .ok_or(RetentionPolicyError::CutoffOutOfRange { window_days })?;
    Ok(conforming_timestamp(cutoff))
}

/// The watermark value that results from advancing `existing` to `cutoff`.
///
/// The rule is `max(existing, cutoff)` and it is the reason the watermark can
/// be trusted: rows deleted at a stricter cutoff must stay deleted, so
/// narrowing the configured window later must never let the watermark retreat
/// and resurrect them. A stored watermark that is not conforming cannot be
/// compared at all, so it is discarded with a `warn` in favour of the cutoff,
/// which is strictly better than filtering on a value SQLite cannot order.
///
/// Returns a new value; nothing is mutated in place.
pub fn advanced_watermark(
    existing: Option<&str>,
    cutoff: &str,
) -> Result<String, RetentionPolicyError> {
    if !is_conforming_timestamp(cutoff) {
        return Err(RetentionPolicyError::MalformedCutoff {
            cutoff: cutoff.to_string(),
        });
    }
    let comparable = existing.and_then(|existing| {
        if is_conforming_timestamp(existing) {
            Some(existing)
        } else {
            log::warn!(
                "Replacing non-conforming {RETENTION_WATERMARK_KEY} value {existing:?} with {cutoff:?}"
            );
            None
        }
    });
    Ok(match comparable {
        Some(existing) if existing >= cutoff => existing.to_string(),
        _ => cutoff.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention_fixture::{RetentionFixtureSpec, build_retention_fixture};
    use crate::storage::Storage;
    use serial_test::serial;

    fn fixture_spec() -> RetentionFixtureSpec {
        RetentionFixtureSpec {
            // A fixed anchor keeps every derived boundary in these assertions
            // stable regardless of when the suite runs.
            anchor: DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                .expect("parse anchor")
                .with_timezone(&Utc),
            months: 13,
            owned_rows_per_month: 4,
            live_rows_per_month: 2,
            sources: 2,
        }
    }

    fn anchor() -> DateTime<Utc> {
        fixture_spec().anchor
    }

    // @lat: [[backend#Backend#Database#Retention policy primitive#Retention Policy Primitive Test Specs#Watermark Monotonicity]]
    #[test]
    #[serial]
    fn watermark_advance_is_monotonic() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");

        let ninety = derive_retention_cutoff(anchor(), 90).expect("derive 90-day cutoff");
        let year = derive_retention_cutoff(anchor(), 365).expect("derive 365-day cutoff");
        assert!(year < ninety, "a 365-day cutoff must be the earlier one");

        let advanced = storage
            .advance_retention_watermark(&ninety)
            .expect("advance to 90-day cutoff");
        assert_eq!(ninety, advanced);

        // The 365-day cutoff is earlier, so the advance must be a no-op.
        let retreated = storage
            .advance_retention_watermark(&year)
            .expect("advance to 365-day cutoff");
        assert_eq!(ninety, retreated);
        assert_eq!(
            Some(ninety.clone()),
            storage.read_retention_watermark().expect("read watermark")
        );

        // Clearing the window to never leaves the watermark in place: rows
        // already deleted at the stricter cutoff must stay deleted.
        storage
            .write_retention_window_days(Some(90))
            .expect("write window");
        storage
            .write_retention_window_days(None)
            .expect("clear window to never");
        let policy = storage.get_retention_policy().expect("read policy");
        assert_eq!(None, policy.window_days);
        assert_eq!(Some(ninety), policy.watermark);

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention policy primitive#Retention Policy Primitive Test Specs#Audit Record Round Trip]]
    #[test]
    #[serial]
    fn audit_record_round_trips_through_a_reopen() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");

        let cutoff = derive_retention_cutoff(anchor(), 90).expect("derive cutoff");
        let record = RetentionAuditRecord::new(RetentionRunStatus::Partial, anchor())
            .with_window(90, cutoff.clone())
            .with_error_reason("free space fell below the delete-phase budget")
            .with_deleted(RetentionTableCounts::new(165_912, 523_847))
            .with_skipped_nonconforming(RetentionTableCounts::new(3, 5))
            .with_bytes(7_544_053_760, 5_610_209_280);

        storage
            .write_retention_audit_record(&record)
            .expect("write audit record");
        assert_eq!(
            Some(record.clone()),
            storage
                .get_retention_policy()
                .expect("read policy")
                .last_run
        );

        drop(storage);
        let reopened = Storage::init().expect("reopen storage on fixture");
        let read_back = reopened
            .get_retention_policy()
            .expect("read policy after reopen")
            .last_run
            .expect("audit record survives a reopen");

        assert_eq!(Some(cutoff), read_back.cutoff);
        assert_eq!(record.ran_at, read_back.ran_at);
        assert_eq!(RetentionRunStatus::Partial, read_back.status);
        assert_eq!(
            Some("free space fell below the delete-phase budget".to_string()),
            read_back.error_reason
        );
        assert_eq!(None, read_back.reason);
        assert_eq!(
            RetentionTableCounts::new(165_912, 523_847),
            read_back.deleted
        );
        assert_eq!(
            RetentionTableCounts::new(3, 5),
            read_back.skipped_nonconforming
        );

        drop(reopened);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention policy primitive#Retention Policy Primitive Test Specs#Corrupted Audit Value]]
    #[test]
    #[serial]
    fn corrupted_audit_value_reads_as_absent_and_does_not_block_a_write() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");

        storage
            .set_setting(RETENTION_LAST_RUN_KEY, "{\"status\": truncated")
            .expect("write corrupted audit value");
        assert_eq!(
            None,
            storage
                .get_retention_policy()
                .expect("read policy")
                .last_run,
            "an unparseable audit value must read as absent"
        );

        let record = RetentionAuditRecord::new(RetentionRunStatus::Skipped, anchor())
            .with_reason("retention disabled");
        storage
            .write_retention_audit_record(&record)
            .expect("a corrupted value must not block a subsequent write");
        assert_eq!(
            Some(record),
            storage
                .get_retention_policy()
                .expect("read policy")
                .last_run
        );

        drop(storage);
        drop(fixture);
    }

    #[test]
    fn cutoff_derivation_is_byte_comparable_and_floored() {
        let cutoff = derive_retention_cutoff(anchor(), 90).expect("derive cutoff");
        assert_eq!("2026-04-02T00:00:00.000Z", cutoff);
        assert!(is_conforming_timestamp(&cutoff));
        assert_eq!(
            Err(RetentionPolicyError::UnsupportedWindow { window_days: 7 }),
            derive_retention_cutoff(anchor(), 7)
        );
    }

    #[test]
    fn stored_values_outside_the_grammar_degrade_to_never() {
        assert_eq!(Some(90), parse_window_days_setting("90"));
        assert_eq!(None, parse_window_days_setting(RETENTION_NEVER));
        assert_eq!(None, parse_window_days_setting("7"));
        assert_eq!(None, parse_window_days_setting("ninety"));
        assert_eq!(None, parse_watermark_setting("2026-04-02T00:00:00Z"));
        assert_eq!(
            Some("2026-04-02T00:00:00.000Z".to_string()),
            parse_watermark_setting("2026-04-02T00:00:00.000Z")
        );
    }

    #[test]
    fn a_partial_record_without_an_error_reason_is_refused() {
        let record = RetentionAuditRecord::new(RetentionRunStatus::Partial, anchor());
        assert_eq!(
            Err(RetentionPolicyError::PartialWithoutErrorReason),
            record.to_setting_value()
        );
    }

    #[test]
    fn a_malformed_cutoff_cannot_advance_the_watermark() {
        assert_eq!(
            Err(RetentionPolicyError::MalformedCutoff {
                cutoff: "2026-04-02T00:00:00Z".to_string()
            }),
            advanced_watermark(None, "2026-04-02T00:00:00Z")
        );
        // A stored watermark that cannot be ordered is discarded rather than
        // compared, so the conforming cutoff wins.
        assert_eq!(
            "2026-04-02T00:00:00.000Z",
            advanced_watermark(Some("whenever"), "2026-04-02T00:00:00.000Z")
                .expect("advance over a non-conforming watermark")
        );
    }
}
