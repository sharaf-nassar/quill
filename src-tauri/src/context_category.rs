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

/// SQL CASE expression that mirrors [`derive_category`] for use in bulk
/// `UPDATE` statements. Kept as a sibling const so reviewers can scan both
/// the Rust match arms and the SQL together when adding new event types;
/// the `derive_category_sql_matches_function` test asserts both stay in
/// agreement for every case the unit tests cover.
pub const DERIVE_CATEGORY_CASE_SQL: &str = "
CASE
    WHEN event_type IN ('mcp.index', 'mcp.fetch') THEN 'preservation'
    WHEN event_type = 'mcp.execute' AND decision = 'indexed' THEN 'preservation'
    WHEN event_type = 'mcp.execute' THEN 'routing'
    WHEN event_type = 'mcp.search' THEN 'routing'
    WHEN event_type = 'mcp.source_read' THEN 'retrieval'
    WHEN event_type = 'mcp.snapshot' AND decision = 'created' THEN 'preservation'
    WHEN event_type = 'mcp.snapshot' THEN 'retrieval'
    WHEN event_type = 'mcp.continuity' THEN 'telemetry'
    WHEN event_type IN ('router.guidance', 'router.denial') THEN 'routing'
    WHEN event_type IN ('capture.event', 'capture.snapshot') THEN 'telemetry'
    WHEN event_type = 'capture.guidance' THEN 'routing'
    ELSE 'unknown'
END
";

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

    /// Every (event_type, decision) pair the unit tests cover, paired with
    /// the expected category. Shared between the function-level tests and
    /// the SQL-equivalence test below so a future contributor adding a case
    /// to one place automatically extends both.
    const TEST_CASES: &[(&str, &str, &str)] = &[
        ("mcp.index", "recorded", PRESERVATION),
        ("mcp.fetch", "indexed", PRESERVATION),
        ("mcp.fetch", "cache_hit", PRESERVATION),
        ("mcp.execute", "indexed", PRESERVATION),
        ("mcp.execute", "returned", ROUTING),
        ("mcp.search", "returned", ROUTING),
        ("mcp.source_read", "chunk", RETRIEVAL),
        ("mcp.source_read", "source", RETRIEVAL),
        ("mcp.snapshot", "created", PRESERVATION),
        ("mcp.snapshot", "returned", RETRIEVAL),
        ("mcp.continuity", "recorded", TELEMETRY),
        ("router.guidance", "guide", ROUTING),
        ("router.denial", "deny", ROUTING),
        ("capture.event", "recorded", TELEMETRY),
        ("capture.snapshot", "recorded", TELEMETRY),
        ("capture.guidance", "session-start-directive", ROUTING),
        ("future.event", "x", UNKNOWN),
    ];

    #[test]
    fn derive_category_sql_matches_function() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (event_type TEXT NOT NULL, decision TEXT NOT NULL)")
            .unwrap();
        let mut insert = conn
            .prepare("INSERT INTO t (event_type, decision) VALUES (?1, ?2)")
            .unwrap();
        for (event_type, decision, _) in TEST_CASES {
            insert
                .execute(rusqlite::params![event_type, decision])
                .unwrap();
        }
        let sql =
            format!("SELECT event_type, decision, ({DERIVE_CATEGORY_CASE_SQL}) AS category FROM t");
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (event_type, decision, sql_category) in &rows {
            let function_category = derive_category(event_type, decision);
            assert_eq!(
                sql_category, function_category,
                "SQL CASE disagrees with derive_category for ({event_type}, {decision})"
            );
        }
        for (event_type, decision, expected) in TEST_CASES {
            let row = rows
                .iter()
                .find(|(et, dec, _)| et == event_type && dec == decision)
                .expect("seeded row missing");
            assert_eq!(row.2, *expected, "{event_type}/{decision}");
        }
    }
}
