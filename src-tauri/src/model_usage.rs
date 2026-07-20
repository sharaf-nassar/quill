//! Provider-neutral model transcript analytics.
//!
//! This module owns transcript parsing, source reconciliation, resumable
//! backfills, and post-commit analytics update notifications.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use serde_json::Value;
use tauri::Emitter;

use crate::integrations::IntegrationProvider;
use crate::models::{
    ModelAnalyticsUpdatedEvent, ModelBackfillDiagnostic, ModelBackfillState, ModelBackfillStatus,
};
use crate::sessions::{
    DiscoveredRetainedJsonlSource, ProviderRootEnumerationOutcome, ProviderSourceRoot,
    RetainedJsonlSourceLayoutHint,
};
use crate::storage::{
    ModelSourceChange, ModelSourceFastFingerprint, ModelSourceFingerprint,
    ModelSourceReplacementOutcome, Storage, StoredModelSource, classify_model_source_change,
    model_source_fast_fingerprint,
};

const MODEL_ID_MAX_SCALARS: usize = 256;
const TOKEN_COUNT_MAX: i64 = 100_000_000;
const DIAGNOSTIC_MAX_SCALARS: usize = 240;
const SOURCE_RECORD_KEY_VERSION: &str = "v1";
const RETAINED_SOURCE_COMMIT_BATCH_SIZE: usize = 32;

/// Emitted after model analytics changes have been committed.
pub const MODEL_ANALYTICS_UPDATED_EVENT: &str = "model-analytics-updated";

/// Provider-neutral metadata for one locally discovered transcript source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizedSource {
    pub provider: IntegrationProvider,
    pub source_root_key: String,
    pub source_key: String,
    pub path: PathBuf,
    pub layout_hint: RetainedJsonlSourceLayoutHint,
    pub source_session_id: Option<String>,
    pub analytics_session_id: Option<String>,
    pub chain_id: Option<String>,
    pub parent_chain_id: Option<String>,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub hostname: Option<String>,
    pub first_activity_at_ms: Option<i64>,
    pub last_activity_at_ms: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub size_bytes: Option<i64>,
    pub content_sha256: Option<String>,
    pub last_error: Option<ModelUsageDiagnostic>,
    pub suppressed_sha256: Option<String>,
    pub suppressed_at_ms: Option<i64>,
    pub seen_generation: i64,
    pub processing_status: SourceProcessingStatus,
    pub observation_count: i64,
    pub last_attempt_at_ms: Option<i64>,
    pub last_success_at_ms: Option<i64>,
}

/// One provider-neutral turn or token observation parsed from a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizedObservation {
    metadata: NormalizedObservationMetadata,
    model_attribution: attribution::ModelAttribution,
    token_attribution: attribution::TokenAttribution,
}

/// Observation fields that cannot contradict model or token attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizedObservationMetadata {
    pub provider: IntegrationProvider,
    pub source_key: String,
    pub source_record_key: String,
    pub source_ordinal: u64,
    pub kind: ObservationKind,
    pub source_session_id: String,
    /// Updated only through `ProviderAdapterParseResult::resolve_analytics_root`.
    pub analytics_session_id: String,
    pub chain_id: String,
    pub parent_chain_id: Option<String>,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub turn_id: Option<String>,
    pub observed_at_ms: i64,
    pub cwd: Option<PathBuf>,
    pub hostname: Option<String>,
}

impl NormalizedObservation {
    fn new(
        metadata: NormalizedObservationMetadata,
        model_attribution: attribution::ModelAttribution,
        token_attribution: attribution::TokenAttribution,
    ) -> Self {
        Self {
            metadata,
            model_attribution,
            token_attribution,
        }
    }

    pub(crate) fn metadata(&self) -> &NormalizedObservationMetadata {
        &self.metadata
    }

    pub(crate) fn raw_model_id(&self) -> Option<&str> {
        self.model_attribution.raw_model_id()
    }

    pub(crate) fn model_evidence(&self) -> ModelEvidence {
        self.model_attribution.evidence()
    }

    pub(crate) fn input_tokens(&self) -> Option<i64> {
        self.token_attribution.dimensions().input_tokens()
    }

    pub(crate) fn output_tokens(&self) -> Option<i64> {
        self.token_attribution.dimensions().output_tokens()
    }

    pub(crate) fn cache_creation_tokens(&self) -> Option<i64> {
        self.token_attribution.dimensions().cache_creation_tokens()
    }

    pub(crate) fn cache_read_tokens(&self) -> Option<i64> {
        self.token_attribution.dimensions().cache_read_tokens()
    }

    pub(crate) fn token_evidence(&self) -> TokenEvidence {
        self.token_attribution.evidence()
    }
}

/// Persisted observation category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationKind {
    Turn,
    Token,
}

impl ObservationKind {
    const TURN: &'static str = "turn";
    const TOKEN: &'static str = "token";

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => Self::TURN,
            Self::Token => Self::TOKEN,
        }
    }
}

/// Persisted model-attribution evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelEvidence {
    Explicit,
    Missing,
    Invalid,
}

impl ModelEvidence {
    const EXPLICIT: &'static str = "explicit";
    const MISSING: &'static str = "missing";
    const INVALID: &'static str = "invalid";

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => Self::EXPLICIT,
            Self::Missing => Self::MISSING,
            Self::Invalid => Self::INVALID,
        }
    }
}

/// Persisted token-attribution evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenEvidence {
    Direct,
    CumulativeDelta,
    Unavailable,
}

/// Persisted lifecycle state for one discovered transcript source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceProcessingStatus {
    Pending,
    Ok,
    Stale,
    Failed,
    Suppressed,
}

impl SourceProcessingStatus {
    const PENDING: &'static str = "pending";
    const OK: &'static str = "ok";
    const STALE: &'static str = "stale";
    const FAILED: &'static str = "failed";
    const SUPPRESSED: &'static str = "suppressed";

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => Self::PENDING,
            Self::Ok => Self::OK,
            Self::Stale => Self::STALE,
            Self::Failed => Self::FAILED,
            Self::Suppressed => Self::SUPPRESSED,
        }
    }
}

impl TokenEvidence {
    const DIRECT: &'static str = "direct";
    const CUMULATIVE_DELTA: &'static str = "cumulative_delta";
    const UNAVAILABLE: &'static str = "unavailable";

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => Self::DIRECT,
            Self::CumulativeDelta => Self::CUMULATIVE_DELTA,
            Self::Unavailable => Self::UNAVAILABLE,
        }
    }
}

/// Error returned when reading a canonical enum value from persistence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParsePersistedValueError {
    value_kind: &'static str,
}

impl ParsePersistedValueError {
    const fn new(value_kind: &'static str) -> Self {
        Self { value_kind }
    }
}

impl fmt::Display for ParsePersistedValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid persisted {}", self.value_kind)
    }
}

impl std::error::Error for ParsePersistedValueError {}

impl TryFrom<&str> for ObservationKind {
    type Error = ParsePersistedValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::TURN => Ok(Self::Turn),
            Self::TOKEN => Ok(Self::Token),
            _ => Err(ParsePersistedValueError::new("observation kind")),
        }
    }
}

impl TryFrom<&str> for ModelEvidence {
    type Error = ParsePersistedValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::EXPLICIT => Ok(Self::Explicit),
            Self::MISSING => Ok(Self::Missing),
            Self::INVALID => Ok(Self::Invalid),
            _ => Err(ParsePersistedValueError::new("model evidence")),
        }
    }
}

impl TryFrom<&str> for TokenEvidence {
    type Error = ParsePersistedValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::DIRECT => Ok(Self::Direct),
            Self::CUMULATIVE_DELTA => Ok(Self::CumulativeDelta),
            Self::UNAVAILABLE => Ok(Self::Unavailable),
            _ => Err(ParsePersistedValueError::new("token evidence")),
        }
    }
}

impl TryFrom<&str> for SourceProcessingStatus {
    type Error = ParsePersistedValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::PENDING => Ok(Self::Pending),
            Self::OK => Ok(Self::Ok),
            Self::STALE => Ok(Self::Stale),
            Self::FAILED => Ok(Self::Failed),
            Self::SUPPRESSED => Ok(Self::Suppressed),
            _ => Err(ParsePersistedValueError::new("source processing status")),
        }
    }
}

/// Why a source model identifier could not be retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelIdValidationError {
    Empty,
    TooLong,
    ControlCharacter,
}

impl fmt::Display for ModelIdValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "model identifier is empty after trimming",
            Self::TooLong => "model identifier exceeds 256 Unicode scalar values",
            Self::ControlCharacter => "model identifier contains a control character",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ModelIdValidationError {}

/// Validate an opaque model identifier without catalog or family semantics.
pub(crate) fn validate_model_id(raw: &str) -> Result<String, ModelIdValidationError> {
    let trimmed = raw.trim();
    let scalar_count = trimmed.chars().count();

    if scalar_count == 0 {
        return Err(ModelIdValidationError::Empty);
    }
    if scalar_count > MODEL_ID_MAX_SCALARS {
        return Err(ModelIdValidationError::TooLong);
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ModelIdValidationError::ControlCharacter);
    }

    Ok(trimmed.to_owned())
}

/// Independently validated nullable token dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ValidatedTokenDimensions {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
}

impl ValidatedTokenDimensions {
    pub(crate) const fn input_tokens(self) -> Option<i64> {
        self.input_tokens
    }

    pub(crate) const fn output_tokens(self) -> Option<i64> {
        self.output_tokens
    }

    pub(crate) const fn cache_creation_tokens(self) -> Option<i64> {
        self.cache_creation_tokens
    }

    pub(crate) const fn cache_read_tokens(self) -> Option<i64> {
        self.cache_read_tokens
    }

    const fn has_any(self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_creation_tokens.is_some()
            || self.cache_read_tokens.is_some()
    }

    fn values_are_valid(self) -> bool {
        [
            self.input_tokens,
            self.output_tokens,
            self.cache_creation_tokens,
            self.cache_read_tokens,
        ]
        .into_iter()
        .flatten()
        .all(|value| (0..=TOKEN_COUNT_MAX).contains(&value))
    }
}

/// Validate four JSON token fields without coercing other JSON types.
pub(crate) fn validate_token_dimensions(
    input_tokens: Option<&Value>,
    output_tokens: Option<&Value>,
    cache_creation_tokens: Option<&Value>,
    cache_read_tokens: Option<&Value>,
) -> ValidatedTokenDimensions {
    ValidatedTokenDimensions {
        input_tokens: validate_token_dimension(input_tokens),
        output_tokens: validate_token_dimension(output_tokens),
        cache_creation_tokens: validate_token_dimension(cache_creation_tokens),
        cache_read_tokens: validate_token_dimension(cache_read_tokens),
    }
}

fn validate_token_dimension(value: Option<&Value>) -> Option<i64> {
    let value = match value {
        Some(Value::Number(number)) => number.as_i64()?,
        Some(Value::Null) | None => return None,
        Some(Value::Bool(_) | Value::String(_) | Value::Array(_) | Value::Object(_)) => {
            return None;
        }
    };

    (0..=TOKEN_COUNT_MAX).contains(&value).then_some(value)
}

fn invalid_token_dimension_count(values: [Option<&Value>; 4]) -> u64 {
    values
        .into_iter()
        .filter(|value| match value {
            None | Some(Value::Null) => false,
            Some(value) => validate_token_dimension(Some(value)).is_none(),
        })
        .count()
        .try_into()
        .unwrap_or(u64::MAX)
}

mod attribution {
    use super::{
        ModelEvidence, ModelIdValidationError, TOKEN_COUNT_MAX, TokenEvidence,
        ValidatedTokenDimensions, validate_model_id,
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct ModelAttribution(ModelAttributionValue);

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ModelAttributionValue {
        Explicit(String),
        Missing,
        Invalid,
    }

    impl ModelAttribution {
        pub(super) fn explicit(raw: &str) -> Result<Self, ModelIdValidationError> {
            validate_model_id(raw).map(|model_id| Self(ModelAttributionValue::Explicit(model_id)))
        }

        pub(super) const fn missing() -> Self {
            Self(ModelAttributionValue::Missing)
        }

        pub(super) const fn invalid() -> Self {
            Self(ModelAttributionValue::Invalid)
        }

        pub(super) fn raw_model_id(&self) -> Option<&str> {
            match &self.0 {
                ModelAttributionValue::Explicit(model_id) => Some(model_id),
                ModelAttributionValue::Missing | ModelAttributionValue::Invalid => None,
            }
        }

        pub(super) const fn evidence(&self) -> ModelEvidence {
            match &self.0 {
                ModelAttributionValue::Explicit(_) => ModelEvidence::Explicit,
                ModelAttributionValue::Missing => ModelEvidence::Missing,
                ModelAttributionValue::Invalid => ModelEvidence::Invalid,
            }
        }
    }

    /// Why direct or cumulative token evidence could not be constructed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum TokenAttributionError {
        NoTokenData,
        OutOfRange,
    }

    impl std::fmt::Display for TokenAttributionError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::NoTokenData => formatter.write_str("token evidence has no token data"),
                Self::OutOfRange => write!(
                    formatter,
                    "token evidence is outside the inclusive 0..={TOKEN_COUNT_MAX} range"
                ),
            }
        }
    }

    impl std::error::Error for TokenAttributionError {}

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct TokenAttribution(TokenAttributionValue);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TokenAttributionValue {
        Direct(ValidatedTokenDimensions),
        CumulativeDelta(ValidatedTokenDimensions),
        Unavailable,
    }

    impl TokenAttribution {
        pub(super) fn direct(
            dimensions: ValidatedTokenDimensions,
        ) -> Result<Self, TokenAttributionError> {
            Self::with_dimensions(dimensions, TokenAttributionValue::Direct)
        }

        pub(super) fn cumulative_delta(
            dimensions: ValidatedTokenDimensions,
        ) -> Result<Self, TokenAttributionError> {
            Self::with_dimensions(dimensions, TokenAttributionValue::CumulativeDelta)
        }

        pub(super) const fn unavailable() -> Self {
            Self(TokenAttributionValue::Unavailable)
        }

        fn with_dimensions(
            dimensions: ValidatedTokenDimensions,
            constructor: fn(ValidatedTokenDimensions) -> TokenAttributionValue,
        ) -> Result<Self, TokenAttributionError> {
            if !dimensions.values_are_valid() {
                return Err(TokenAttributionError::OutOfRange);
            }
            if !dimensions.has_any() {
                return Err(TokenAttributionError::NoTokenData);
            }

            Ok(Self(constructor(dimensions)))
        }

        pub(super) const fn dimensions(&self) -> ValidatedTokenDimensions {
            match self.0 {
                TokenAttributionValue::Direct(dimensions)
                | TokenAttributionValue::CumulativeDelta(dimensions) => dimensions,
                TokenAttributionValue::Unavailable => ValidatedTokenDimensions {
                    input_tokens: None,
                    output_tokens: None,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            }
        }

        pub(super) const fn evidence(&self) -> TokenEvidence {
            match self.0 {
                TokenAttributionValue::Direct(_) => TokenEvidence::Direct,
                TokenAttributionValue::CumulativeDelta(_) => TokenEvidence::CumulativeDelta,
                TokenAttributionValue::Unavailable => TokenEvidence::Unavailable,
            }
        }
    }
}

/// Provider-native source record forms understood by transcript adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRecordShape {
    ClaudeAssistant,
    CodexTurnContext,
    CodexTokenCount,
}

impl SourceRecordShape {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeAssistant => "claude_assistant",
            Self::CodexTurnContext => "codex_turn_context",
            Self::CodexTokenCount => "codex_token_count",
        }
    }
}

/// Build a deterministic key unique to an observation within one source.
pub(crate) fn stable_source_record_key(
    record_shape: SourceRecordShape,
    source_ordinal: u64,
    observation_index: u32,
) -> String {
    format!(
        "{SOURCE_RECORD_KEY_VERSION}:{}:{source_ordinal}:{observation_index}",
        record_shape.as_str()
    )
}

/// Safe diagnostic categories emitted while inventorying or parsing sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelUsageDiagnosticKind {
    SourceReadFailed,
    SourceParseFailed,
    RecordSkipped,
    InvalidModelValue,
    InvalidTokenDimension,
    InvalidTokenRelationship,
    ContextualMetadataConflict,
    CumulativeTokenReset,
    LastTokenUsageUnavailable,
}

impl ModelUsageDiagnosticKind {
    const fn message(self) -> &'static str {
        match self {
            Self::SourceReadFailed => "A model history source could not be read.",
            Self::SourceParseFailed => "A model history source could not be parsed.",
            Self::RecordSkipped => "A model history record was invalid and was skipped.",
            Self::InvalidModelValue => {
                "A model value was invalid and was retained as an attribution gap."
            }
            Self::InvalidTokenDimension => {
                "A token dimension was invalid and was omitted from this observation."
            }
            Self::InvalidTokenRelationship => {
                "Cumulative input dimensions had an invalid subset relationship; unsafe usage was omitted."
            }
            Self::ContextualMetadataConflict => {
                "A later working-directory value conflicted and was omitted."
            }
            Self::CumulativeTokenReset => {
                "A cumulative token counter decreased and its baseline was reset."
            }
            Self::LastTokenUsageUnavailable => {
                "A last-turn token count could not be uniquely attributed and was omitted."
            }
        }
    }
}

/// Whitespace-normalized, scalar-bounded diagnostic safe for persistence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelUsageDiagnostic(Cow<'static, str>);

impl ModelUsageDiagnostic {
    pub fn new(kind: ModelUsageDiagnosticKind) -> Self {
        let message = kind.message();
        let value = if is_normalized_bounded_diagnostic(message) {
            Cow::Borrowed(message)
        } else {
            bound_diagnostic(message)
        };
        Self(value)
    }

    /// Bound a generic message that has already been stripped of raw details.
    ///
    /// Callers must not include source contents, paths, or underlying errors.
    pub fn from_user_safe_message(message: &str) -> Self {
        Self(bound_diagnostic(message))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for ModelUsageDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.as_ref())
    }
}

fn bound_diagnostic(message: &str) -> Cow<'static, str> {
    const FALLBACK: &str = "Model history processing encountered an error.";

    let mut normalized = String::with_capacity(message.len().min(DIAGNOSTIC_MAX_SCALARS * 4));
    let mut scalar_count = 0;
    let mut pending_space = false;

    for character in message.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !normalized.is_empty();
            continue;
        }

        if pending_space {
            if push_bounded_diagnostic_char(&mut normalized, &mut scalar_count, ' ') {
                return Cow::Owned(normalized);
            }
            pending_space = false;
        }

        if push_bounded_diagnostic_char(&mut normalized, &mut scalar_count, character) {
            return Cow::Owned(normalized);
        }
    }

    if normalized.is_empty() {
        Cow::Borrowed(FALLBACK)
    } else {
        Cow::Owned(normalized)
    }
}

fn is_normalized_bounded_diagnostic(message: &str) -> bool {
    if message.is_empty() || message.chars().count() > DIAGNOSTIC_MAX_SCALARS {
        return false;
    }

    let mut previous_was_space = true;
    for character in message.chars() {
        if character.is_control() {
            return false;
        }
        if character.is_whitespace() {
            if character != ' ' || previous_was_space {
                return false;
            }
            previous_was_space = true;
        } else {
            previous_was_space = false;
        }
    }

    !previous_was_space
}

fn push_bounded_diagnostic_char(
    output: &mut String,
    scalar_count: &mut usize,
    character: char,
) -> bool {
    if *scalar_count < DIAGNOSTIC_MAX_SCALARS {
        output.push(character);
        *scalar_count += 1;
        return false;
    }

    output.pop();
    output.push('\u{2026}');
    true
}

const ADAPTER_MAX_DIAGNOSTICS: usize = 64;

/// Trusted source context supplied to the Claude transcript adapter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClaudeAdapterContext<'a> {
    pub source_key: &'a str,
    pub layout_hint: &'a RetainedJsonlSourceLayoutHint,
    pub hostname: Option<&'a str>,
}

/// Trusted source context supplied to the Codex transcript adapter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CodexAdapterContext<'a> {
    pub source_key: &'a str,
    pub layout_hint: &'a RetainedJsonlSourceLayoutHint,
    pub hostname: Option<&'a str>,
}

/// Adapter-owned source identity resolved from the first valid provider record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderNativeSourceMetadata {
    pub provider: IntegrationProvider,
    pub source_key: String,
    pub source_session_id: String,
    /// Updated only through `ProviderAdapterParseResult::resolve_analytics_root`.
    analytics_session_id: String,
    pub chain_id: String,
    pub parent_chain_id: Option<String>,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub hostname: Option<String>,
    pub first_activity_at_ms: i64,
    pub last_activity_at_ms: i64,
}

impl ProviderNativeSourceMetadata {
    pub(crate) fn analytics_session_id(&self) -> &str {
        &self.analytics_session_id
    }
}

/// Native identity evidence discovered within one provider transcript source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderNativeIdentityState {
    Absent,
    Valid(Box<ProviderNativeSourceMetadata>),
    Conflicted,
}

/// Why an analytics root could not be stamped across one adapter result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnalyticsRootResolutionError {
    EmptyResolvedRoot,
    NativeIdentityAbsent,
    NativeIdentityConflicted,
    ObservationIdentityMismatch,
    ExistingAnalyticsIdentityMismatch,
}

impl fmt::Display for AnalyticsRootResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyResolvedRoot => "resolved analytics root is empty",
            Self::NativeIdentityAbsent => "native source identity is absent",
            Self::NativeIdentityConflicted => "native source identity is conflicted",
            Self::ObservationIdentityMismatch => {
                "observation identity does not match native source identity"
            }
            Self::ExistingAnalyticsIdentityMismatch => {
                "analytics identity copies are already inconsistent"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for AnalyticsRootResolutionError {}

/// Bounded parse counters retained even when detailed diagnostics are capped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderAdapterParseCounts {
    pub lines_seen: u64,
    pub assistant_records_seen: u64,
    pub session_metadata_records_seen: u64,
    pub turn_context_records_seen: u64,
    pub token_count_records_seen: u64,
    pub ignored_records: u64,
    pub malformed_json_records: u64,
    pub unsupported_shape_records: u64,
    pub invalid_timestamp_records: u64,
    pub invalid_identity_records: u64,
    pub invalid_model_values: u64,
    pub invalid_token_dimension_values: u64,
    pub invalid_token_relationship_records: u64,
    pub native_metadata_conflict_records: u64,
    pub contextual_metadata_conflict_records: u64,
    pub layout_hint_conflict_records: u64,
    pub cumulative_reset_dimensions: u64,
    pub last_token_usage_only_records: u64,
    pub observations_emitted: u64,
    pub diagnostics_dropped: u64,
}

/// Safe record-level diagnostic with no transcript, path, or raw error data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderAdapterDiagnostic {
    pub source_ordinal: u64,
    pub diagnostic: ModelUsageDiagnostic,
}

/// Complete, non-mutating output from one provider transcript adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderAdapterParseResult {
    pub native_identity: ProviderNativeIdentityState,
    pub observations: Vec<NormalizedObservation>,
    pub diagnostics: Vec<ProviderAdapterDiagnostic>,
    pub counts: ProviderAdapterParseCounts,
}

impl ProviderAdapterParseResult {
    fn valid_native_source(&self) -> Option<&ProviderNativeSourceMetadata> {
        match &self.native_identity {
            ProviderNativeIdentityState::Valid(native_source) => Some(native_source),
            ProviderNativeIdentityState::Absent | ProviderNativeIdentityState::Conflicted => None,
        }
    }

    fn valid_native_source_mut(&mut self) -> Option<&mut ProviderNativeSourceMetadata> {
        match &mut self.native_identity {
            ProviderNativeIdentityState::Valid(native_source) => Some(native_source),
            ProviderNativeIdentityState::Absent | ProviderNativeIdentityState::Conflicted => None,
        }
    }

    /// Atomically stamp a coordinator-resolved root after validating every
    /// duplicated native identity field across this adapter result.
    pub(crate) fn resolve_analytics_root(
        &mut self,
        resolved_analytics_session_id: &str,
    ) -> Result<(), AnalyticsRootResolutionError> {
        if resolved_analytics_session_id.trim().is_empty() {
            return Err(AnalyticsRootResolutionError::EmptyResolvedRoot);
        }

        let native_source = match &self.native_identity {
            ProviderNativeIdentityState::Absent => {
                return Err(AnalyticsRootResolutionError::NativeIdentityAbsent);
            }
            ProviderNativeIdentityState::Conflicted => {
                return Err(AnalyticsRootResolutionError::NativeIdentityConflicted);
            }
            ProviderNativeIdentityState::Valid(native_source) => native_source,
        };
        for observation in &self.observations {
            let metadata = &observation.metadata;
            if metadata.provider != native_source.provider
                || metadata.source_key != native_source.source_key
                || metadata.source_session_id != native_source.source_session_id
                || metadata.chain_id != native_source.chain_id
                || metadata.parent_chain_id != native_source.parent_chain_id
                || metadata.is_sidechain != native_source.is_sidechain
                || metadata.agent_id != native_source.agent_id
            {
                return Err(AnalyticsRootResolutionError::ObservationIdentityMismatch);
            }
            if metadata.analytics_session_id != native_source.analytics_session_id() {
                return Err(AnalyticsRootResolutionError::ExistingAnalyticsIdentityMismatch);
            }
        }

        let resolved_analytics_session_id = resolved_analytics_session_id.to_owned();
        let native_source = self
            .valid_native_source_mut()
            .ok_or(AnalyticsRootResolutionError::NativeIdentityAbsent)?;
        native_source.analytics_session_id = resolved_analytics_session_id.clone();
        for observation in &mut self.observations {
            observation.metadata.analytics_session_id = resolved_analytics_session_id.clone();
        }
        Ok(())
    }
}

/// Parse complete Claude JSONL content without allowing one record to abort later records.
pub(crate) fn parse_claude_model_usage_jsonl(
    contents: &str,
    context: ClaudeAdapterContext<'_>,
) -> ProviderAdapterParseResult {
    let mut result = ProviderAdapterParseResult {
        native_identity: ProviderNativeIdentityState::Absent,
        observations: Vec::new(),
        diagnostics: Vec::new(),
        counts: ProviderAdapterParseCounts::default(),
    };

    for (line_index, line) in contents.lines().enumerate() {
        result.counts.lines_seen = result.counts.lines_seen.saturating_add(1);
        let source_ordinal = u64::try_from(line_index).unwrap_or(u64::MAX);

        if line.trim().is_empty() {
            result.counts.ignored_records = result.counts.ignored_records.saturating_add(1);
            continue;
        }

        let record = match serde_json::from_str::<Value>(line) {
            Ok(record) => record,
            Err(_) => {
                result.counts.malformed_json_records =
                    result.counts.malformed_json_records.saturating_add(1);
                push_claude_adapter_diagnostic(&mut result, source_ordinal);
                continue;
            }
        };
        let Some(record) = record.as_object() else {
            result.counts.unsupported_shape_records =
                result.counts.unsupported_shape_records.saturating_add(1);
            push_claude_adapter_diagnostic(&mut result, source_ordinal);
            continue;
        };

        match record.get("type") {
            Some(Value::String(record_type)) if record_type == "assistant" => {}
            Some(Value::String(_)) => {
                result.counts.ignored_records = result.counts.ignored_records.saturating_add(1);
                continue;
            }
            _ => {
                result.counts.unsupported_shape_records =
                    result.counts.unsupported_shape_records.saturating_add(1);
                push_claude_adapter_diagnostic(&mut result, source_ordinal);
                continue;
            }
        }
        result.counts.assistant_records_seen =
            result.counts.assistant_records_seen.saturating_add(1);

        let Some(message) = record.get("message").and_then(Value::as_object) else {
            result.counts.unsupported_shape_records =
                result.counts.unsupported_shape_records.saturating_add(1);
            push_claude_adapter_diagnostic(&mut result, source_ordinal);
            continue;
        };

        let Some(observed_at_ms) = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
            .map(|timestamp| timestamp.timestamp_millis())
            .filter(|timestamp| *timestamp >= 0)
        else {
            result.counts.invalid_timestamp_records =
                result.counts.invalid_timestamp_records.saturating_add(1);
            push_claude_adapter_diagnostic(&mut result, source_ordinal);
            continue;
        };

        let Some(source_session_id) = nonempty_record_string(record.get("sessionId")) else {
            result.counts.invalid_identity_records =
                result.counts.invalid_identity_records.saturating_add(1);
            push_claude_adapter_diagnostic(&mut result, source_ordinal);
            continue;
        };

        let native_sidechain = record.get("isSidechain").and_then(Value::as_bool) == Some(true);
        let native_agent_id = nonempty_record_string(record.get("agentId"));
        let (chain_id, parent_chain_id, is_sidechain, agent_id) = if native_sidechain {
            let Some(agent_id) = native_agent_id else {
                result.counts.invalid_identity_records =
                    result.counts.invalid_identity_records.saturating_add(1);
                push_claude_adapter_diagnostic(&mut result, source_ordinal);
                continue;
            };
            (
                agent_id.clone(),
                Some(source_session_id.clone()),
                true,
                Some(agent_id),
            )
        } else {
            (source_session_id.clone(), None, false, None)
        };

        let cwd = nonempty_record_string(record.get("cwd")).map(PathBuf::from);
        if !accept_claude_native_source(
            &mut result,
            ProviderNativeSourceMetadata {
                provider: IntegrationProvider::Claude,
                source_key: context.source_key.to_owned(),
                source_session_id: source_session_id.clone(),
                analytics_session_id: source_session_id.clone(),
                chain_id: chain_id.clone(),
                parent_chain_id: parent_chain_id.clone(),
                is_sidechain,
                agent_id: agent_id.clone(),
                cwd: cwd.clone(),
                hostname: context.hostname.map(str::to_owned),
                first_activity_at_ms: observed_at_ms,
                last_activity_at_ms: observed_at_ms,
            },
            source_ordinal,
        ) {
            continue;
        }

        let hint_is_sidechain = matches!(
            context.layout_hint,
            RetainedJsonlSourceLayoutHint::ClaudeSubagent { .. }
        );
        if hint_is_sidechain != is_sidechain {
            result.counts.layout_hint_conflict_records =
                result.counts.layout_hint_conflict_records.saturating_add(1);
        }

        let turn_id = nonempty_record_string(record.get("uuid"));
        let model_attribution = match message.get("model") {
            None => attribution::ModelAttribution::missing(),
            Some(Value::String(model_id)) => {
                match attribution::ModelAttribution::explicit(model_id) {
                    Ok(model_attribution) => model_attribution,
                    Err(_) => {
                        record_invalid_model(&mut result, source_ordinal);
                        attribution::ModelAttribution::invalid()
                    }
                }
            }
            Some(_) => {
                record_invalid_model(&mut result, source_ordinal);
                attribution::ModelAttribution::invalid()
            }
        };
        let usage = message.get("usage").and_then(Value::as_object);
        let input_tokens = usage.and_then(|usage| usage.get("input_tokens"));
        let output_tokens = usage.and_then(|usage| usage.get("output_tokens"));
        let cache_creation_tokens =
            usage.and_then(|usage| usage.get("cache_creation_input_tokens"));
        let cache_read_tokens = usage.and_then(|usage| usage.get("cache_read_input_tokens"));
        let invalid_token_dimensions = invalid_token_dimension_count([
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        ]);
        if invalid_token_dimensions > 0 {
            result.counts.invalid_token_dimension_values = result
                .counts
                .invalid_token_dimension_values
                .saturating_add(invalid_token_dimensions);
            push_adapter_diagnostic(
                &mut result,
                source_ordinal,
                ModelUsageDiagnosticKind::InvalidTokenDimension,
            );
        }
        let token_dimensions = validate_token_dimensions(
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        );
        let token_attribution = attribution::TokenAttribution::direct(token_dimensions)
            .unwrap_or_else(|_| attribution::TokenAttribution::unavailable());

        let metadata = NormalizedObservationMetadata {
            provider: IntegrationProvider::Claude,
            source_key: context.source_key.to_owned(),
            source_record_key: stable_source_record_key(
                SourceRecordShape::ClaudeAssistant,
                source_ordinal,
                0,
            ),
            source_ordinal,
            kind: ObservationKind::Turn,
            source_session_id: source_session_id.clone(),
            analytics_session_id: source_session_id.clone(),
            chain_id: chain_id.clone(),
            parent_chain_id: parent_chain_id.clone(),
            is_sidechain,
            agent_id: agent_id.clone(),
            turn_id,
            observed_at_ms,
            cwd: cwd.clone(),
            hostname: context.hostname.map(str::to_owned),
        };
        result.observations.push(NormalizedObservation::new(
            metadata,
            model_attribution,
            token_attribution,
        ));
        result.counts.observations_emitted = result.counts.observations_emitted.saturating_add(1);
    }

    result
}

fn nonempty_record_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn push_claude_adapter_diagnostic(result: &mut ProviderAdapterParseResult, source_ordinal: u64) {
    push_adapter_diagnostic(
        result,
        source_ordinal,
        ModelUsageDiagnosticKind::RecordSkipped,
    );
}

fn push_adapter_diagnostic(
    result: &mut ProviderAdapterParseResult,
    source_ordinal: u64,
    kind: ModelUsageDiagnosticKind,
) {
    if result.diagnostics.len() < ADAPTER_MAX_DIAGNOSTICS {
        result.diagnostics.push(ProviderAdapterDiagnostic {
            source_ordinal,
            diagnostic: ModelUsageDiagnostic::new(kind),
        });
    } else {
        result.counts.diagnostics_dropped = result.counts.diagnostics_dropped.saturating_add(1);
    }
}

fn record_invalid_model(result: &mut ProviderAdapterParseResult, source_ordinal: u64) {
    result.counts.invalid_model_values = result.counts.invalid_model_values.saturating_add(1);
    push_adapter_diagnostic(
        result,
        source_ordinal,
        ModelUsageDiagnosticKind::InvalidModelValue,
    );
}

fn accept_claude_native_source(
    result: &mut ProviderAdapterParseResult,
    record_metadata: ProviderNativeSourceMetadata,
    source_ordinal: u64,
) -> bool {
    let native_source = match &mut result.native_identity {
        ProviderNativeIdentityState::Absent => {
            result.native_identity = ProviderNativeIdentityState::Valid(Box::new(record_metadata));
            return true;
        }
        ProviderNativeIdentityState::Valid(native_source) => native_source,
        ProviderNativeIdentityState::Conflicted => return false,
    };

    let identity_conflicts = native_source.source_session_id != record_metadata.source_session_id
        || native_source.analytics_session_id() != record_metadata.analytics_session_id()
        || native_source.chain_id != record_metadata.chain_id
        || native_source.parent_chain_id != record_metadata.parent_chain_id
        || native_source.is_sidechain != record_metadata.is_sidechain
        || native_source.agent_id != record_metadata.agent_id;
    if identity_conflicts {
        result.counts.native_metadata_conflict_records = result
            .counts
            .native_metadata_conflict_records
            .saturating_add(1);
        push_claude_adapter_diagnostic(result, source_ordinal);
        return false;
    }

    native_source.first_activity_at_ms = native_source
        .first_activity_at_ms
        .min(record_metadata.first_activity_at_ms);
    native_source.last_activity_at_ms = native_source
        .last_activity_at_ms
        .max(record_metadata.last_activity_at_ms);
    if native_source.cwd.is_none() {
        native_source.cwd = record_metadata.cwd;
    }

    true
}

#[derive(Clone, Copy, Debug, Default)]
struct CodexCumulativeBaselines {
    raw_inclusive_input_tokens: Option<i64>,
    paired_inclusive_input_tokens: Option<i64>,
    inclusive_input_reset_since_anchor: bool,
    cache_read_reset_since_anchor: bool,
    output_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    pending_emitted_cache_read_tokens: i128,
}

#[derive(Clone, Copy, Debug, Default)]
struct CodexInputCacheDeltas {
    input_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    inclusive_input_reset: bool,
    cache_read_reset: bool,
}

/// Parse complete Codex JSONL content using provider-native metadata and
/// independent cumulative-token baselines.
pub(crate) fn parse_codex_model_usage_jsonl(
    contents: &str,
    context: CodexAdapterContext<'_>,
) -> ProviderAdapterParseResult {
    let mut result = ProviderAdapterParseResult {
        native_identity: ProviderNativeIdentityState::Absent,
        observations: Vec::new(),
        diagnostics: Vec::new(),
        counts: ProviderAdapterParseCounts::default(),
    };

    // Resolve session_meta in a complete first pass so an observation can be
    // attributed even when provider metadata appears later in the source.
    // Observation-bearing records are parsed once here and buffered for the
    // second pass so no line is deserialized twice.
    let mut deferred_records = Vec::<(u64, serde_json::Map<String, Value>)>::new();
    for (line_index, line) in contents.lines().enumerate() {
        result.counts.lines_seen = result.counts.lines_seen.saturating_add(1);
        let source_ordinal = u64::try_from(line_index).unwrap_or(u64::MAX);

        if line.trim().is_empty() {
            result.counts.ignored_records = result.counts.ignored_records.saturating_add(1);
            continue;
        }

        let record = match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(record)) => record,
            Ok(_) => {
                result.counts.unsupported_shape_records =
                    result.counts.unsupported_shape_records.saturating_add(1);
                push_adapter_diagnostic(
                    &mut result,
                    source_ordinal,
                    ModelUsageDiagnosticKind::RecordSkipped,
                );
                continue;
            }
            Err(_) => {
                result.counts.malformed_json_records =
                    result.counts.malformed_json_records.saturating_add(1);
                push_adapter_diagnostic(
                    &mut result,
                    source_ordinal,
                    ModelUsageDiagnosticKind::RecordSkipped,
                );
                continue;
            }
        };

        let Some(record_type) = record.get("type").and_then(Value::as_str) else {
            result.counts.unsupported_shape_records =
                result.counts.unsupported_shape_records.saturating_add(1);
            push_adapter_diagnostic(
                &mut result,
                source_ordinal,
                ModelUsageDiagnosticKind::RecordSkipped,
            );
            continue;
        };

        if record_type != "session_meta" {
            // Buffer observation records so the second pass can dispatch them
            // without re-parsing; no other record type has second-pass work.
            if matches!(record_type, "turn_context" | "event_msg") {
                deferred_records.push((source_ordinal, record));
            } else {
                result.counts.ignored_records = result.counts.ignored_records.saturating_add(1);
            }
            continue;
        }

        result.counts.session_metadata_records_seen = result
            .counts
            .session_metadata_records_seen
            .saturating_add(1);
        let Some(payload) = record.get("payload").and_then(Value::as_object) else {
            result.counts.unsupported_shape_records =
                result.counts.unsupported_shape_records.saturating_add(1);
            push_adapter_diagnostic(
                &mut result,
                source_ordinal,
                ModelUsageDiagnosticKind::RecordSkipped,
            );
            continue;
        };
        let Some(observed_at_ms) = codex_record_timestamp(&record) else {
            result.counts.invalid_timestamp_records =
                result.counts.invalid_timestamp_records.saturating_add(1);
            push_adapter_diagnostic(
                &mut result,
                source_ordinal,
                ModelUsageDiagnosticKind::RecordSkipped,
            );
            continue;
        };
        let Some(source_session_id) = nonempty_record_string(payload.get("id")) else {
            result.counts.invalid_identity_records =
                result.counts.invalid_identity_records.saturating_add(1);
            push_adapter_diagnostic(
                &mut result,
                source_ordinal,
                ModelUsageDiagnosticKind::RecordSkipped,
            );
            continue;
        };
        let parent_chain_id = match optional_nonempty_record_string(payload.get("parent_thread_id"))
        {
            Ok(parent_chain_id) => parent_chain_id,
            Err(()) => {
                result.counts.invalid_identity_records =
                    result.counts.invalid_identity_records.saturating_add(1);
                push_adapter_diagnostic(
                    &mut result,
                    source_ordinal,
                    ModelUsageDiagnosticKind::RecordSkipped,
                );
                continue;
            }
        };
        let cwd = nonempty_record_string(payload.get("cwd")).map(PathBuf::from);
        let is_sidechain = parent_chain_id.is_some();
        let record_metadata = ProviderNativeSourceMetadata {
            provider: IntegrationProvider::Codex,
            source_key: context.source_key.to_owned(),
            source_session_id: source_session_id.clone(),
            analytics_session_id: source_session_id.clone(),
            chain_id: source_session_id,
            parent_chain_id,
            is_sidechain,
            agent_id: None,
            cwd,
            hostname: context.hostname.map(str::to_owned),
            first_activity_at_ms: observed_at_ms,
            last_activity_at_ms: observed_at_ms,
        };

        if !accept_codex_native_source(&mut result, record_metadata, source_ordinal) {
            result.counts.native_metadata_conflict_records = result
                .counts
                .native_metadata_conflict_records
                .saturating_add(1);
            push_adapter_diagnostic(
                &mut result,
                source_ordinal,
                ModelUsageDiagnosticKind::RecordSkipped,
            );
        }
    }

    if matches!(
        &result.native_identity,
        ProviderNativeIdentityState::Valid(_)
    ) && !matches!(
        context.layout_hint,
        RetainedJsonlSourceLayoutHint::CodexTranscript
    ) {
        result.counts.layout_hint_conflict_records =
            result.counts.layout_hint_conflict_records.saturating_add(1);
    }

    // Dispatch observations from the records buffered in the first pass so each
    // line is deserialized only once. First-pass syntax and shape failures were
    // already counted and never entered the buffer.
    let mut baselines = CodexCumulativeBaselines::default();
    for (source_ordinal, record) in &deferred_records {
        match record.get("type").and_then(Value::as_str) {
            Some("turn_context") => {
                parse_codex_turn_context(record, *source_ordinal, context, &mut result)
            }
            Some("event_msg") => parse_codex_event_message(
                record,
                *source_ordinal,
                context,
                &mut baselines,
                &mut result,
            ),
            _ => {}
        }
    }

    result
}

fn accept_codex_native_source(
    result: &mut ProviderAdapterParseResult,
    record_metadata: ProviderNativeSourceMetadata,
    source_ordinal: u64,
) -> bool {
    match &result.native_identity {
        ProviderNativeIdentityState::Absent => {
            result.native_identity = ProviderNativeIdentityState::Valid(Box::new(record_metadata));
            return true;
        }
        ProviderNativeIdentityState::Conflicted => return false,
        ProviderNativeIdentityState::Valid(native_source)
            if native_source.source_session_id != record_metadata.source_session_id
                || native_source.parent_chain_id != record_metadata.parent_chain_id =>
        {
            result.native_identity = ProviderNativeIdentityState::Conflicted;
            return false;
        }
        ProviderNativeIdentityState::Valid(_) => {}
    }

    let cwd_conflicts = result.valid_native_source().is_some_and(|native_source| {
        native_source.cwd.is_some()
            && record_metadata.cwd.is_some()
            && native_source.cwd != record_metadata.cwd
    });
    if cwd_conflicts {
        // CWD is contextual rather than graph identity. Isolate the later
        // value while retaining the valid thread and its observations.
        result.counts.contextual_metadata_conflict_records = result
            .counts
            .contextual_metadata_conflict_records
            .saturating_add(1);
        push_adapter_diagnostic(
            result,
            source_ordinal,
            ModelUsageDiagnosticKind::ContextualMetadataConflict,
        );
    }

    let Some(native_source) = result.valid_native_source_mut() else {
        return false;
    };

    native_source.first_activity_at_ms = native_source
        .first_activity_at_ms
        .min(record_metadata.first_activity_at_ms);
    native_source.last_activity_at_ms = native_source
        .last_activity_at_ms
        .max(record_metadata.last_activity_at_ms);
    if native_source.cwd.is_none() {
        native_source.cwd = record_metadata.cwd;
    }

    true
}

fn parse_codex_turn_context(
    record: &serde_json::Map<String, Value>,
    source_ordinal: u64,
    context: CodexAdapterContext<'_>,
    result: &mut ProviderAdapterParseResult,
) {
    result.counts.turn_context_records_seen =
        result.counts.turn_context_records_seen.saturating_add(1);
    let Some(payload) = record.get("payload").and_then(Value::as_object) else {
        record_codex_unsupported_shape(result, source_ordinal);
        return;
    };
    let Some(observed_at_ms) = codex_record_timestamp(record) else {
        record_codex_invalid_timestamp(result, source_ordinal);
        return;
    };
    let Some(native_source) = result.valid_native_source().cloned() else {
        record_codex_invalid_identity(result, source_ordinal);
        return;
    };
    update_codex_activity_bounds(result, observed_at_ms);

    let model_attribution = match payload.get("model") {
        None => attribution::ModelAttribution::missing(),
        Some(Value::String(model_id)) => match attribution::ModelAttribution::explicit(model_id) {
            Ok(model_attribution) => model_attribution,
            Err(_) => {
                record_invalid_model(result, source_ordinal);
                attribution::ModelAttribution::invalid()
            }
        },
        Some(_) => {
            record_invalid_model(result, source_ordinal);
            attribution::ModelAttribution::invalid()
        }
    };
    let metadata = codex_observation_metadata(
        context,
        &native_source,
        SourceRecordShape::CodexTurnContext,
        ObservationKind::Turn,
        source_ordinal,
        nonempty_record_string(payload.get("turn_id")),
        observed_at_ms,
    );
    result.observations.push(NormalizedObservation::new(
        metadata,
        model_attribution,
        attribution::TokenAttribution::unavailable(),
    ));
    result.counts.observations_emitted = result.counts.observations_emitted.saturating_add(1);
}

fn parse_codex_event_message(
    record: &serde_json::Map<String, Value>,
    source_ordinal: u64,
    context: CodexAdapterContext<'_>,
    baselines: &mut CodexCumulativeBaselines,
    result: &mut ProviderAdapterParseResult,
) {
    let Some(payload) = record.get("payload").and_then(Value::as_object) else {
        record_codex_unsupported_shape(result, source_ordinal);
        return;
    };
    match payload.get("type") {
        Some(Value::String(event_type)) if event_type == "token_count" => {}
        Some(Value::String(_)) => {
            result.counts.ignored_records = result.counts.ignored_records.saturating_add(1);
            return;
        }
        _ => {
            record_codex_unsupported_shape(result, source_ordinal);
            return;
        }
    }
    result.counts.token_count_records_seen =
        result.counts.token_count_records_seen.saturating_add(1);

    let Some(observed_at_ms) = codex_record_timestamp(record) else {
        record_codex_invalid_timestamp(result, source_ordinal);
        return;
    };
    let Some(native_source) = result.valid_native_source().cloned() else {
        record_codex_invalid_identity(result, source_ordinal);
        return;
    };
    let Some(info) = payload.get("info").and_then(Value::as_object) else {
        record_codex_unsupported_shape(result, source_ordinal);
        return;
    };
    let total_usage = match info.get("total_token_usage") {
        Some(Value::Object(total_usage)) => total_usage,
        None | Some(Value::Null) if info.get("last_token_usage").is_some_and(Value::is_object) => {
            result.counts.last_token_usage_only_records = result
                .counts
                .last_token_usage_only_records
                .saturating_add(1);
            push_adapter_diagnostic(
                result,
                source_ordinal,
                ModelUsageDiagnosticKind::LastTokenUsageUnavailable,
            );
            update_codex_activity_bounds(result, observed_at_ms);
            return;
        }
        _ => {
            record_codex_unsupported_shape(result, source_ordinal);
            return;
        }
    };
    update_codex_activity_bounds(result, observed_at_ms);

    let total_input_tokens_value = total_usage.get("input_tokens");
    let output_tokens_value = total_usage.get("output_tokens");
    let cache_creation_tokens_value = total_usage.get("cache_creation_tokens");
    let cached_input_tokens_value = total_usage.get("cached_input_tokens");
    let invalid_token_dimensions = invalid_token_dimension_count([
        total_input_tokens_value,
        output_tokens_value,
        cache_creation_tokens_value,
        cached_input_tokens_value,
    ]);
    if invalid_token_dimensions > 0 {
        result.counts.invalid_token_dimension_values = result
            .counts
            .invalid_token_dimension_values
            .saturating_add(invalid_token_dimensions);
        push_adapter_diagnostic(
            result,
            source_ordinal,
            ModelUsageDiagnosticKind::InvalidTokenDimension,
        );
    }

    let total_input_tokens = validate_token_dimension(total_input_tokens_value);
    let cached_input_tokens = validate_token_dimension(cached_input_tokens_value);
    let input_cache_deltas = normalize_codex_input_cache_deltas(
        total_input_tokens,
        cached_input_tokens,
        baselines,
        result,
        source_ordinal,
    );
    let (output_tokens, output_reset) = cumulative_dimension_delta(
        validate_token_dimension(output_tokens_value),
        &mut baselines.output_tokens,
    );
    let (cache_creation_tokens, cache_creation_reset) = cumulative_dimension_delta(
        validate_token_dimension(cache_creation_tokens_value),
        &mut baselines.cache_creation_tokens,
    );
    for reset in [
        input_cache_deltas.inclusive_input_reset,
        output_reset,
        cache_creation_reset,
        input_cache_deltas.cache_read_reset,
    ] {
        if reset {
            result.counts.cumulative_reset_dimensions =
                result.counts.cumulative_reset_dimensions.saturating_add(1);
            push_adapter_diagnostic(
                result,
                source_ordinal,
                ModelUsageDiagnosticKind::CumulativeTokenReset,
            );
        }
    }

    let deltas = ValidatedTokenDimensions {
        input_tokens: input_cache_deltas.input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens: input_cache_deltas.cache_read_tokens,
    };
    if ![
        deltas.input_tokens(),
        deltas.output_tokens(),
        deltas.cache_creation_tokens(),
        deltas.cache_read_tokens(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value > 0)
    {
        return;
    }
    let Ok(token_attribution) = attribution::TokenAttribution::cumulative_delta(deltas) else {
        record_codex_unsupported_shape(result, source_ordinal);
        return;
    };

    let metadata = codex_observation_metadata(
        context,
        &native_source,
        SourceRecordShape::CodexTokenCount,
        ObservationKind::Token,
        source_ordinal,
        None,
        observed_at_ms,
    );
    result.observations.push(NormalizedObservation::new(
        metadata,
        attribution::ModelAttribution::missing(),
        token_attribution,
    ));
    result.counts.observations_emitted = result.counts.observations_emitted.saturating_add(1);
}

fn codex_observation_metadata(
    context: CodexAdapterContext<'_>,
    native_source: &ProviderNativeSourceMetadata,
    record_shape: SourceRecordShape,
    kind: ObservationKind,
    source_ordinal: u64,
    turn_id: Option<String>,
    observed_at_ms: i64,
) -> NormalizedObservationMetadata {
    NormalizedObservationMetadata {
        provider: IntegrationProvider::Codex,
        source_key: context.source_key.to_owned(),
        source_record_key: stable_source_record_key(record_shape, source_ordinal, 0),
        source_ordinal,
        kind,
        source_session_id: native_source.source_session_id.clone(),
        analytics_session_id: native_source.analytics_session_id().to_owned(),
        chain_id: native_source.chain_id.clone(),
        parent_chain_id: native_source.parent_chain_id.clone(),
        is_sidechain: native_source.is_sidechain,
        agent_id: None,
        turn_id,
        observed_at_ms,
        cwd: native_source.cwd.clone(),
        hostname: context.hostname.map(str::to_owned),
    }
}

fn normalize_codex_input_cache_deltas(
    inclusive_input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    baselines: &mut CodexCumulativeBaselines,
    result: &mut ProviderAdapterParseResult,
    source_ordinal: u64,
) -> CodexInputCacheDeltas {
    // Keep the raw inclusive baseline current for reset detection, while the
    // paired baseline remains fixed until input and cache can be decomposed.
    let (_, inclusive_input_reset) = cumulative_dimension_delta(
        inclusive_input_tokens,
        &mut baselines.raw_inclusive_input_tokens,
    );
    if inclusive_input_reset {
        baselines.inclusive_input_reset_since_anchor = true;
    }

    // Cache advancement is speculative until its relationship with inclusive
    // input is proven valid. This lets unrelated dimensions continue safely.
    let mut prospective_cache_baseline = baselines.cache_read_tokens;
    let (cache_read_delta, cache_read_reset) =
        cumulative_dimension_delta(cached_input_tokens, &mut prospective_cache_baseline);
    if cache_read_reset {
        baselines.cache_read_reset_since_anchor = true;
        // Pending cache was already emitted from the prior epoch. Keep it out
        // of all later alignment, even if this record cannot yet reanchor.
        baselines.pending_emitted_cache_read_tokens = 0;
    }
    let mut deltas = CodexInputCacheDeltas {
        inclusive_input_reset,
        cache_read_reset,
        ..CodexInputCacheDeltas::default()
    };

    if matches!(
        (inclusive_input_tokens, cached_input_tokens),
        (Some(inclusive), Some(cached)) if cached > inclusive
    ) {
        record_codex_invalid_token_relationship(result, source_ordinal);
        return deltas;
    }

    match (inclusive_input_tokens, cached_input_tokens) {
        (None, None) | (Some(_), None) => deltas,
        (None, Some(_)) => {
            let Some(cache_read_delta) = cache_read_delta else {
                return deltas;
            };
            let Some(pending_cache_read_tokens) = baselines
                .pending_emitted_cache_read_tokens
                .checked_add(i128::from(cache_read_delta))
            else {
                record_codex_invalid_token_relationship(result, source_ordinal);
                return deltas;
            };

            baselines.cache_read_tokens = prospective_cache_baseline;
            baselines.pending_emitted_cache_read_tokens = pending_cache_read_tokens;
            deltas.cache_read_tokens = Some(cache_read_delta);
            deltas
        }
        (Some(inclusive_input_tokens), Some(_)) => {
            let Some(cache_read_delta) = cache_read_delta else {
                return deltas;
            };

            let (inclusive_interval, cache_interval) = match baselines.paired_inclusive_input_tokens
            {
                _ if baselines.inclusive_input_reset_since_anchor
                    || baselines.cache_read_reset_since_anchor =>
                {
                    if inclusive_input_reset && cache_read_reset {
                        // Simultaneous resets establish a complete new
                        // segment. Pending cache belongs to the prior segment.
                        (
                            i128::from(inclusive_input_tokens),
                            i128::from(cache_read_delta),
                        )
                    } else {
                        // An asymmetric counter reset makes only derived input
                        // unsafe. Preserve the independently valid cache delta
                        // and use this complete pair as the next interval's
                        // anchor so reset history cannot poison later records.
                        baselines.cache_read_tokens = prospective_cache_baseline;
                        baselines.paired_inclusive_input_tokens = Some(inclusive_input_tokens);
                        baselines.inclusive_input_reset_since_anchor = false;
                        baselines.cache_read_reset_since_anchor = false;
                        baselines.pending_emitted_cache_read_tokens = 0;
                        deltas.cache_read_tokens = Some(cache_read_delta);
                        return deltas;
                    }
                }
                None => {
                    let Some(cache_interval) = baselines
                        .pending_emitted_cache_read_tokens
                        .checked_add(i128::from(cache_read_delta))
                    else {
                        record_codex_invalid_token_relationship(result, source_ordinal);
                        return deltas;
                    };
                    (i128::from(inclusive_input_tokens), cache_interval)
                }
                Some(paired_inclusive_input_tokens) => {
                    let Some(inclusive_interval) = i128::from(inclusive_input_tokens)
                        .checked_sub(i128::from(paired_inclusive_input_tokens))
                    else {
                        record_codex_invalid_token_relationship(result, source_ordinal);
                        return deltas;
                    };
                    let Some(cache_interval) = baselines
                        .pending_emitted_cache_read_tokens
                        .checked_add(i128::from(cache_read_delta))
                    else {
                        record_codex_invalid_token_relationship(result, source_ordinal);
                        return deltas;
                    };
                    (inclusive_interval, cache_interval)
                }
            };

            let Some(input_delta) = inclusive_interval.checked_sub(cache_interval) else {
                record_codex_invalid_token_relationship(result, source_ordinal);
                return deltas;
            };
            let Ok(input_delta) = i64::try_from(input_delta) else {
                record_codex_invalid_token_relationship(result, source_ordinal);
                return deltas;
            };
            if input_delta < 0 {
                record_codex_invalid_token_relationship(result, source_ordinal);
                return deltas;
            }

            baselines.cache_read_tokens = prospective_cache_baseline;
            baselines.paired_inclusive_input_tokens = Some(inclusive_input_tokens);
            baselines.inclusive_input_reset_since_anchor = false;
            baselines.cache_read_reset_since_anchor = false;
            baselines.pending_emitted_cache_read_tokens = 0;
            deltas.input_tokens = Some(input_delta);
            deltas.cache_read_tokens = Some(cache_read_delta);
            deltas
        }
    }
}

fn cumulative_dimension_delta(
    current: Option<i64>,
    previous: &mut Option<i64>,
) -> (Option<i64>, bool) {
    let Some(current) = current else {
        return (None, false);
    };

    let (delta, reset) = match *previous {
        Some(previous) if current >= previous => (current - previous, false),
        Some(_) => (current, true),
        None => (current, false),
    };
    *previous = Some(current);
    (Some(delta), reset)
}

fn codex_record_timestamp(record: &serde_json::Map<String, Value>) -> Option<i64> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp_millis())
        .filter(|timestamp| *timestamp >= 0)
}

fn optional_nonempty_record_string(value: Option<&Value>) -> Result<Option<String>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.to_owned())),
        Some(_) => Err(()),
    }
}

fn update_codex_activity_bounds(result: &mut ProviderAdapterParseResult, observed_at_ms: i64) {
    if let Some(native_source) = result.valid_native_source_mut() {
        native_source.first_activity_at_ms = native_source.first_activity_at_ms.min(observed_at_ms);
        native_source.last_activity_at_ms = native_source.last_activity_at_ms.max(observed_at_ms);
    }
}

fn record_codex_unsupported_shape(result: &mut ProviderAdapterParseResult, source_ordinal: u64) {
    result.counts.unsupported_shape_records =
        result.counts.unsupported_shape_records.saturating_add(1);
    push_adapter_diagnostic(
        result,
        source_ordinal,
        ModelUsageDiagnosticKind::RecordSkipped,
    );
}

fn record_codex_invalid_timestamp(result: &mut ProviderAdapterParseResult, source_ordinal: u64) {
    result.counts.invalid_timestamp_records =
        result.counts.invalid_timestamp_records.saturating_add(1);
    push_adapter_diagnostic(
        result,
        source_ordinal,
        ModelUsageDiagnosticKind::RecordSkipped,
    );
}

fn record_codex_invalid_identity(result: &mut ProviderAdapterParseResult, source_ordinal: u64) {
    result.counts.invalid_identity_records =
        result.counts.invalid_identity_records.saturating_add(1);
    push_adapter_diagnostic(
        result,
        source_ordinal,
        ModelUsageDiagnosticKind::RecordSkipped,
    );
}

fn record_codex_invalid_token_relationship(
    result: &mut ProviderAdapterParseResult,
    source_ordinal: u64,
) {
    result.counts.invalid_token_relationship_records = result
        .counts
        .invalid_token_relationship_records
        .saturating_add(1);
    push_adapter_diagnostic(
        result,
        source_ordinal,
        ModelUsageDiagnosticKind::InvalidTokenRelationship,
    );
}

// One permit covers both retained-history passes and live/startup reconciliation.
// Queue ownership lives in lib.rs; this process-wide guard is the final defense
// against two queue drains replacing the same source graph concurrently.
static MODEL_USAGE_RUNNER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Exclusive process-wide permission to run model source reconciliation.
pub(crate) struct ModelUsageRunnerPermit {
    _not_sync: PhantomData<Cell<()>>,
}

impl Drop for ModelUsageRunnerPermit {
    fn drop(&mut self) {
        MODEL_USAGE_RUNNER_ACTIVE.store(false, Ordering::Release);
    }
}

/// Acquire the model-usage runner without blocking a Tauri command thread.
pub(crate) fn try_acquire_model_usage_runner() -> Option<ModelUsageRunnerPermit> {
    MODEL_USAGE_RUNNER_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| ModelUsageRunnerPermit {
            _not_sync: PhantomData,
        })
}

/// Durable outcome for one discovered source in a reconciliation batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSourceReconciliationDisposition {
    Processed,
    Skipped,
    Failed,
}

/// Bounded, provider-qualified result for one source. Paths never enter this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSourceReconciliationResult {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_key: String,
    pub(crate) disposition: ModelSourceReconciliationDisposition,
    pub(crate) observations_written: i64,
    pub(crate) data_changed: bool,
    pub(crate) diagnostic: Option<ModelUsageDiagnostic>,
    retained_last_good: bool,
}

/// Aggregate source results from one provider-root inventory snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelSourceReconciliationBatchResult {
    pub(crate) sources: Vec<ModelSourceReconciliationResult>,
    pub(crate) data_changed: bool,
}

impl ModelSourceReconciliationBatchResult {
    pub(crate) fn processed_sources(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| source.disposition == ModelSourceReconciliationDisposition::Processed)
            .count()
    }

    pub(crate) fn skipped_sources(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| source.disposition == ModelSourceReconciliationDisposition::Skipped)
            .count()
    }

    pub(crate) fn failed_sources(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| source.disposition == ModelSourceReconciliationDisposition::Failed)
            .count()
    }

    pub(crate) fn observations_written(&self) -> i64 {
        self.sources.iter().fold(0_i64, |total, source| {
            total.saturating_add(source.observations_written)
        })
    }

    fn retained_last_good_sources(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| source.retained_last_good)
            .count()
    }
}

/// A bounded commit failure plus every source outcome already committed in the batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSourceReconciliationBatchError {
    pub(crate) error: String,
    pub(crate) committed: ModelSourceReconciliationBatchResult,
}

impl fmt::Display for ModelSourceReconciliationBatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.error)
    }
}

impl std::error::Error for ModelSourceReconciliationBatchError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StagedSourceAction {
    FastUnchanged,
    ContentUnchanged,
    SuppressedUnchanged,
    Replace,
    Fail,
}

struct StagedModelSource {
    discovered: DiscoveredRetainedJsonlSource,
    existing: Option<StoredModelSource>,
    action: StagedSourceAction,
    fast: Option<ModelSourceFastFingerprint>,
    fingerprint: Option<ModelSourceFingerprint>,
    unchanged_contents: Option<Vec<u8>>,
    parsed: Option<ProviderAdapterParseResult>,
    diagnostic: Option<ModelUsageDiagnostic>,
}

impl StagedModelSource {
    fn native_graph_metadata(&self) -> Option<SourceGraphMetadata> {
        if self.action == StagedSourceAction::Replace {
            return self
                .parsed
                .as_ref()
                .and_then(ProviderAdapterParseResult::valid_native_source)
                .map(SourceGraphMetadata::from_native);
        }

        self.existing
            .as_ref()
            .and_then(SourceGraphMetadata::from_stored)
    }

    fn fail(&mut self, diagnostic: ModelUsageDiagnostic) {
        self.action = StagedSourceAction::Fail;
        self.parsed = None;
        self.unchanged_contents = None;
        self.diagnostic = Some(diagnostic);
    }

    fn can_force_root_replacement(&self) -> bool {
        matches!(
            self.action,
            StagedSourceAction::FastUnchanged | StagedSourceAction::ContentUnchanged
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProviderSourceKey {
    provider: &'static str,
    source_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProviderRootKey {
    provider: &'static str,
    source_root_key: String,
}

impl ProviderRootKey {
    fn new(provider: IntegrationProvider, source_root_key: &str) -> Self {
        Self {
            provider: provider.as_str(),
            source_root_key: source_root_key.to_owned(),
        }
    }
}

impl ProviderSourceKey {
    fn new(provider: IntegrationProvider, source_key: &str) -> Self {
        Self {
            provider: provider.as_str(),
            source_key: source_key.to_owned(),
        }
    }
}

/// Unforgeable proof captured only from a complete root in a prepared inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompletedModelSourceRoot {
    provider: IntegrationProvider,
    source_root_key: String,
    generation: i64,
}

impl CompletedModelSourceRoot {
    fn from_completed_inventory(
        root: &ProviderSourceRoot,
        generation: i64,
    ) -> Result<Self, String> {
        if !matches!(root.outcome, ProviderRootEnumerationOutcome::Complete) {
            return Err("Cannot prove model source removal from an incomplete root".to_string());
        }
        if root.source_root_key.is_empty() {
            return Err("Completed model source root key cannot be empty".to_string());
        }
        if generation < 0 {
            return Err("Completed model source generation cannot be negative".to_string());
        }
        Ok(Self {
            provider: root.provider,
            source_root_key: root.source_root_key.to_owned(),
            generation,
        })
    }

    pub(crate) const fn provider(&self) -> IntegrationProvider {
        self.provider
    }

    pub(crate) fn source_root_key(&self) -> &str {
        &self.source_root_key
    }

    pub(crate) const fn generation(&self) -> i64 {
        self.generation
    }
}

#[derive(Clone, Debug)]
struct SourceGraphMetadata {
    provider: IntegrationProvider,
    chain_id: String,
    parent_chain_id: Option<String>,
}

impl SourceGraphMetadata {
    fn from_native(native: &ProviderNativeSourceMetadata) -> Self {
        Self {
            provider: native.provider,
            chain_id: native.chain_id.clone(),
            parent_chain_id: native.parent_chain_id.clone(),
        }
    }

    fn from_stored(stored: &StoredModelSource) -> Option<Self> {
        Some(Self {
            provider: stored.provider,
            chain_id: stored.last_good.chain_id.clone()?,
            parent_chain_id: stored.last_good.parent_chain_id.clone(),
        })
    }
}

#[derive(Clone, Debug)]
struct RootGraphNode {
    parent_chain_id: Option<String>,
    conflicted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProviderChainKey {
    provider: &'static str,
    chain_id: String,
}

impl ProviderChainKey {
    fn new(provider: IntegrationProvider, chain_id: &str) -> Self {
        Self {
            provider: provider.as_str(),
            chain_id: chain_id.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootGraphResolutionError {
    ConflictingParents,
    ParentCycle,
}

impl fmt::Display for RootGraphResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConflictingParents => "provider-native chain has conflicting parents",
            Self::ParentCycle => "provider-native parent graph contains a cycle",
        })
    }
}

struct SourceRootGraph {
    nodes: HashMap<ProviderChainKey, RootGraphNode>,
}

impl SourceRootGraph {
    fn from_metadata(metadata: impl IntoIterator<Item = SourceGraphMetadata>) -> Self {
        let mut nodes = HashMap::<ProviderChainKey, RootGraphNode>::new();
        for source in metadata {
            let key = ProviderChainKey::new(source.provider, &source.chain_id);
            match nodes.get_mut(&key) {
                Some(node) if node.parent_chain_id != source.parent_chain_id => {
                    node.conflicted = true;
                }
                Some(_) => {}
                None => {
                    nodes.insert(
                        key,
                        RootGraphNode {
                            parent_chain_id: source.parent_chain_id,
                            conflicted: false,
                        },
                    );
                }
            }
        }
        Self { nodes }
    }

    fn resolve(
        &self,
        provider: IntegrationProvider,
        chain_id: &str,
    ) -> Result<String, RootGraphResolutionError> {
        let mut current = chain_id.to_owned();
        let mut visited = HashSet::<String>::new();
        loop {
            if !visited.insert(current.clone()) {
                return Err(RootGraphResolutionError::ParentCycle);
            }
            let key = ProviderChainKey::new(provider, &current);
            let Some(node) = self.nodes.get(&key) else {
                // A native parent ID remains reliable even when the parent's
                // transcript has not been retained or discovered yet.
                return Ok(current);
            };
            if node.conflicted {
                return Err(RootGraphResolutionError::ConflictingParents);
            }
            let Some(parent) = &node.parent_chain_id else {
                return Ok(current);
            };
            current.clone_from(parent);
        }
    }
}

/// Owned, fully parsed reconciliation work for one complete inventory snapshot.
///
/// Keeping every graph decision in this plan lets the runner commit bounded
/// batches and yield without dropping ancestors needed to resolve descendants.
pub(crate) struct PreparedModelSourceReconciliation {
    generation: i64,
    total_sources: usize,
    pending_sources: VecDeque<StagedModelSource>,
    completed_root_proofs: HashMap<ProviderRootKey, CompletedModelSourceRoot>,
}

impl PreparedModelSourceReconciliation {
    pub(crate) fn total_sources(&self) -> usize {
        self.total_sources
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.pending_sources.is_empty()
    }
}

/// Stage one inventory into an owned plan before starting any write transaction.
///
/// All changed/forced sources are read, hashed, parsed, and root-resolved here.
/// Provider-native parent metadata is the only graph authority; layout hints are
/// diagnostic only. Descendants whose resolved root changes are force-parsed
/// before this function returns.
pub(crate) fn prepare_model_source_reconciliation(
    storage: &Storage,
    roots: &[ProviderSourceRoot],
    generation: i64,
    _permit: &mut ModelUsageRunnerPermit,
) -> Result<PreparedModelSourceReconciliation, String> {
    if generation < 0 {
        return Err("Model reconciliation generation cannot be negative".to_string());
    }

    let hostname = crate::sessions::SessionIndex::local_hostname();
    let mut persisted_by_key = HashMap::<ProviderSourceKey, StoredModelSource>::new();
    let mut retain_missing_persisted = HashSet::<(&'static str, String)>::new();
    let mut source_roots = HashSet::<ProviderRootKey>::new();
    let mut completed_root_proofs = HashMap::<ProviderRootKey, CompletedModelSourceRoot>::new();
    let mut discovered_keys = HashSet::<ProviderSourceKey>::new();
    let mut discovered_sources = Vec::<DiscoveredRetainedJsonlSource>::new();

    for root in roots {
        let root_key = ProviderRootKey::new(root.provider, root.source_root_key);
        if !source_roots.insert(root_key.clone()) {
            return Err("Duplicate provider-qualified model source root in inventory".to_string());
        }
        if matches!(root.outcome, ProviderRootEnumerationOutcome::Complete) {
            let proof = CompletedModelSourceRoot::from_completed_inventory(root, generation)?;
            completed_root_proofs.insert(root_key, proof);
        }
        let persisted = storage.list_model_sources_for_root(root.provider, root.source_root_key)?;
        for source in persisted {
            persisted_by_key.insert(
                ProviderSourceKey::new(source.provider, &source.source_key),
                source,
            );
        }
        if matches!(root.outcome, ProviderRootEnumerationOutcome::Failed { .. }) {
            retain_missing_persisted.insert((root.provider.as_str(), root.source_root_key.into()));
        }
        for source in &root.sources {
            if source.provider != root.provider || source.source_root_key != root.source_root_key {
                return Err("Discovered model source does not match its provider root".to_string());
            }
            let key = ProviderSourceKey::new(source.provider, &source.source_key);
            if !discovered_keys.insert(key) {
                return Err(
                    "Duplicate provider-qualified model source key in inventory".to_string()
                );
            }
            discovered_sources.push(source.clone());
        }
    }

    discovered_sources.sort_by(|left, right| {
        left.provider
            .as_str()
            .cmp(right.provider.as_str())
            .then_with(|| left.source_key.cmp(&right.source_key))
    });

    let mut staged = Vec::with_capacity(discovered_sources.len());
    for discovered in discovered_sources {
        let key = ProviderSourceKey::new(discovered.provider, &discovered.source_key);
        let existing = persisted_by_key.get(&key).cloned();
        staged.push(stage_model_source(discovered, existing, &hostname));
    }

    stabilize_root_graph(
        &mut staged,
        &persisted_by_key,
        &discovered_keys,
        &retain_missing_persisted,
        &hostname,
    );
    for source in &mut staged {
        // Equal-content bytes are needed only while deciding forced fan-out.
        // Parsed replacements retain normalized rows, not full transcript text.
        source.unchanged_contents = None;
    }

    let total_sources = staged.len();
    Ok(PreparedModelSourceReconciliation {
        generation,
        total_sources,
        pending_sources: staged.into(),
        completed_root_proofs,
    })
}

/// Whether a reconciliation commit belongs to a run that will later prune.
///
/// The retained backfill prunes stale sources, so it must re-stamp every seen
/// source to the current generation to survive that prune. The live path never
/// prunes, so a fast-unchanged source needs no `seen_generation`/
/// `last_attempt_at_ms` re-stamp and can skip its per-source storage read and
/// write entirely.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelSourceCommitMode {
    Live,
    Backfill,
}

/// Commit at most `limit` prepared sources while retaining the full graph result.
///
/// A zero limit is rejected so a worker cannot spin forever while reporting
/// progress. Source work is removed only after its commit succeeds. Any
/// pre-commit status-read failure leaves the current source queued for retry.
pub(crate) fn commit_next_model_source_batch(
    plan: &mut PreparedModelSourceReconciliation,
    storage: &Storage,
    app_handle: &tauri::AppHandle,
    limit: usize,
    _permit: &mut ModelUsageRunnerPermit,
    mode: ModelSourceCommitMode,
) -> Result<ModelSourceReconciliationBatchResult, ModelSourceReconciliationBatchError> {
    if limit == 0 {
        return Err(ModelSourceReconciliationBatchError {
            error: "Model reconciliation batch limit must be greater than zero".to_string(),
            committed: ModelSourceReconciliationBatchResult::default(),
        });
    }

    let mut batch = ModelSourceReconciliationBatchResult::default();
    let batch_size = limit.min(plan.pending_sources.len());
    for _ in 0..batch_size {
        let outcome = {
            let source = plan
                .pending_sources
                .front()
                .expect("batch size is bounded by pending source count");
            commit_staged_model_source(storage, source, plan.generation, mode)
        };
        let committed = match outcome {
            Ok(committed) => committed,
            Err(error) => {
                return Err(ModelSourceReconciliationBatchError {
                    error,
                    committed: batch,
                });
            }
        };
        plan.pending_sources.pop_front();
        batch.data_changed |= committed.result.data_changed;
        if let Some(notify) = committed.notify {
            emit_model_analytics_updated(app_handle, &notify.snapshot, notify.data_changed);
        }
        batch.sources.push(committed.result);
    }

    Ok(batch)
}

#[derive(Default)]
struct RetainedBackfillProgress {
    processed_sources: usize,
    failed_sources: usize,
    skipped_sources: usize,
    retained_last_good_sources: usize,
    observations_written: i64,
    pruned_sources: usize,
}

impl RetainedBackfillProgress {
    fn record(&mut self, batch: &ModelSourceReconciliationBatchResult) {
        self.processed_sources = self
            .processed_sources
            .saturating_add(batch.processed_sources());
        self.failed_sources = self.failed_sources.saturating_add(batch.failed_sources());
        self.skipped_sources = self.skipped_sources.saturating_add(batch.skipped_sources());
        self.retained_last_good_sources = self
            .retained_last_good_sources
            .saturating_add(batch.retained_last_good_sources());
        self.observations_written = self
            .observations_written
            .saturating_add(batch.observations_written());
    }

    fn made_useful_progress(&self) -> bool {
        self.processed_sources != 0
            || self.skipped_sources != 0
            || self.retained_last_good_sources != 0
            || self.pruned_sources != 0
    }
}

fn emit_committed_backfill_status(
    app_handle: &tauri::AppHandle,
    status: &ModelBackfillStatus,
    data_changed: bool,
) {
    let snapshot = ModelAnalyticsEventSnapshot {
        generation: status.generation,
        status: status.status,
        updated_at: status.updated_at.clone(),
    };
    emit_model_analytics_updated(app_handle, &snapshot, data_changed);
}

async fn finish_retained_model_history_backfill(
    storage: &'static Storage,
    app_handle: &tauri::AppHandle,
    terminal_state: ModelBackfillState,
    inventory_complete: bool,
    diagnostic: Option<ModelBackfillDiagnostic>,
) -> Result<ModelBackfillStatus, String> {
    let status = tauri::async_runtime::spawn_blocking(move || {
        storage.finish_model_backfill(terminal_state, inventory_complete, diagnostic.as_ref())
    })
    .await
    .map_err(|error| format!("Model backfill terminal task failed: {error}"))??;
    emit_committed_backfill_status(app_handle, &status, false);
    Ok(status)
}

async fn finish_retained_model_history_backfill_after_error(
    storage: &'static Storage,
    app_handle: &tauri::AppHandle,
    progress: &RetainedBackfillProgress,
    inventory_complete: bool,
) -> Result<ModelBackfillStatus, String> {
    let state = if progress.made_useful_progress() {
        ModelBackfillState::Partial
    } else {
        ModelBackfillState::Failed
    };
    finish_retained_model_history_backfill(
        storage,
        app_handle,
        state,
        inventory_complete,
        Some(ModelBackfillDiagnostic::storage_error()),
    )
    .await
}

/// Log a stage failure and drive the backfill to its recoverable terminal state.
///
/// Centralizes the log-and-finish tail shared by every fallible backfill stage.
/// `context` is the exact message prefix; `inventory_complete` varies per stage
/// (`false` mid-run, `failed_roots == 0` once every root is accounted for).
async fn fail_backfill_stage(
    storage: &'static Storage,
    app_handle: &tauri::AppHandle,
    progress: &RetainedBackfillProgress,
    inventory_complete: bool,
    context: &str,
    error: impl fmt::Display,
) -> Result<ModelBackfillStatus, String> {
    log::error!("{context}: {error}");
    finish_retained_model_history_backfill_after_error(
        storage,
        app_handle,
        progress,
        inventory_complete,
    )
    .await
}

/// Reconcile every retained Claude and Codex source without blocking Tauri's
/// async command executor.
///
/// The caller owns scheduling and supplies the process-wide permit. This
/// worker persists discovery before publishing source totals, commits bounded
/// source batches with durable progress, yields between them, and prunes only
/// roots whose exact inventory completed.
pub(crate) async fn run_retained_model_history_backfill(
    storage: &'static Storage,
    app_handle: tauri::AppHandle,
    mut permit: ModelUsageRunnerPermit,
) -> Result<ModelBackfillStatus, String> {
    let started = tauri::async_runtime::spawn_blocking(move || storage.start_model_backfill())
        .await
        .map_err(|error| format!("Model backfill start task failed: {error}"))??;
    emit_committed_backfill_status(&app_handle, &started, false);
    let generation = started.generation;
    let mut progress = RetainedBackfillProgress::default();
    let mut run_diagnostic = None::<ModelBackfillDiagnostic>;

    let total_roots = 2_usize;
    let root_enumerators: [fn() -> ProviderSourceRoot; 2] = [
        crate::sessions::enumerate_claude_retained_jsonl_source_root,
        crate::sessions::enumerate_codex_retained_jsonl_source_root,
    ];
    let mut roots = Vec::with_capacity(total_roots);
    let mut completed_roots = 0_usize;
    let mut failed_roots = 0_usize;
    for enumerate_root in root_enumerators {
        let root = match tauri::async_runtime::spawn_blocking(enumerate_root).await {
            Ok(root) => root,
            Err(error) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    false,
                    "Retained model root inventory task failed",
                    error,
                )
                .await;
            }
        };
        match &root.outcome {
            ProviderRootEnumerationOutcome::Complete => {
                completed_roots = completed_roots.saturating_add(1);
            }
            ProviderRootEnumerationOutcome::Failed { diagnostic } => {
                failed_roots = failed_roots.saturating_add(1);
                run_diagnostic.get_or_insert_with(|| {
                    ModelBackfillDiagnostic::from_user_safe_message(diagnostic)
                });
            }
        }
        let diagnostic = run_diagnostic.clone();
        let status = match tauri::async_runtime::spawn_blocking(move || {
            storage.update_model_backfill_roots(
                total_roots,
                completed_roots,
                failed_roots,
                diagnostic.as_ref(),
            )
        })
        .await
        {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    false,
                    "Could not persist retained model root progress",
                    error,
                )
                .await;
            }
            Err(error) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    false,
                    "Retained model root progress task failed",
                    error,
                )
                .await;
            }
        };
        emit_committed_backfill_status(&app_handle, &status, false);
        roots.push(root);
    }

    let completed_root_keys = roots
        .iter()
        .filter(|root| matches!(root.outcome, ProviderRootEnumerationOutcome::Complete))
        .map(|root| (root.provider, root.source_root_key))
        .collect::<Vec<_>>();
    let prepare = tauri::async_runtime::spawn_blocking(move || {
        let result = prepare_model_source_reconciliation(storage, &roots, generation, &mut permit);
        (result, permit)
    })
    .await;
    let (prepared, returned_permit) = match prepare {
        Ok(result) => result,
        Err(error) => {
            return fail_backfill_stage(
                storage,
                &app_handle,
                &progress,
                false,
                "Retained model source preparation task failed",
                error,
            )
            .await;
        }
    };
    permit = returned_permit;
    let mut plan = match prepared {
        Ok(plan) => plan,
        Err(error) => {
            return fail_backfill_stage(
                storage,
                &app_handle,
                &progress,
                false,
                "Could not prepare retained model sources",
                error,
            )
            .await;
        }
    };
    let total_sources = plan.total_sources();
    let source_total = match tauri::async_runtime::spawn_blocking(move || {
        storage.set_model_backfill_source_total(total_sources)
    })
    .await
    {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            return fail_backfill_stage(
                storage,
                &app_handle,
                &progress,
                false,
                "Could not publish retained model source total",
                error,
            )
            .await;
        }
        Err(error) => {
            return fail_backfill_stage(
                storage,
                &app_handle,
                &progress,
                false,
                "Retained model source-total task failed",
                error,
            )
            .await;
        }
    };
    emit_committed_backfill_status(&app_handle, &source_total, false);

    while !plan.is_complete() {
        let batch_handle = app_handle.clone();
        let commit = tauri::async_runtime::spawn_blocking(move || {
            let result = commit_next_model_source_batch(
                &mut plan,
                storage,
                &batch_handle,
                RETAINED_SOURCE_COMMIT_BATCH_SIZE,
                &mut permit,
                ModelSourceCommitMode::Backfill,
            );
            (plan, permit, result)
        })
        .await;
        let (returned_plan, returned_permit, result) = match commit {
            Ok(result) => result,
            Err(error) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    false,
                    "Retained model source commit task failed",
                    error,
                )
                .await;
            }
        };
        plan = returned_plan;
        permit = returned_permit;

        let (batch, commit_error) = match result {
            Ok(batch) => (batch, None),
            Err(error) => (error.committed, Some(error.error)),
        };
        progress.record(&batch);
        let batch_diagnostic = batch
            .sources
            .iter()
            .find_map(|source| source.diagnostic.as_ref())
            .map(|diagnostic| ModelBackfillDiagnostic::from_user_safe_message(diagnostic.as_str()));
        if run_diagnostic.is_none() {
            run_diagnostic.clone_from(&batch_diagnostic);
        }
        let processed_sources = batch.processed_sources();
        let failed_sources = batch.failed_sources();
        let skipped_sources = batch.skipped_sources();
        let observations_written = batch.observations_written();
        let progress_diagnostic = batch_diagnostic.clone();
        let status = match tauri::async_runtime::spawn_blocking(move || {
            storage.record_model_backfill_progress(
                processed_sources,
                failed_sources,
                skipped_sources,
                observations_written,
                progress_diagnostic.as_ref(),
            )
        })
        .await
        {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    false,
                    "Could not persist retained model batch progress",
                    error,
                )
                .await;
            }
            Err(error) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    false,
                    "Retained model batch-progress task failed",
                    error,
                )
                .await;
            }
        };
        emit_committed_backfill_status(&app_handle, &status, batch.data_changed);
        if let Some(error) = commit_error {
            return fail_backfill_stage(
                storage,
                &app_handle,
                &progress,
                false,
                "Retained model source batch stopped after committed progress",
                error,
            )
            .await;
        }
        if !plan.is_complete() {
            tokio::task::yield_now().await;
        }
    }

    for (provider, source_root_key) in completed_root_keys {
        let prune_handle = app_handle.clone();
        let prune = tauri::async_runtime::spawn_blocking(move || {
            let result = prune_completed_model_source_root(
                storage,
                &prune_handle,
                &plan,
                provider,
                source_root_key,
                &mut permit,
            );
            (plan, permit, result)
        })
        .await;
        let (returned_plan, returned_permit, result) = match prune {
            Ok(result) => result,
            Err(error) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    failed_roots == 0,
                    "Retained model root-prune task failed",
                    error,
                )
                .await;
            }
        };
        plan = returned_plan;
        permit = returned_permit;
        match result {
            Ok(pruned) => {
                progress.pruned_sources = progress.pruned_sources.saturating_add(pruned);
            }
            Err(error) => {
                return fail_backfill_stage(
                    storage,
                    &app_handle,
                    &progress,
                    failed_roots == 0,
                    "Could not prune completed retained model root",
                    error,
                )
                .await;
            }
        }
        tokio::task::yield_now().await;
    }

    let inventory_complete = failed_roots == 0;
    let terminal_state = if inventory_complete && progress.failed_sources == 0 {
        ModelBackfillState::Complete
    } else if progress.made_useful_progress() {
        ModelBackfillState::Partial
    } else {
        ModelBackfillState::Failed
    };
    finish_retained_model_history_backfill(
        storage,
        &app_handle,
        terminal_state,
        inventory_complete,
        run_diagnostic,
    )
    .await
}

fn stage_model_source(
    discovered: DiscoveredRetainedJsonlSource,
    existing: Option<StoredModelSource>,
    hostname: &str,
) -> StagedModelSource {
    let mut staged = StagedModelSource {
        discovered,
        existing,
        action: StagedSourceAction::Fail,
        fast: None,
        fingerprint: None,
        unchanged_contents: None,
        parsed: None,
        diagnostic: None,
    };

    let fast = match source_fast_fingerprint(&staged.discovered) {
        Ok(fast) => fast,
        Err(error) => {
            log::warn!(
                "Failed to inspect model source {}: {error}",
                staged.discovered.canonical_path.display()
            );
            staged.fail(ModelUsageDiagnostic::new(
                ModelUsageDiagnosticKind::SourceReadFailed,
            ));
            return staged;
        }
    };
    staged.fast = Some(fast);

    match classify_model_source_change(staged.existing.as_ref(), fast, None) {
        ModelSourceChange::FastUnchanged => {
            staged.action = StagedSourceAction::FastUnchanged;
            staged
        }
        ModelSourceChange::ContentHashRequired => {
            stage_source_content(&mut staged, hostname);
            staged
        }
        ModelSourceChange::ContentUnchanged
        | ModelSourceChange::ContentChanged
        | ModelSourceChange::SuppressedUnchanged
        | ModelSourceChange::SuppressedChanged => {
            staged.fail(ModelUsageDiagnostic::new(
                ModelUsageDiagnosticKind::SourceParseFailed,
            ));
            staged
        }
    }
}

fn source_fast_fingerprint(
    discovered: &DiscoveredRetainedJsonlSource,
) -> Result<ModelSourceFastFingerprint, String> {
    let metadata = std::fs::metadata(&discovered.canonical_path).map_err(|error| {
        format!(
            "read metadata for {}: {error}",
            discovered.canonical_path.display()
        )
    })?;
    model_source_fast_fingerprint(&discovered.canonical_path, &metadata)
}

fn stage_source_content(staged: &mut StagedModelSource, hostname: &str) {
    let Some(expected_fast) = staged.fast else {
        staged.fail(ModelUsageDiagnostic::new(
            ModelUsageDiagnosticKind::SourceReadFailed,
        ));
        return;
    };
    let contents = match read_stable_source_bytes(&staged.discovered, expected_fast) {
        Ok(contents) => contents,
        Err(error) => {
            log::warn!(
                "Failed to read model source {}: {error}",
                staged.discovered.canonical_path.display()
            );
            staged.fail(ModelUsageDiagnostic::new(
                ModelUsageDiagnosticKind::SourceReadFailed,
            ));
            return;
        }
    };
    let fingerprint = ModelSourceFingerprint::from_content(expected_fast, &contents);
    let change = classify_model_source_change(
        staged.existing.as_ref(),
        expected_fast,
        Some(fingerprint.content_sha256()),
    );
    staged.fingerprint = Some(fingerprint);

    match change {
        ModelSourceChange::ContentUnchanged => {
            staged.action = StagedSourceAction::ContentUnchanged;
            staged.unchanged_contents = Some(contents);
        }
        ModelSourceChange::SuppressedUnchanged => {
            staged.action = StagedSourceAction::SuppressedUnchanged;
        }
        ModelSourceChange::ContentChanged | ModelSourceChange::SuppressedChanged => {
            match parse_model_source(&staged.discovered, &contents, hostname) {
                Ok(parsed) => {
                    staged.action = StagedSourceAction::Replace;
                    staged.parsed = Some(parsed);
                }
                Err(diagnostic) => staged.fail(diagnostic),
            }
        }
        ModelSourceChange::FastUnchanged | ModelSourceChange::ContentHashRequired => {
            staged.fail(ModelUsageDiagnostic::new(
                ModelUsageDiagnosticKind::SourceParseFailed,
            ));
        }
    }
}

/// Maximum retained transcript size accepted for model reconciliation.
///
/// A single `.jsonl` larger than this is treated as unreadable instead of being
/// read into memory, so a pathological or corrupt transcript cannot OOM the
/// indexer. Oversize sources fail like any other unreadable source, which
/// conservatively suppresses pruning for their root until they shrink again.
const MODEL_SOURCE_MAX_BYTES: u64 = 256 * 1024 * 1024;

fn read_stable_source_bytes(
    discovered: &DiscoveredRetainedJsonlSource,
    expected_fast: ModelSourceFastFingerprint,
) -> Result<Vec<u8>, String> {
    use std::io::Read;

    // Reject a known-oversize source from its already-collected fingerprint
    // before opening it, so a huge transcript is never read into memory.
    if u64::try_from(expected_fast.size_bytes()).is_ok_and(|size| size > MODEL_SOURCE_MAX_BYTES) {
        return Err(format!(
            "source exceeds {MODEL_SOURCE_MAX_BYTES}-byte model reconciliation cap"
        ));
    }

    // Bound the read itself so a source that grows past the cap between
    // fingerprinting and reading still cannot exhaust memory: read at most one
    // byte beyond the cap and reject anything that reaches it, keeping the
    // oversize path consistent with the read/re-fingerprint stability check.
    let file = std::fs::File::open(&discovered.canonical_path)
        .map_err(|error| format!("open complete source bytes: {error}"))?;
    let mut contents = Vec::new();
    file.take(MODEL_SOURCE_MAX_BYTES.saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|error| format!("read complete source bytes: {error}"))?;
    if contents.len() as u64 > MODEL_SOURCE_MAX_BYTES {
        return Err(format!(
            "source grew past {MODEL_SOURCE_MAX_BYTES}-byte model reconciliation cap while reading"
        ));
    }

    let observed_fast = source_fast_fingerprint(discovered)?;
    if observed_fast != expected_fast {
        return Err("source changed while it was being read".to_string());
    }
    Ok(contents)
}

fn parse_model_source(
    discovered: &DiscoveredRetainedJsonlSource,
    contents: &[u8],
    hostname: &str,
) -> Result<ProviderAdapterParseResult, ModelUsageDiagnostic> {
    let contents = std::str::from_utf8(contents).map_err(|error| {
        log::warn!(
            "Model source {} is not UTF-8 JSONL: {error}",
            discovered.canonical_path.display()
        );
        ModelUsageDiagnostic::new(ModelUsageDiagnosticKind::SourceParseFailed)
    })?;
    let result = match discovered.provider {
        IntegrationProvider::Claude => parse_claude_model_usage_jsonl(
            contents,
            ClaudeAdapterContext {
                source_key: &discovered.source_key,
                layout_hint: &discovered.layout_hint,
                hostname: Some(hostname),
            },
        ),
        IntegrationProvider::Codex => parse_codex_model_usage_jsonl(
            contents,
            CodexAdapterContext {
                source_key: &discovered.source_key,
                layout_hint: &discovered.layout_hint,
                hostname: Some(hostname),
            },
        ),
        IntegrationProvider::MiniMax => {
            log::warn!(
                "Unsupported provider source reached model reconciliation: {}",
                discovered.canonical_path.display()
            );
            return Err(ModelUsageDiagnostic::new(
                ModelUsageDiagnosticKind::SourceParseFailed,
            ));
        }
    };

    if matches!(
        result.native_identity,
        ProviderNativeIdentityState::Conflicted
    ) {
        log::warn!(
            "Model source {} contains conflicting provider-native identity metadata",
            discovered.canonical_path.display()
        );
        Err(ModelUsageDiagnostic::new(
            ModelUsageDiagnosticKind::SourceParseFailed,
        ))
    } else {
        Ok(result)
    }
}

fn stabilize_root_graph(
    staged: &mut [StagedModelSource],
    persisted_by_key: &HashMap<ProviderSourceKey, StoredModelSource>,
    discovered_keys: &HashSet<ProviderSourceKey>,
    retain_missing_persisted: &HashSet<(&'static str, String)>,
    hostname: &str,
) {
    // Each pass either fails one replacement or forces one unchanged source.
    // Both transitions are one-way, so this bound cannot truncate convergence.
    let max_passes = staged.len().saturating_mul(2).saturating_add(1);
    for _ in 0..max_passes {
        let graph = build_source_root_graph(
            staged,
            persisted_by_key,
            discovered_keys,
            retain_missing_persisted,
        );
        let mut changed = false;

        let mut stamp_failed = false;
        for source in staged.iter_mut() {
            if source.action != StagedSourceAction::Replace {
                continue;
            }
            let Some(native) = source
                .parsed
                .as_ref()
                .and_then(ProviderAdapterParseResult::valid_native_source)
            else {
                continue;
            };
            if let Err(error) = graph.resolve(native.provider, &native.chain_id) {
                log::warn!(
                    "Cannot resolve model source graph for {}: {error}",
                    source.discovered.canonical_path.display()
                );
                source.fail(ModelUsageDiagnostic::new(
                    ModelUsageDiagnosticKind::SourceParseFailed,
                ));
                changed = true;
            }
        }
        if changed {
            continue;
        }

        for source in staged.iter_mut() {
            if !source.can_force_root_replacement() {
                continue;
            }
            let Some(native) = source.native_graph_metadata() else {
                continue;
            };
            let resolved_root = match graph.resolve(native.provider, &native.chain_id) {
                Ok(root) => root,
                Err(error) => {
                    log::warn!(
                        "Cannot resolve retained model source graph for {}: {error}",
                        source.discovered.canonical_path.display()
                    );
                    continue;
                }
            };
            let stored_root = source
                .existing
                .as_ref()
                .and_then(|existing| existing.last_good.analytics_session_id.as_deref());
            if stored_root == Some(resolved_root.as_str()) {
                continue;
            }

            force_parse_model_source(source, hostname);
            changed = true;
        }
        if changed {
            continue;
        }

        for source in staged.iter_mut() {
            if source.action != StagedSourceAction::Replace {
                continue;
            }
            let Some(native) = source
                .parsed
                .as_ref()
                .and_then(ProviderAdapterParseResult::valid_native_source)
            else {
                continue;
            };
            let resolved_root = match graph.resolve(native.provider, &native.chain_id) {
                Ok(root) => root,
                Err(error) => {
                    log::warn!(
                        "Cannot finalize model source graph for {}: {error}",
                        source.discovered.canonical_path.display()
                    );
                    source.fail(ModelUsageDiagnostic::new(
                        ModelUsageDiagnosticKind::SourceParseFailed,
                    ));
                    stamp_failed = true;
                    continue;
                }
            };
            let resolution = source
                .parsed
                .as_mut()
                .expect("replace action always retains a parse result")
                .resolve_analytics_root(&resolved_root);
            if let Err(error) = resolution {
                log::warn!(
                    "Cannot stamp model source graph for {}: {error}",
                    source.discovered.canonical_path.display()
                );
                source.fail(ModelUsageDiagnostic::new(
                    ModelUsageDiagnosticKind::SourceParseFailed,
                ));
                stamp_failed = true;
            }
        }
        if stamp_failed {
            continue;
        }
        return;
    }

    for source in staged.iter_mut() {
        if source.action == StagedSourceAction::Replace {
            log::warn!(
                "Model source graph did not converge for {}",
                source.discovered.canonical_path.display()
            );
            source.fail(ModelUsageDiagnostic::new(
                ModelUsageDiagnosticKind::SourceParseFailed,
            ));
        }
    }
}

fn build_source_root_graph(
    staged: &[StagedModelSource],
    persisted_by_key: &HashMap<ProviderSourceKey, StoredModelSource>,
    discovered_keys: &HashSet<ProviderSourceKey>,
    retain_missing_persisted: &HashSet<(&'static str, String)>,
) -> SourceRootGraph {
    let mut metadata = staged
        .iter()
        .filter_map(StagedModelSource::native_graph_metadata)
        .collect::<Vec<_>>();

    for (key, source) in persisted_by_key {
        if discovered_keys.contains(key)
            || !retain_missing_persisted
                .contains(&(source.provider.as_str(), source.source_root_key.clone()))
        {
            continue;
        }
        if let Some(source_metadata) = SourceGraphMetadata::from_stored(source) {
            metadata.push(source_metadata);
        }
    }

    SourceRootGraph::from_metadata(metadata)
}

fn force_parse_model_source(source: &mut StagedModelSource, hostname: &str) {
    let contents = match source.unchanged_contents.take() {
        Some(contents) => contents,
        None => {
            let Some(expected_fast) = source.fast else {
                source.fail(ModelUsageDiagnostic::new(
                    ModelUsageDiagnosticKind::SourceReadFailed,
                ));
                return;
            };
            match read_stable_source_bytes(&source.discovered, expected_fast) {
                Ok(contents) => {
                    source.fingerprint = Some(ModelSourceFingerprint::from_content(
                        expected_fast,
                        &contents,
                    ));
                    contents
                }
                Err(error) => {
                    log::warn!(
                        "Failed to reread model source {} for root fan-out: {error}",
                        source.discovered.canonical_path.display()
                    );
                    source.fail(ModelUsageDiagnostic::new(
                        ModelUsageDiagnosticKind::SourceReadFailed,
                    ));
                    return;
                }
            }
        }
    };

    match parse_model_source(&source.discovered, &contents, hostname) {
        Ok(parsed) => {
            source.action = StagedSourceAction::Replace;
            source.parsed = Some(parsed);
            source.diagnostic = None;
        }
        Err(diagnostic) => source.fail(diagnostic),
    }
}

/// Authoritative backfill event fields captured before a source mutation.
#[derive(Clone, Debug)]
pub(crate) struct ModelAnalyticsEventSnapshot {
    generation: i64,
    status: ModelBackfillState,
    updated_at: String,
}

/// Capture every fallible event field before starting a source transaction.
pub(crate) fn read_model_analytics_event_snapshot(
    storage: &Storage,
) -> Result<ModelAnalyticsEventSnapshot, String> {
    let status = storage
        .get_model_backfill_status()
        .map_err(|error| format!("Read model analytics status before mutation: {error}"))?;
    Ok(ModelAnalyticsEventSnapshot {
        generation: status.generation,
        status: status.status,
        updated_at: status.updated_at,
    })
}

/// A committed source's advisory refresh, captured only when something worth
/// emitting actually changed.
struct ModelAnalyticsNotify {
    snapshot: ModelAnalyticsEventSnapshot,
    data_changed: bool,
}

struct CommittedModelSource {
    result: ModelSourceReconciliationResult,
    notify: Option<ModelAnalyticsNotify>,
}

fn commit_staged_model_source(
    storage: &Storage,
    staged: &StagedModelSource,
    generation: i64,
    mode: ModelSourceCommitMode,
) -> Result<CommittedModelSource, String> {
    // A live fast-unchanged source causes no observation or status change and no
    // refresh event. Its only effect is re-stamping seen_generation/
    // last_attempt_at_ms, which is consumed solely by the backfill prune -- and
    // the backfill re-stamps every source it sees before pruning. On the live
    // path (which never prunes) that per-source read and write are pure
    // amplification, so skip them and report the source as skipped.
    if mode == ModelSourceCommitMode::Live
        && matches!(staged.action, StagedSourceAction::FastUnchanged)
    {
        return Ok(CommittedModelSource {
            result: ModelSourceReconciliationResult {
                provider: staged.discovered.provider,
                source_key: staged.discovered.source_key.clone(),
                disposition: ModelSourceReconciliationDisposition::Skipped,
                observations_written: 0,
                data_changed: false,
                diagnostic: None,
                retained_last_good: false,
            },
            notify: None,
        });
    }

    let event_snapshot = read_model_analytics_event_snapshot(storage)?;
    let attempted_at_ms = Utc::now().timestamp_millis();
    let source_key = staged.discovered.source_key.clone();
    let provider = staged.discovered.provider;
    let mut result = ModelSourceReconciliationResult {
        provider,
        source_key,
        disposition: ModelSourceReconciliationDisposition::Skipped,
        observations_written: 0,
        data_changed: false,
        diagnostic: None,
        retained_last_good: false,
    };
    let mut notify_data_changed = None;

    match staged.action {
        StagedSourceAction::FastUnchanged => {
            let fast = staged
                .fast
                .ok_or_else(|| "Fast-unchanged source lost its fingerprint".to_string())?;
            let normalized = normalized_source_from_existing(
                &staged.discovered,
                staged.existing.as_ref(),
                Some(fast),
                None,
                generation,
                SourceProcessingStatus::Ok,
                None,
                attempted_at_ms,
            );
            storage.mark_model_source_fast_unchanged(&normalized, fast, attempted_at_ms)?;
        }
        StagedSourceAction::ContentUnchanged => {
            let fingerprint = staged
                .fingerprint
                .as_ref()
                .ok_or_else(|| "Content-unchanged source lost its fingerprint".to_string())?;
            let status_changed = staged.existing.as_ref().is_some_and(|existing| {
                existing.processing_status != SourceProcessingStatus::Ok
                    || existing.last_error.is_some()
            });
            let normalized = normalized_source_from_existing(
                &staged.discovered,
                staged.existing.as_ref(),
                Some(fingerprint.fast()),
                Some(fingerprint.content_sha256()),
                generation,
                SourceProcessingStatus::Ok,
                None,
                attempted_at_ms,
            );
            storage.refresh_model_source_unchanged_content(
                &normalized,
                fingerprint,
                attempted_at_ms,
            )?;
            if status_changed {
                notify_data_changed = Some(false);
            }
        }
        StagedSourceAction::SuppressedUnchanged => {
            let fingerprint = staged
                .fingerprint
                .as_ref()
                .ok_or_else(|| "Suppressed source lost its fingerprint".to_string())?;
            let normalized = normalized_source_from_existing(
                &staged.discovered,
                staged.existing.as_ref(),
                Some(fingerprint.fast()),
                Some(fingerprint.content_sha256()),
                generation,
                SourceProcessingStatus::Suppressed,
                None,
                attempted_at_ms,
            );
            storage.mark_suppressed_model_source_unchanged(
                &normalized,
                fingerprint,
                attempted_at_ms,
            )?;
        }
        StagedSourceAction::Replace => {
            let fingerprint = staged
                .fingerprint
                .as_ref()
                .ok_or_else(|| "Replacement source lost its fingerprint".to_string())?;
            let parsed = staged
                .parsed
                .as_ref()
                .ok_or_else(|| "Replacement source lost its parsed output".to_string())?;
            let observation_count = i64::try_from(parsed.observations.len()).map_err(|_| {
                "Model source observation count exceeds SQLite INTEGER range".to_string()
            })?;
            let normalized = normalized_source_from_parse(
                &staged.discovered,
                parsed,
                fingerprint,
                generation,
                attempted_at_ms,
                observation_count,
            );
            let replacement =
                storage.replace_model_source(&normalized, &parsed.observations, fingerprint)?;
            match replacement.outcome {
                ModelSourceReplacementOutcome::Replaced => {
                    result.disposition = ModelSourceReconciliationDisposition::Processed;
                    result.observations_written = replacement.observation_count;
                    result.data_changed = replacement.data_changed;
                    if replacement.data_changed {
                        notify_data_changed = Some(true);
                    }
                }
                ModelSourceReplacementOutcome::SuppressedUnchanged => {}
            }
        }
        StagedSourceAction::Fail => {
            let diagnostic = staged.diagnostic.clone().unwrap_or_else(|| {
                ModelUsageDiagnostic::new(ModelUsageDiagnosticKind::SourceParseFailed)
            });
            let status_changed = staged.existing.as_ref().is_none_or(|existing| {
                !matches!(
                    existing.processing_status,
                    SourceProcessingStatus::Stale
                        | SourceProcessingStatus::Failed
                        | SourceProcessingStatus::Suppressed
                ) || existing.last_error.as_ref() != Some(&diagnostic)
            });
            let normalized = normalized_source_from_existing(
                &staged.discovered,
                staged.existing.as_ref(),
                staged.fast,
                staged
                    .fingerprint
                    .as_ref()
                    .map(ModelSourceFingerprint::content_sha256),
                generation,
                SourceProcessingStatus::Failed,
                Some(diagnostic.clone()),
                attempted_at_ms,
            );
            storage.mark_model_source_failure(
                &normalized,
                staged.fast,
                &diagnostic,
                attempted_at_ms,
            )?;
            if status_changed {
                notify_data_changed = Some(false);
            }
            result.disposition = ModelSourceReconciliationDisposition::Failed;
            result.diagnostic = Some(diagnostic);
            result.retained_last_good = staged.existing.as_ref().is_some_and(|existing| {
                existing.processing_status != SourceProcessingStatus::Suppressed
                    && existing.suppressed_sha256.is_none()
                    && (existing.last_good.last_success_at_ms.is_some()
                        || existing.content_sha256.is_some())
            });
        }
    }

    let notify = notify_data_changed.map(|data_changed| ModelAnalyticsNotify {
        snapshot: event_snapshot,
        data_changed,
    });
    Ok(CommittedModelSource { result, notify })
}

#[allow(clippy::too_many_arguments)]
fn normalized_source_from_existing(
    discovered: &DiscoveredRetainedJsonlSource,
    existing: Option<&StoredModelSource>,
    fast: Option<ModelSourceFastFingerprint>,
    content_sha256: Option<&str>,
    generation: i64,
    processing_status: SourceProcessingStatus,
    last_error: Option<ModelUsageDiagnostic>,
    attempted_at_ms: i64,
) -> NormalizedSource {
    let last_good = existing.map(|source| &source.last_good);
    NormalizedSource {
        provider: discovered.provider,
        source_root_key: discovered.source_root_key.to_owned(),
        source_key: discovered.source_key.clone(),
        path: discovered.canonical_path.clone(),
        layout_hint: discovered.layout_hint.clone(),
        source_session_id: last_good.and_then(|source| source.source_session_id.clone()),
        analytics_session_id: last_good.and_then(|source| source.analytics_session_id.clone()),
        chain_id: last_good.and_then(|source| source.chain_id.clone()),
        parent_chain_id: last_good.and_then(|source| source.parent_chain_id.clone()),
        is_sidechain: last_good.is_some_and(|source| source.is_sidechain),
        agent_id: last_good.and_then(|source| source.agent_id.clone()),
        cwd: last_good.and_then(|source| source.cwd.clone()),
        hostname: last_good.and_then(|source| source.hostname.clone()),
        first_activity_at_ms: last_good.and_then(|source| source.first_activity_at_ms),
        last_activity_at_ms: last_good.and_then(|source| source.last_activity_at_ms),
        mtime_ns: fast.map(ModelSourceFastFingerprint::mtime_ns),
        size_bytes: fast.map(ModelSourceFastFingerprint::size_bytes),
        content_sha256: content_sha256
            .map(str::to_owned)
            .or_else(|| existing.and_then(|source| source.content_sha256.clone())),
        last_error,
        suppressed_sha256: existing.and_then(|source| source.suppressed_sha256.clone()),
        suppressed_at_ms: existing.and_then(|source| source.suppressed_at_ms),
        seen_generation: generation,
        processing_status,
        observation_count: last_good.map_or(0, |source| source.observation_count),
        last_attempt_at_ms: Some(attempted_at_ms),
        last_success_at_ms: last_good.and_then(|source| source.last_success_at_ms),
    }
}

fn normalized_source_from_parse(
    discovered: &DiscoveredRetainedJsonlSource,
    parsed: &ProviderAdapterParseResult,
    fingerprint: &ModelSourceFingerprint,
    generation: i64,
    attempted_at_ms: i64,
    observation_count: i64,
) -> NormalizedSource {
    let native = parsed.valid_native_source();
    NormalizedSource {
        provider: discovered.provider,
        source_root_key: discovered.source_root_key.to_owned(),
        source_key: discovered.source_key.clone(),
        path: discovered.canonical_path.clone(),
        layout_hint: discovered.layout_hint.clone(),
        source_session_id: native.map(|source| source.source_session_id.clone()),
        analytics_session_id: native.map(|source| source.analytics_session_id().to_owned()),
        chain_id: native.map(|source| source.chain_id.clone()),
        parent_chain_id: native.and_then(|source| source.parent_chain_id.clone()),
        is_sidechain: native.is_some_and(|source| source.is_sidechain),
        agent_id: native.and_then(|source| source.agent_id.clone()),
        cwd: native.and_then(|source| source.cwd.clone()),
        hostname: native.and_then(|source| source.hostname.clone()),
        first_activity_at_ms: native.map(|source| source.first_activity_at_ms),
        last_activity_at_ms: native.map(|source| source.last_activity_at_ms),
        mtime_ns: Some(fingerprint.fast().mtime_ns()),
        size_bytes: Some(fingerprint.fast().size_bytes()),
        content_sha256: Some(fingerprint.content_sha256().to_owned()),
        last_error: None,
        suppressed_sha256: None,
        suppressed_at_ms: None,
        seen_generation: generation,
        processing_status: SourceProcessingStatus::Ok,
        observation_count,
        last_attempt_at_ms: Some(attempted_at_ms),
        last_success_at_ms: Some(attempted_at_ms),
    }
}

/// Prune one root only from an explicit complete-inventory proof, then notify.
pub(crate) fn prune_completed_model_source_root(
    storage: &Storage,
    app_handle: &tauri::AppHandle,
    plan: &PreparedModelSourceReconciliation,
    provider: IntegrationProvider,
    source_root_key: &str,
    _permit: &mut ModelUsageRunnerPermit,
) -> Result<usize, String> {
    if !plan.is_complete() {
        return Err("Cannot prune model sources before all prepared sources commit".to_string());
    }
    let root_key = ProviderRootKey::new(provider, source_root_key);
    let completed_root = plan.completed_root_proofs.get(&root_key).ok_or_else(|| {
        "Cannot prune a model source root without captured complete inventory".to_string()
    })?;
    let event_snapshot = read_model_analytics_event_snapshot(storage)?;
    let result = storage.prune_model_sources_for_completed_root(completed_root)?;
    if result.data_changed {
        emit_model_analytics_updated(app_handle, &event_snapshot, true);
    }
    Ok(result.sources_pruned)
}

/// Emit the advisory refresh event from fields captured before the caller's commit.
/// No fallible storage work occurs here; window delivery remains best-effort.
pub(crate) fn emit_model_analytics_updated(
    app_handle: &tauri::AppHandle,
    snapshot: &ModelAnalyticsEventSnapshot,
    data_changed: bool,
) {
    let event = ModelAnalyticsUpdatedEvent {
        generation: snapshot.generation,
        status: snapshot.status,
        data_changed,
        updated_at: snapshot.updated_at.clone(),
    };
    if let Err(error) = app_handle.emit(MODEL_ANALYTICS_UPDATED_EVENT, event) {
        log::warn!("Model analytics update event could not be delivered: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_context<'a>(
        source_key: &'a str,
        layout_hint: &'a RetainedJsonlSourceLayoutHint,
    ) -> ClaudeAdapterContext<'a> {
        ClaudeAdapterContext {
            source_key,
            layout_hint,
            hostname: Some("host-a"),
        }
    }

    fn codex_context<'a>(
        source_key: &'a str,
        layout_hint: &'a RetainedJsonlSourceLayoutHint,
    ) -> CodexAdapterContext<'a> {
        CodexAdapterContext {
            source_key,
            layout_hint,
            hostname: Some("host-a"),
        }
    }

    fn claude_parent_hint() -> RetainedJsonlSourceLayoutHint {
        RetainedJsonlSourceLayoutHint::ClaudeParent {
            default_project: "proj".to_string(),
        }
    }

    fn codex_token_count_line(
        timestamp: &str,
        input_tokens: i64,
        cached_input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
    ) -> String {
        format!(
            "{{\"type\":\"event_msg\",\"timestamp\":\"{timestamp}\",\"payload\":{{\
             \"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\
             \"input_tokens\":{input_tokens},\"cached_input_tokens\":{cached_input_tokens},\
             \"output_tokens\":{output_tokens},\"cache_creation_tokens\":{cache_creation_tokens}}}}}}}}}"
        )
    }

    fn codex_session_meta_line(timestamp: &str, session_id: &str) -> String {
        format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"{timestamp}\",\
             \"payload\":{{\"id\":\"{session_id}\"}}}}"
        )
    }

    fn codex_turn_context_line(timestamp: &str, model: &str) -> String {
        format!(
            "{{\"type\":\"turn_context\",\"timestamp\":\"{timestamp}\",\
             \"payload\":{{\"model\":\"{model}\",\"turn_id\":\"turn-1\"}}}}"
        )
    }

    fn token_observations(result: &ProviderAdapterParseResult) -> Vec<&NormalizedObservation> {
        result
            .observations
            .iter()
            .filter(|observation| observation.metadata().kind == ObservationKind::Token)
            .collect()
    }

    fn has_diagnostic(result: &ProviderAdapterParseResult, kind: ModelUsageDiagnosticKind) -> bool {
        let expected = ModelUsageDiagnostic::new(kind);
        result
            .diagnostics
            .iter()
            .any(|entry| entry.diagnostic.as_str() == expected.as_str())
    }

    // ---- Claude transcript adapter edge cases ---------------------------

    // @lat: [[backend#Backend#Database#Schema#Model Analytics Test Specs#Claude Transcript Adapter Edge Cases]]
    #[test]
    fn claude_truncated_final_line_preserves_prior_observations() {
        let hint = claude_parent_hint();
        let contents = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-1","message":{"model":"claude-opus-4","usage":{"input_tokens":10,"output_tokens":5}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:01:00Z","sessionId":"sess-1","message":{"model":"claude-opus-4","usage":{"input_tokens":7,"output_tokens":3}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-"#,
        );

        let result = parse_claude_model_usage_jsonl(contents, claude_context("sk-claude", &hint));

        assert_eq!(result.observations.len(), 2);
        assert_eq!(result.counts.malformed_json_records, 1);
        assert_eq!(result.counts.observations_emitted, 2);
    }

    #[test]
    fn claude_sidechain_without_agent_id_is_dropped() {
        let hint = claude_parent_hint();
        let contents = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-1","isSidechain":true,"message":{"model":"claude-opus-4"}}"#;

        let result = parse_claude_model_usage_jsonl(contents, claude_context("sk-claude", &hint));

        assert!(result.observations.is_empty());
        assert_eq!(result.counts.invalid_identity_records, 1);
    }

    #[test]
    fn claude_sidechain_maps_chain_and_parent_from_agent_and_session() {
        let hint = RetainedJsonlSourceLayoutHint::ClaudeSubagent {
            default_project: "proj".to_string(),
        };
        let contents = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-1","isSidechain":true,"agentId":"agent-x","message":{"model":"claude-opus-4"}}"#;

        let result = parse_claude_model_usage_jsonl(contents, claude_context("sk-claude", &hint));

        assert_eq!(result.observations.len(), 1);
        let metadata = result.observations[0].metadata();
        assert_eq!(metadata.chain_id, "agent-x");
        assert_eq!(metadata.parent_chain_id.as_deref(), Some("sess-1"));
        assert!(metadata.is_sidechain);
        assert_eq!(metadata.agent_id.as_deref(), Some("agent-x"));
    }

    #[test]
    fn claude_negative_epoch_timestamp_is_rejected() {
        let hint = claude_parent_hint();
        let contents = r#"{"type":"assistant","timestamp":"1969-01-01T00:00:00Z","sessionId":"sess-1","message":{"model":"claude-opus-4"}}"#;

        let result = parse_claude_model_usage_jsonl(contents, claude_context("sk-claude", &hint));

        assert!(result.observations.is_empty());
        assert_eq!(result.counts.invalid_timestamp_records, 1);
    }

    #[test]
    fn claude_missing_or_non_string_type_counts_unsupported_shape() {
        let hint = claude_parent_hint();
        let contents = concat!(
            "{}",
            "\n",
            r#"{"type":123,"timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-1","message":{}}"#,
        );

        let result = parse_claude_model_usage_jsonl(contents, claude_context("sk-claude", &hint));

        assert!(result.observations.is_empty());
        assert_eq!(result.counts.unsupported_shape_records, 2);
    }

    #[test]
    fn claude_invalid_token_dimension_emits_observation_with_unavailable_tokens() {
        let hint = claude_parent_hint();
        // Each row carries a single invalid dimension and no valid ones, so the
        // observation still lands but with unavailable token evidence.
        for usage in [r#"{"input_tokens":-5}"#, r#"{"input_tokens":1.5}"#] {
            let contents = format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-1","message":{{"model":"claude-opus-4","usage":{usage}}}}}"#
            );

            let result =
                parse_claude_model_usage_jsonl(&contents, claude_context("sk-claude", &hint));

            assert_eq!(result.observations.len(), 1, "usage {usage}");
            assert_eq!(
                result.counts.invalid_token_dimension_values, 1,
                "usage {usage}"
            );
            assert!(
                has_diagnostic(&result, ModelUsageDiagnosticKind::InvalidTokenDimension),
                "usage {usage} missing invalid-dimension diagnostic"
            );
            let observation = &result.observations[0];
            assert_eq!(
                observation.token_evidence(),
                TokenEvidence::Unavailable,
                "usage {usage}"
            );
            assert_eq!(observation.input_tokens(), None, "usage {usage}");
            assert_eq!(
                observation.raw_model_id(),
                Some("claude-opus-4"),
                "usage {usage}"
            );
        }
    }

    #[test]
    fn claude_model_id_trims_whitespace_and_keeps_blank_or_missing_as_none() {
        let hint = claude_parent_hint();
        struct Case {
            model_field: &'static str,
            expected_raw: Option<&'static str>,
            expected_evidence: ModelEvidence,
        }
        let cases = [
            Case {
                model_field: r#","model":"  claude-opus-4  ""#,
                expected_raw: Some("claude-opus-4"),
                expected_evidence: ModelEvidence::Explicit,
            },
            Case {
                model_field: r#","model":"   ""#,
                expected_raw: None,
                expected_evidence: ModelEvidence::Invalid,
            },
            Case {
                model_field: "",
                expected_raw: None,
                expected_evidence: ModelEvidence::Missing,
            },
        ];

        for case in cases {
            let contents = format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-1","message":{{"usage":{{"input_tokens":1}}{}}}}}"#,
                case.model_field
            );

            let result =
                parse_claude_model_usage_jsonl(&contents, claude_context("sk-claude", &hint));

            assert_eq!(result.observations.len(), 1, "field {}", case.model_field);
            let observation = &result.observations[0];
            assert_eq!(
                observation.raw_model_id(),
                case.expected_raw,
                "field {}",
                case.model_field
            );
            assert_eq!(
                observation.model_evidence(),
                case.expected_evidence,
                "field {}",
                case.model_field
            );
        }
    }

    // ---- Codex cumulative-delta reconstruction --------------------------

    // @lat: [[backend#Backend#Database#Schema#Model Analytics Test Specs#Codex Cumulative Delta Reconstruction]]
    #[test]
    fn codex_monotonic_totals_reconstruct_per_turn_deltas() {
        let hint = RetainedJsonlSourceLayoutHint::CodexTranscript;
        let contents = [
            codex_session_meta_line("2026-01-01T00:00:00Z", "sess-1"),
            codex_token_count_line("2026-01-01T00:00:01Z", 100, 0, 50, 0),
            codex_token_count_line("2026-01-01T00:00:02Z", 250, 40, 130, 0),
            codex_token_count_line("2026-01-01T00:00:03Z", 400, 90, 200, 0),
        ]
        .join("\n");

        let result = parse_codex_model_usage_jsonl(&contents, codex_context("sk-codex", &hint));

        let tokens = token_observations(&result);
        assert_eq!(tokens.len(), 3);
        // Inclusive input decomposes into new-input plus cache-read per interval.
        let expected = [
            (100_i64, 50_i64, 0_i64, 0_i64),
            (110, 80, 0, 40),
            (100, 70, 0, 50),
        ];
        for (index, (input, output, cache_creation, cache_read)) in expected.iter().enumerate() {
            let observation = tokens[index];
            assert_eq!(observation.token_evidence(), TokenEvidence::CumulativeDelta);
            assert_eq!(
                observation.input_tokens(),
                Some(*input),
                "turn {index} input"
            );
            assert_eq!(
                observation.output_tokens(),
                Some(*output),
                "turn {index} output"
            );
            assert_eq!(
                observation.cache_creation_tokens(),
                Some(*cache_creation),
                "turn {index} cache_creation"
            );
            assert_eq!(
                observation.cache_read_tokens(),
                Some(*cache_read),
                "turn {index} cache_read"
            );
        }
        assert_eq!(result.counts.cumulative_reset_dimensions, 0);
    }

    #[test]
    fn codex_decreasing_counter_resets_baseline_without_negative_delta() {
        let hint = RetainedJsonlSourceLayoutHint::CodexTranscript;
        let contents = [
            codex_session_meta_line("2026-01-01T00:00:00Z", "sess-1"),
            codex_token_count_line("2026-01-01T00:00:01Z", 100, 0, 200, 0),
            // Output counter drops from 200 to 50: baseline resets to the new
            // value instead of underflowing into a negative delta.
            codex_token_count_line("2026-01-01T00:00:02Z", 200, 0, 50, 0),
        ]
        .join("\n");

        let result = parse_codex_model_usage_jsonl(&contents, codex_context("sk-codex", &hint));

        let tokens = token_observations(&result);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].output_tokens(), Some(200));
        assert_eq!(tokens[1].output_tokens(), Some(50));
        assert_eq!(result.counts.cumulative_reset_dimensions, 1);
        assert!(has_diagnostic(
            &result,
            ModelUsageDiagnosticKind::CumulativeTokenReset
        ));
        for observation in &result.observations {
            for dimension in [
                observation.input_tokens(),
                observation.output_tokens(),
                observation.cache_creation_tokens(),
                observation.cache_read_tokens(),
            ]
            .into_iter()
            .flatten()
            {
                assert!(dimension >= 0, "no reconstructed delta may be negative");
            }
        }
    }

    #[test]
    fn codex_session_meta_after_events_still_attributes_observations() {
        let hint = RetainedJsonlSourceLayoutHint::CodexTranscript;
        // session_meta trails the observation records; the two-pass parser must
        // still resolve native identity before attributing them.
        let contents = [
            codex_token_count_line("2026-01-01T00:00:01Z", 100, 0, 50, 0),
            codex_turn_context_line("2026-01-01T00:00:02Z", "gpt-5-codex"),
            codex_session_meta_line("2026-01-01T00:00:03Z", "sess-1"),
        ]
        .join("\n");

        let result = parse_codex_model_usage_jsonl(&contents, codex_context("sk-codex", &hint));

        assert_eq!(result.observations.len(), 2);
        assert_eq!(result.counts.invalid_identity_records, 0);
        assert!(matches!(
            result.native_identity,
            ProviderNativeIdentityState::Valid(_)
        ));
        for observation in &result.observations {
            assert_eq!(observation.metadata().source_session_id, "sess-1");
            assert_eq!(observation.metadata().analytics_session_id, "sess-1");
        }
    }

    #[test]
    fn codex_turn_context_model_stays_separate_from_token_deltas() {
        let hint = RetainedJsonlSourceLayoutHint::CodexTranscript;
        let contents = [
            codex_session_meta_line("2026-01-01T00:00:00Z", "sess-1"),
            codex_turn_context_line("2026-01-01T00:00:01Z", "gpt-5-codex"),
            codex_token_count_line("2026-01-01T00:00:02Z", 100, 0, 50, 0),
        ]
        .join("\n");

        let result = parse_codex_model_usage_jsonl(&contents, codex_context("sk-codex", &hint));

        assert_eq!(result.observations.len(), 2);
        // turn_context carries model identity but never invents token deltas.
        let turn = &result.observations[0];
        assert_eq!(turn.metadata().kind, ObservationKind::Turn);
        assert_eq!(turn.model_evidence(), ModelEvidence::Explicit);
        assert_eq!(turn.raw_model_id(), Some("gpt-5-codex"));
        assert_eq!(turn.token_evidence(), TokenEvidence::Unavailable);
        assert_eq!(turn.input_tokens(), None);
        // token_count carries deltas but never invents model identity.
        let token = &result.observations[1];
        assert_eq!(token.metadata().kind, ObservationKind::Token);
        assert_eq!(token.model_evidence(), ModelEvidence::Missing);
        assert_eq!(token.raw_model_id(), None);
        assert_eq!(token.token_evidence(), TokenEvidence::CumulativeDelta);
        assert_eq!(token.input_tokens(), Some(100));
        assert_eq!(token.output_tokens(), Some(50));
    }
}
