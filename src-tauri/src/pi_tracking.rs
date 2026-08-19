use std::collections::HashSet;

use chrono::DateTime;
use serde_json::{Map, Value};

pub use crate::models::{
    PI_PROTOCOL_V2, PI_PROTOCOL_V2_CAPABILITIES, PI_PROTOCOL_V2_CAPABILITY_DIGEST,
    PI_PROTOCOL_V2_QUILL_BUILD, PI_PROTOCOL_V2_REPORTER_VERSION, PI_PROTOCOL_V2_TRACKING_SCHEMA,
    PiProtocolV2DeliverySource, PiProtocolV2EndReason, PiProtocolV2Envelope, PiProtocolV2ErrorCode,
    PiProtocolV2Event, PiProtocolV2EventKind, PiProtocolV2Generation, PiProtocolV2Lineage,
    PiProtocolV2OpenEnvelope, PiProtocolV2Outcome, PiProtocolV2Provider, PiProtocolV2Reporter,
    PiProtocolV2Response, PiProtocolV2StartReason, PiProtocolV2TrackingData,
    PiProtocolV2TrackingEntry,
};

const MAX_EVENTS: usize = 200;
const MAX_NAME_BYTES: usize = 256;
pub(crate) const PI_REPORTER_ACTIVE_TTL_MS: i64 = 15 * 60 * 1_000;
pub(crate) const PI_REPORTER_TERMINAL_RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;
pub(crate) const PI_REPORTER_ROWS_PER_HOST: usize = 4_096;

pub(crate) const PI_REPORTER_HOST_HEADER: &str = "x-quill-pi-host";
pub(crate) const PI_REPORTER_PROCESS_HEADER: &str = "x-quill-pi-process";
pub(crate) const PI_REPORTER_CHANNEL_HEADER: &str = "x-quill-pi-channel";
pub(crate) const PI_REPORTER_PROTOCOL_HEADER: &str = "x-quill-pi-protocol";
pub(crate) const PI_REPORTER_VERSION_HEADER: &str = "x-quill-pi-reporter";
pub(crate) const PI_REPORTER_BUILD_HEADER: &str = "x-quill-pi-build";
pub(crate) const PI_REPORTER_CAPABILITY_HEADER: &str = "x-quill-pi-capability";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PiReporterHealthSubject {
    pub(crate) normalized_hostname: String,
    pub(crate) process_instance_id: String,
    pub(crate) install_channel: String,
    pub(crate) protocol: u32,
    pub(crate) reporter_version: String,
    pub(crate) quill_build: String,
    pub(crate) capability_digest: String,
}

impl PiReporterHealthSubject {
    pub(crate) fn new(
        normalized_hostname: &str,
        process_instance_id: &str,
        install_channel: &str,
        protocol: u32,
        reporter_version: &str,
        quill_build: &str,
        capability_digest: &str,
    ) -> Result<Self, String> {
        validate_health_name(process_instance_id, "process instance")?;
        validate_health_name(reporter_version, "reporter version")?;
        validate_health_name(quill_build, "Quill build")?;
        validate_health_name(capability_digest, "capability digest")?;
        if !matches!(
            install_channel,
            "managed" | "npm" | "project" | "development" | "unknown" | "live"
        ) {
            return Err("Invalid reporter install channel".to_owned());
        }
        validate_host(normalized_hostname).map_err(|error| error.message)?;
        Ok(Self {
            normalized_hostname: normalized_hostname.to_owned(),
            process_instance_id: process_instance_id.to_owned(),
            install_channel: install_channel.to_owned(),
            protocol,
            reporter_version: reporter_version.to_owned(),
            quill_build: quill_build.to_owned(),
            capability_digest: capability_digest.to_owned(),
        })
    }

    pub(crate) fn from_envelope(
        envelope: &PiProtocolV2Envelope,
        install_channel: &str,
    ) -> Result<Self, String> {
        let first = envelope
            .events
            .first()
            .ok_or_else(|| "Reporter envelope has no events".to_owned())?;
        if envelope.events.iter().any(|event| {
            event.normalized_host != first.normalized_host
                || event.process_instance_id != first.process_instance_id
        }) {
            return Err("Reporter envelope mixes host or process identities".to_owned());
        }
        Self::new(
            &first.normalized_host,
            &first.process_instance_id,
            install_channel,
            envelope.protocol,
            &envelope.reporter_version,
            &envelope.quill_build,
            &envelope.capability_digest,
        )
    }
}

fn validate_health_name(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_NAME_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!("Invalid {label}"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PiReporterHealthDimension {
    Compatibility,
    Lifecycle,
    ChildAck,
    Source,
    Transport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PiReporterHealthCode {
    ProtocolMismatch,
    UnknownSession,
    ChildReporterMissing,
    SourceRecovering,
    ReconciliationFailed,
    TelemetryRejected,
    Saturated,
}

impl PiReporterHealthCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::UnknownSession => "unknown_session",
            Self::ChildReporterMissing => "child_reporter_missing",
            Self::SourceRecovering => "source_recovering",
            Self::ReconciliationFailed => "reconciliation_failed",
            Self::TelemetryRejected => "telemetry_rejected",
            Self::Saturated => "saturated",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "protocol_mismatch"
            | "reporter_version_mismatch"
            | "quill_build_mismatch"
            | "capability_mismatch"
            | "tracking_schema_mismatch" => Some(Self::ProtocolMismatch),
            "unknown_session" | "reannounce_required" => Some(Self::UnknownSession),
            "child_reporter_missing" => Some(Self::ChildReporterMissing),
            "source_not_persisted" | "source_recovering" => Some(Self::SourceRecovering),
            "reconciliation_failed" => Some(Self::ReconciliationFailed),
            "saturated" => Some(Self::Saturated),
            "rate_limited" | "unavailable" | "telemetry_rejected" | "invalid_telemetry" => {
                Some(Self::TelemetryRejected)
            }
            _ => None,
        }
    }

    pub(crate) fn dimension(self) -> PiReporterHealthDimension {
        match self {
            Self::ProtocolMismatch => PiReporterHealthDimension::Compatibility,
            Self::UnknownSession => PiReporterHealthDimension::Lifecycle,
            Self::ChildReporterMissing => PiReporterHealthDimension::ChildAck,
            Self::SourceRecovering | Self::ReconciliationFailed => {
                PiReporterHealthDimension::Source
            }
            Self::TelemetryRejected | Self::Saturated => PiReporterHealthDimension::Transport,
        }
    }

    fn severity(self) -> u8 {
        match self {
            Self::ProtocolMismatch => 7,
            Self::ReconciliationFailed => 6,
            Self::ChildReporterMissing => 5,
            Self::UnknownSession => 4,
            Self::Saturated => 3,
            Self::TelemetryRejected => 2,
            Self::SourceRecovering => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PiReporterHealthRow {
    pub(crate) subject: PiReporterHealthSubject,
    pub(crate) last_acceptance_ms: Option<i64>,
    pub(crate) last_heartbeat_ms: Option<i64>,
    pub(crate) compatibility_state: String,
    pub(crate) lifecycle_state: String,
    pub(crate) child_ack_state: String,
    pub(crate) source_state: String,
    pub(crate) transport_state: String,
    pub(crate) latest_code: Option<String>,
    pub(crate) affected_sessions: usize,
    pub(crate) recovered_at_ms: Option<i64>,
    pub(crate) resolved_at_ms: Option<i64>,
}

impl PiReporterHealthRow {
    fn active_code(&self) -> Option<PiReporterHealthCode> {
        if self.compatibility_state != "compatible" {
            return Some(PiReporterHealthCode::ProtocolMismatch);
        }
        if self.source_state == "reconciliation_failed" {
            return Some(PiReporterHealthCode::ReconciliationFailed);
        }
        if self.child_ack_state == "missing" {
            return Some(PiReporterHealthCode::ChildReporterMissing);
        }
        if self.lifecycle_state == "unknown_session" {
            return Some(PiReporterHealthCode::UnknownSession);
        }
        if self.transport_state == "failed" {
            return self
                .latest_code
                .as_deref()
                .and_then(PiReporterHealthCode::from_str)
                .or(Some(PiReporterHealthCode::TelemetryRejected));
        }
        if matches!(
            self.source_state.as_str(),
            "source_not_persisted" | "recovering"
        ) || self.lifecycle_state == "recovering"
        {
            return Some(PiReporterHealthCode::SourceRecovering);
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PiReporterHealthSummary {
    pub(crate) worst_code: Option<PiReporterHealthCode>,
    pub(crate) worst_subject: Option<PiReporterHealthSubject>,
    pub(crate) affected_reporters: usize,
    pub(crate) affected_sessions: usize,
    pub(crate) active_reporters: usize,
    pub(crate) saturated_reporters: usize,
    pub(crate) last_acceptance_ms: Option<i64>,
    pub(crate) last_heartbeat_ms: Option<i64>,
    pub(crate) recovered_at_ms: Option<i64>,
}

pub(crate) fn summarize_reporter_health(
    rows: &[PiReporterHealthRow],
    saturated_reporters: usize,
    now_ms: i64,
) -> Option<PiReporterHealthSummary> {
    if rows.is_empty() && saturated_reporters == 0 {
        return None;
    }
    let active_cutoff = now_ms.saturating_sub(PI_REPORTER_ACTIVE_TTL_MS);
    let active = rows
        .iter()
        .filter(|row| {
            row.resolved_at_ms.is_none()
                && row
                    .last_heartbeat_ms
                    .is_some_and(|heartbeat| heartbeat >= active_cutoff)
        })
        .collect::<Vec<_>>();
    let mut affected = active
        .iter()
        .filter_map(|row| row.active_code().map(|code| (*row, code)))
        .collect::<Vec<_>>();
    affected.sort_by(|(left_row, left_code), (right_row, right_code)| {
        right_code
            .severity()
            .cmp(&left_code.severity())
            .then_with(|| right_row.last_heartbeat_ms.cmp(&left_row.last_heartbeat_ms))
            .then_with(|| {
                left_row
                    .subject
                    .process_instance_id
                    .cmp(&right_row.subject.process_instance_id)
            })
    });
    let worst = affected.first().copied();
    let saturation_wins = saturated_reporters > 0
        && worst
            .is_none_or(|(_, code)| PiReporterHealthCode::Saturated.severity() > code.severity());
    Some(PiReporterHealthSummary {
        worst_code: saturation_wins
            .then_some(PiReporterHealthCode::Saturated)
            .or_else(|| worst.map(|(_, code)| code)),
        worst_subject: (!saturation_wins)
            .then(|| worst.map(|(row, _)| row.subject.clone()))
            .flatten(),
        affected_reporters: affected
            .len()
            .saturating_add(usize::from(saturated_reporters > 0)),
        affected_sessions: affected.iter().map(|(row, _)| row.affected_sessions).sum(),
        active_reporters: active.len(),
        saturated_reporters,
        last_acceptance_ms: rows.iter().filter_map(|row| row.last_acceptance_ms).max(),
        last_heartbeat_ms: rows.iter().filter_map(|row| row.last_heartbeat_ms).max(),
        recovered_at_ms: rows.iter().filter_map(|row| row.recovered_at_ms).max(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiProtocolV2DecodeError {
    pub code: PiProtocolV2ErrorCode,
    pub message: String,
}

impl PiProtocolV2DecodeError {
    fn new(code: PiProtocolV2ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn protocol_field(object: &Map<String, Value>) -> Option<u32> {
    object
        .get("protocol")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

/// The protocol a tracking envelope declares, read before any generation
/// validation so the route can dispatch the shape it actually received.
/// `None` leaves the typed rejection to the decoder for that shape.
pub fn envelope_protocol(bytes: &[u8]) -> Option<u32> {
    protocol_field(parse_json(bytes).ok()?.as_object()?)
}

pub fn decode_protocol_v2_envelope(
    bytes: &[u8],
) -> Result<PiProtocolV2Envelope, PiProtocolV2DecodeError> {
    let value = parse_json(bytes)?;
    let object = object(&value, PiProtocolV2ErrorCode::InvalidEnvelope, "envelope")?;
    let protocol = protocol_field(object).ok_or_else(|| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEnvelope,
            "Envelope protocol must be an integer",
        )
    })?;
    if protocol != PI_PROTOCOL_V2 {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::ProtocolMismatch,
            format!("Unsupported protocol {protocol}; expected {PI_PROTOCOL_V2}"),
        ));
    }
    exact_keys(
        object,
        &[
            "protocol",
            "reporter_version",
            "quill_build",
            "capability_digest",
            "events",
        ],
        PiProtocolV2ErrorCode::InvalidEnvelope,
    )?;
    let open: PiProtocolV2OpenEnvelope =
        serde_json::from_value(value.clone()).map_err(|error| {
            PiProtocolV2DecodeError::new(
                PiProtocolV2ErrorCode::InvalidEnvelope,
                format!("Invalid protocol-v2 envelope: {error}"),
            )
        })?;
    validate_generation(&PiProtocolV2Generation {
        protocol: open.protocol,
        reporter_version: open.reporter_version.clone(),
        quill_build: open.quill_build.clone(),
        capability_digest: open.capability_digest.clone(),
    })?;
    if open.events.is_empty() || open.events.len() > MAX_EVENTS {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEnvelope,
            format!("events must contain 1..={MAX_EVENTS} items"),
        ));
    }
    for event in &open.events {
        validate_event_value(event)?;
    }

    let envelope: PiProtocolV2Envelope = serde_json::from_slice(bytes).map_err(|error| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            format!("Invalid protocol-v2 event: {error}"),
        )
    })?;
    let mut event_ids = HashSet::with_capacity(envelope.events.len());
    for event in &envelope.events {
        validate_event(event)?;
        if !event_ids.insert(event.event_uuid.as_str()) {
            return Err(PiProtocolV2DecodeError::new(
                PiProtocolV2ErrorCode::InvalidEvent,
                "Duplicate event_uuid",
            ));
        }
    }
    Ok(envelope)
}

pub fn decode_protocol_v2_tracking_entry(
    bytes: &[u8],
) -> Result<PiProtocolV2TrackingEntry, PiProtocolV2DecodeError> {
    let value = parse_json(bytes)?;
    let outer = object(
        &value,
        PiProtocolV2ErrorCode::InvalidEntry,
        "tracking entry",
    )?;
    exact_keys(
        outer,
        &["type", "customType", "data"],
        PiProtocolV2ErrorCode::InvalidEntry,
    )?;
    if outer.get("type").and_then(Value::as_str) != Some("custom")
        || outer.get("customType").and_then(Value::as_str) != Some("quill-tracking")
    {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEntry,
            "Expected a quill-tracking custom entry",
        ));
    }

    let data = object(
        outer.get("data").unwrap_or(&Value::Null),
        PiProtocolV2ErrorCode::InvalidEntry,
        "tracking data",
    )?;
    let mut event = data.clone();
    let schema = event.remove("schema").ok_or_else(|| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEntry,
            "Tracking data is missing schema",
        )
    })?;
    let reporter = event.remove("reporter").ok_or_else(|| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEntry,
            "Tracking data is missing reporter metadata",
        )
    })?;
    let schema = schema.as_u64().ok_or_else(|| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEntry,
            "Tracking schema must be an integer",
        )
    })?;
    if schema != u64::from(PI_PROTOCOL_V2_TRACKING_SCHEMA) {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::TrackingSchemaMismatch,
            format!(
                "Unsupported tracking schema {schema}; expected {PI_PROTOCOL_V2_TRACKING_SCHEMA}"
            ),
        ));
    }
    validate_reporter_value(&reporter)?;
    validate_event_value(&Value::Object(event))?;

    let entry: PiProtocolV2TrackingEntry = serde_json::from_slice(bytes).map_err(|error| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEntry,
            format!("Invalid quill-tracking entry: {error}"),
        )
    })?;
    validate_event(&entry.data.event)?;
    Ok(entry)
}

pub fn decode_protocol_v2_response(
    bytes: &[u8],
) -> Result<PiProtocolV2Response, PiProtocolV2DecodeError> {
    let response: PiProtocolV2Response = serde_json::from_slice(bytes).map_err(|error| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::MalformedJson,
            format!("Invalid protocol-v2 response: {error}"),
        )
    })?;
    match &response {
        PiProtocolV2Response::Accepted {
            quill_build,
            protocol,
            reporter_version,
            capability_digest,
            outcomes,
        } => {
            validate_generation(&PiProtocolV2Generation {
                protocol: *protocol,
                reporter_version: reporter_version.clone(),
                quill_build: quill_build.clone(),
                capability_digest: capability_digest.clone(),
            })?;
            if outcomes.is_empty()
                || outcomes.iter().any(|outcome| {
                    matches!(outcome, crate::models::PiProtocolV2Outcome::UnknownSession)
                })
            {
                return Err(PiProtocolV2DecodeError::new(
                    PiProtocolV2ErrorCode::InvalidEnvelope,
                    "Accepted responses require applied, duplicate, or stale outcomes",
                ));
            }
        }
        PiProtocolV2Response::Error { code, required, .. } => {
            if matches!(
                code,
                PiProtocolV2ErrorCode::ProtocolMismatch
                    | PiProtocolV2ErrorCode::ReporterVersionMismatch
                    | PiProtocolV2ErrorCode::QuillBuildMismatch
                    | PiProtocolV2ErrorCode::CapabilityMismatch
            ) {
                let required = required.as_ref().ok_or_else(|| {
                    PiProtocolV2DecodeError::new(
                        PiProtocolV2ErrorCode::InvalidEnvelope,
                        "Compatibility errors require exact generation metadata",
                    )
                })?;
                validate_generation(required)?;
            }
        }
    }
    Ok(response)
}

fn parse_json(bytes: &[u8]) -> Result<Value, PiProtocolV2DecodeError> {
    serde_json::from_slice(bytes).map_err(|error| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::MalformedJson,
            format!("Malformed JSON: {error}"),
        )
    })
}

fn object<'a>(
    value: &'a Value,
    code: PiProtocolV2ErrorCode,
    label: &str,
) -> Result<&'a Map<String, Value>, PiProtocolV2DecodeError> {
    value
        .as_object()
        .ok_or_else(|| PiProtocolV2DecodeError::new(code, format!("{label} must be a JSON object")))
}

fn exact_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    code: PiProtocolV2ErrorCode,
) -> Result<(), PiProtocolV2DecodeError> {
    let allowed = allowed.iter().copied().collect::<HashSet<_>>();
    if let Some(key) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(PiProtocolV2DecodeError::new(
            code,
            format!("Unknown field {key}"),
        ));
    }
    Ok(())
}

fn validate_generation(generation: &PiProtocolV2Generation) -> Result<(), PiProtocolV2DecodeError> {
    if generation.protocol != PI_PROTOCOL_V2 {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::ProtocolMismatch,
            format!(
                "Unsupported protocol {}; expected {PI_PROTOCOL_V2}",
                generation.protocol
            ),
        ));
    }
    if generation.reporter_version != PI_PROTOCOL_V2_REPORTER_VERSION {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::ReporterVersionMismatch,
            format!(
                "Unsupported reporter {}; expected {PI_PROTOCOL_V2_REPORTER_VERSION}",
                generation.reporter_version
            ),
        ));
    }
    if generation.quill_build != PI_PROTOCOL_V2_QUILL_BUILD {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::QuillBuildMismatch,
            format!(
                "Unsupported Quill build {}; expected {PI_PROTOCOL_V2_QUILL_BUILD}",
                generation.quill_build
            ),
        ));
    }
    if generation.capability_digest != PI_PROTOCOL_V2_CAPABILITY_DIGEST {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::CapabilityMismatch,
            "Unsupported capability digest",
        ));
    }
    Ok(())
}

fn validate_reporter_value(value: &Value) -> Result<(), PiProtocolV2DecodeError> {
    let reporter = object(
        value,
        PiProtocolV2ErrorCode::InvalidEntry,
        "reporter metadata",
    )?;
    exact_keys(
        reporter,
        &["protocol", "version", "quill_build", "capability_digest"],
        PiProtocolV2ErrorCode::InvalidEntry,
    )?;
    let generation = PiProtocolV2Generation {
        protocol: reporter
            .get("protocol")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                PiProtocolV2DecodeError::new(
                    PiProtocolV2ErrorCode::InvalidEntry,
                    "Reporter protocol must be an integer",
                )
            })?,
        reporter_version: reporter
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        quill_build: reporter
            .get("quill_build")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        capability_digest: reporter
            .get("capability_digest")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    };
    validate_generation(&generation)
}

fn validate_event_value(value: &Value) -> Result<(), PiProtocolV2DecodeError> {
    let event = object(value, PiProtocolV2ErrorCode::InvalidEvent, "event")?;
    let event_kind = event.get("event").and_then(Value::as_str).ok_or_else(|| {
        PiProtocolV2DecodeError::new(PiProtocolV2ErrorCode::InvalidEvent, "Missing event kind")
    })?;
    let mut allowed = vec![
        "event_uuid",
        "event",
        "provider",
        "normalized_host",
        "session_id",
        "process_instance_id",
        "sequence",
        "origin_at",
        "occurred_at",
        "delivery_source",
    ];
    match event_kind {
        "session_start" => {
            allowed.extend(["reason", "previous_session_id", "lineage", "agent_role"])
        }
        "session_end" => allowed.push("reason"),
        "lineage" => allowed.extend(["lineage", "agent_role"]),
        _ => {
            return Err(PiProtocolV2DecodeError::new(
                PiProtocolV2ErrorCode::InvalidEvent,
                format!("Unknown event kind {event_kind}"),
            ));
        }
    }
    exact_keys(event, &allowed, PiProtocolV2ErrorCode::InvalidEvent)?;
    for optional in ["previous_session_id", "agent_role"] {
        if event.get(optional).is_some_and(Value::is_null) {
            return Err(PiProtocolV2DecodeError::new(
                PiProtocolV2ErrorCode::InvalidEvent,
                format!("Optional field {optional} must be omitted, not null"),
            ));
        }
    }
    if let Some(lineage) = event.get("lineage") {
        validate_lineage_value(lineage)?;
    }
    Ok(())
}

fn validate_lineage_value(value: &Value) -> Result<(), PiProtocolV2DecodeError> {
    let lineage = object(value, PiProtocolV2ErrorCode::InvalidEvent, "lineage")?;
    match lineage.get("kind").and_then(Value::as_str) {
        Some("root") => exact_keys(lineage, &["kind"], PiProtocolV2ErrorCode::InvalidEvent),
        Some("linked" | "agent") => exact_keys(
            lineage,
            &["kind", "parent_session_id"],
            PiProtocolV2ErrorCode::InvalidEvent,
        ),
        Some("unresolved") => exact_keys(
            lineage,
            &["kind", "reason"],
            PiProtocolV2ErrorCode::InvalidEvent,
        ),
        Some(kind) => Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            format!("Unknown lineage kind {kind}"),
        )),
        None => Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            "Missing lineage kind",
        )),
    }
}

fn validate_event(event: &PiProtocolV2Event) -> Result<(), PiProtocolV2DecodeError> {
    validate_name(&event.event_uuid, "event_uuid")?;
    validate_host(&event.normalized_host)?;
    validate_name(&event.session_id, "session_id")?;
    validate_name(&event.process_instance_id, "process_instance_id")?;
    if event.sequence == 0 || i64::try_from(event.sequence).is_err() {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            "sequence must fit a positive signed integer",
        ));
    }
    let origin = parse_timestamp(&event.origin_at, "origin_at")?;
    let occurred = parse_timestamp(&event.occurred_at, "occurred_at")?;
    if origin.timestamp_millis() < 0 || occurred < origin {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            "occurred_at cannot precede origin_at",
        ));
    }

    match &event.kind {
        PiProtocolV2EventKind::SessionStart {
            reason,
            previous_session_id,
            lineage,
            agent_role,
        } => {
            if let Some(previous) = previous_session_id {
                validate_name(previous, "previous_session_id")?;
                if previous == &event.session_id {
                    return Err(PiProtocolV2DecodeError::new(
                        PiProtocolV2ErrorCode::InvalidEvent,
                        "previous_session_id cannot equal session_id",
                    ));
                }
            }
            if matches!(
                reason,
                crate::models::PiProtocolV2StartReason::Startup
                    | crate::models::PiProtocolV2StartReason::Reload
            ) && previous_session_id.is_some()
            {
                return Err(PiProtocolV2DecodeError::new(
                    PiProtocolV2ErrorCode::InvalidEvent,
                    "startup and reload must omit previous_session_id",
                ));
            }
            validate_lineage(lineage, &event.session_id)?;
            validate_agent_role(agent_role.as_deref(), lineage)?;
        }
        PiProtocolV2EventKind::SessionEnd { .. } => {}
        PiProtocolV2EventKind::Lineage {
            lineage,
            agent_role,
        } => {
            validate_lineage(lineage, &event.session_id)?;
            validate_agent_role(agent_role.as_deref(), lineage)?;
        }
    }
    Ok(())
}

fn validate_lineage(
    lineage: &PiProtocolV2Lineage,
    session_id: &str,
) -> Result<(), PiProtocolV2DecodeError> {
    match lineage {
        PiProtocolV2Lineage::Root => Ok(()),
        PiProtocolV2Lineage::Linked { parent_session_id }
        | PiProtocolV2Lineage::Agent { parent_session_id } => {
            validate_name(parent_session_id, "parent_session_id")?;
            if parent_session_id == session_id {
                return Err(PiProtocolV2DecodeError::new(
                    PiProtocolV2ErrorCode::InvalidEvent,
                    "parent_session_id cannot equal session_id",
                ));
            }
            Ok(())
        }
        PiProtocolV2Lineage::Unresolved { reason } => validate_name(reason, "lineage reason"),
    }
}

fn validate_agent_role(
    role: Option<&str>,
    lineage: &PiProtocolV2Lineage,
) -> Result<(), PiProtocolV2DecodeError> {
    let Some(role) = role else {
        return Ok(());
    };
    validate_name(role, "agent_role")?;
    if !matches!(
        lineage,
        PiProtocolV2Lineage::Agent { .. } | PiProtocolV2Lineage::Unresolved { .. }
    ) {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            "agent_role requires agent or unresolved lineage",
        ));
    }
    Ok(())
}

fn validate_name(value: &str, label: &str) -> Result<(), PiProtocolV2DecodeError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_NAME_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            format!("Invalid {label}"),
        ));
    }
    Ok(())
}

fn validate_host(value: &str) -> Result<(), PiProtocolV2DecodeError> {
    validate_name(value, "normalized_host")?;
    if value.contains('.')
        || value != value.to_ascii_lowercase()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            "normalized_host must be a lowercase short hostname",
        ));
    }
    Ok(())
}

fn parse_timestamp(
    value: &str,
    label: &str,
) -> Result<DateTime<chrono::FixedOffset>, PiProtocolV2DecodeError> {
    if value.len() > MAX_NAME_BYTES {
        return Err(PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            format!("Invalid {label}"),
        ));
    }
    DateTime::parse_from_rfc3339(value).map_err(|_| {
        PiProtocolV2DecodeError::new(
            PiProtocolV2ErrorCode::InvalidEvent,
            format!("Invalid {label}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::*;
    const FIXTURE: &str = include_str!("../pi-integration/fixtures/protocol-v2.jsonl");

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum FixtureKind {
        Envelope,
        Entry,
        Response,
        Wire,
    }

    #[derive(Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum FixtureExpectation {
        Accept,
        Reject,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureCase {
        name: String,
        kind: FixtureKind,
        expectation: FixtureExpectation,
        coverage: Vec<String>,
        #[serde(default)]
        error_code: Option<PiProtocolV2ErrorCode>,
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        headers: std::collections::BTreeMap<String, String>,
        wire: String,
    }

    fn cases() -> Vec<FixtureCase> {
        FIXTURE
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid fixture record"))
            .collect()
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Protocol v2 decoder contract]]
    #[test]
    fn fixture_accepts_only_the_exact_generation() {
        for case in cases() {
            let result = match case.kind {
                FixtureKind::Envelope => {
                    decode_protocol_v2_envelope(case.wire.as_bytes()).map(|_| ())
                }
                FixtureKind::Entry => {
                    decode_protocol_v2_tracking_entry(case.wire.as_bytes()).map(|_| ())
                }
                FixtureKind::Response => {
                    decode_protocol_v2_response(case.wire.as_bytes()).map(|_| ())
                }
                // Wire records are replayed against the real router in
                // server.rs. Pin only the lifecycle request they describe.
                FixtureKind::Wire => {
                    assert!(
                        case.status.is_some(),
                        "{} declares its intended status",
                        case.name
                    );
                    assert!(
                        case.headers
                            .keys()
                            .any(|key| key.eq_ignore_ascii_case(PI_REPORTER_HOST_HEADER)),
                        "{} carries the reporter headers the extension sends",
                        case.name
                    );
                    continue;
                }
            };
            match case.expectation {
                FixtureExpectation::Accept => {
                    result.unwrap_or_else(|error| panic!("{}: {error:?}", case.name));
                }
                FixtureExpectation::Reject => {
                    let error = result.expect_err(&case.name);
                    assert_eq!(Some(error.code), case.error_code, "{}", case.name);
                }
            }
        }
    }

    #[test]
    fn fixture_covers_every_frozen_variant() {
        let coverage = cases()
            .into_iter()
            .flat_map(|case| case.coverage)
            .collect::<HashSet<_>>();
        for required in [
            "start:startup",
            "start:reload",
            "start:new",
            "start:resume",
            "start:fork",
            "end:quit",
            "end:reload",
            "end:new",
            "end:resume",
            "end:fork",
            "delivery:live",
            "delivery:reconciliation",
            "lineage:root",
            "lineage:linked",
            "lineage:agent",
            "lineage:unresolved",
            "option:previous_session_id:omitted",
            "option:previous_session_id:present",
            "option:agent_role:omitted",
            "option:agent_role:present",
            "option:required:omitted",
            "option:required:present",
            "option:retry_after_ms:omitted",
            "option:retry_after_ms:present",
            "invalid:field",
            "mismatch:protocol:older",
            "mismatch:protocol:newer",
            "mismatch:reporter_version",
            "mismatch:quill_build",
            "mismatch:capability_digest",
            "mismatch:schema:older",
            "mismatch:schema:newer",
            "outcome:applied",
            "outcome:duplicate",
            "outcome:stale",
            "outcome:unknown_session",
            "handshake:accepted",
        ] {
            assert!(coverage.contains(required), "missing {required}");
        }
    }

    #[test]
    fn capability_digest_and_tracking_entry_privacy_are_pinned() {
        let digest = Sha256::digest(PI_PROTOCOL_V2_CAPABILITIES.join("\n"));
        assert_eq!(crate::hex_encode(digest), PI_PROTOCOL_V2_CAPABILITY_DIGEST);

        for case in cases().into_iter().filter(|case| {
            matches!(case.kind, FixtureKind::Entry)
                && case.expectation == FixtureExpectation::Accept
        }) {
            let wire = case.wire.to_ascii_lowercase();
            assert!(!wire.contains("prompt"));
            assert!(!wire.contains("message"));
            assert!(!wire.contains("tool_output"));
        }
    }
}
