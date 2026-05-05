//! Canonical taxonomy for context_savings_events.
//!
//! Producers (JS hooks, Python MCP tools) set `category` explicitly per call
//! site. The server uses [`derive_category`] only as a fallback for legacy
//! callers that omit the field, and migration 18 backfills historical rows
//! with the same mapping. The same logic is mirrored in the JS and Python
//! producers — keep them in sync when adding new event types.

pub const PRESERVATION: &str = "preservation";
pub const RETRIEVAL: &str = "retrieval";
pub const ROUTING: &str = "routing";
pub const TELEMETRY: &str = "telemetry";
pub const UNKNOWN: &str = "unknown";

pub fn is_known(category: &str) -> bool {
    matches!(category, PRESERVATION | RETRIEVAL | ROUTING | TELEMETRY)
}

/// Derive the category for a `(event_type, decision)` pair when the producer
/// did not set one. Returns [`UNKNOWN`] for unrecognised event types so that
/// new event types added without an explicit category at least get visibly
/// flagged in the breakdown rather than silently inflating preservation
/// totals.
pub fn derive_category(event_type: &str, decision: &str) -> &'static str {
    match event_type {
        "mcp.index" | "mcp.fetch" => PRESERVATION,
        "mcp.execute" => match decision {
            "indexed" => PRESERVATION,
            _ => ROUTING,
        },
        "mcp.search" => ROUTING,
        "mcp.source_read" => RETRIEVAL,
        "mcp.snapshot" => match decision {
            "created" => PRESERVATION,
            _ => RETRIEVAL,
        },
        "mcp.continuity" => TELEMETRY,
        "router.guidance" | "router.denial" => ROUTING,
        "capture.event" | "capture.snapshot" => TELEMETRY,
        "capture.guidance" => ROUTING,
        _ => UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_preservation_writes() {
        assert_eq!(derive_category("mcp.index", "recorded"), PRESERVATION);
        assert_eq!(derive_category("mcp.fetch", "indexed"), PRESERVATION);
        assert_eq!(derive_category("mcp.fetch", "cache_hit"), PRESERVATION);
        assert_eq!(derive_category("mcp.execute", "indexed"), PRESERVATION);
        assert_eq!(derive_category("mcp.snapshot", "created"), PRESERVATION);
    }

    #[test]
    fn maps_retrieval_reads() {
        assert_eq!(derive_category("mcp.source_read", "chunk"), RETRIEVAL);
        assert_eq!(derive_category("mcp.source_read", "source"), RETRIEVAL);
        assert_eq!(derive_category("mcp.snapshot", "returned"), RETRIEVAL);
    }

    #[test]
    fn maps_routing_transcript_costs() {
        assert_eq!(derive_category("mcp.search", "returned"), ROUTING);
        assert_eq!(derive_category("mcp.execute", "returned"), ROUTING);
        assert_eq!(derive_category("router.guidance", "guide"), ROUTING);
        assert_eq!(derive_category("router.denial", "deny"), ROUTING);
        assert_eq!(
            derive_category("capture.guidance", "session-start-directive"),
            ROUTING
        );
    }

    #[test]
    fn maps_telemetry_observations() {
        assert_eq!(derive_category("capture.event", "recorded"), TELEMETRY);
        assert_eq!(derive_category("capture.snapshot", "recorded"), TELEMETRY);
        assert_eq!(derive_category("mcp.continuity", "recorded"), TELEMETRY);
    }

    #[test]
    fn unknown_for_unmapped_types() {
        assert_eq!(derive_category("future.event", "x"), UNKNOWN);
    }
}
