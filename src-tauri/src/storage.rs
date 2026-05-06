use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::integrations::IntegrationProvider;
use crate::models::{
    BucketStats, CodeStats, CodeStatsHistoryPoint, ContextSavingsAnalytics,
    ContextSavingsBreakdownItem, ContextSavingsBreakdowns, ContextSavingsEvent,
    ContextSavingsEventPayload, ContextSavingsInsertResult, ContextSavingsSummary,
    ContextSavingsTimeseriesPoint, DataPoint, GitSnapshot, HostBreakdown, LanguageBreakdown,
    LearnedRule, LearnedRulePayload, LearningRun, LearningRunPayload, LearningStatus,
    LlmRuntimeStats, ObservationPayload, ProjectBreakdown, ProjectTokens, SessionBreakdown,
    SessionCodeStats, SessionRef, SessionStats, TokenDataPoint, TokenReportPayload, TokenStats,
    ToolCount, UsageBucket,
};

const PROVIDER_SETTINGS_KEY: &str = "integration.providers.v1";
#[allow(dead_code)]
const INDICATOR_PRIMARY_PROVIDER_KEY: &str = "indicator.primary_provider.v1";
const CONTEXT_SAVINGS_EVENTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS context_savings_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    schema_version INTEGER NOT NULL,
    provider TEXT NOT NULL,
    session_id TEXT,
    hostname TEXT NOT NULL,
    cwd TEXT,
    timestamp TEXT NOT NULL,
    event_type TEXT NOT NULL,
    source TEXT NOT NULL,
    decision TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'unknown',
    reason TEXT,
    delivered INTEGER NOT NULL,
    indexed_bytes INTEGER,
    returned_bytes INTEGER,
    input_bytes INTEGER,
    tokens_indexed_est INTEGER,
    tokens_returned_est INTEGER,
    tokens_saved_est INTEGER,
    tokens_preserved_est INTEGER,
    estimate_method TEXT,
    estimate_confidence REAL,
    source_ref TEXT,
    snapshot_ref TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_context_savings_timestamp
    ON context_savings_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_provider_timestamp
    ON context_savings_events(provider, timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_provider_session_timestamp
    ON context_savings_events(provider, session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_event_type_timestamp
    ON context_savings_events(event_type, timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_cwd_timestamp
    ON context_savings_events(cwd, timestamp);
-- The `idx_context_savings_category_timestamp` index is created in
-- migration 18, after the `category` column has been added via ALTER
-- TABLE. Keeping it here would fail on databases that were created
-- before migration 18 because the column does not yet exist when this
-- batch runs (the early init batch executes before migrations).
"#;
// Aggregate fragment shared by summary, timeseries, and breakdowns.
//
// Byte and token-indexed/returned columns sum every event so breakdown rows
// still surface router/telemetry traffic accurately.  The saved and preserved
// token columns are filtered to `category IN ('preservation', 'retrieval')`
// so capture-hook telemetry no longer pollutes the headline metric — rows
// outside that scope contribute zero to those two columns.
const CONTEXT_SAVINGS_AGGREGATES_SQL: &str = r#"
COUNT(*),
COALESCE(SUM(CASE WHEN delivered != 0 THEN 1 ELSE 0 END), 0),
COALESCE(SUM(COALESCE(indexed_bytes, 0)), 0),
COALESCE(SUM(COALESCE(returned_bytes, 0)), 0),
COALESCE(SUM(COALESCE(input_bytes, 0)), 0),
COALESCE(SUM(COALESCE(tokens_indexed_est, (COALESCE(indexed_bytes, 0) + 3) / 4)), 0),
COALESCE(SUM(COALESCE(tokens_returned_est, (COALESCE(returned_bytes, 0) + 3) / 4)), 0),
COALESCE(SUM(
    CASE WHEN category IN ('preservation', 'retrieval') THEN
        CASE
            WHEN tokens_saved_est IS NOT NULL
                AND NOT (
                    tokens_saved_est = 0
                    AND delivered = 0
                    AND indexed_bytes IS NOT NULL
                    AND returned_bytes IS NULL
                ) THEN tokens_saved_est
            WHEN indexed_bytes IS NOT NULL OR input_bytes IS NOT NULL OR returned_bytes IS NOT NULL THEN
                CASE
                    WHEN COALESCE(indexed_bytes, input_bytes, 0) > COALESCE(returned_bytes, 0) THEN
                        (COALESCE(indexed_bytes, input_bytes, 0) - COALESCE(returned_bytes, 0) + 3) / 4
                    ELSE 0
                END
            ELSE 0
        END
    ELSE 0 END
), 0),
COALESCE(SUM(
    CASE WHEN category IN ('preservation', 'retrieval')
        THEN COALESCE(tokens_preserved_est, (COALESCE(indexed_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0)
"#;

// Category-scoped totals returned alongside the legacy aggregate.  Each
// token CASE picks from exactly one category, so the three numbers partition
// the active range cleanly without double-counting.  Routing and telemetry
// also expose their own event counts because the `routerEventCount` field
// derived from the breakdown only sees `router.*` event-type strings, while
// the routing *category* additionally includes `mcp.search`, bounded
// `mcp.execute` results, and `capture.guidance` — so the headline token
// total and the supporting subtitle stay aligned.
const CONTEXT_SAVINGS_CATEGORY_TOTALS_SQL: &str = r#"
COALESCE(SUM(
    CASE WHEN category = 'preservation'
        THEN COALESCE(tokens_preserved_est, (COALESCE(indexed_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0),
COALESCE(SUM(
    CASE WHEN category = 'retrieval'
        THEN COALESCE(tokens_returned_est, (COALESCE(returned_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0),
COALESCE(SUM(
    CASE WHEN category = 'routing'
        THEN COALESCE(tokens_returned_est, (COALESCE(returned_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0),
COALESCE(SUM(CASE WHEN category = 'telemetry' THEN 1 ELSE 0 END), 0),
COALESCE(SUM(CASE WHEN category = 'routing' THEN 1 ELSE 0 END), 0)
"#;

// Per-source efficiency: distinct source_refs that were preserved within the
// window, and the subset that were also retrieved within the window.  We
// require both events to fall in-window so the ratio stays bounded in [0, 1]
// and reflects actual engagement, not pre-window leftovers.
const CONTEXT_SAVINGS_RETENTION_SQL: &str = r#"
WITH source_activity AS (
    SELECT source_ref,
        MAX(CASE WHEN category = 'preservation' THEN 1 ELSE 0 END) AS was_preserved,
        MAX(CASE WHEN category = 'retrieval'   THEN 1 ELSE 0 END) AS was_retrieved
    FROM context_savings_events
    WHERE timestamp >= ?1
      AND source_ref IS NOT NULL
      AND source_ref != ''
      AND category IN ('preservation', 'retrieval')
    GROUP BY source_ref
)
SELECT
    COALESCE(SUM(was_preserved), 0),
    COALESCE(SUM(CASE WHEN was_preserved = 1 AND was_retrieved = 1 THEN 1 ELSE 0 END), 0)
FROM source_activity
"#;

fn insert_tool_actions(
    stmt: &mut rusqlite::CachedStatement<'_>,
    provider: IntegrationProvider,
    actions: &[crate::sessions::ToolAction],
    message_id: &str,
    session_id: &str,
) -> Result<(), String> {
    for action in actions {
        stmt.execute(rusqlite::params![
            provider.as_str(),
            message_id,
            session_id,
            action.tool_name,
            action.category,
            action.file_path,
            action.summary,
            action.full_input,
            action.full_output,
            action.timestamp,
        ])
        .map_err(|e| format!("Insert tool_action: {e}"))?;
    }

    Ok(())
}

fn wilson_lower_bound(alpha: f64, beta: f64) -> f64 {
    let n = alpha + beta;
    if n < 0.01 {
        return 0.5;
    }
    let p = alpha / n;
    let z = 1.96_f64;
    let denominator = 1.0 + z * z / n;
    let center = p + z * z / (2.0 * n);
    let spread = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt();
    ((center - spread) / denominator).clamp(0.0, 0.95)
}

fn compute_state(confidence: f64, _alpha: f64, _beta: f64, freshness: f64) -> &'static str {
    if freshness < 0.3 {
        "stale"
    } else if confidence < 0.4 {
        "invalidated"
    } else if confidence >= 0.6 {
        "confirmed"
    } else {
        "emerging"
    }
}

fn freshness_factor(last_evidence_at: Option<&str>) -> f64 {
    let Some(ts) = last_evidence_at else {
        return 1.0;
    };
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return 1.0;
    };
    let days = (Utc::now() - last.with_timezone(&Utc)).num_seconds().max(0) as f64 / 86400.0;
    0.5_f64.powf(days / 90.0)
}

fn db_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_local_dir()
        .or_else(|| {
            dirs::home_dir().map(|h| {
                if cfg!(target_os = "macos") {
                    h.join("Library").join("Application Support")
                } else {
                    h.join(".local").join("share")
                }
            })
        })
        .ok_or("Cannot determine data directory")?;
    let app_dir = data_dir.join("com.quilltoolkit.app");
    std::fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app data dir: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o700));
    }

    Ok(app_dir.join("usage.db"))
}

fn ext_to_language(file_path: &str) -> &'static str {
    let ext = file_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" => "JavaScript",
        "rs" => "Rust",
        "py" => "Python",
        "css" | "scss" => "CSS",
        "html" => "HTML",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "md" => "Markdown",
        "sql" => "SQL",
        "go" => "Go",
        "sh" => "Shell",
        _ => "Other",
    }
}

fn parse_code_change(tool_name: &str, full_input: &str) -> Option<(i64, i64, String)> {
    if tool_name == "apply_patch" {
        return parse_apply_patch_change(full_input);
    }

    let parsed: serde_json::Value = serde_json::from_str(full_input).ok()?;

    let file_path = parsed
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match tool_name {
        "Edit" => {
            let old = parsed.get("old_string").and_then(|v| v.as_str())?;
            let new = parsed.get("new_string").and_then(|v| v.as_str())?;
            let removed = old.lines().count() as i64;
            let added = new.lines().count() as i64;
            Some((added, removed, file_path))
        }
        "Write" => {
            let content = parsed.get("content").and_then(|v| v.as_str())?;
            let added = content.lines().count() as i64;
            Some((added, 0, file_path))
        }
        _ => None,
    }
}

fn parse_apply_patch_change(patch: &str) -> Option<(i64, i64, String)> {
    let mut added = 0i64;
    let mut removed = 0i64;
    let mut file_path = String::new();
    let mut mode = "";

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            if file_path.is_empty() {
                file_path = path.to_string();
            }
            mode = "add";
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if file_path.is_empty() {
                file_path = path.to_string();
            }
            mode = "update";
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            if file_path.is_empty() {
                file_path = path.to_string();
            }
            mode = "delete";
            continue;
        }

        if line.starts_with("*** ") {
            mode = "";
            continue;
        }

        match mode {
            "add" => {
                if line.starts_with('+') {
                    added += 1;
                }
            }
            "update" => {
                if line.starts_with('+') {
                    added += 1;
                } else if line.starts_with('-') {
                    removed += 1;
                }
            }
            _ => {}
        }
    }

    if added == 0 && removed == 0 && file_path.is_empty() {
        return None;
    }

    Some((added, removed, file_path))
}

fn session_key(provider: IntegrationProvider, session_id: &str) -> String {
    format!("{provider}:{session_id}")
}

fn normalized_provider_scope(provider_scope: &[IntegrationProvider]) -> Vec<IntegrationProvider> {
    let mut scope = provider_scope.to_vec();
    scope.sort_by_key(|provider| provider.as_str());
    scope.dedup();
    if scope.is_empty() {
        scope.push(IntegrationProvider::Claude);
    }
    scope
}

fn provider_scope_json(provider_scope: &[IntegrationProvider]) -> String {
    serde_json::to_string(&normalized_provider_scope(provider_scope))
        .unwrap_or_else(|_| "[\"claude\"]".to_string())
}

fn parse_provider_scope(value: Option<String>) -> Vec<IntegrationProvider> {
    let raw = value.unwrap_or_else(|| "[\"claude\"]".to_string());
    let parsed = serde_json::from_str::<Vec<IntegrationProvider>>(&raw)
        .unwrap_or_else(|_| vec![IntegrationProvider::Claude]);
    normalized_provider_scope(&parsed)
}

fn provider_scope_contains_json(provider: IntegrationProvider) -> String {
    format!("\"{}\"", provider.as_str())
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if !dirs.contains(&path) {
        dirs.push(path);
    }
}

fn learned_rule_dirs(provider: Option<IntegrationProvider>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let claude_dir = crate::learning::learned_rules_dir_for_scope(&[IntegrationProvider::Claude]);
    let codex_dir = crate::learning::learned_rules_dir_for_scope(&[IntegrationProvider::Codex]);
    let shared_dir = crate::learning::learned_rules_dir_for_scope(&[
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
    ]);

    match provider {
        Some(IntegrationProvider::Claude) => {
            push_unique_dir(&mut dirs, claude_dir);
            push_unique_dir(&mut dirs, shared_dir);
        }
        Some(IntegrationProvider::Codex) => {
            push_unique_dir(&mut dirs, codex_dir);
            push_unique_dir(&mut dirs, shared_dir);
        }
        Some(IntegrationProvider::MiniMax) => {}
        None => {
            push_unique_dir(&mut dirs, claude_dir);
            push_unique_dir(&mut dirs, codex_dir);
            push_unique_dir(&mut dirs, shared_dir);
        }
    }

    dirs
}

fn learned_rule_dirs_for_scope(provider_scope: &[IntegrationProvider]) -> Vec<PathBuf> {
    let scope = normalized_provider_scope(provider_scope);
    match scope.as_slice() {
        [IntegrationProvider::Claude] => learned_rule_dirs(Some(IntegrationProvider::Claude)),
        [IntegrationProvider::Codex] => learned_rule_dirs(Some(IntegrationProvider::Codex)),
        _ => learned_rule_dirs(None),
    }
}

fn inferred_rule_provider_scope(path: &std::path::Path) -> Vec<IntegrationProvider> {
    let shared_dir = crate::learning::learned_rules_dir_for_scope(&[
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
    ]);
    if path.starts_with(&shared_dir) {
        return vec![IntegrationProvider::Claude, IntegrationProvider::Codex];
    }

    let codex_dir = crate::learning::learned_rules_dir_for_scope(&[IntegrationProvider::Codex]);
    if path.starts_with(&codex_dir) {
        return vec![IntegrationProvider::Codex];
    }

    vec![IntegrationProvider::Claude]
}

#[allow(dead_code)]
fn parse_rule_frontmatter(content: &str) -> (Option<String>, bool, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, false, content.to_string());
    }
    let after_opening = &trimmed[3..];
    let Some(end_idx) = after_opening.find("\n---") else {
        return (None, false, content.to_string());
    };
    let frontmatter = &after_opening[..end_idx];
    let body = after_opening[end_idx + 4..]
        .trim_start_matches('\n')
        .to_string();

    let mut domain = None;
    let mut is_anti = false;
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("domain:") {
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                domain = Some(val.to_string());
            }
        } else if let Some(val) = line.strip_prefix("anti_pattern:") {
            is_anti = val.trim().eq_ignore_ascii_case("true");
        }
    }
    (domain, is_anti, body)
}

fn range_to_duration(range: &str) -> TimeDelta {
    match range {
        "1h" => TimeDelta::hours(1),
        "24h" => TimeDelta::hours(24),
        "7d" => TimeDelta::days(7),
        "30d" => TimeDelta::days(30),
        _ => TimeDelta::hours(24),
    }
}

fn context_savings_from_timestamp(range: &str) -> String {
    if range == "all" {
        return "1970-01-01T00:00:00Z".to_string();
    }

    (Utc::now() - range_to_duration(range)).to_rfc3339()
}

fn context_savings_bucket_expr(range: &str) -> &'static str {
    match range {
        "1h" => "substr(timestamp, 1, 16) || ':00Z'",
        "30d" | "all" => "substr(timestamp, 1, 10) || 'T00:00:00Z'",
        _ => "substr(timestamp, 1, 13) || ':00:00Z'",
    }
}

fn context_savings_summary_from_row_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ContextSavingsSummary> {
    Ok(ContextSavingsSummary {
        event_count: row.get(offset)?,
        delivered_count: row.get(offset + 1)?,
        indexed_bytes: row.get(offset + 2)?,
        returned_bytes: row.get(offset + 3)?,
        input_bytes: row.get(offset + 4)?,
        tokens_indexed_est: row.get(offset + 5)?,
        tokens_returned_est: row.get(offset + 6)?,
        tokens_saved_est: row.get(offset + 7)?,
        tokens_preserved_est: row.get(offset + 8)?,
        // New category-scoped fields default to zero; populated by the summary
        // path via [`apply_category_totals`] and [`apply_retention_metrics`].
        tokens_preserved: 0,
        tokens_retrieved: 0,
        tokens_routing: 0,
        telemetry_event_count: 0,
        routing_event_count: 0,
        sources_preserved: 0,
        sources_retrieved: 0,
        retention_ratio: 0.0,
    })
}

fn apply_category_totals(
    summary: &mut ContextSavingsSummary,
    conn: &Connection,
    from: &str,
) -> Result<(), String> {
    let sql = format!(
        "SELECT {CONTEXT_SAVINGS_CATEGORY_TOTALS_SQL}
         FROM context_savings_events
         WHERE timestamp >= ?1"
    );
    let (preserved, retrieved, routing, telemetry, routing_events) = conn
        .query_row(&sql, params![from], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("Query context savings category totals error: {e}"))?;
    summary.tokens_preserved = preserved;
    summary.tokens_retrieved = retrieved;
    summary.tokens_routing = routing;
    summary.telemetry_event_count = telemetry;
    summary.routing_event_count = routing_events;
    Ok(())
}

fn apply_retention_metrics(
    summary: &mut ContextSavingsSummary,
    conn: &Connection,
    from: &str,
) -> Result<(), String> {
    let (preserved, retrieved) = conn
        .query_row(CONTEXT_SAVINGS_RETENTION_SQL, params![from], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| format!("Query context savings retention error: {e}"))?;
    summary.sources_preserved = preserved;
    summary.sources_retrieved = retrieved;
    summary.retention_ratio = if preserved > 0 {
        (retrieved as f64 / preserved as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Ok(())
}

fn context_savings_breakdown_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ContextSavingsBreakdownItem> {
    let summary = context_savings_summary_from_row_at(row, 1)?;
    Ok(ContextSavingsBreakdownItem {
        key: row.get(0)?,
        event_count: summary.event_count,
        delivered_count: summary.delivered_count,
        indexed_bytes: summary.indexed_bytes,
        returned_bytes: summary.returned_bytes,
        input_bytes: summary.input_bytes,
        tokens_indexed_est: summary.tokens_indexed_est,
        tokens_returned_est: summary.tokens_returned_est,
        tokens_saved_est: summary.tokens_saved_est,
        tokens_preserved_est: summary.tokens_preserved_est,
    })
}

fn parse_context_savings_provider(value: String) -> IntegrationProvider {
    value
        .parse::<IntegrationProvider>()
        .unwrap_or(IntegrationProvider::Claude)
}

fn parse_context_savings_metadata(raw: Option<String>) -> Option<serde_json::Value> {
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
        .is_ok()
}

fn backfill_context_event_categories(conn: &Connection) -> Result<(), String> {
    // Single bulk UPDATE driven by the SQL CASE mirror of `derive_category`.
    // Replaces an earlier per-row UPDATE loop that scaled poorly on tables
    // with millions of historical events.  The CASE expression is unit-
    // tested against `derive_category` in [`crate::context_category`].
    let sql = format!(
        "UPDATE context_savings_events
         SET category = ({case})
         WHERE category = 'unknown' OR category IS NULL OR category = ''",
        case = crate::context_category::DERIVE_CATEGORY_CASE_SQL,
    );
    conn.execute(&sql, [])
        .map_err(|e| format!("Bulk backfill context savings categories error: {e}"))?;
    Ok(())
}

fn normalize_context_event_token_estimates(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE context_savings_events
         SET tokens_saved_est = 0,
             tokens_preserved_est = 0
         WHERE category NOT IN ('preservation', 'retrieval')
           AND (COALESCE(tokens_saved_est, 0) != 0
                OR COALESCE(tokens_preserved_est, 0) != 0)",
        [],
    )
    .map_err(|e| format!("Normalize context savings token estimates error: {e}"))?;
    Ok(())
}

fn ensure_startup_indexes(conn: &Connection) -> Result<(), String> {
    let conditional_indexes = [
        (
            table_has_column(conn, "usage_snapshots", "provider")
                && table_has_column(conn, "usage_snapshots", "bucket_key"),
            "CREATE INDEX IF NOT EXISTS idx_snapshots_provider_bucket ON usage_snapshots(provider, bucket_key);
             CREATE INDEX IF NOT EXISTS idx_snapshots_ts_provider_bucket ON usage_snapshots(timestamp, provider, bucket_key);",
            "usage snapshot provider indexes",
        ),
        (
            table_has_column(conn, "usage_hourly", "provider")
                && table_has_column(conn, "usage_hourly", "bucket_key"),
            "CREATE INDEX IF NOT EXISTS idx_hourly_provider_bucket ON usage_hourly(provider, bucket_key);",
            "usage hourly provider indexes",
        ),
        (
            table_has_column(conn, "token_snapshots", "provider"),
            "CREATE INDEX IF NOT EXISTS idx_token_snap_provider_ts ON token_snapshots(provider, timestamp);
             CREATE INDEX IF NOT EXISTS idx_token_snap_provider_session_ts ON token_snapshots(provider, session_id, timestamp DESC);
             CREATE INDEX IF NOT EXISTS idx_token_snap_provider_cwd ON token_snapshots(provider, cwd, timestamp);",
            "token snapshot provider indexes",
        ),
        (
            table_has_column(conn, "token_hourly", "provider"),
            "CREATE INDEX IF NOT EXISTS idx_token_hourly_provider_hour ON token_hourly(provider, hour);",
            "token hourly provider indexes",
        ),
        (
            table_has_column(conn, "observations", "provider"),
            "CREATE INDEX IF NOT EXISTS idx_obs_provider_created ON observations(provider, created_at);
             CREATE INDEX IF NOT EXISTS idx_obs_provider_created_tool ON observations(provider, created_at, tool_name);
             CREATE INDEX IF NOT EXISTS idx_obs_created_tool ON observations(created_at, tool_name);",
            "observation provider indexes",
        ),
        (
            table_has_column(conn, "tool_actions", "category")
                && table_has_column(conn, "tool_actions", "provider")
                && table_has_column(conn, "tool_actions", "session_id"),
            "CREATE INDEX IF NOT EXISTS idx_tool_actions_category_timestamp ON tool_actions(category, timestamp);
             CREATE INDEX IF NOT EXISTS idx_tool_actions_category_provider_session ON tool_actions(category, provider, session_id);",
            "tool action query indexes",
        ),
        (
            table_has_column(conn, "context_savings_events", "event_id"),
            "CREATE INDEX IF NOT EXISTS idx_context_savings_timestamp
                 ON context_savings_events(timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_provider_timestamp
                 ON context_savings_events(provider, timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_provider_session_timestamp
                 ON context_savings_events(provider, session_id, timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_event_type_timestamp
                 ON context_savings_events(event_type, timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_cwd_timestamp
                 ON context_savings_events(cwd, timestamp);",
            "context savings indexes",
        ),
    ];

    for (enabled, sql, label) in conditional_indexes {
        if enabled {
            conn.execute_batch(sql)
                .map_err(|e| format!("Failed to create {label}: {e}"))?;
        }
    }

    Ok(())
}

pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn init() -> Result<Self, String> {
        let path = db_path()?;
        let conn = Connection::open(&path).map_err(|e| format!("Failed to open database: {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| format!("Failed to set pragmas: {e}"))?;

        conn.execute_batch(
            r#"CREATE TABLE IF NOT EXISTS usage_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                bucket_key TEXT NOT NULL,
                bucket_label TEXT NOT NULL,
                utilization REAL NOT NULL,
                resets_at TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON usage_snapshots(timestamp);

            CREATE TABLE IF NOT EXISTS usage_hourly (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hour TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                bucket_key TEXT NOT NULL,
                bucket_label TEXT NOT NULL,
                avg_utilization REAL NOT NULL,
                max_utilization REAL NOT NULL,
                min_utilization REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                UNIQUE(hour, provider, bucket_key)
            );
            CREATE INDEX IF NOT EXISTS idx_hourly_hour ON usage_hourly(hour);

            CREATE TABLE IF NOT EXISTS token_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL DEFAULT 'claude',
                session_id TEXT NOT NULL,
                hostname TEXT NOT NULL DEFAULT 'local',
                timestamp TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cwd TEXT DEFAULT NULL,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_token_snap_ts ON token_snapshots(timestamp);
            CREATE INDEX IF NOT EXISTS idx_token_snap_host ON token_snapshots(hostname);
            CREATE INDEX IF NOT EXISTS idx_token_snap_session_ts ON token_snapshots(session_id, timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_token_snap_cwd ON token_snapshots(cwd, timestamp);

            CREATE TABLE IF NOT EXISTS token_hourly (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hour TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                hostname TEXT NOT NULL DEFAULT 'local',
                total_input INTEGER NOT NULL,
                total_output INTEGER NOT NULL,
                total_cache_creation INTEGER NOT NULL DEFAULT 0,
                total_cache_read INTEGER NOT NULL DEFAULT 0,
                turn_count INTEGER NOT NULL,
                UNIQUE(hour, hostname, provider)
            );
            CREATE INDEX IF NOT EXISTS idx_token_hourly_hour ON token_hourly(hour);

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Learning system tables
            CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL DEFAULT 'claude',
                session_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                hook_phase TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                tool_input TEXT,
                tool_output TEXT,
                cwd TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id);
            CREATE INDEX IF NOT EXISTS idx_obs_timestamp ON observations(timestamp);
            CREATE INDEX IF NOT EXISTS idx_obs_created ON observations(created_at);

            CREATE TABLE IF NOT EXISTS learning_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trigger_mode TEXT NOT NULL,
                observations_analyzed INTEGER NOT NULL DEFAULT 0,
                rules_created INTEGER NOT NULL DEFAULT 0,
                rules_updated INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER,
                status TEXT NOT NULL DEFAULT 'running',
                error TEXT,
                provider_scope TEXT NOT NULL DEFAULT '["claude"]',
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS learned_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                domain TEXT,
                confidence REAL NOT NULL DEFAULT 0.5,
                observation_count INTEGER NOT NULL DEFAULT 0,
                file_path TEXT NOT NULL,
                provider_scope TEXT NOT NULL DEFAULT '["claude"]',
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS context_savings_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                schema_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                session_id TEXT,
                hostname TEXT NOT NULL,
                cwd TEXT,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                source TEXT NOT NULL,
                decision TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'unknown',
                reason TEXT,
                delivered INTEGER NOT NULL,
                indexed_bytes INTEGER,
                returned_bytes INTEGER,
                input_bytes INTEGER,
                tokens_indexed_est INTEGER,
                tokens_returned_est INTEGER,
                tokens_saved_est INTEGER,
                tokens_preserved_est INTEGER,
                estimate_method TEXT,
                estimate_confidence REAL,
                source_ref TEXT,
                snapshot_ref TEXT,
                metadata_json TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_context_savings_timestamp
                ON context_savings_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_provider_timestamp
                ON context_savings_events(provider, timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_provider_session_timestamp
                ON context_savings_events(provider, session_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_event_type_timestamp
                ON context_savings_events(event_type, timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_cwd_timestamp
                ON context_savings_events(cwd, timestamp);
            -- The category index is created by migration 18 after ALTER
            -- TABLE adds the column.  Keeping it here would crash startup
            -- for any database created before migration 18 because the
            -- column does not exist when this batch runs.
            "#,
        )
        .map_err(|e| format!("Failed to create tables: {e}"))?;

        // Schema versioning
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            )",
        )
        .map_err(|e| format!("Failed to create schema_version table: {e}"))?;

        let current_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Migration 1: add cwd column to token_snapshots
        if current_version < 1 {
            let has_cwd: bool = conn
                .prepare("SELECT cwd FROM token_snapshots LIMIT 0")
                .is_ok();
            if !has_cwd {
                conn.execute_batch("ALTER TABLE token_snapshots ADD COLUMN cwd TEXT DEFAULT NULL;")
                    .map_err(|e| format!("Migration 1 (add cwd column) error: {e}"))?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
                .map_err(|e| format!("Failed to record migration 1: {e}"))?;
        }

        // Migration 2: add logs column to learning_runs
        if current_version < 2 {
            let has_logs: bool = conn
                .prepare("SELECT logs FROM learning_runs LIMIT 0")
                .is_ok();
            if !has_logs {
                conn.execute_batch("ALTER TABLE learning_runs ADD COLUMN logs TEXT DEFAULT NULL;")
                    .map_err(|e| format!("Migration 2 (add logs column) error: {e}"))?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])
                .map_err(|e| format!("Failed to record migration 2: {e}"))?;
        }

        // Migration 3: add Beta-Binomial columns to learned_rules
        if current_version < 3 {
            let cols = [
                ("alpha", "REAL NOT NULL DEFAULT 1.0"),
                ("beta_param", "REAL NOT NULL DEFAULT 1.0"),
                ("last_evidence_at", "TEXT DEFAULT NULL"),
                ("state", "TEXT NOT NULL DEFAULT 'emerging'"),
                ("project", "TEXT DEFAULT NULL"),
            ];
            for (col, typ) in &cols {
                let has_col: bool = conn
                    .prepare(&format!("SELECT {col} FROM learned_rules LIMIT 0"))
                    .is_ok();
                if !has_col {
                    conn.execute_batch(&format!(
                        "ALTER TABLE learned_rules ADD COLUMN {col} {typ};"
                    ))
                    .map_err(|e| format!("Migration 3 (add {col}) error: {e}"))?;
                }
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (3)", [])
                .map_err(|e| format!("Failed to record migration 3: {e}"))?;
        }

        // Migration 4: anti-patterns, cross-project tracking, observation summaries
        if current_version < 4 {
            let new_cols = [
                ("is_anti_pattern", "INTEGER NOT NULL DEFAULT 0"),
                ("confirmed_projects", "TEXT DEFAULT NULL"),
            ];
            for (col, typ) in &new_cols {
                let has_col: bool = conn
                    .prepare(&format!("SELECT {col} FROM learned_rules LIMIT 0"))
                    .is_ok();
                if !has_col {
                    conn.execute_batch(&format!(
                        "ALTER TABLE learned_rules ADD COLUMN {col} {typ};"
                    ))
                    .map_err(|e| format!("Migration 4 (add {col}) error: {e}"))?;
                }
            }

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS observation_summaries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    period TEXT NOT NULL,
                    provider TEXT NOT NULL DEFAULT 'claude',
                    project TEXT,
                    tool_counts TEXT NOT NULL,
                    error_count INTEGER NOT NULL DEFAULT 0,
                    total_observations INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT DEFAULT (datetime('now')),
                    UNIQUE(period, provider, project)
                );
                CREATE INDEX IF NOT EXISTS idx_obs_summaries_period ON observation_summaries(period);
                CREATE INDEX IF NOT EXISTS idx_obs_summaries_provider_period ON observation_summaries(provider, period);",
            )
            .map_err(|e| format!("Migration 4 (observation_summaries) error: {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (4)", [])
                .map_err(|e| format!("Failed to record migration 4: {e}"))?;
        }

        // Migration 5: tool_actions table for MCP server
        if current_version < 5 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS tool_actions (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    provider      TEXT NOT NULL DEFAULT 'claude',
                    message_id    TEXT NOT NULL,
                    session_id    TEXT NOT NULL,
                    tool_name     TEXT NOT NULL,
                    category      TEXT NOT NULL,
                    file_path     TEXT,
                    summary       TEXT NOT NULL,
                    full_input    TEXT,
                    full_output   TEXT,
                    timestamp     TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_tool_actions_provider_session ON tool_actions(provider, session_id);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_session  ON tool_actions(session_id);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_message  ON tool_actions(message_id);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_file     ON tool_actions(file_path);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_category ON tool_actions(category);",
            )
            .map_err(|e| format!("Migration 5 (tool_actions table) error: {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (5)", [])
                .map_err(|e| format!("Failed to record migration 5: {e}"))?;
        }

        // Migration 6: memory optimizer tables
        if current_version < 6 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS memory_files (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    project_path    TEXT NOT NULL,
                    file_path       TEXT NOT NULL,
                    content_hash    TEXT NOT NULL,
                    last_scanned_at TEXT NOT NULL,
                    UNIQUE(project_path, file_path)
                );
                CREATE INDEX IF NOT EXISTS idx_memfiles_project ON memory_files(project_path);

                CREATE TABLE IF NOT EXISTS optimization_runs (
                    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                    project_path        TEXT NOT NULL,
                    provider_scope      TEXT NOT NULL DEFAULT '[\"claude\"]',
                    trigger             TEXT NOT NULL,
                    memories_scanned    INTEGER NOT NULL DEFAULT 0,
                    suggestions_created INTEGER NOT NULL DEFAULT 0,
                    context_sources     TEXT NOT NULL DEFAULT '{}',
                    status              TEXT NOT NULL DEFAULT 'running',
                    error               TEXT,
                    started_at          TEXT NOT NULL,
                    completed_at        TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_optrun_project_started ON optimization_runs(project_path, started_at);

                CREATE TABLE IF NOT EXISTS optimization_suggestions (
                    id               INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id           INTEGER NOT NULL REFERENCES optimization_runs(id),
                    project_path     TEXT NOT NULL,
                    provider_scope   TEXT NOT NULL DEFAULT '[\"claude\"]',
                    action_type      TEXT NOT NULL,
                    target_file      TEXT,
                    reasoning        TEXT NOT NULL,
                    proposed_content TEXT,
                    merge_sources    TEXT,
                    status           TEXT NOT NULL DEFAULT 'pending',
                    error            TEXT,
                    resolved_at      TEXT,
                    created_at       TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_optsug_run ON optimization_suggestions(run_id);
                CREATE INDEX IF NOT EXISTS idx_optsug_project_status ON optimization_suggestions(project_path, status);",
            )
            .map_err(|e| format!("Migration 6 (memory optimizer tables) error: {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (6)", [])
                .map_err(|e| format!("Failed to record migration 6: {e}"))?;
        }

        // Migration 7: memory optimizer redesign — diff, backup, and cleanup support
        if current_version < 7 {
            let has_col = |col: &str| -> bool {
                conn.prepare(&format!(
                    "SELECT {col} FROM optimization_suggestions LIMIT 0"
                ))
                .is_ok()
            };
            if !has_col("original_content") {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN original_content TEXT;",
                )
                .map_err(|e| format!("Migration 7 (original_content): {e}"))?;
            }
            if !has_col("diff_summary") {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN diff_summary TEXT;",
                )
                .map_err(|e| format!("Migration 7 (diff_summary): {e}"))?;
            }
            if !has_col("backup_data") {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN backup_data TEXT;",
                )
                .map_err(|e| format!("Migration 7 (backup_data): {e}"))?;
            }
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_optsug_status_created ON optimization_suggestions(status, created_at);
                 CREATE INDEX IF NOT EXISTS idx_optsug_status_resolved ON optimization_suggestions(status, resolved_at);",
            )
            .map_err(|e| format!("Migration 7 (indexes): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (7)", [])
                .map_err(|e| format!("Failed to record migration 7: {e}"))?;
        }

        // Migration 8: multi-stream learning — git cache, rule source, run phases
        if current_version < 8 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS git_snapshots (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    project      TEXT NOT NULL UNIQUE,
                    commit_hash  TEXT NOT NULL,
                    commit_count INTEGER NOT NULL,
                    raw_data     TEXT NOT NULL,
                    created_at   TEXT DEFAULT (datetime('now'))
                );",
            )
            .map_err(|e| format!("Migration 8 (git_snapshots table): {e}"))?;

            let has_source: bool = conn
                .prepare("SELECT source FROM learned_rules LIMIT 0")
                .is_ok();
            if !has_source {
                conn.execute_batch(
                    "ALTER TABLE learned_rules ADD COLUMN source TEXT DEFAULT 'observations';",
                )
                .map_err(|e| format!("Migration 8 (source column): {e}"))?;
            }

            let has_phases: bool = conn
                .prepare("SELECT phases FROM learning_runs LIMIT 0")
                .is_ok();
            if !has_phases {
                conn.execute_batch(
                    "ALTER TABLE learning_runs ADD COLUMN phases TEXT DEFAULT NULL;",
                )
                .map_err(|e| format!("Migration 8 (phases column): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (8)", [])
                .map_err(|e| format!("Failed to record migration 8: {e}"))?;
        }

        // Migration 9: suggestion conflict groups
        if current_version < 9 {
            let has_group_id: bool = conn
                .prepare("SELECT group_id FROM optimization_suggestions LIMIT 0")
                .is_ok();
            if !has_group_id {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN group_id TEXT;",
                )
                .map_err(|e| format!("Migration 9 (group_id): {e}"))?;
            }
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_optsug_group ON optimization_suggestions(group_id);",
            )
            .map_err(|e| format!("Migration 9 (group_id index): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (9)", [])
                .map_err(|e| format!("Failed to record migration 9: {e}"))?;
        }

        // Migration 10: response_times table for response/idle latency tracking
        if current_version < 10 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS response_times (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    provider     TEXT NOT NULL DEFAULT 'claude',
                    session_id   TEXT NOT NULL,
                    timestamp    TEXT NOT NULL,
                    response_secs REAL,
                    idle_secs    REAL,
                    created_at   TEXT DEFAULT (datetime('now')),
                    UNIQUE(provider, session_id, timestamp)
                );
                CREATE INDEX IF NOT EXISTS idx_rt_provider_session ON response_times(provider, session_id);
                CREATE INDEX IF NOT EXISTS idx_rt_session ON response_times(session_id);
                CREATE INDEX IF NOT EXISTS idx_rt_timestamp ON response_times(timestamp);",
            )
            .map_err(|e| format!("Migration 10 (response_times table): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (10)", [])
                .map_err(|e| format!("Failed to record migration 10: {e}"))?;
        }

        if current_version < 11 {
            let has_content: bool = conn
                .prepare("SELECT content FROM learned_rules LIMIT 0")
                .is_ok();
            if !has_content {
                conn.execute_batch(
                    "ALTER TABLE learned_rules ADD COLUMN content TEXT DEFAULT NULL;",
                )
                .map_err(|e| format!("Migration 11 (content column): {e}"))?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (11)", [])
                .map_err(|e| format!("Failed to record migration 11: {e}"))?;
        }

        if current_version < 12 {
            let has_token_provider: bool = conn
                .prepare("SELECT provider FROM token_snapshots LIMIT 0")
                .is_ok();
            if !has_token_provider {
                conn.execute_batch(
                    "ALTER TABLE token_snapshots ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude';
                     CREATE INDEX IF NOT EXISTS idx_token_snap_provider_ts ON token_snapshots(provider, timestamp);
                     CREATE INDEX IF NOT EXISTS idx_token_snap_provider_session_ts ON token_snapshots(provider, session_id, timestamp DESC);
                     CREATE INDEX IF NOT EXISTS idx_token_snap_provider_cwd ON token_snapshots(provider, cwd, timestamp);",
                )
                .map_err(|e| format!("Migration 12 (token_snapshots provider): {e}"))?;
            }

            let has_token_hourly_provider: bool = conn
                .prepare("SELECT provider FROM token_hourly LIMIT 0")
                .is_ok();
            if !has_token_hourly_provider {
                conn.execute_batch(
                    "ALTER TABLE token_hourly RENAME TO token_hourly_legacy;
                     CREATE TABLE token_hourly (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         hour TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         hostname TEXT NOT NULL DEFAULT 'local',
                         total_input INTEGER NOT NULL,
                         total_output INTEGER NOT NULL,
                         total_cache_creation INTEGER NOT NULL DEFAULT 0,
                         total_cache_read INTEGER NOT NULL DEFAULT 0,
                         turn_count INTEGER NOT NULL,
                         UNIQUE(hour, hostname, provider)
                     );
                     INSERT INTO token_hourly (hour, provider, hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count)
                     SELECT hour, 'claude', hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count
                     FROM token_hourly_legacy;
                     DROP TABLE token_hourly_legacy;
                     CREATE INDEX IF NOT EXISTS idx_token_hourly_hour ON token_hourly(hour);
                     CREATE INDEX IF NOT EXISTS idx_token_hourly_provider_hour ON token_hourly(provider, hour);",
                )
                .map_err(|e| format!("Migration 12 (token_hourly provider): {e}"))?;
            }

            let has_obs_provider: bool = conn
                .prepare("SELECT provider FROM observations LIMIT 0")
                .is_ok();
            if !has_obs_provider {
                conn.execute_batch(
                    "ALTER TABLE observations ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude';
                     CREATE INDEX IF NOT EXISTS idx_obs_provider_created ON observations(provider, created_at);",
                )
                .map_err(|e| format!("Migration 12 (observations provider): {e}"))?;
            }

            let has_obs_summaries_table: bool = conn
                .prepare("SELECT 1 FROM observation_summaries LIMIT 0")
                .is_ok();
            let has_obs_summary_provider = has_obs_summaries_table
                && conn
                    .prepare("SELECT provider FROM observation_summaries LIMIT 0")
                    .is_ok();
            if has_obs_summaries_table && !has_obs_summary_provider {
                conn.execute_batch(
                    "ALTER TABLE observation_summaries RENAME TO observation_summaries_legacy;
                     CREATE TABLE observation_summaries (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         period TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         project TEXT,
                         tool_counts TEXT NOT NULL,
                         error_count INTEGER NOT NULL DEFAULT 0,
                         total_observations INTEGER NOT NULL DEFAULT 0,
                         created_at TEXT DEFAULT (datetime('now')),
                         UNIQUE(period, provider, project)
                     );
                     INSERT INTO observation_summaries (period, provider, project, tool_counts, error_count, total_observations, created_at)
                     SELECT period, 'claude', project, tool_counts, error_count, total_observations, created_at
                     FROM observation_summaries_legacy;
                     DROP TABLE observation_summaries_legacy;
                     CREATE INDEX IF NOT EXISTS idx_obs_summaries_period ON observation_summaries(period);
                     CREATE INDEX IF NOT EXISTS idx_obs_summaries_provider_period ON observation_summaries(provider, period);",
                )
                .map_err(|e| format!("Migration 12 (observation_summaries provider): {e}"))?;
            }

            let has_tool_provider: bool = conn
                .prepare("SELECT provider FROM tool_actions LIMIT 0")
                .is_ok();
            if !has_tool_provider {
                conn.execute_batch(
                    "ALTER TABLE tool_actions ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude';
                     CREATE INDEX IF NOT EXISTS idx_tool_actions_provider_session ON tool_actions(provider, session_id);",
                )
                .map_err(|e| format!("Migration 12 (tool_actions provider): {e}"))?;
            }

            let has_response_provider: bool = conn
                .prepare("SELECT provider FROM response_times LIMIT 0")
                .is_ok();
            if !has_response_provider {
                conn.execute_batch(
                    "ALTER TABLE response_times RENAME TO response_times_legacy;
                     CREATE TABLE response_times (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         session_id TEXT NOT NULL,
                         timestamp TEXT NOT NULL,
                         response_secs REAL,
                         idle_secs REAL,
                         created_at TEXT DEFAULT (datetime('now')),
                         UNIQUE(provider, session_id, timestamp)
                     );
                     INSERT INTO response_times (provider, session_id, timestamp, response_secs, idle_secs, created_at)
                     SELECT 'claude', session_id, timestamp, response_secs, idle_secs, created_at
                     FROM response_times_legacy;
                     DROP TABLE response_times_legacy;
                     CREATE INDEX IF NOT EXISTS idx_rt_provider_session ON response_times(provider, session_id);
                     CREATE INDEX IF NOT EXISTS idx_rt_session ON response_times(session_id);
                     CREATE INDEX IF NOT EXISTS idx_rt_timestamp ON response_times(timestamp);",
                )
                .map_err(|e| format!("Migration 12 (response_times provider): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (12)", [])
                .map_err(|e| format!("Failed to record migration 12: {e}"))?;
        }

        if current_version < 13 {
            let migration_checks = [
                (
                    "SELECT provider_scope FROM learning_runs LIMIT 0",
                    "ALTER TABLE learning_runs ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "learning_runs provider_scope",
                ),
                (
                    "SELECT provider_scope FROM learned_rules LIMIT 0",
                    "ALTER TABLE learned_rules ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "learned_rules provider_scope",
                ),
                (
                    "SELECT provider_scope FROM optimization_runs LIMIT 0",
                    "ALTER TABLE optimization_runs ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "optimization_runs provider_scope",
                ),
                (
                    "SELECT provider_scope FROM optimization_suggestions LIMIT 0",
                    "ALTER TABLE optimization_suggestions ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "optimization_suggestions provider_scope",
                ),
            ];

            for (check_sql, migrate_sql, label) in migration_checks {
                let has_column = conn.prepare(check_sql).is_ok();
                if !has_column {
                    conn.execute_batch(migrate_sql)
                        .map_err(|e| format!("Migration 13 ({label}): {e}"))?;
                }
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (13)", [])
                .map_err(|e| format!("Failed to record migration 13: {e}"))?;
        }

        if current_version < 14 {
            let bucket_key_case = "CASE bucket_label
                WHEN '5 hours' THEN 'five_hour'
                WHEN '7 days' THEN 'seven_day'
                WHEN 'Sonnet' THEN 'seven_day_sonnet'
                WHEN 'Opus' THEN 'seven_day_opus'
                WHEN 'Code' THEN 'seven_day_cowork'
                WHEN 'OAuth' THEN 'seven_day_oauth_apps'
                WHEN 'Extra' THEN 'extra_usage'
                ELSE lower(replace(bucket_label, ' ', '_'))
            END";

            let usage_snapshots_has_provider = conn
                .prepare("SELECT provider FROM usage_snapshots LIMIT 0")
                .is_ok();
            let usage_snapshots_has_bucket_key = conn
                .prepare("SELECT bucket_key FROM usage_snapshots LIMIT 0")
                .is_ok();
            if !usage_snapshots_has_provider || !usage_snapshots_has_bucket_key {
                let migrate_sql = format!(
                    "ALTER TABLE usage_snapshots RENAME TO usage_snapshots_legacy;
                     CREATE TABLE usage_snapshots (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         timestamp TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         bucket_key TEXT NOT NULL,
                         bucket_label TEXT NOT NULL,
                         utilization REAL NOT NULL,
                         resets_at TEXT,
                         created_at TEXT DEFAULT (datetime('now'))
                     );
                     INSERT INTO usage_snapshots (timestamp, provider, bucket_key, bucket_label, utilization, resets_at, created_at)
                     SELECT
                         timestamp,
                         'claude',
                         {bucket_key_case},
                         bucket_label,
                         utilization,
                         resets_at,
                         created_at
                     FROM usage_snapshots_legacy;
                     DROP TABLE usage_snapshots_legacy;
                     CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON usage_snapshots(timestamp);
                     CREATE INDEX IF NOT EXISTS idx_snapshots_provider_bucket ON usage_snapshots(provider, bucket_key);
                     CREATE INDEX IF NOT EXISTS idx_snapshots_ts_provider_bucket ON usage_snapshots(timestamp, provider, bucket_key);"
                );
                conn.execute_batch(&migrate_sql)
                    .map_err(|e| format!("Migration 14 (usage_snapshots): {e}"))?;
            }

            let usage_hourly_has_provider = conn
                .prepare("SELECT provider FROM usage_hourly LIMIT 0")
                .is_ok();
            let usage_hourly_has_bucket_key = conn
                .prepare("SELECT bucket_key FROM usage_hourly LIMIT 0")
                .is_ok();
            if !usage_hourly_has_provider || !usage_hourly_has_bucket_key {
                let migrate_sql = format!(
                    "ALTER TABLE usage_hourly RENAME TO usage_hourly_legacy;
                     CREATE TABLE usage_hourly (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         hour TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         bucket_key TEXT NOT NULL,
                         bucket_label TEXT NOT NULL,
                         avg_utilization REAL NOT NULL,
                         max_utilization REAL NOT NULL,
                         min_utilization REAL NOT NULL,
                         sample_count INTEGER NOT NULL,
                         UNIQUE(hour, provider, bucket_key)
                     );
                     INSERT INTO usage_hourly (hour, provider, bucket_key, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count)
                     SELECT
                         hour,
                         'claude',
                         {bucket_key_case},
                         bucket_label,
                         avg_utilization,
                         max_utilization,
                         min_utilization,
                         sample_count
                     FROM usage_hourly_legacy;
                     DROP TABLE usage_hourly_legacy;
                     CREATE INDEX IF NOT EXISTS idx_hourly_hour ON usage_hourly(hour);
                     CREATE INDEX IF NOT EXISTS idx_hourly_provider_bucket ON usage_hourly(provider, bucket_key);"
                );
                conn.execute_batch(&migrate_sql)
                    .map_err(|e| format!("Migration 14 (usage_hourly): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (14)", [])
                .map_err(|e| format!("Failed to record migration 14: {e}"))?;
        }

        if current_version < 15 {
            let has_content_hash: bool = conn
                .prepare("SELECT content_hash FROM learned_rules LIMIT 0")
                .is_ok();
            if !has_content_hash {
                conn.execute(
                    "ALTER TABLE learned_rules ADD COLUMN content_hash TEXT DEFAULT NULL;",
                    [],
                )
                .map_err(|e| format!("Migration 15 (content_hash): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (15)", [])
                .map_err(|e| format!("Failed to record migration 15: {e}"))?;
        }

        if current_version < 16 {
            conn.execute_batch(CONTEXT_SAVINGS_EVENTS_SCHEMA)
                .map_err(|e| format!("Migration 16 (context_savings_events): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (16)", [])
                .map_err(|e| format!("Failed to record migration 16: {e}"))?;
        }

        if current_version < 17 {
            conn.execute(
                "UPDATE context_savings_events
                 SET tokens_saved_est =
                     (COALESCE(indexed_bytes, input_bytes, 0) - COALESCE(returned_bytes, 0) + 3) / 4
                 WHERE tokens_saved_est = 0
                   AND delivered = 0
                   AND indexed_bytes IS NOT NULL
                   AND returned_bytes IS NULL
                   AND COALESCE(indexed_bytes, input_bytes, 0) > COALESCE(returned_bytes, 0)",
                [],
            )
            .map_err(|e| format!("Migration 17 (context savings saved estimate backfill): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (17)", [])
                .map_err(|e| format!("Failed to record migration 17: {e}"))?;
        }

        if current_version < 18 {
            if !table_has_column(&conn, "context_savings_events", "category") {
                conn.execute(
                    "ALTER TABLE context_savings_events
                         ADD COLUMN category TEXT NOT NULL DEFAULT 'unknown'",
                    [],
                )
                .map_err(|e| format!("Migration 18 (add category column): {e}"))?;
            }

            backfill_context_event_categories(&conn)?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_context_savings_category_timestamp
                     ON context_savings_events(category, timestamp)",
                [],
            )
            .map_err(|e| format!("Migration 18 (category index): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (18)", [])
                .map_err(|e| format!("Failed to record migration 18: {e}"))?;
        }

        if current_version < 19 {
            backfill_context_event_categories(&conn)?;
            normalize_context_event_token_estimates(&conn)?;

            conn.execute("INSERT INTO schema_version (version) VALUES (19)", [])
                .map_err(|e| format!("Failed to record migration 19: {e}"))?;
        }

        ensure_startup_indexes(&conn)?;

        let storage = Self {
            conn: Mutex::new(conn),
        };

        if let Err(e) = storage.aggregate_and_cleanup() {
            log::warn!("Cleanup on startup failed: {e}");
        }

        if let Err(e) = storage.aggregate_and_cleanup_tokens() {
            log::warn!("Token cleanup on startup failed: {e}");
        }

        if let Err(e) = storage.cleanup_old_observations() {
            log::warn!("Observation cleanup on startup failed: {e}");
        }

        Ok(storage)
    }

    pub fn store_snapshot(&self, buckets: &[UsageBucket]) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO usage_snapshots (timestamp, provider, bucket_key, bucket_label, utilization, resets_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            for bucket in buckets {
                stmt.execute(params![
                    now,
                    bucket.provider.as_str(),
                    bucket.key,
                    bucket.label,
                    bucket.utilization,
                    bucket.resets_at
                ])
                .map_err(|e| format!("Insert error: {e}"))?;
            }
        }
        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(())
    }

    pub fn aggregate_and_cleanup(&self) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let cutoff = (Utc::now() - TimeDelta::days(30)).to_rfc3339();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        tx.execute(
            "INSERT INTO usage_hourly (hour, provider, bucket_key, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count)
             SELECT
                 strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour,
                 provider,
                 bucket_key,
                 bucket_label,
                 AVG(utilization),
                 MAX(utilization),
                 MIN(utilization),
                 COUNT(*)
             FROM usage_snapshots
             WHERE timestamp < ?1
             GROUP BY hour, provider, bucket_key, bucket_label
             ON CONFLICT(hour, provider, bucket_key) DO UPDATE SET
                 avg_utilization = (usage_hourly.avg_utilization * usage_hourly.sample_count + excluded.avg_utilization * excluded.sample_count)
                     / (usage_hourly.sample_count + excluded.sample_count),
                 max_utilization = MAX(usage_hourly.max_utilization, excluded.max_utilization),
                 min_utilization = MIN(usage_hourly.min_utilization, excluded.min_utilization),
                 sample_count = usage_hourly.sample_count + excluded.sample_count",
            params![cutoff],
        )
        .map_err(|e| format!("Aggregation insert error: {e}"))?;

        tx.execute(
            "DELETE FROM usage_snapshots WHERE timestamp < ?1",
            params![cutoff],
        )
        .map_err(|e| format!("Aggregation delete error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(())
    }

    pub fn get_usage_history(
        &self,
        provider: IntegrationProvider,
        bucket_key: &str,
        range: &str,
    ) -> Result<Vec<DataPoint>, String> {
        let conn = self.conn.lock();
        let now = Utc::now();

        let (from, use_hourly) = match range {
            "1h" => (now - TimeDelta::hours(1), false),
            "24h" => (now - TimeDelta::hours(24), false),
            "7d" => (now - TimeDelta::days(7), false),
            "30d" => (now - TimeDelta::days(30), true),
            "all" => (now - TimeDelta::days(365), true),
            _ => (now - TimeDelta::hours(24), false),
        };

        let from_str = from.to_rfc3339();

        if use_hourly {
            let from_hour = from.format("%Y-%m-%dT%H:00:00Z").to_string();
            let mut points = Vec::new();

            // First get hourly aggregates for older data
            let mut stmt = conn
                .prepare_cached(
                    "SELECT hour, avg_utilization FROM usage_hourly
                     WHERE provider = ?1 AND bucket_key = ?2 AND hour >= ?3
                     ORDER BY hour ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let hourly_rows = stmt
                .query_map(params![provider.as_str(), bucket_key, from_hour], |row| {
                    Ok(DataPoint {
                        timestamp: row.get(0)?,
                        utilization: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            for row in hourly_rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }

            // Then append recent granular snapshots
            let mut stmt2 = conn
                .prepare_cached(
                    "SELECT timestamp, utilization FROM usage_snapshots
                     WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let snap_rows = stmt2
                .query_map(params![provider.as_str(), bucket_key, from_str], |row| {
                    Ok(DataPoint {
                        timestamp: row.get(0)?,
                        utilization: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            for row in snap_rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }

            // Downsample if too many points (max ~720 for charts)
            Ok(downsample(points, 720))
        } else {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT timestamp, utilization FROM usage_snapshots
                     WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let rows = stmt
                .query_map(params![provider.as_str(), bucket_key, from_str], |row| {
                    Ok(DataPoint {
                        timestamp: row.get(0)?,
                        utilization: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            let mut points = Vec::new();
            for row in rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }

            let max_points = match range {
                "1h" => 60,
                "7d" => 672,
                _ => 1440,
            };

            Ok(downsample(points, max_points))
        }
    }

    pub fn get_usage_stats(
        &self,
        provider: IntegrationProvider,
        bucket_key: &str,
        days: i32,
    ) -> Result<BucketStats, String> {
        let conn = self.conn.lock();
        Self::get_usage_stats_with_conn(&conn, provider, bucket_key, days)
    }

    fn get_usage_stats_with_conn(
        conn: &Connection,
        provider: IntegrationProvider,
        bucket_key: &str,
        days: i32,
    ) -> Result<BucketStats, String> {
        let days = days.clamp(1, 365);
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     MIN(bucket_label),
                     AVG(utilization),
                     MAX(utilization),
                     MIN(utilization),
                     COUNT(*),
                     (SELECT COUNT(*) FROM usage_snapshots
                      WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3 AND utilization >= 80.0)
                 FROM usage_snapshots
                 WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let stats = stmt
            .query_row(params![provider.as_str(), bucket_key, from], |row| {
                let label = row
                    .get::<_, Option<String>>(0)?
                    .unwrap_or_else(|| bucket_key.to_string());
                let total: i64 = row.get(4)?;
                let above_80: i64 = row.get(5)?;
                let pct_above_80 = if total > 0 {
                    (above_80 as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                Ok(BucketStats {
                    provider,
                    key: bucket_key.to_string(),
                    label,
                    current: 0.0, // filled in by caller
                    avg: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    max: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    min: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    time_above_80: pct_above_80,
                    trend: String::new(), // filled in below
                    sample_count: total,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let trend = calc_trend(conn, provider, bucket_key)?;

        Ok(BucketStats { trend, ..stats })
    }

    pub fn get_all_bucket_stats(
        &self,
        current_buckets: &[UsageBucket],
        days: i32,
    ) -> Result<Vec<BucketStats>, String> {
        let conn = self.conn.lock();
        let mut results = Vec::new();
        for bucket in current_buckets {
            let mut stats =
                Self::get_usage_stats_with_conn(&conn, bucket.provider, &bucket.key, days)?;
            stats.current = bucket.utilization;
            results.push(stats);
        }
        Ok(results)
    }

    pub fn get_latest_usage_buckets(
        &self,
        provider: IntegrationProvider,
    ) -> Result<Vec<UsageBucket>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                // Avoid the correlated MAX(timestamp) subquery here. On large
                // usage_snapshots tables it scales poorly enough to stall the
                // startup fetch path that restores cached live buckets.
                "SELECT snapshot.bucket_key, snapshot.bucket_label, snapshot.utilization, snapshot.resets_at
                 FROM usage_snapshots snapshot
                 INNER JOIN (
                     SELECT bucket_key, MAX(timestamp) AS latest_timestamp
                     FROM usage_snapshots
                     WHERE provider = ?1
                     GROUP BY bucket_key
                 ) latest
                   ON latest.bucket_key = snapshot.bucket_key
                  AND latest.latest_timestamp = snapshot.timestamp
                 WHERE snapshot.provider = ?1
                 ORDER BY
                     CASE snapshot.provider WHEN 'claude' THEN 0 ELSE 1 END,
                     snapshot.bucket_label ASC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![provider.as_str()], |row| {
                Ok(UsageBucket {
                    provider,
                    key: row.get(0)?,
                    label: row.get(1)?,
                    utilization: row.get(2)?,
                    resets_at: row.get(3)?,
                    sort_order: 0,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut buckets = Vec::new();
        for row in rows {
            buckets.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(buckets)
    }

    pub fn get_latest_usage_snapshot_timestamp(
        &self,
        provider: IntegrationProvider,
    ) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT MAX(timestamp) FROM usage_snapshots WHERE provider = ?1",
            params![provider.as_str()],
            |row| row.get(0),
        )
        .map_err(|e| format!("Query error: {e}"))
    }

    pub fn get_snapshot_count(&self) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM usage_snapshots", [], |row| row.get(0))
            .map_err(|e| format!("Count error: {e}"))
    }

    pub fn store_token_snapshot(&self, payload: &TokenReportPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO token_snapshots (provider, session_id, hostname, timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                payload.provider.as_str(),
                payload.session_id,
                payload.hostname,
                now,
                payload.input_tokens,
                payload.output_tokens,
                payload.cache_creation_input_tokens,
                payload.cache_read_input_tokens,
                payload.cwd
            ],
        )
        .map_err(|e| format!("Insert token snapshot error: {e}"))?;

        Ok(())
    }

    pub fn get_token_history(
        &self,
        range: &str,
        provider: Option<IntegrationProvider>,
        hostname: Option<&str>,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<TokenDataPoint>, String> {
        let conn = self.conn.lock();
        let now = Utc::now();

        // Skip hourly aggregates when filtering by session_id or cwd since
        // token_hourly doesn't store those fields
        let needs_granular = session_id.is_some() || cwd.is_some();
        let (from, use_hourly) = match range {
            "1h" => (now - TimeDelta::hours(1), false),
            "24h" => (now - TimeDelta::hours(24), false),
            "7d" => (now - TimeDelta::days(7), false),
            "30d" => (now - TimeDelta::days(30), !needs_granular),
            "all" => (now - TimeDelta::days(365), !needs_granular),
            _ => (now - TimeDelta::hours(24), false),
        };

        let from_str = from.to_rfc3339();
        let mut points = Vec::new();

        if use_hourly {
            let from_hour = from.format("%Y-%m-%dT%H:00:00Z").to_string();

            let (hourly_sql, hourly_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                match (provider, hostname) {
                    (Some(provider), Some(host)) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1 AND provider = ?2 AND hostname = ?3
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![
                            Box::new(from_hour.clone()),
                            Box::new(provider.as_str().to_string()),
                            Box::new(host.to_string()),
                        ],
                    ),
                    (Some(provider), None) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1 AND provider = ?2
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![
                            Box::new(from_hour.clone()),
                            Box::new(provider.as_str().to_string()),
                        ],
                    ),
                    (None, Some(host)) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1 AND hostname = ?2
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![Box::new(from_hour.clone()), Box::new(host.to_string())],
                    ),
                    (None, None) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![Box::new(from_hour.clone())],
                    ),
                };

            let mut stmt = conn
                .prepare(&hourly_sql)
                .map_err(|e| format!("Prepare error: {e}"))?;

            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                hourly_params.iter().map(|p| p.as_ref()).collect();

            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    let inp: i64 = row.get(1)?;
                    let out: i64 = row.get(2)?;
                    let cc: i64 = row.get(3)?;
                    let cr: i64 = row.get(4)?;
                    Ok(TokenDataPoint {
                        timestamp: row.get(0)?,
                        input_tokens: inp,
                        output_tokens: out,
                        cache_creation_input_tokens: cc,
                        cache_read_input_tokens: cr,
                        total_tokens: inp + out + cc + cr,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            for row in rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }
        }

        // Append granular snapshots
        let mut snap_sql = String::from(
            "SELECT timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
             FROM token_snapshots
             WHERE timestamp >= ?1",
        );
        let mut snap_params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(from_str.clone())];
        let mut next_param = 2;

        if let Some(provider) = provider {
            snap_sql.push_str(&format!(" AND provider = ?{next_param}"));
            snap_params.push(Box::new(provider.as_str().to_string()));
            next_param += 1;
        }

        if let Some(sid) = session_id {
            snap_sql.push_str(&format!(" AND session_id = ?{next_param}"));
            snap_params.push(Box::new(sid.to_string()));
        } else if let Some(project) = cwd {
            snap_sql.push_str(&format!(" AND cwd = ?{next_param}"));
            snap_params.push(Box::new(project.to_string()));
        } else if let Some(host) = hostname {
            snap_sql.push_str(&format!(" AND hostname = ?{next_param}"));
            snap_params.push(Box::new(host.to_string()));
        }
        snap_sql.push_str(" ORDER BY timestamp ASC");

        let mut stmt2 = conn
            .prepare(&snap_sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs2: Vec<&dyn rusqlite::types::ToSql> =
            snap_params.iter().map(|p| p.as_ref()).collect();

        let snap_rows = stmt2
            .query_map(params_refs2.as_slice(), |row| {
                let inp: i64 = row.get(1)?;
                let out: i64 = row.get(2)?;
                let cc: i64 = row.get(3)?;
                let cr: i64 = row.get(4)?;
                Ok(TokenDataPoint {
                    timestamp: row.get(0)?,
                    input_tokens: inp,
                    output_tokens: out,
                    cache_creation_input_tokens: cc,
                    cache_read_input_tokens: cr,
                    total_tokens: inp + out + cc + cr,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        for row in snap_rows {
            points.push(row.map_err(|e| format!("Row error: {e}"))?);
        }

        let max_points = match range {
            "1h" => 60,
            "7d" => 672,
            "30d" | "all" => 720,
            _ => 1440,
        };

        Ok(downsample_tokens(points, max_points))
    }

    pub fn get_token_stats(
        &self,
        days: i32,
        provider: Option<IntegrationProvider>,
        hostname: Option<&str>,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<TokenStats, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut sql = String::from(
            "SELECT
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cache_creation_input_tokens), 0),
                 COALESCE(SUM(cache_read_input_tokens), 0),
                 COUNT(*)
             FROM token_snapshots
             WHERE timestamp >= ?1",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(from)];
        let mut next_param = 2;

        if let Some(provider) = provider {
            sql.push_str(&format!(" AND provider = ?{next_param}"));
            params_vec.push(Box::new(provider.as_str().to_string()));
            next_param += 1;
        }

        if let Some(session_id) = session_id {
            sql.push_str(&format!(" AND session_id = ?{next_param}"));
            params_vec.push(Box::new(session_id.to_string()));
        } else if let Some(project) = cwd {
            sql.push_str(&format!(" AND cwd = ?{next_param}"));
            params_vec.push(Box::new(project.to_string()));
        } else if let Some(host) = hostname {
            sql.push_str(&format!(" AND hostname = ?{next_param}"));
            params_vec.push(Box::new(host.to_string()));
        }

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        stmt.query_row(params_refs.as_slice(), |row| {
            let total_input: i64 = row.get(0)?;
            let total_output: i64 = row.get(1)?;
            let total_cache_creation: i64 = row.get(2)?;
            let total_cache_read: i64 = row.get(3)?;
            let turn_count: i64 = row.get(4)?;
            let total_tokens = total_input + total_output + total_cache_creation + total_cache_read;

            Ok(TokenStats {
                total_input,
                total_output,
                total_cache_creation,
                total_cache_read,
                total_tokens,
                turn_count,
                avg_input_per_turn: if turn_count > 0 {
                    total_input as f64 / turn_count as f64
                } else {
                    0.0
                },
                avg_output_per_turn: if turn_count > 0 {
                    total_output as f64 / turn_count as f64
                } else {
                    0.0
                },
            })
        })
        .map_err(|e| format!("Query error: {e}"))
    }

    pub fn store_context_savings_events(
        &self,
        events: &[ContextSavingsEventPayload],
    ) -> Result<ContextSavingsInsertResult, String> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Context savings transaction error: {e}"))?;

        let mut inserted = 0i64;
        let mut ignored = 0i64;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO context_savings_events (
                         event_id,
                         schema_version,
                         provider,
                         session_id,
                         hostname,
                         cwd,
                         timestamp,
                         event_type,
                         source,
                         decision,
                         category,
                         reason,
                         delivered,
                         indexed_bytes,
                         returned_bytes,
                         input_bytes,
                         tokens_indexed_est,
                         tokens_returned_est,
                         tokens_saved_est,
                         tokens_preserved_est,
                         estimate_method,
                         estimate_confidence,
                         source_ref,
                         snapshot_ref,
                         metadata_json
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                )
                .map_err(|e| format!("Prepare context savings insert error: {e}"))?;

            for event in events {
                let metadata_json = event
                    .metadata_json
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| format!("Serialize context savings metadata error: {e}"))?;

                let category = event
                    .category
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        crate::context_category::derive_category(&event.event_type, &event.decision)
                            .to_string()
                    });
                let token_scope = matches!(category.as_str(), "preservation" | "retrieval");
                let tokens_saved_est = if token_scope {
                    event.tokens_saved_est
                } else {
                    Some(0)
                };
                let tokens_preserved_est = if token_scope {
                    event.tokens_preserved_est
                } else {
                    Some(0)
                };

                let changed = stmt
                    .execute(params![
                        event.event_id,
                        event.schema_version,
                        event.provider.as_str(),
                        event.session_id.as_deref(),
                        event.hostname,
                        event.cwd.as_deref(),
                        event.timestamp,
                        event.event_type,
                        event.source,
                        event.decision,
                        category,
                        event.reason.as_deref(),
                        event.delivered as i32,
                        event.indexed_bytes,
                        event.returned_bytes,
                        event.input_bytes,
                        event.tokens_indexed_est,
                        event.tokens_returned_est,
                        tokens_saved_est,
                        tokens_preserved_est,
                        event.estimate_method.as_deref(),
                        event.estimate_confidence,
                        event.source_ref.as_deref(),
                        event.snapshot_ref.as_deref(),
                        metadata_json,
                    ])
                    .map_err(|e| format!("Insert context savings event error: {e}"))?;

                if changed == 0 {
                    ignored += 1;
                } else {
                    inserted += 1;
                }
            }
        }

        tx.commit()
            .map_err(|e| format!("Commit context savings events error: {e}"))?;

        Ok(ContextSavingsInsertResult { inserted, ignored })
    }

    pub fn get_context_savings_analytics(
        &self,
        range: &str,
        limit: Option<i64>,
    ) -> Result<ContextSavingsAnalytics, String> {
        let conn = self.conn.lock();
        let from = context_savings_from_timestamp(range);
        let recent_limit = limit.unwrap_or(50).clamp(1, 500);
        let breakdown_limit = 20i64;

        let summary_sql = format!(
            "SELECT {CONTEXT_SAVINGS_AGGREGATES_SQL}
             FROM context_savings_events
             WHERE timestamp >= ?1"
        );
        let mut summary = conn
            .query_row(&summary_sql, params![from], |row| {
                context_savings_summary_from_row_at(row, 0)
            })
            .map_err(|e| format!("Query context savings summary error: {e}"))?;
        apply_category_totals(&mut summary, &conn, &from)?;
        apply_retention_metrics(&mut summary, &conn, &from)?;

        let bucket_expr = context_savings_bucket_expr(range);
        let timeseries_sql = format!(
            "SELECT {bucket_expr} AS bucket, {CONTEXT_SAVINGS_AGGREGATES_SQL}
             FROM context_savings_events
             WHERE timestamp >= ?1
             GROUP BY bucket
             ORDER BY bucket ASC"
        );
        let mut timeseries_stmt = conn
            .prepare(&timeseries_sql)
            .map_err(|e| format!("Prepare context savings timeseries error: {e}"))?;
        let timeseries_rows = timeseries_stmt
            .query_map(params![from], |row| {
                let summary = context_savings_summary_from_row_at(row, 1)?;
                Ok(ContextSavingsTimeseriesPoint {
                    timestamp: row.get(0)?,
                    event_count: summary.event_count,
                    delivered_count: summary.delivered_count,
                    indexed_bytes: summary.indexed_bytes,
                    returned_bytes: summary.returned_bytes,
                    input_bytes: summary.input_bytes,
                    tokens_indexed_est: summary.tokens_indexed_est,
                    tokens_returned_est: summary.tokens_returned_est,
                    tokens_saved_est: summary.tokens_saved_est,
                    tokens_preserved_est: summary.tokens_preserved_est,
                })
            })
            .map_err(|e| format!("Query context savings timeseries error: {e}"))?;

        let mut timeseries = Vec::new();
        for row in timeseries_rows {
            timeseries.push(row.map_err(|e| format!("Context savings timeseries row error: {e}"))?);
        }

        let breakdowns = ContextSavingsBreakdowns {
            by_provider: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "provider",
                &from,
                breakdown_limit,
            )?,
            by_event_type: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "event_type",
                &from,
                breakdown_limit,
            )?,
            by_source: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "source",
                &from,
                breakdown_limit,
            )?,
            by_decision: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "decision",
                &from,
                breakdown_limit,
            )?,
            by_cwd: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "cwd",
                &from,
                breakdown_limit,
            )?,
        };

        let recent_events =
            Self::get_recent_context_savings_events_with_conn(&conn, &from, recent_limit)?;

        Ok(ContextSavingsAnalytics {
            summary,
            timeseries,
            breakdowns,
            recent_events,
        })
    }

    pub fn has_context_savings_events(&self) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM context_savings_events LIMIT 1)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|e| format!("Context savings existence query error: {e}"))
    }

    fn get_context_savings_breakdown_with_conn(
        conn: &Connection,
        dimension: &str,
        from: &str,
        limit: i64,
    ) -> Result<Vec<ContextSavingsBreakdownItem>, String> {
        let sql = format!(
            "SELECT COALESCE(NULLIF({dimension}, ''), '(unknown)') AS key,
                    {CONTEXT_SAVINGS_AGGREGATES_SQL}
             FROM context_savings_events
             WHERE timestamp >= ?1
             GROUP BY key
             ORDER BY 9 DESC, 2 DESC
             LIMIT ?2"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare context savings breakdown error: {e}"))?;
        let rows = stmt
            .query_map(params![from, limit], context_savings_breakdown_from_row)
            .map_err(|e| format!("Query context savings breakdown error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Context savings breakdown row error: {e}"))?);
        }
        Ok(results)
    }

    fn get_recent_context_savings_events_with_conn(
        conn: &Connection,
        from: &str,
        limit: i64,
    ) -> Result<Vec<ContextSavingsEvent>, String> {
        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     event_id,
                     schema_version,
                     provider,
                     session_id,
                     hostname,
                     cwd,
                     timestamp,
                     event_type,
                     source,
                     decision,
                     category,
                     reason,
                     delivered,
                     indexed_bytes,
                     returned_bytes,
                     input_bytes,
                     tokens_indexed_est,
                     tokens_returned_est,
                     CASE
                         WHEN category NOT IN ('preservation', 'retrieval') THEN 0
                         WHEN tokens_saved_est IS NOT NULL
                             AND NOT (
                                 tokens_saved_est = 0
                                 AND delivered = 0
                                 AND indexed_bytes IS NOT NULL
                                 AND returned_bytes IS NULL
                             ) THEN tokens_saved_est
                         WHEN indexed_bytes IS NOT NULL OR input_bytes IS NOT NULL OR returned_bytes IS NOT NULL THEN
                             CASE
                                 WHEN COALESCE(indexed_bytes, input_bytes, 0) > COALESCE(returned_bytes, 0) THEN
                                     (COALESCE(indexed_bytes, input_bytes, 0) - COALESCE(returned_bytes, 0) + 3) / 4
                                 ELSE 0
                             END
                         ELSE 0
                     END AS tokens_saved_est,
                     CASE
                         WHEN category IN ('preservation', 'retrieval')
                             THEN tokens_preserved_est
                         ELSE 0
                     END AS tokens_preserved_est,
                     estimate_method,
                     estimate_confidence,
                     source_ref,
                     snapshot_ref,
                     metadata_json,
                     created_at
                 FROM context_savings_events
                 WHERE timestamp >= ?1
                 ORDER BY timestamp DESC, created_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("Prepare recent context savings events error: {e}"))?;

        let rows = stmt
            .query_map(params![from, limit], |row| {
                Ok(ContextSavingsEvent {
                    event_id: row.get(0)?,
                    schema_version: row.get(1)?,
                    provider: parse_context_savings_provider(row.get(2)?),
                    session_id: row.get(3)?,
                    hostname: row.get(4)?,
                    cwd: row.get(5)?,
                    timestamp: row.get(6)?,
                    event_type: row.get(7)?,
                    source: row.get(8)?,
                    decision: row.get(9)?,
                    category: row.get(10)?,
                    reason: row.get(11)?,
                    delivered: row.get::<_, i64>(12)? != 0,
                    indexed_bytes: row.get(13)?,
                    returned_bytes: row.get(14)?,
                    input_bytes: row.get(15)?,
                    tokens_indexed_est: row.get(16)?,
                    tokens_returned_est: row.get(17)?,
                    tokens_saved_est: row.get(18)?,
                    tokens_preserved_est: row.get(19)?,
                    estimate_method: row.get(20)?,
                    estimate_confidence: row.get(21)?,
                    source_ref: row.get(22)?,
                    snapshot_ref: row.get(23)?,
                    metadata_json: parse_context_savings_metadata(row.get(24)?),
                    created_at: row.get(25)?,
                })
            })
            .map_err(|e| format!("Query recent context savings events error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Recent context savings event row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_token_hostnames(&self) -> Result<Vec<String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached("SELECT DISTINCT hostname FROM token_snapshots ORDER BY hostname ASC")
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Query error: {e}"))?;

        let mut hostnames = Vec::new();
        for row in rows {
            hostnames.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(hostnames)
    }

    pub fn get_host_breakdown(&self, days: i32) -> Result<Vec<HostBreakdown>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     hostname,
                     SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens,
                     COUNT(*) as turn_count,
                     MAX(timestamp) as last_active
                 FROM token_snapshots
                 WHERE timestamp >= ?1
                 GROUP BY hostname
                 ORDER BY total_tokens DESC
                 LIMIT 50",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![from], |row| {
                Ok(HostBreakdown {
                    hostname: row.get(0)?,
                    total_tokens: row.get(1)?,
                    turn_count: row.get(2)?,
                    last_active: row.get(3)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_project_breakdown(&self, days: i32) -> Result<Vec<ProjectBreakdown>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     cwd,
                     hostname,
                     SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens,
                     COUNT(*) as turn_count,
                     COUNT(DISTINCT provider || ':' || session_id) as session_count,
                     MAX(timestamp) as last_active
                 FROM token_snapshots
                 WHERE timestamp >= ?1 AND cwd IS NOT NULL
                 GROUP BY cwd, hostname
                 ORDER BY total_tokens DESC
                 LIMIT 200",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![from], |row| {
                Ok(ProjectBreakdown {
                    project: row.get(0)?,
                    hostname: row.get(1)?,
                    total_tokens: row.get(2)?,
                    turn_count: row.get(3)?,
                    session_count: row.get(4)?,
                    last_active: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut raw: Vec<ProjectBreakdown> = Vec::new();
        for row in rows {
            raw.push(row.map_err(|e| format!("Row error: {e}"))?);
        }

        // Merge subdirectories into their parent project root.
        // If /a/b is a prefix of /a/b/c, fold /a/b/c into /a/b.
        Ok(merge_project_subdirs(raw))
    }

    pub fn get_session_breakdown(
        &self,
        days: i32,
        hostname: Option<&str>,
        provider: Option<IntegrationProvider>,
        limit: Option<i32>,
    ) -> Result<Vec<SessionBreakdown>, String> {
        let days = days.clamp(1, 365);
        let limit = limit.unwrap_or(10).clamp(1, 500);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut sql = String::from(
            "SELECT
                 s.provider,
                 s.session_id,
                 s.hostname,
                 SUM(s.input_tokens + s.output_tokens + s.cache_creation_input_tokens + s.cache_read_input_tokens) as total_tokens,
                 COUNT(*) as turn_count,
                 MIN(s.timestamp) as first_seen,
                 MAX(s.timestamp) as last_active,
                 (SELECT t.cwd FROM token_snapshots t
                  WHERE t.provider = s.provider AND t.session_id = s.session_id AND t.cwd IS NOT NULL
                  ORDER BY t.timestamp DESC LIMIT 1) as project
             FROM token_snapshots s
             WHERE s.timestamp >= ?1",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(from)];
        let hostname_param = if provider.is_some() { 3 } else { 2 };

        if let Some(provider) = provider {
            sql.push_str(" AND s.provider = ?2");
            params_vec.push(Box::new(provider.as_str().to_string()));
        }

        if let Some(host) = hostname {
            sql.push_str(&format!(" AND s.hostname = ?{hostname_param}"));
            params_vec.push(Box::new(host.to_string()));
        }

        sql.push_str(
            " GROUP BY s.provider, s.session_id, s.hostname
              ORDER BY last_active DESC",
        );
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(SessionBreakdown {
                    provider: row.get(0)?,
                    session_id: row.get(1)?,
                    hostname: row.get(2)?,
                    total_tokens: row.get(3)?,
                    turn_count: row.get(4)?,
                    first_seen: row.get(5)?,
                    last_active: row.get(6)?,
                    project: row.get(7)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_project_tokens(&self, days: i32) -> Result<Vec<ProjectTokens>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT
                    project,
                    SUM(total_tokens) as total_tokens,
                    COUNT(*) as session_count
                FROM (
                    SELECT
                        s.provider,
                        s.session_id,
                        (SELECT t.cwd FROM token_snapshots t
                         WHERE t.provider = s.provider AND t.session_id = s.session_id AND t.cwd IS NOT NULL
                         ORDER BY t.timestamp DESC LIMIT 1) as project,
                        SUM(s.input_tokens + s.output_tokens + s.cache_creation_input_tokens + s.cache_read_input_tokens) as total_tokens
                    FROM token_snapshots s
                    WHERE s.timestamp >= ?1
                    GROUP BY s.provider, s.session_id
                )
                WHERE project IS NOT NULL
                GROUP BY project
                ORDER BY total_tokens DESC
                LIMIT 20",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(rusqlite::params![from], |row| {
                Ok(ProjectTokens {
                    project: row.get(0)?,
                    total_tokens: row.get(1)?,
                    session_count: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_session_stats(&self, days: i32) -> Result<SessionStats, String> {
        let conn = self.conn.lock();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT
                    AVG(duration_seconds) as avg_duration_seconds,
                    AVG(total_tokens) as avg_tokens,
                    COUNT(*) as session_count,
                    SUM(total_tokens) as total_tokens
                FROM (
                    SELECT
                        provider,
                        session_id,
                        (strftime('%s', MAX(timestamp)) - strftime('%s', MIN(timestamp))) as duration_seconds,
                        SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens
                    FROM token_snapshots
                    WHERE timestamp >= ?1
                    GROUP BY provider, session_id
                    HAVING COUNT(*) > 1
                )",
            )
            .map_err(|e| e.to_string())?;

        let result = stmt
            .query_row(rusqlite::params![cutoff_str], |row| {
                Ok(SessionStats {
                    avg_duration_seconds: row.get::<_, Option<f64>>(0)?.unwrap_or(0.0),
                    avg_tokens: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    session_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    total_tokens: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                })
            })
            .map_err(|e| e.to_string())?;

        Ok(result)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")
            .map_err(|e| format!("Prepare error: {e}"))?;
        let result = stmt.query_row(params![key], |row| row.get(0)).ok();
        Ok(result)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| format!("Setting write error: {e}"))?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
            .map_err(|e| format!("Setting delete error: {e}"))?;
        Ok(())
    }

    pub fn get_provider_settings_json(&self) -> Result<Option<String>, String> {
        self.get_setting(PROVIDER_SETTINGS_KEY)
    }

    pub fn set_provider_settings_json(&self, value: &str) -> Result<(), String> {
        self.set_setting(PROVIDER_SETTINGS_KEY, value)
    }

    #[allow(dead_code)]
    pub fn get_indicator_primary_provider(&self) -> Result<Option<IntegrationProvider>, String> {
        let Some(raw) = self.get_setting(INDICATOR_PRIMARY_PROVIDER_KEY)? else {
            return Ok(None);
        };

        match serde_json::from_str(&raw) {
            Ok(provider) => Ok(provider),
            Err(_) => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub fn set_indicator_primary_provider(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<(), String> {
        match provider {
            Some(provider) => {
                let encoded = serde_json::to_string(&Some(provider)).map_err(|e| {
                    format!("Serialize error for indicator.primary_provider.v1: {e}")
                })?;
                self.set_setting(INDICATOR_PRIMARY_PROVIDER_KEY, &encoded)
            }
            None => self.delete_setting(INDICATOR_PRIMARY_PROVIDER_KEY),
        }
    }

    pub fn delete_host_data(&self, hostname: &str) -> Result<u64, String> {
        let mut conn = self.conn.lock();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        let snap_count = tx
            .execute(
                "DELETE FROM token_snapshots WHERE hostname = ?1",
                params![hostname],
            )
            .map_err(|e| format!("Delete snapshots error: {e}"))?;

        let hourly_count = tx
            .execute(
                "DELETE FROM token_hourly WHERE hostname = ?1",
                params![hostname],
            )
            .map_err(|e| format!("Delete hourly error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok((snap_count + hourly_count) as u64)
    }

    pub fn delete_session_data(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<u64, String> {
        let conn = self.conn.lock();

        let count = conn
            .execute(
                "DELETE FROM token_snapshots WHERE provider = ?1 AND session_id = ?2",
                params![provider.as_str(), session_id],
            )
            .map_err(|e| format!("Delete error: {e}"))?;

        Ok(count as u64)
    }

    pub fn delete_project_data(&self, cwd: &str) -> Result<u64, String> {
        let conn = self.conn.lock();

        let count = conn
            .execute("DELETE FROM token_snapshots WHERE cwd = ?1", params![cwd])
            .map_err(|e| format!("Delete error: {e}"))?;

        Ok(count as u64)
    }

    pub fn rename_project(&self, old_cwd: &str, new_cwd: &str) -> Result<u64, String> {
        let new_cwd = new_cwd.trim();
        if new_cwd.is_empty() {
            return Err("New project path cannot be empty".to_string());
        }
        if new_cwd == old_cwd {
            return Err("New path is the same as the current path".to_string());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        let snap_count = tx
            .execute(
                "UPDATE token_snapshots SET cwd = ?2 WHERE cwd = ?1",
                params![old_cwd, new_cwd],
            )
            .map_err(|e| format!("Update token_snapshots error: {e}"))?;

        tx.execute(
            "UPDATE observations SET cwd = ?2 WHERE cwd = ?1",
            params![old_cwd, new_cwd],
        )
        .map_err(|e| format!("Update observations error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(snap_count as u64)
    }

    pub fn get_project_provider_map(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<std::collections::HashMap<String, Vec<IntegrationProvider>>, String> {
        let conn = self.conn.lock();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT provider, cwd FROM token_snapshots
                 WHERE cwd IS NOT NULL AND provider = ?1
                 UNION
                 SELECT provider, cwd FROM observations
                 WHERE cwd IS NOT NULL AND provider = ?1",
                vec![Box::new(provider.as_str().to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
            None => (
                "SELECT provider, cwd FROM token_snapshots
                 WHERE cwd IS NOT NULL
                 UNION
                 SELECT provider, cwd FROM observations
                 WHERE cwd IS NOT NULL",
                Vec::new(),
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare project providers error: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|param| param.as_ref()).collect();
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Query project providers error: {e}"))?;

        let mut project_providers: std::collections::HashMap<String, Vec<IntegrationProvider>> =
            std::collections::HashMap::new();
        for row in rows {
            let (provider_name, cwd) = row.map_err(|e| format!("Project provider row: {e}"))?;
            let parsed_provider: IntegrationProvider = provider_name.parse()?;
            let entry = project_providers.entry(cwd).or_default();
            if !entry.contains(&parsed_provider) {
                entry.push(parsed_provider);
                entry.sort_by_key(|provider| provider.as_str());
            }
        }

        Ok(project_providers)
    }

    // --- Learning system methods ---

    pub fn store_observation(&self, payload: &ObservationPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO observations (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                payload.provider.as_str(),
                payload.session_id,
                now,
                payload.hook_phase,
                payload.tool_name,
                payload.tool_input,
                payload.tool_output,
                payload.cwd,
            ],
        )
        .map_err(|e| format!("Insert observation error: {e}"))?;
        Ok(())
    }

    pub fn get_recent_observations(
        &self,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<serde_json::Value>, String> {
        self.get_observations_since(None, limit, provider)
    }

    pub fn get_unanalyzed_observations(
        &self,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let since = self.latest_completed_learning_run_created_at(provider)?;
        self.get_observations_since(Some(&since), limit, provider)
    }

    fn latest_completed_learning_run_created_at(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<String, String> {
        let conn = self.conn.lock();
        match provider {
            Some(provider) => conn
                .query_row(
                    "SELECT COALESCE(MAX(created_at), '1970-01-01')
                     FROM learning_runs
                     WHERE status = 'completed'
                       AND instr(provider_scope, ?1) > 0",
                    params![provider_scope_contains_json(provider)],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Query last run error: {e}")),
            None => conn
                .query_row(
                    "SELECT COALESCE(MAX(created_at), '1970-01-01')
                     FROM learning_runs
                     WHERE status = 'completed'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Query last run error: {e}")),
        }
    }

    fn get_observations_since(
        &self,
        since: Option<&str>,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.conn.lock();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match since {
            Some(s) if provider.is_some() => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE created_at > ?1 AND provider = ?2
                 ORDER BY created_at DESC
                 LIMIT ?3",
                vec![
                    Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider.expect("provider checked above").as_str().to_string()),
                    Box::new(limit),
                ],
            ),
            Some(s) => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE created_at > ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
                vec![Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>, Box::new(limit)],
            ),
            None if provider.is_some() => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE provider = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
                vec![
                    Box::new(provider.expect("provider checked above").as_str().to_string())
                        as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 ORDER BY created_at DESC
                 LIMIT ?1",
                vec![Box::new(limit) as Box<dyn rusqlite::types::ToSql>],
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, i64>(0)?,
                    "provider": row.get::<_, String>(1)?,
                    "session_id": row.get::<_, String>(2)?,
                    "timestamp": row.get::<_, String>(3)?,
                    "hook_phase": row.get::<_, String>(4)?,
                    "tool_name": row.get::<_, String>(5)?,
                    "tool_input": row.get::<_, Option<String>>(6)?,
                    "tool_output": row.get::<_, Option<String>>(7)?,
                    "cwd": row.get::<_, Option<String>>(8)?,
                    "created_at": row.get::<_, String>(9)?,
                }))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_observation_count(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        match provider {
            Some(provider) => conn
                .query_row(
                    "SELECT COUNT(*) FROM observations WHERE provider = ?1",
                    params![provider.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count error: {e}")),
            None => conn
                .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
                .map_err(|e| format!("Count error: {e}")),
        }
    }

    pub fn get_unanalyzed_observation_count(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<i64, String> {
        let since = self.latest_completed_learning_run_created_at(provider)?;
        let conn = self.conn.lock();
        match provider {
            Some(provider) => conn
                .query_row(
                    "SELECT COUNT(*) FROM observations
                     WHERE created_at > ?1 AND provider = ?2",
                    params![since, provider.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count error: {e}")),
            None => conn
                .query_row(
                    "SELECT COUNT(*) FROM observations WHERE created_at > ?1",
                    params![since],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count error: {e}")),
        }
    }

    pub fn get_top_tools(
        &self,
        limit: i64,
        days: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<ToolCount>, String> {
        let conn = self.conn.lock();
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT tool_name, COUNT(*) as count FROM observations
                 WHERE created_at >= datetime('now', '-' || ?1 || ' days')
                   AND provider = ?2
                 GROUP BY tool_name ORDER BY count DESC LIMIT ?3",
                vec![
                    Box::new(days) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider.as_str().to_string()),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT tool_name, COUNT(*) as count FROM observations
                 WHERE created_at >= datetime('now', '-' || ?1 || ' days')
                 GROUP BY tool_name ORDER BY count DESC LIMIT ?2",
                vec![
                    Box::new(days) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|param| param.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(ToolCount {
                    tool_name: row.get(0)?,
                    count: row.get(1)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_observation_sparkline(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<i64>, String> {
        let conn = self.conn.lock();
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT DATE(created_at) as day, COUNT(*) as count
                 FROM observations
                 WHERE created_at >= DATE('now', '-6 days')
                   AND provider = ?1
                 GROUP BY day ORDER BY day ASC",
                vec![Box::new(provider.as_str().to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
            None => (
                "SELECT DATE(created_at) as day, COUNT(*) as count
                 FROM observations
                 WHERE created_at >= DATE('now', '-6 days')
                 GROUP BY day ORDER BY day ASC",
                Vec::new(),
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|param| param.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut day_map = std::collections::HashMap::new();
        for row in rows {
            let (day, count) = row.map_err(|e| format!("Row error: {e}"))?;
            day_map.insert(day, count);
        }

        // Build a 7-element array, zero-filled for missing days
        let today = Utc::now().date_naive();
        let mut result = Vec::with_capacity(7);
        for i in (0..7).rev() {
            let day = (today - chrono::Duration::days(i))
                .format("%Y-%m-%d")
                .to_string();
            let count = day_map.get(&day).copied().unwrap_or(0);
            result.push(count);
        }
        Ok(result)
    }

    pub fn store_learning_run(&self, payload: &LearningRunPayload) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, phases, provider_scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                payload.trigger_mode,
                payload.observations_analyzed,
                payload.rules_created,
                payload.rules_updated,
                payload.duration_ms,
                payload.status,
                payload.error,
                payload.logs,
                payload.phases,
                provider_scope_json(&payload.provider_scope),
            ],
        )
        .map_err(|e| format!("Insert learning run error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn create_learning_run(
        &self,
        trigger_mode: &str,
        provider_scope: &[IntegrationProvider],
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO learning_runs (trigger_mode, status, observations_analyzed, rules_created, rules_updated, provider_scope)
             VALUES (?1, 'running', 0, 0, 0, ?2)",
            params![trigger_mode, provider_scope_json(provider_scope)],
        )
        .map_err(|e| format!("Create learning run error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_learning_run(&self, id: i64, payload: &LearningRunPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE learning_runs SET observations_analyzed=?2, rules_created=?3, rules_updated=?4,
             duration_ms=?5, status=?6, error=?7, logs=?8, phases=?9, provider_scope=?10
             WHERE id=?1",
            params![
                id,
                payload.observations_analyzed,
                payload.rules_created,
                payload.rules_updated,
                payload.duration_ms,
                payload.status,
                payload.error,
                payload.logs,
                payload.phases,
                provider_scope_json(&payload.provider_scope),
            ],
        )
        .map_err(|e| format!("Update learning run error: {e}"))?;
        Ok(())
    }

    pub fn cleanup_interrupted_runs(&self) -> Result<u64, String> {
        let conn = self.conn.lock();
        let count = conn
            .execute(
                "UPDATE learning_runs SET status='interrupted' WHERE status='running'",
                [],
            )
            .map_err(|e| format!("Cleanup interrupted runs error: {e}"))?;
        Ok(count as u64)
    }

    pub fn get_learning_runs(
        &self,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<LearningRun>, String> {
        let conn = self.conn.lock();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT id, trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, created_at, phases, provider_scope
                 FROM learning_runs
                 WHERE instr(provider_scope, ?1) > 0
                 ORDER BY created_at DESC LIMIT ?2",
                vec![
                    Box::new(provider_scope_contains_json(provider))
                        as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, created_at, phases, provider_scope
                 FROM learning_runs
                 ORDER BY created_at DESC LIMIT ?1",
                vec![Box::new(limit) as Box<dyn rusqlite::types::ToSql>],
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|param| param.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(LearningRun {
                    id: row.get(0)?,
                    trigger_mode: row.get(1)?,
                    observations_analyzed: row.get(2)?,
                    rules_created: row.get(3)?,
                    rules_updated: row.get(4)?,
                    duration_ms: row.get(5)?,
                    status: row.get(6)?,
                    error: row.get(7)?,
                    logs: row.get(8)?,
                    created_at: row.get(9)?,
                    phases: row.get(10)?,
                    provider_scope: parse_provider_scope(row.get(11)?),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn store_learned_rule(&self, payload: &LearnedRulePayload) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        // Dynamic evidence scaling: more observations → more weight.
        // Clamped to [5, 20] so tiny batches don't over-commit and large batches
        // don't overwhelm existing evidence.
        let evidence_scale = (payload.observation_count as f64).clamp(5.0, 20.0);
        let alpha = payload.confidence * evidence_scale;
        let beta = (1.0 - payload.confidence) * evidence_scale;
        let is_anti = payload.is_anti_pattern as i32;
        let existing_scope: Option<String> = conn
            .query_row(
                "SELECT provider_scope FROM learned_rules WHERE name = ?1",
                params![payload.name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Read learned rule scope error: {e}"))?;
        let mut merged_scope = parse_provider_scope(existing_scope);
        merged_scope.extend(payload.provider_scope.iter().copied());
        let merged_scope = normalized_provider_scope(&merged_scope);
        conn.execute(
            "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, last_evidence_at, state, project, is_anti_pattern, source, content, provider_scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'emerging', ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(name) DO UPDATE SET
                 domain = excluded.domain,
                 alpha = learned_rules.alpha + excluded.alpha,
                 beta_param = learned_rules.beta_param + excluded.beta_param,
                 observation_count = learned_rules.observation_count + excluded.observation_count,
                 file_path = CASE WHEN length(excluded.file_path) > 0 THEN excluded.file_path ELSE learned_rules.file_path END,
                 last_evidence_at = excluded.last_evidence_at,
                 is_anti_pattern = excluded.is_anti_pattern,
                 source = CASE WHEN excluded.source IS NOT NULL THEN excluded.source ELSE learned_rules.source END,
                 content = CASE WHEN excluded.content IS NOT NULL THEN excluded.content ELSE learned_rules.content END,
                 provider_scope = excluded.provider_scope,
                 updated_at = datetime('now')",
            params![
                payload.name,
                payload.domain,
                payload.confidence,
                payload.observation_count,
                payload.file_path,
                alpha,
                beta,
                now,
                payload.project,
                is_anti,
                payload.source,
                payload.content,
                provider_scope_json(&merged_scope),
            ],
        )
        .map_err(|e| format!("Insert learned rule error: {e}"))?;
        Ok(())
    }

    pub fn reinforce_rule(&self, name: &str, strength: f64) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE learned_rules SET alpha = alpha + ?1, last_evidence_at = ?2, updated_at = datetime('now') WHERE name = ?3",
            params![strength, now, name],
        )
        .map_err(|e| format!("Reinforce rule error: {e}"))?;
        Ok(())
    }

    pub fn contradict_rule(&self, name: &str, strength: f64) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE learned_rules SET beta_param = beta_param + ?1, last_evidence_at = ?2, updated_at = datetime('now') WHERE name = ?3",
            params![strength, now, name],
        )
        .map_err(|e| format!("Contradict rule error: {e}"))?;
        Ok(())
    }

    pub fn get_learned_rules(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<LearnedRule>, String> {
        let mut meta_map = {
            let conn = self.conn.lock();
            let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
                Some(provider) => (
                    "SELECT name, domain, alpha, beta_param, observation_count, last_evidence_at, state, project, created_at, updated_at, is_anti_pattern, source, content, provider_scope
                     FROM learned_rules
                     WHERE state != 'suppressed' AND instr(provider_scope, ?1) > 0",
                    vec![
                        Box::new(provider_scope_contains_json(provider))
                            as Box<dyn rusqlite::types::ToSql>,
                    ],
                ),
                None => (
                    "SELECT name, domain, alpha, beta_param, observation_count, last_evidence_at, state, project, created_at, updated_at, is_anti_pattern, source, content, provider_scope
                     FROM learned_rules
                     WHERE state != 'suppressed'",
                    Vec::new(),
                ),
            };
            let mut stmt = conn
                .prepare_cached(sql)
                .map_err(|e| format!("Prepare error: {e}"))?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params_vec.iter().map(|param| param.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, i32>(10).unwrap_or(0),
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<String>>(13)?,
                    ))
                })
                .map_err(|e| format!("Query error: {e}"))?;

            let mut map = std::collections::HashMap::new();
            for row in rows {
                let (
                    name,
                    domain,
                    alpha,
                    beta,
                    obs_count,
                    last_ev,
                    state,
                    project,
                    created,
                    updated,
                    is_anti,
                    source,
                    content,
                    provider_scope,
                ) = row.map_err(|e| format!("Row error: {e}"))?;
                map.insert(
                    name,
                    (
                        domain,
                        alpha,
                        beta,
                        obs_count,
                        last_ev,
                        state,
                        project,
                        created,
                        updated,
                        is_anti,
                        source,
                        content,
                        parse_provider_scope(provider_scope),
                    ),
                );
            }
            map
        };

        let mut rules = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        fn collect_rule_files(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        collect_rule_files(&path, out);
                    } else if path.extension().is_some_and(|ext| ext == "md") {
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let file_path = path.to_string_lossy().to_string();
                        out.push((name, file_path));
                    }
                }
            }
        }

        for rules_dir in learned_rule_dirs(provider) {
            if !rules_dir.exists() {
                continue;
            }

            let mut files = Vec::new();
            collect_rule_files(&rules_dir, &mut files);

            for (name, file_path) in files {
                if !seen_names.insert(name.clone()) {
                    continue;
                }

                let inferred_scope = inferred_rule_provider_scope(std::path::Path::new(&file_path));
                let now = Utc::now().to_rfc3339();
                let (
                    domain,
                    alpha,
                    beta,
                    observation_count,
                    last_ev,
                    _db_state,
                    project,
                    created_at,
                    updated_at,
                    is_anti,
                    source,
                    _content,
                    provider_scope,
                ) = meta_map.remove(&name).unwrap_or((
                    None,
                    1.0,
                    1.0,
                    0,
                    None,
                    "emerging".to_string(),
                    None,
                    now.clone(),
                    now,
                    0,
                    None,
                    None,
                    inferred_scope,
                ));

                let fresh = freshness_factor(last_ev.as_deref());
                let eff_alpha = alpha * fresh;
                let eff_beta = beta * fresh;
                let confidence = wilson_lower_bound(eff_alpha, eff_beta);
                let state = compute_state(confidence, alpha, beta, fresh).to_string();

                rules.push(LearnedRule {
                    name,
                    domain,
                    confidence,
                    observation_count,
                    file_path,
                    created_at,
                    updated_at,
                    state,
                    project,
                    is_anti_pattern: is_anti != 0,
                    source,
                    content: None,
                    provider_scope,
                });
            }
        }

        // Include DB-only rules (candidates that haven't met the confidence threshold yet)
        for (
            name,
            (
                domain,
                alpha,
                beta,
                observation_count,
                last_ev,
                _db_state,
                project,
                created_at,
                updated_at,
                is_anti,
                source,
                content_val,
                provider_scope,
            ),
        ) in meta_map
        {
            let fresh = freshness_factor(last_ev.as_deref());
            let eff_alpha = alpha * fresh;
            let eff_beta = beta * fresh;
            let confidence = wilson_lower_bound(eff_alpha, eff_beta);
            let state = "candidate".to_string();

            rules.push(LearnedRule {
                name,
                domain,
                confidence,
                observation_count,
                file_path: String::new(),
                created_at,
                updated_at,
                state,
                project,
                is_anti_pattern: is_anti != 0,
                source,
                content: content_val,
                provider_scope,
            });
        }

        // Impact-based sorting: confidence * ln(observation_count + 1)
        // This ranks impactful, well-evidenced rules higher than trivial high-confidence ones
        rules.sort_by(|a, b| {
            let score_a = a.confidence * (a.observation_count as f64 + 1.0).ln();
            let score_b = b.confidence * (b.observation_count as f64 + 1.0).ln();
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(rules)
    }

    pub fn get_git_snapshot(&self, project: &str) -> Result<Option<GitSnapshot>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT project, commit_hash, commit_count, raw_data
                 FROM git_snapshots WHERE project = ?1",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let result = stmt
            .query_row(params![project], |row| {
                Ok(GitSnapshot {
                    project: row.get(0)?,
                    commit_hash: row.get(1)?,
                    commit_count: row.get(2)?,
                    raw_data: row.get(3)?,
                })
            })
            .optional()
            .map_err(|e| format!("Query error: {e}"))?;

        Ok(result)
    }

    pub fn upsert_git_snapshot(&self, snapshot: &GitSnapshot) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO git_snapshots (project, commit_hash, commit_count, raw_data)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project) DO UPDATE SET
                 commit_hash = excluded.commit_hash,
                 commit_count = excluded.commit_count,
                 raw_data = excluded.raw_data,
                 created_at = datetime('now')",
            params![
                snapshot.project,
                snapshot.commit_hash,
                snapshot.commit_count,
                snapshot.raw_data,
            ],
        )
        .map_err(|e| format!("Upsert git snapshot error: {e}"))?;
        Ok(())
    }

    pub fn delete_learned_rule(&self, name: &str) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }

        // Soft-delete: keep the DB record with strong negative feedback so the
        // LLM can't trivially re-promote the same pattern. Boost beta by 5.0,
        // clear the file_path, and set state to 'suppressed' so the rule is
        // hidden from the UI but still tracked to prevent re-creation.
        let (file_path, provider_scope) = {
            let conn = self.conn.lock();
            let record: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT file_path, provider_scope FROM learned_rules WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("Read learned rule metadata error: {e}"))?;

            conn.execute(
                "UPDATE learned_rules SET beta_param = beta_param + 5.0, file_path = '', state = 'suppressed', updated_at = datetime('now') WHERE name = ?1",
                params![name],
            )
            .ok();
            let (file_path, provider_scope) = record.unwrap_or((None, None));
            (file_path, parse_provider_scope(provider_scope))
        };

        // Delete the .md file from disk
        if let Some(fp) = &file_path
            && !fp.is_empty()
        {
            let path = std::path::Path::new(fp);
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| format!("Delete file error: {e}"))?;
            }
        }

        // Also search recursively as fallback
        fn find_and_delete(dir: &std::path::Path, name: &str) -> bool {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if find_and_delete(&path, name) {
                            return true;
                        }
                    } else if path.file_stem().is_some_and(|s| s == name)
                        && path.extension().is_some_and(|e| e == "md")
                    {
                        let _ = std::fs::remove_file(&path);
                        return true;
                    }
                }
            }
            false
        }
        for rules_dir in learned_rule_dirs_for_scope(&provider_scope) {
            if find_and_delete(&rules_dir, name) {
                break;
            }
        }
        Ok(())
    }

    pub fn promote_learned_rule(&self, name: &str) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }

        let (content, provider_scope): (String, Vec<IntegrationProvider>) = {
            let conn = self.conn.lock();
            let row: (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT content, provider_scope FROM learned_rules WHERE name = ?1 AND state != 'suppressed'",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| format!("Rule not found: {e}"))?;
            let (content_opt, provider_scope) = row;
            (
                content_opt.ok_or_else(|| {
                    "No stored content for this rule — re-run analysis to capture content"
                        .to_string()
                })?,
                parse_provider_scope(provider_scope),
            )
        };

        let rules_dir = crate::learning::learned_rules_dir_for_scope(&provider_scope);
        std::fs::create_dir_all(&rules_dir).map_err(|e| format!("Cannot create rules dir: {e}"))?;

        let file_path = rules_dir.join(format!("{name}.md"));
        let canonical_dir = rules_dir
            .canonicalize()
            .map_err(|e| format!("Canonicalize error: {e}"))?;
        let canonical_parent = file_path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_default();
        if !canonical_parent.starts_with(&canonical_dir) {
            return Err(format!("Path traversal detected for rule: {name}"));
        }

        let sanitized = crate::learning::sanitize_rule_content(&content);
        std::fs::write(&file_path, &sanitized)
            .map_err(|e| format!("Failed to write rule file: {e}"))?;

        // Update DB to record file_path
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE learned_rules SET file_path = ?1, updated_at = datetime('now') WHERE name = ?2",
            params![file_path.to_string_lossy().as_ref(), name],
        )
        .map_err(|e| format!("Update file_path error: {e}"))?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_rule_content_hash(&self, name: &str, hash: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE learned_rules SET content_hash = ?1, updated_at = datetime('now') WHERE name = ?2",
            params![hash, name],
        )
        .map_err(|e| format!("Update content_hash error: {e}"))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn reconcile_learned_rules(&self) -> Result<bool, String> {
        use sha2::{Digest, Sha256};

        // Step 1: Scan filesystem
        let mut fs_rules: std::collections::HashMap<
            String,
            (
                std::path::PathBuf,
                Vec<crate::integrations::IntegrationProvider>,
            ),
        > = std::collections::HashMap::new();

        for rules_dir in learned_rule_dirs(None) {
            if !rules_dir.exists() {
                continue;
            }
            fn collect_files(dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            collect_files(&path, out);
                        } else if path.extension().is_some_and(|ext| ext == "md") {
                            let name = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("unknown")
                                .to_string();
                            out.push((name, path));
                        }
                    }
                }
            }
            let mut files = Vec::new();
            collect_files(&rules_dir, &mut files);
            for (name, path) in files {
                fs_rules.entry(name).or_insert_with(|| {
                    let scope = inferred_rule_provider_scope(&path);
                    (path, scope)
                });
            }
        }

        // Step 2: Query DB for non-suppressed rules
        let db_rules: std::collections::HashMap<String, (String, Option<String>, Option<String>)> = {
            let conn = self.conn.lock();
            let mut stmt = conn
                .prepare_cached(
                    "SELECT name, file_path, content_hash, provider_scope FROM learned_rules WHERE state != 'suppressed'",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|e| format!("Query error: {e}"))?;
            let mut map = std::collections::HashMap::new();
            for row in rows {
                let (name, file_path, content_hash, _provider_scope) =
                    row.map_err(|e| format!("Row error: {e}"))?;
                map.insert(name, (file_path, content_hash, _provider_scope));
            }
            map
        };

        let mut changed = false;

        // Step 3a: Files on disk but not in DB -> INSERT
        for (name, (path, scope)) in &fs_rules {
            if db_rules.contains_key(name) {
                continue;
            }
            if !crate::learning::is_safe_rule_name(name) {
                continue;
            }
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let content_str = String::from_utf8_lossy(&bytes);
            let (domain, is_anti, body) = parse_rule_frontmatter(&content_str);
            let scope_json =
                serde_json::to_string(&scope.iter().map(|p| p.to_string()).collect::<Vec<_>>())
                    .unwrap_or_else(|_| "[\"claude\"]".to_string());
            let file_path_str = path.to_string_lossy();

            let conn = self.conn.lock();
            conn.execute(
                "INSERT OR IGNORE INTO learned_rules (name, domain, alpha, beta_param, observation_count, state, file_path, provider_scope, source, content, content_hash, is_anti_pattern)
                 VALUES (?1, ?2, 1.0, 1.0, 0, 'emerging', ?3, ?4, 'manual', ?5, ?6, ?7)",
                params![name, domain, file_path_str.as_ref(), scope_json, body, hash, is_anti as i32],
            )
            .map_err(|e| format!("Insert reconciled rule error: {e}"))?;
            changed = true;
        }

        // Step 3b: DB rows with file_path but file missing -> soft-suppress
        for (name, (file_path, _content_hash, _provider_scope)) in &db_rules {
            if file_path.is_empty() {
                continue;
            }
            if fs_rules.contains_key(name) {
                continue;
            }
            let path = std::path::Path::new(file_path.as_str());
            if !path.exists() {
                let conn = self.conn.lock();
                conn.execute(
                    "UPDATE learned_rules SET beta_param = beta_param + 5.0, state = 'suppressed', file_path = '', updated_at = datetime('now') WHERE name = ?1",
                    params![name],
                )
                .map_err(|e| format!("Suppress reconciled rule error: {e}"))?;
                changed = true;
            }
        }

        // Step 3c: Both exist, check content hash
        for (name, (path, _scope)) in &fs_rules {
            let Some((file_path, existing_hash, _provider_scope)) = db_rules.get(name) else {
                continue;
            };
            if file_path.is_empty() {
                continue;
            }
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let hash = format!("{:x}", Sha256::digest(&bytes));
            if existing_hash.as_deref() == Some(hash.as_str()) {
                continue;
            }
            let content_str = String::from_utf8_lossy(&bytes);
            let (_domain, _is_anti, body) = parse_rule_frontmatter(&content_str);
            let conn = self.conn.lock();
            conn.execute(
                "UPDATE learned_rules SET content = ?1, content_hash = ?2, updated_at = datetime('now') WHERE name = ?3",
                params![body, hash, name],
            )
            .map_err(|e| format!("Update reconciled rule content error: {e}"))?;
            changed = true;
        }

        Ok(changed)
    }

    pub fn get_learning_status(&self) -> Result<LearningStatus, String> {
        let observation_count = self.get_observation_count(None)?;
        let unanalyzed_count = self.get_unanalyzed_observation_count(None)?;
        let rules = self.get_learned_rules(None)?;
        let runs = self.get_learning_runs(1, None)?;

        Ok(LearningStatus {
            observation_count,
            unanalyzed_count,
            rules_count: rules.len() as i64,
            last_run: runs.into_iter().next(),
        })
    }

    pub fn cleanup_old_observations(&self) -> Result<(), String> {
        let conn = self.conn.lock();

        // Store compressed summary of observations about to be deleted,
        // preserving historical trends even after raw data is gone.
        let cutoff_ts: String = conn
            .query_row(
                "SELECT MIN(
                    COALESCE((SELECT MAX(created_at) FROM learning_runs WHERE status = 'completed'), datetime('now', '-30 days')),
                    datetime('now', '-7 days')
                )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| Utc::now().to_rfc3339());

        // Aggregate tool counts and error counts by project for the period being cleaned
        let mut summary_stmt = conn
            .prepare_cached(
                "SELECT provider, cwd, tool_name, COUNT(*) as cnt,
                        SUM(CASE WHEN tool_output LIKE '%error%' OR tool_output LIKE '%Error%' THEN 1 ELSE 0 END) as err_cnt
                 FROM observations
                 WHERE created_at < ?1
                 GROUP BY provider, cwd, tool_name",
            )
            .map_err(|e| format!("Summary prepare error: {e}"))?;

        let summary_rows = summary_stmt
            .query_map(params![cutoff_ts], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| format!("Summary query error: {e}"))?;

        // Group by provider and project
        type ObservationSummaryKey = (String, Option<String>);
        type ObservationSummaryValue = (serde_json::Map<String, serde_json::Value>, i64, i64);

        let mut project_summaries: std::collections::HashMap<
            ObservationSummaryKey,
            ObservationSummaryValue,
        > = std::collections::HashMap::new();
        for row in summary_rows {
            let (provider, project, tool, count, errors) =
                row.map_err(|e| format!("Summary row: {e}"))?;
            let entry = project_summaries
                .entry((provider, project))
                .or_insert_with(|| (serde_json::Map::new(), 0, 0));
            entry.0.insert(tool, serde_json::Value::from(count));
            entry.1 += errors;
            entry.2 += count;
        }
        drop(summary_stmt);

        let period = Utc::now().format("%Y-%m-%d").to_string();
        for ((provider, project), (tool_counts, error_count, total)) in &project_summaries {
            if *total == 0 {
                continue;
            }
            let tc_json = serde_json::Value::Object(tool_counts.clone()).to_string();
            conn.execute(
                "INSERT OR REPLACE INTO observation_summaries (period, provider, project, tool_counts, error_count, total_observations)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![period, provider, project, tc_json, error_count, total],
            )
            .ok(); // Best-effort: don't fail cleanup if summary write fails
        }

        conn.execute(
            "DELETE FROM observations WHERE created_at < ?1",
            params![cutoff_ts],
        )
        .map_err(|e| format!("Observation cleanup error: {e}"))?;
        Ok(())
    }

    pub fn aggregate_and_cleanup_tokens(&self) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let cutoff = (Utc::now() - TimeDelta::days(30)).to_rfc3339();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        tx.execute(
            "INSERT INTO token_hourly (hour, provider, hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count)
             SELECT
                 strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour,
                 provider,
                 hostname,
                 SUM(input_tokens),
                 SUM(output_tokens),
                 SUM(cache_creation_input_tokens),
                 SUM(cache_read_input_tokens),
                 COUNT(*)
             FROM token_snapshots
             WHERE timestamp < ?1
             GROUP BY hour, provider, hostname
             ON CONFLICT(hour, hostname, provider) DO UPDATE SET
                 total_input = token_hourly.total_input + excluded.total_input,
                 total_output = token_hourly.total_output + excluded.total_output,
                 total_cache_creation = token_hourly.total_cache_creation + excluded.total_cache_creation,
                 total_cache_read = token_hourly.total_cache_read + excluded.total_cache_read,
                 turn_count = token_hourly.turn_count + excluded.turn_count",
            params![cutoff],
        )
        .map_err(|e| format!("Token aggregation insert error: {e}"))?;

        tx.execute(
            "DELETE FROM token_snapshots WHERE timestamp < ?1",
            params![cutoff],
        )
        .map_err(|e| format!("Token aggregation delete error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(())
    }

    /// Delete all tool_actions for a session (used before re-indexing to prevent duplicates).
    pub fn delete_tool_actions_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM tool_actions WHERE provider = ?1 AND session_id = ?2",
            rusqlite::params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Delete tool_actions for session: {e}"))?;
        Ok(())
    }

    pub fn store_tool_actions_for_messages(
        &self,
        provider: IntegrationProvider,
        messages: &[crate::sessions::ExtractedMessage],
    ) -> Result<(), String> {
        if !messages
            .iter()
            .any(|message| !message.tool_actions.is_empty())
        {
            return Ok(());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin tool_actions batch transaction: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO tool_actions (provider, message_id, session_id, tool_name, category, file_path, summary, full_input, full_output, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )
                .map_err(|e| format!("Prepare store_tool_actions batch: {e}"))?;

            for message in messages {
                if message.tool_actions.is_empty() {
                    continue;
                }

                insert_tool_actions(
                    &mut stmt,
                    provider,
                    &message.tool_actions,
                    &message.uuid,
                    &message.session_id,
                )?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Commit tool_actions batch: {e}"))?;
        Ok(())
    }

    // --- Memory optimizer storage methods ---

    /// Create a new optimization run record. Returns the run ID.
    /// Uses atomic INSERT...WHERE NOT EXISTS to prevent TOCTOU race.
    #[allow(dead_code)]
    pub fn create_optimization_run(
        &self,
        project_path: &str,
        trigger: &str,
        provider_scope: &[IntegrationProvider],
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        // Atomic: insert only if no running run exists for this project
        conn.execute(
            "INSERT INTO optimization_runs (project_path, provider_scope, trigger, memories_scanned, suggestions_created, context_sources, status, started_at)
             SELECT ?1, ?2, ?3, 0, 0, '{}', 'running', ?4
             WHERE NOT EXISTS (
                 SELECT 1 FROM optimization_runs WHERE project_path = ?1 AND status = 'running'
             )",
            rusqlite::params![project_path, provider_scope_json(provider_scope), trigger, now],
        )
        .map_err(|e| format!("Failed to create optimization run: {e}"))?;

        if conn.changes() == 0 {
            return Err(format!(
                "An optimization is already running for {project_path}"
            ));
        }
        Ok(conn.last_insert_rowid())
    }

    /// Update an optimization run with results.
    #[allow(dead_code)]
    pub fn update_optimization_run(
        &self,
        run_id: i64,
        memories_scanned: i64,
        suggestions_created: i64,
        context_sources: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE optimization_runs SET memories_scanned = ?1, suggestions_created = ?2,
             context_sources = ?3, status = ?4, error = ?5, completed_at = ?6 WHERE id = ?7",
            rusqlite::params![
                memories_scanned,
                suggestions_created,
                context_sources,
                status,
                error,
                now,
                run_id
            ],
        )
        .map_err(|e| format!("Failed to update optimization run: {e}"))?;
        Ok(())
    }

    /// Get optimization runs for a project.
    #[allow(dead_code)]
    pub fn get_optimization_runs(
        &self,
        project_path: &str,
        provider: Option<IntegrationProvider>,
        limit: i64,
    ) -> Result<Vec<crate::models::OptimizationRun>, String> {
        let conn = self.conn.lock();
        let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT id, project_path, provider_scope, trigger, memories_scanned, suggestions_created,
                        status, error, started_at, completed_at
                 FROM optimization_runs
                 WHERE project_path = ?1 AND instr(provider_scope, ?2) > 0
                 ORDER BY started_at DESC LIMIT ?3"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider_scope_contains_json(provider)),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, project_path, provider_scope, trigger, memories_scanned, suggestions_created,
                        status, error, started_at, completed_at
                 FROM optimization_runs
                 WHERE project_path = ?1
                 ORDER BY started_at DESC LIMIT ?2"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
        };
        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare optimization runs query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(crate::models::OptimizationRun {
                    id: row.get(0)?,
                    project_path: row.get(1)?,
                    provider_scope: parse_provider_scope(row.get(2)?),
                    trigger: row.get(3)?,
                    memories_scanned: row.get(4)?,
                    suggestions_created: row.get(5)?,
                    status: row.get(6)?,
                    error: row.get(7)?,
                    started_at: row.get(8)?,
                    completed_at: row.get(9)?,
                })
            })
            .map_err(|e| format!("Failed to query optimization runs: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect optimization runs: {e}"))
    }

    /// Store an optimization suggestion.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn store_optimization_suggestion(
        &self,
        run_id: i64,
        project_path: &str,
        action_type: &str,
        target_file: Option<&str>,
        reasoning: &str,
        proposed_content: Option<&str>,
        merge_sources: Option<&str>,
        original_content: Option<&str>,
        diff_summary: Option<&str>,
        backup_data: Option<&str>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        let provider_scope: String = conn
            .query_row(
                "SELECT provider_scope FROM optimization_runs WHERE id = ?1",
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read optimization run scope: {e}"))?;
        conn.execute(
            "INSERT INTO optimization_suggestions
             (run_id, project_path, provider_scope, action_type, target_file, reasoning, proposed_content, merge_sources, status, created_at, original_content, diff_summary, backup_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11, ?12)",
            rusqlite::params![run_id, project_path, provider_scope, action_type, target_file, reasoning, proposed_content, merge_sources, now, original_content, diff_summary, backup_data],
        )
        .map_err(|e| format!("Failed to store optimization suggestion: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    /// Get optimization suggestions for a project with pagination, optionally filtered by status.
    #[allow(dead_code)]
    pub fn get_optimization_suggestions(
        &self,
        project_path: &str,
        provider: Option<IntegrationProvider>,
        status_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();

        let mut query = "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                                proposed_content, merge_sources, status, error, resolved_at, created_at,
                                original_content, diff_summary, backup_data, group_id
                         FROM optimization_suggestions WHERE project_path = ?1"
            .to_string();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>];
        let mut next_param = 2;
        if let Some(provider) = provider {
            query.push_str(&format!(" AND instr(provider_scope, ?{next_param}) > 0"));
            params.push(Box::new(provider_scope_contains_json(provider)));
            next_param += 1;
        }
        if let Some(status) = status_filter {
            query.push_str(&format!(" AND status = ?{next_param}"));
            params.push(Box::new(status.to_string()));
            next_param += 1;
        }
        query.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ?{next_param} OFFSET ?{}",
            next_param + 1
        ));
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare suggestions query: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query suggestions: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect suggestions: {e}"))
    }

    /// Get denied suggestions for a project (for denial context in optimizer prompt).
    #[allow(dead_code)]
    pub fn get_denied_suggestions(
        &self,
        project_path: &str,
        provider: Option<IntegrationProvider>,
        limit: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions
                 WHERE project_path = ?1 AND status = 'denied' AND instr(provider_scope, ?2) > 0
                 ORDER BY resolved_at DESC LIMIT ?3"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider_scope_contains_json(provider)),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions
                 WHERE project_path = ?1 AND status = 'denied'
                 ORDER BY resolved_at DESC LIMIT ?2"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
        };
        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare denied suggestions query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query denied suggestions: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect denied suggestions: {e}"))
    }

    /// Update a suggestion status (approve/deny).
    #[allow(dead_code)]
    pub fn update_suggestion_status(
        &self,
        suggestion_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE optimization_suggestions SET status = ?1, error = ?2, resolved_at = ?3 WHERE id = ?4",
            rusqlite::params![status, error, now, suggestion_id],
        )
        .map_err(|e| format!("Failed to update suggestion status: {e}"))?;
        Ok(())
    }

    /// Get a single suggestion by ID.
    #[allow(dead_code)]
    pub fn get_suggestion_by_id(
        &self,
        suggestion_id: i64,
    ) -> Result<crate::models::OptimizationSuggestion, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                    proposed_content, merge_sources, status, error, resolved_at, created_at,
                    original_content, diff_summary, backup_data, group_id
             FROM optimization_suggestions WHERE id = ?1",
            rusqlite::params![suggestion_id],
            |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            },
        )
        .map_err(|e| format!("Suggestion not found: {e}"))
    }

    /// Upsert a memory file record (scan tracking).
    #[allow(dead_code)]
    pub fn upsert_memory_file(
        &self,
        project_path: &str,
        file_path: &str,
        content_hash: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO memory_files (project_path, file_path, content_hash, last_scanned_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_path, file_path) DO UPDATE SET content_hash = ?3, last_scanned_at = ?4",
            rusqlite::params![project_path, file_path, content_hash, now],
        )
        .map_err(|e| format!("Failed to upsert memory file: {e}"))?;
        Ok(())
    }

    /// Get previously recorded memory file hashes for change detection.
    #[allow(dead_code)]
    pub fn get_memory_file_hashes(
        &self,
        project_path: &str,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT file_path, content_hash FROM memory_files WHERE project_path = ?1",
            )
            .map_err(|e| format!("Failed to prepare memory files query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![project_path], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query memory files: {e}"))?;
        rows.collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(|e| format!("Failed to collect memory file hashes: {e}"))
    }

    /// Delete suggestion (for undeny — removes from DB so it's no longer in denial blocklist).
    #[allow(dead_code)]
    pub fn delete_suggestion(&self, suggestion_id: i64) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM optimization_suggestions WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| format!("Failed to delete suggestion: {e}"))?;
        Ok(())
    }

    /// Get all suggestions for a specific optimization run.
    #[allow(dead_code)]
    pub fn get_suggestions_for_run(
        &self,
        run_id: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions WHERE run_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare suggestions-for-run query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![run_id], |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query suggestions for run: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect suggestions for run: {e}"))
    }

    /// Clean up stale optimization suggestions:
    /// - Expire pending/undone suggestions older than 14 days (set status to 'expired')
    /// - Delete denied suggestions older than 90 days
    /// - Clear original_content/backup_data from approved suggestions older than 30 days
    #[allow(dead_code)]
    pub fn cleanup_stale_suggestions(&self, project_path: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now();
        let expire_cutoff = (now - TimeDelta::days(14)).to_rfc3339();
        let delete_cutoff = (now - TimeDelta::days(90)).to_rfc3339();
        let clear_cutoff = (now - TimeDelta::days(30)).to_rfc3339();

        // Expire pending/undone suggestions older than 14 days
        conn.execute(
            "UPDATE optimization_suggestions SET status = 'expired', resolved_at = datetime('now')
             WHERE project_path = ?1 AND status IN ('pending', 'undone') AND created_at < ?2",
            rusqlite::params![project_path, expire_cutoff],
        )
        .map_err(|e| format!("Failed to expire stale suggestions: {e}"))?;

        // Delete denied suggestions older than 90 days
        conn.execute(
            "DELETE FROM optimization_suggestions
             WHERE project_path = ?1 AND status = 'denied' AND resolved_at < ?2",
            rusqlite::params![project_path, delete_cutoff],
        )
        .map_err(|e| format!("Failed to delete old denied suggestions: {e}"))?;

        // Clear content from approved suggestions older than 30 days
        conn.execute(
            "UPDATE optimization_suggestions
             SET original_content = NULL, backup_data = NULL
             WHERE project_path = ?1 AND status = 'approved' AND resolved_at < ?2",
            rusqlite::params![project_path, clear_cutoff],
        )
        .map_err(|e| format!("Failed to clear old approved suggestion content: {e}"))?;

        Ok(())
    }

    /// Check if there's already a pending suggestion with the same action type and target file.
    #[allow(dead_code)]
    pub fn has_duplicate_pending(
        &self,
        project_path: &str,
        action_type: &str,
        target_file: Option<&str>,
    ) -> Result<bool, String> {
        let conn = self.conn.lock();
        let count: i64 = if let Some(tf) = target_file {
            conn.query_row(
                "SELECT COUNT(*) FROM optimization_suggestions
                 WHERE project_path = ?1 AND action_type = ?2 AND target_file = ?3 AND status = 'pending'",
                rusqlite::params![project_path, action_type, tf],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check duplicate pending: {e}"))?
        } else {
            conn.query_row(
                "SELECT COUNT(*) FROM optimization_suggestions
                 WHERE project_path = ?1 AND action_type = ?2 AND target_file IS NULL AND status = 'pending'",
                rusqlite::params![project_path, action_type],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check duplicate pending: {e}"))?
        };
        Ok(count > 0)
    }

    /// Set group_id for a suggestion.
    #[allow(dead_code)]
    pub fn set_suggestion_group_id(
        &self,
        suggestion_id: i64,
        group_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE optimization_suggestions SET group_id = ?1 WHERE id = ?2",
            rusqlite::params![group_id, suggestion_id],
        )
        .map_err(|e| format!("Failed to set suggestion group_id: {e}"))?;
        Ok(())
    }

    /// Get all pending suggestions in a group.
    #[allow(dead_code)]
    pub fn get_suggestions_by_group(
        &self,
        group_id: &str,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions WHERE group_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare group suggestions query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![group_id], |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query group suggestions: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect group suggestions: {e}"))
    }

    pub fn get_code_stats(&self, range: &str) -> Result<CodeStats, String> {
        let conn = self.conn.lock();
        let from = (Utc::now() - range_to_duration(range)).to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT tool_name, file_path, full_input, session_id
				 FROM tool_actions
				 WHERE category = 'code_change' AND timestamp >= ?1 AND full_input IS NOT NULL",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let mut total_added: i64 = 0;
        let mut total_removed: i64 = 0;
        let mut sessions: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut lang_lines: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();

        let rows = stmt
            .query_map([&from], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        for row in rows {
            let (tool_name, file_path, full_input, session_id) =
                row.map_err(|e| format!("Row error: {e}"))?;

            if let Some((added, removed, parsed_path)) = parse_code_change(&tool_name, &full_input)
            {
                total_added += added;
                total_removed += removed;
                sessions.insert(session_id);

                let path = if parsed_path.is_empty() {
                    file_path.unwrap_or_default()
                } else {
                    parsed_path
                };
                let lang = ext_to_language(&path);
                *lang_lines.entry(lang).or_insert(0) += added + removed;
            }
        }

        let session_count = sessions.len() as i64;
        let total_changed = total_added + total_removed;
        let avg_per_session = if session_count > 0 {
            total_changed as f64 / session_count as f64
        } else {
            0.0
        };

        let mut by_language: Vec<LanguageBreakdown> = lang_lines
            .into_iter()
            .map(|(language, lines)| {
                let percentage = if total_changed > 0 {
                    (lines as f64 / total_changed as f64) * 100.0
                } else {
                    0.0
                };
                LanguageBreakdown {
                    language: language.to_string(),
                    lines,
                    percentage,
                }
            })
            .collect();
        by_language.sort_by(|a, b| b.lines.cmp(&a.lines));

        Ok(CodeStats {
            lines_added: total_added,
            lines_removed: total_removed,
            net_change: total_added - total_removed,
            session_count,
            avg_per_session,
            by_language,
        })
    }

    pub fn get_code_stats_history(
        &self,
        range: &str,
    ) -> Result<Vec<CodeStatsHistoryPoint>, String> {
        let conn = self.conn.lock();
        let now = Utc::now();
        let from = now - range_to_duration(range);
        let from_str = from.to_rfc3339();

        let bucket_secs: i64 = match range {
            "1h" => 60,
            "24h" => 15 * 60,
            "7d" => 3600,
            "30d" => 86400,
            _ => 15 * 60,
        };

        let mut stmt = conn
            .prepare(
                "SELECT tool_name, full_input, timestamp
				 FROM tool_actions
				 WHERE category = 'code_change' AND timestamp >= ?1 AND full_input IS NOT NULL
				 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        struct RawChange {
            added: i64,
            removed: i64,
            ts: DateTime<Utc>,
        }
        let mut changes: Vec<RawChange> = Vec::new();

        let rows = stmt
            .query_map([&from_str], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        for row in rows {
            let (tool_name, full_input, timestamp) = row.map_err(|e| format!("Row error: {e}"))?;

            if let Some((added, removed, _)) = parse_code_change(&tool_name, &full_input)
                && let Ok(ts) = timestamp.parse::<DateTime<Utc>>()
            {
                changes.push(RawChange { added, removed, ts });
            }
        }

        let from_ts = from.timestamp();
        let now_ts = now.timestamp();
        let mut points: Vec<CodeStatsHistoryPoint> = Vec::new();
        let mut bucket_start = from_ts;

        while bucket_start < now_ts {
            let bucket_end = bucket_start + bucket_secs;
            let mut added = 0i64;
            let mut removed = 0i64;

            for c in &changes {
                let ct = c.ts.timestamp();
                if ct >= bucket_start && ct < bucket_end {
                    added += c.added;
                    removed += c.removed;
                }
            }

            let ts = DateTime::from_timestamp(bucket_start, 0)
                .unwrap_or(from)
                .to_rfc3339();

            points.push(CodeStatsHistoryPoint {
                timestamp: ts,
                lines_added: added,
                lines_removed: removed,
                total_changed: added + removed,
            });

            bucket_start = bucket_end;
        }

        Ok(points)
    }

    pub fn get_batch_session_code_stats(
        &self,
        session_refs: &[SessionRef],
    ) -> Result<std::collections::HashMap<String, SessionCodeStats>, String> {
        if session_refs.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self.conn.lock();
        let mut seen = std::collections::HashSet::new();
        let unique_refs: Vec<&SessionRef> = session_refs
            .iter()
            .filter(|session_ref| {
                seen.insert(session_key(session_ref.provider, &session_ref.session_id))
            })
            .collect();
        let sql = unique_refs
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                let provider_pos = idx * 2 + 1;
                let session_pos = provider_pos + 1;
                format!(
                    "SELECT provider, session_id, tool_name, full_input
                     FROM tool_actions
                     WHERE category = 'code_change'
                       AND full_input IS NOT NULL
                       AND provider = ?{provider_pos}
                       AND session_id = ?{session_pos}"
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params: Vec<Box<dyn rusqlite::types::ToSql>> = unique_refs
            .iter()
            .flat_map(|session_ref| {
                [
                    Box::new(session_ref.provider.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(session_ref.session_id.clone()) as Box<dyn rusqlite::types::ToSql>,
                ]
            })
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut result: std::collections::HashMap<String, SessionCodeStats> =
            std::collections::HashMap::new();

        for row in rows {
            let (provider, session_id, tool_name, full_input) =
                row.map_err(|e| format!("Row error: {e}"))?;
            let provider: IntegrationProvider = provider.parse()?;

            if let Some((added, removed, _)) = parse_code_change(&tool_name, &full_input) {
                let entry =
                    result
                        .entry(session_key(provider, &session_id))
                        .or_insert(SessionCodeStats {
                            lines_added: 0,
                            lines_removed: 0,
                            net_change: 0,
                        });
                entry.lines_added += added;
                entry.lines_removed += removed;
                entry.net_change = entry.lines_added - entry.lines_removed;
            }
        }

        Ok(result)
    }

    pub fn delete_response_times_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM response_times WHERE provider = ?1 AND session_id = ?2",
            params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Failed to delete response_times for session: {e}"))?;
        Ok(())
    }

    pub fn ingest_response_times(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        messages: &[(&str, &str)],
    ) -> Result<(), String> {
        if messages.is_empty() {
            return Ok(());
        }

        // Sort by timestamp
        let mut sorted: Vec<(&str, &str)> = messages.to_vec();
        sorted.sort_by(|a, b| a.1.cmp(b.1));

        // Query last assistant timestamp for this session (for cross-batch continuity)
        let last_assistant_ts: Option<String> = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT timestamp FROM response_times
                 WHERE provider = ?1 AND session_id = ?2 AND response_secs IS NOT NULL
                 ORDER BY timestamp DESC LIMIT 1",
                params![provider.as_str(), session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query last assistant ts: {e}"))?
        };

        // Build (user_ts, Option<prev_assistant_ts>) pairs
        // We walk the sorted messages and track state
        struct Turn {
            user_ts: String,
            assistant_ts: String,
            prev_assistant_ts: Option<String>,
        }

        let mut turns: Vec<Turn> = Vec::new();
        let mut prev_assistant: Option<String> = last_assistant_ts;
        let mut pending_user: Option<String> = None;

        for (role, timestamp) in &sorted {
            match *role {
                "user" => {
                    pending_user = Some((*timestamp).to_string());
                }
                "assistant" => {
                    if let Some(user_ts) = pending_user.take() {
                        turns.push(Turn {
                            user_ts,
                            assistant_ts: (*timestamp).to_string(),
                            prev_assistant_ts: prev_assistant.clone(),
                        });
                    }
                    prev_assistant = Some((*timestamp).to_string());
                }
                _ => {}
            }
        }

        if turns.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO response_times (provider, session_id, timestamp, response_secs, idle_secs)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            for turn in &turns {
                let response_secs = parse_ts_diff(&turn.assistant_ts, &turn.user_ts);
                let idle_secs = turn
                    .prev_assistant_ts
                    .as_deref()
                    .and_then(|prev| parse_ts_diff(&turn.user_ts, prev));

                let response_val = response_secs.filter(|&s| s > 0.0 && s <= 600.0);
                let idle_val = idle_secs.filter(|&s| s > 0.0 && s <= 600.0);

                if response_val.is_none() && idle_val.is_none() {
                    continue;
                }

                stmt.execute(params![
                    provider.as_str(),
                    session_id,
                    turn.assistant_ts,
                    response_val,
                    idle_val
                ])
                .map_err(|e| format!("Insert error: {e}"))?;
            }
        }

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;
        Ok(())
    }

    pub fn get_llm_runtime_stats(&self, range: &str) -> Result<LlmRuntimeStats, String> {
        let conn = self.conn.lock();
        let now = Utc::now();
        let from = now - range_to_duration(range);
        let from_str = from.to_rfc3339();

        let range_secs = range_to_duration(range).num_seconds() as f64;
        let bucket_secs = range_secs / 7.0;
        let from_epoch = from.timestamp_millis() as f64;

        // Logical turn grouping: consecutive rows in the same session where
        // idle_secs is present (gap <= 600s) belong to the same working cycle
        // (tool execution between LLM calls). A new logical turn starts when
        // idle_secs is NULL (first turn or gap > 600s) or the session changes.
        // Each logical turn's duration = last assistant_ts - first user_ts,
        // capturing full working time including tool execution.
        let mut total_runtime_secs: f64 = 0.0;
        let mut turn_count: i64 = 0;
        let mut sessions: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut bucket_sums: [f64; 7] = [0.0; 7];

        // Current logical turn state
        let mut cur_session: Option<String> = None;
        let mut turn_start_ms: f64 = 0.0; // first user_ts in logical turn
        let mut turn_end_ms: f64 = 0.0; // last assistant_ts in logical turn

        let flush_turn = |start_ms: f64,
                          end_ms: f64,
                          total: &mut f64,
                          count: &mut i64,
                          buckets: &mut [f64; 7],
                          from_ep: f64,
                          bkt_secs: f64| {
            let dur = (end_ms - start_ms) / 1000.0;
            if dur > 0.0 {
                *total += dur;
                *count += 1;
                let offset_ms = start_ms - from_ep;
                let bucket = ((offset_ms / 1000.0) / bkt_secs) as usize;
                buckets[bucket.min(6)] += dur;
            }
        };

        {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT timestamp, response_secs, idle_secs, provider, session_id
                     FROM response_times
                     WHERE timestamp >= ?1 AND response_secs IS NOT NULL
                     ORDER BY provider, session_id, timestamp",
                )
                .map_err(|e| format!("Prepare runtime query: {e}"))?;

            let rows = stmt
                .query_map(params![from_str], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(|e| format!("Runtime query error: {e}"))?;

            for row in rows.flatten() {
                let (ts_str, response_secs, idle_secs, provider, session_id) = row;
                let ts = match DateTime::parse_from_rfc3339(&ts_str) {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                let assistant_ms = ts.timestamp_millis() as f64;
                let user_ms = assistant_ms - (response_secs * 1000.0);
                let session_key = format!("{provider}:{session_id}");

                sessions.insert(session_key.clone());

                // New logical turn if: different session, or idle_secs is NULL
                // (first turn / gap > 600s)
                let is_new = cur_session.as_ref() != Some(&session_key) || idle_secs.is_none();

                if is_new {
                    // Flush previous logical turn
                    if cur_session.is_some() {
                        flush_turn(
                            turn_start_ms,
                            turn_end_ms,
                            &mut total_runtime_secs,
                            &mut turn_count,
                            &mut bucket_sums,
                            from_epoch,
                            bucket_secs,
                        );
                    }
                    cur_session = Some(session_key);
                    turn_start_ms = user_ms;
                }

                turn_end_ms = assistant_ms;
            }

            // Flush last logical turn
            if cur_session.is_some() {
                flush_turn(
                    turn_start_ms,
                    turn_end_ms,
                    &mut total_runtime_secs,
                    &mut turn_count,
                    &mut bucket_sums,
                    from_epoch,
                    bucket_secs,
                );
            }
        }

        let avg_per_turn_secs = if turn_count > 0 {
            total_runtime_secs / turn_count as f64
        } else {
            0.0
        };

        Ok(LlmRuntimeStats {
            total_runtime_secs,
            turn_count,
            session_count: sessions.len() as i64,
            avg_per_turn_secs,
            sparkline: bucket_sums.to_vec(),
        })
    }
}

/// Parse the difference in seconds between two ISO 8601 timestamps (end - start).
/// Returns None if either timestamp fails to parse.
fn parse_ts_diff(end_ts: &str, start_ts: &str) -> Option<f64> {
    let end = DateTime::parse_from_rfc3339(end_ts).ok()?;
    let start = DateTime::parse_from_rfc3339(start_ts).ok()?;
    let diff = (end - start).num_milliseconds() as f64 / 1000.0;
    if diff < 0.0 { None } else { Some(diff) }
}

/// Merge project breakdown entries where one cwd is a subdirectory of another.
/// For example, `/home/user/work/foo` and `/home/user/work/foo/bar` are merged
/// into a single entry under `/home/user/work/foo`.
fn merge_project_subdirs(mut rows: Vec<ProjectBreakdown>) -> Vec<ProjectBreakdown> {
    if rows.len() <= 1 {
        return rows;
    }

    // Sort by path so parents come before children
    rows.sort_by(|a, b| a.project.cmp(&b.project));

    // Collect all unique project paths (across all hostnames)
    let paths: Vec<String> = {
        let mut p: Vec<String> = rows.iter().map(|r| r.project.clone()).collect();
        p.sort();
        p.dedup();
        p
    };

    // Home directories are too generic to act as merge parents.
    // A session run from ~ should stay its own row, not absorb every project.
    let home_dir = dirs::home_dir().map(|h| h.to_string_lossy().to_string());

    // Build a mapping: child path → parent root
    let mut parent_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for path in &paths {
        // Check if any other path is a proper prefix of this one
        let mut best_parent: Option<&str> = None;
        for candidate in &paths {
            if candidate == path {
                continue;
            }
            // Skip home directory — it's not a real project root
            if home_dir.as_deref() == Some(candidate.as_str()) {
                continue;
            }
            // candidate must be a proper prefix with a '/' boundary
            if path.starts_with(candidate.as_str())
                && path.as_bytes().get(candidate.len()) == Some(&b'/')
            {
                // Pick the longest (most specific) parent so subdirs merge
                // into their closest project root, not a distant ancestor.
                match best_parent {
                    Some(bp) if candidate.len() > bp.len() => {
                        best_parent = Some(candidate);
                    }
                    None => {
                        best_parent = Some(candidate);
                    }
                    _ => {}
                }
            }
        }
        if let Some(parent) = best_parent {
            parent_map.insert(path.clone(), parent.to_string());
        }
    }

    if parent_map.is_empty() {
        // No merging needed — sort by total_tokens desc and return
        rows.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));
        rows.truncate(50);
        return rows;
    }

    // Merge: group by (resolved_project, hostname) and aggregate
    let mut merged: std::collections::HashMap<(String, String), ProjectBreakdown> =
        std::collections::HashMap::new();
    for row in rows {
        let resolved = parent_map
            .get(&row.project)
            .cloned()
            .unwrap_or(row.project.clone());
        let key = (resolved.clone(), row.hostname.clone());
        let entry = merged.entry(key).or_insert_with(|| ProjectBreakdown {
            project: resolved,
            hostname: row.hostname.clone(),
            total_tokens: 0,
            turn_count: 0,
            session_count: 0,
            last_active: String::new(),
        });
        entry.total_tokens += row.total_tokens;
        entry.turn_count += row.turn_count;
        entry.session_count += row.session_count;
        if row.last_active > entry.last_active {
            entry.last_active = row.last_active;
        }
    }

    let mut results: Vec<ProjectBreakdown> = merged.into_values().collect();
    results.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));
    results.truncate(50);
    results
}

fn calc_trend(
    conn: &Connection,
    provider: IntegrationProvider,
    bucket_key: &str,
) -> Result<String, String> {
    let now = Utc::now();
    let one_hour_ago = (now - TimeDelta::hours(1)).to_rfc3339();
    let two_hours_ago = (now - TimeDelta::hours(2)).to_rfc3339();

    let recent_avg: Option<f64> = conn
        .query_row(
            "SELECT AVG(utilization) FROM usage_snapshots
             WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3",
            params![provider.as_str(), bucket_key, one_hour_ago],
            |row| row.get(0),
        )
        .map_err(|e| format!("Trend query error: {e}"))?;

    let prev_avg: Option<f64> = conn
        .query_row(
            "SELECT AVG(utilization) FROM usage_snapshots
             WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3 AND timestamp < ?4",
            params![provider.as_str(), bucket_key, two_hours_ago, one_hour_ago],
            |row| row.get(0),
        )
        .map_err(|e| format!("Trend query error: {e}"))?;

    match (recent_avg, prev_avg) {
        (Some(r), Some(p)) if r > p + 2.0 => Ok("up".into()),
        (Some(r), Some(p)) if r < p - 2.0 => Ok("down".into()),
        (Some(_), Some(_)) => Ok("flat".into()),
        _ => Ok("unknown".into()),
    }
}

fn downsample_tokens(points: Vec<TokenDataPoint>, max: usize) -> Vec<TokenDataPoint> {
    if points.len() <= max {
        return points;
    }

    let chunk_size = points.len().div_ceil(max);
    points
        .chunks(chunk_size)
        .map(|chunk| {
            let inp: i64 = chunk.iter().map(|p| p.input_tokens).sum();
            let out: i64 = chunk.iter().map(|p| p.output_tokens).sum();
            let cc: i64 = chunk.iter().map(|p| p.cache_creation_input_tokens).sum();
            let cr: i64 = chunk.iter().map(|p| p.cache_read_input_tokens).sum();
            TokenDataPoint {
                timestamp: chunk[chunk.len() / 2].timestamp.clone(),
                input_tokens: inp,
                output_tokens: out,
                cache_creation_input_tokens: cc,
                cache_read_input_tokens: cr,
                total_tokens: inp + out + cc + cr,
            }
        })
        .collect()
}

fn downsample(points: Vec<DataPoint>, max: usize) -> Vec<DataPoint> {
    if points.len() <= max {
        return points;
    }

    let chunk_size = points.len().div_ceil(max);
    points
        .chunks(chunk_size)
        .map(|chunk| {
            let avg_util = chunk.iter().map(|p| p.utilization).sum::<f64>() / chunk.len() as f64;
            DataPoint {
                timestamp: chunk[chunk.len() / 2].timestamp.clone(),
                utilization: avg_util,
            }
        })
        .collect()
}
