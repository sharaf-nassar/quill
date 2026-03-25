use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::models::{
    BucketStats, CodeStats, CodeStatsHistoryPoint, DataPoint, GitSnapshot, HostBreakdown,
    LanguageBreakdown, LearnedRule, LearnedRulePayload, LearningRun, LearningRunPayload,
    LearningStatus, ObservationPayload, ProjectBreakdown, ProjectTokens, SessionBreakdown,
    SessionCodeStats, SessionStats, TokenDataPoint, TokenReportPayload, TokenStats, ToolCount,
    UsageBucket,
};

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

fn range_to_duration(range: &str) -> TimeDelta {
    match range {
        "1h" => TimeDelta::hours(1),
        "24h" => TimeDelta::hours(24),
        "7d" => TimeDelta::days(7),
        "30d" => TimeDelta::days(30),
        _ => TimeDelta::hours(24),
    }
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
            "CREATE TABLE IF NOT EXISTS usage_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                bucket_label TEXT NOT NULL,
                utilization REAL NOT NULL,
                resets_at TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON usage_snapshots(timestamp);
            CREATE INDEX IF NOT EXISTS idx_snapshots_bucket ON usage_snapshots(bucket_label);
            CREATE INDEX IF NOT EXISTS idx_snapshots_ts_bucket ON usage_snapshots(timestamp, bucket_label);

            CREATE TABLE IF NOT EXISTS usage_hourly (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hour TEXT NOT NULL,
                bucket_label TEXT NOT NULL,
                avg_utilization REAL NOT NULL,
                max_utilization REAL NOT NULL,
                min_utilization REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                UNIQUE(hour, bucket_label)
            );
            CREATE INDEX IF NOT EXISTS idx_hourly_hour ON usage_hourly(hour);
            CREATE INDEX IF NOT EXISTS idx_hourly_bucket ON usage_hourly(bucket_label);

            CREATE TABLE IF NOT EXISTS token_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
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
                hostname TEXT NOT NULL DEFAULT 'local',
                total_input INTEGER NOT NULL,
                total_output INTEGER NOT NULL,
                total_cache_creation INTEGER NOT NULL DEFAULT 0,
                total_cache_read INTEGER NOT NULL DEFAULT 0,
                turn_count INTEGER NOT NULL,
                UNIQUE(hour, hostname)
            );
            CREATE INDEX IF NOT EXISTS idx_token_hourly_hour ON token_hourly(hour);

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Learning system tables
            CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
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
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS learned_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                domain TEXT,
                confidence REAL NOT NULL DEFAULT 0.5,
                observation_count INTEGER NOT NULL DEFAULT 0,
                file_path TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );",
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
                    project TEXT,
                    tool_counts TEXT NOT NULL,
                    error_count INTEGER NOT NULL DEFAULT 0,
                    total_observations INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT DEFAULT (datetime('now')),
                    UNIQUE(period, project)
                );
                CREATE INDEX IF NOT EXISTS idx_obs_summaries_period ON observation_summaries(period);",
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
                    "INSERT INTO usage_snapshots (timestamp, bucket_label, utilization, resets_at) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            for bucket in buckets {
                stmt.execute(params![
                    now,
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
            "INSERT INTO usage_hourly (hour, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count)
             SELECT
                 strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour,
                 bucket_label,
                 AVG(utilization),
                 MAX(utilization),
                 MIN(utilization),
                 COUNT(*)
             FROM usage_snapshots
             WHERE timestamp < ?1
             GROUP BY hour, bucket_label
             ON CONFLICT(hour, bucket_label) DO UPDATE SET
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

    pub fn get_usage_history(&self, bucket: &str, range: &str) -> Result<Vec<DataPoint>, String> {
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
                     WHERE bucket_label = ?1 AND hour >= ?2
                     ORDER BY hour ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let hourly_rows = stmt
                .query_map(params![bucket, from_hour], |row| {
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
                     WHERE bucket_label = ?1 AND timestamp >= ?2
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let snap_rows = stmt2
                .query_map(params![bucket, from_str], |row| {
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
                     WHERE bucket_label = ?1 AND timestamp >= ?2
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let rows = stmt
                .query_map(params![bucket, from_str], |row| {
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

    pub fn get_usage_stats(&self, bucket: &str, days: i32) -> Result<BucketStats, String> {
        let conn = self.conn.lock();
        Self::get_usage_stats_with_conn(&conn, bucket, days)
    }

    fn get_usage_stats_with_conn(
        conn: &Connection,
        bucket: &str,
        days: i32,
    ) -> Result<BucketStats, String> {
        let days = days.clamp(1, 365);
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     AVG(utilization),
                     MAX(utilization),
                     MIN(utilization),
                     COUNT(*),
                     (SELECT COUNT(*) FROM usage_snapshots
                      WHERE bucket_label = ?1 AND timestamp >= ?2 AND utilization >= 80.0)
                 FROM usage_snapshots
                 WHERE bucket_label = ?1 AND timestamp >= ?2",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let stats = stmt
            .query_row(params![bucket, from], |row| {
                let total: i64 = row.get(3)?;
                let above_80: i64 = row.get(4)?;
                let pct_above_80 = if total > 0 {
                    (above_80 as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                Ok(BucketStats {
                    label: bucket.to_string(),
                    current: 0.0, // filled in by caller
                    avg: row.get::<_, Option<f64>>(0)?.unwrap_or(0.0),
                    max: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    min: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    time_above_80: pct_above_80,
                    trend: String::new(), // filled in below
                    sample_count: total,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let trend = calc_trend(conn, bucket)?;

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
            let mut stats = Self::get_usage_stats_with_conn(&conn, &bucket.label, days)?;
            stats.current = bucket.utilization;
            results.push(stats);
        }
        Ok(results)
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
            "INSERT INTO token_snapshots (session_id, hostname, timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
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
                if let Some(host) = hostname {
                    (
                        "SELECT hour, total_input, total_output, total_cache_creation, total_cache_read
                         FROM token_hourly
                         WHERE hour >= ?1 AND hostname = ?2
                         ORDER BY hour ASC".to_string(),
                        vec![Box::new(from_hour.clone()), Box::new(host.to_string())],
                    )
                } else {
                    (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![Box::new(from_hour.clone())],
                    )
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
        let (snap_sql, snap_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(
            sid,
        ) =
            session_id
        {
            (
                    "SELECT timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
                     FROM token_snapshots
                     WHERE timestamp >= ?1 AND session_id = ?2
                     ORDER BY timestamp ASC".to_string(),
                    vec![Box::new(from_str.clone()), Box::new(sid.to_string())],
                )
        } else if let Some(project) = cwd {
            (
                    "SELECT timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
                     FROM token_snapshots
                     WHERE timestamp >= ?1 AND cwd = ?2
                     ORDER BY timestamp ASC".to_string(),
                    vec![Box::new(from_str.clone()), Box::new(project.to_string())],
                )
        } else if let Some(host) = hostname {
            (
                    "SELECT timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
                     FROM token_snapshots
                     WHERE timestamp >= ?1 AND hostname = ?2
                     ORDER BY timestamp ASC".to_string(),
                    vec![Box::new(from_str.clone()), Box::new(host.to_string())],
                )
        } else {
            (
                    "SELECT timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
                     FROM token_snapshots
                     WHERE timestamp >= ?1
                     ORDER BY timestamp ASC".to_string(),
                    vec![Box::new(from_str.clone())],
                )
        };

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
        hostname: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<TokenStats, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(project) = cwd {
                (
                    "SELECT
                         COALESCE(SUM(input_tokens), 0),
                         COALESCE(SUM(output_tokens), 0),
                         COALESCE(SUM(cache_creation_input_tokens), 0),
                         COALESCE(SUM(cache_read_input_tokens), 0),
                         COUNT(*)
                     FROM token_snapshots
                     WHERE timestamp >= ?1 AND cwd = ?2"
                        .to_string(),
                    vec![Box::new(from), Box::new(project.to_string())],
                )
            } else if let Some(host) = hostname {
                (
                    "SELECT
                         COALESCE(SUM(input_tokens), 0),
                         COALESCE(SUM(output_tokens), 0),
                         COALESCE(SUM(cache_creation_input_tokens), 0),
                         COALESCE(SUM(cache_read_input_tokens), 0),
                         COUNT(*)
                     FROM token_snapshots
                     WHERE timestamp >= ?1 AND hostname = ?2"
                        .to_string(),
                    vec![Box::new(from), Box::new(host.to_string())],
                )
            } else {
                (
                    "SELECT
                         COALESCE(SUM(input_tokens), 0),
                         COALESCE(SUM(output_tokens), 0),
                         COALESCE(SUM(cache_creation_input_tokens), 0),
                         COALESCE(SUM(cache_read_input_tokens), 0),
                         COUNT(*)
                     FROM token_snapshots
                     WHERE timestamp >= ?1"
                        .to_string(),
                    vec![Box::new(from)],
                )
            };

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
                     COUNT(DISTINCT session_id) as session_count,
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
    ) -> Result<Vec<SessionBreakdown>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(host) =
            hostname
        {
            (
                    "SELECT
                         s.session_id,
                         s.hostname,
                         SUM(s.input_tokens + s.output_tokens + s.cache_creation_input_tokens + s.cache_read_input_tokens) as total_tokens,
                         COUNT(*) as turn_count,
                         MIN(s.timestamp) as first_seen,
                         MAX(s.timestamp) as last_active,
                         (SELECT t.cwd FROM token_snapshots t
                          WHERE t.session_id = s.session_id AND t.cwd IS NOT NULL
                          ORDER BY t.timestamp DESC LIMIT 1) as project
                     FROM token_snapshots s
                     WHERE s.timestamp >= ?1 AND s.hostname = ?2
                     GROUP BY s.session_id
                     ORDER BY last_active DESC
                     LIMIT 10".to_string(),
                    vec![Box::new(from), Box::new(host.to_string())],
                )
        } else {
            (
                    "SELECT
                         s.session_id,
                         s.hostname,
                         SUM(s.input_tokens + s.output_tokens + s.cache_creation_input_tokens + s.cache_read_input_tokens) as total_tokens,
                         COUNT(*) as turn_count,
                         MIN(s.timestamp) as first_seen,
                         MAX(s.timestamp) as last_active,
                         (SELECT t.cwd FROM token_snapshots t
                          WHERE t.session_id = s.session_id AND t.cwd IS NOT NULL
                          ORDER BY t.timestamp DESC LIMIT 1) as project
                     FROM token_snapshots s
                     WHERE s.timestamp >= ?1
                     GROUP BY s.session_id
                     ORDER BY last_active DESC
                     LIMIT 10".to_string(),
                    vec![Box::new(from)],
                )
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(SessionBreakdown {
                    session_id: row.get(0)?,
                    hostname: row.get(1)?,
                    total_tokens: row.get(2)?,
                    turn_count: row.get(3)?,
                    first_seen: row.get(4)?,
                    last_active: row.get(5)?,
                    project: row.get(6)?,
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
                        s.session_id,
                        (SELECT t.cwd FROM token_snapshots t
                         WHERE t.session_id = s.session_id AND t.cwd IS NOT NULL
                         ORDER BY t.timestamp DESC LIMIT 1) as project,
                        SUM(s.input_tokens + s.output_tokens + s.cache_creation_input_tokens + s.cache_read_input_tokens) as total_tokens
                    FROM token_snapshots s
                    WHERE s.timestamp >= ?1
                    GROUP BY s.session_id
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
                        session_id,
                        (strftime('%s', MAX(timestamp)) - strftime('%s', MIN(timestamp))) as duration_seconds,
                        SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens
                    FROM token_snapshots
                    WHERE timestamp >= ?1
                    GROUP BY session_id
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

    pub fn delete_session_data(&self, session_id: &str) -> Result<u64, String> {
        let conn = self.conn.lock();

        let count = conn
            .execute(
                "DELETE FROM token_snapshots WHERE session_id = ?1",
                params![session_id],
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

    // --- Learning system methods ---

    pub fn store_observation(&self, payload: &ObservationPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO observations (session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
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

    pub fn get_recent_observations(&self, limit: i64) -> Result<Vec<serde_json::Value>, String> {
        self.get_observations_since(None, limit)
    }

    pub fn get_unanalyzed_observations(
        &self,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.conn.lock();
        let since: String = conn
            .query_row(
                "SELECT COALESCE(MAX(created_at), '1970-01-01') FROM learning_runs WHERE status = 'completed'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query last run error: {e}"))?;
        drop(conn);
        self.get_observations_since(Some(&since), limit)
    }

    fn get_observations_since(
        &self,
        since: Option<&str>,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.conn.lock();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match since {
            Some(s) => (
                "SELECT id, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE created_at > ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
                vec![Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>, Box::new(limit)],
            ),
            None => (
                "SELECT id, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
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
                    "session_id": row.get::<_, String>(1)?,
                    "timestamp": row.get::<_, String>(2)?,
                    "hook_phase": row.get::<_, String>(3)?,
                    "tool_name": row.get::<_, String>(4)?,
                    "tool_input": row.get::<_, Option<String>>(5)?,
                    "tool_output": row.get::<_, Option<String>>(6)?,
                    "cwd": row.get::<_, Option<String>>(7)?,
                    "created_at": row.get::<_, String>(8)?,
                }))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_observation_count(&self) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
            .map_err(|e| format!("Count error: {e}"))
    }

    pub fn get_unanalyzed_observation_count(&self) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM observations WHERE created_at > (
                SELECT COALESCE(MAX(created_at), '1970-01-01') FROM learning_runs WHERE status = 'completed'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Count error: {e}"))
    }

    pub fn get_top_tools(&self, limit: i64, days: i64) -> Result<Vec<ToolCount>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT tool_name, COUNT(*) as count FROM observations
                 WHERE created_at >= datetime('now', '-' || ?1 || ' days')
                 GROUP BY tool_name ORDER BY count DESC LIMIT ?2",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![days, limit], |row| {
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

    pub fn get_observation_sparkline(&self) -> Result<Vec<i64>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT DATE(created_at) as day, COUNT(*) as count
                 FROM observations
                 WHERE created_at >= DATE('now', '-6 days')
                 GROUP BY day ORDER BY day ASC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
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
            "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                payload.trigger_mode,
                payload.observations_analyzed,
                payload.rules_created,
                payload.rules_updated,
                payload.duration_ms,
                payload.status,
                payload.error,
                payload.logs,
            ],
        )
        .map_err(|e| format!("Insert learning run error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn create_learning_run(&self, trigger_mode: &str) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO learning_runs (trigger_mode, status, observations_analyzed, rules_created, rules_updated)
             VALUES (?1, 'running', 0, 0, 0)",
            params![trigger_mode],
        )
        .map_err(|e| format!("Create learning run error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_learning_run(&self, id: i64, payload: &LearningRunPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE learning_runs SET observations_analyzed=?2, rules_created=?3, rules_updated=?4,
             duration_ms=?5, status=?6, error=?7, logs=?8, phases=?9
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

    pub fn get_learning_runs(&self, limit: i64) -> Result<Vec<LearningRun>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, created_at, phases
                 FROM learning_runs ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![limit], |row| {
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
        conn.execute(
            "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, last_evidence_at, state, project, is_anti_pattern, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'emerging', ?9, ?10, ?11)
             ON CONFLICT(name) DO UPDATE SET
                 domain = excluded.domain,
                 alpha = learned_rules.alpha + excluded.alpha,
                 beta_param = learned_rules.beta_param + excluded.beta_param,
                 observation_count = learned_rules.observation_count + excluded.observation_count,
                 file_path = CASE WHEN length(excluded.file_path) > 0 THEN excluded.file_path ELSE learned_rules.file_path END,
                 last_evidence_at = excluded.last_evidence_at,
                 is_anti_pattern = excluded.is_anti_pattern,
                 source = CASE WHEN excluded.source IS NOT NULL THEN excluded.source ELSE learned_rules.source END,
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

    pub fn get_learned_rules(&self) -> Result<Vec<LearnedRule>, String> {
        let mut meta_map = {
            let conn = self.conn.lock();
            let mut stmt = conn
                .prepare_cached(
                    "SELECT name, domain, alpha, beta_param, observation_count, last_evidence_at, state, project, created_at, updated_at, is_anti_pattern, source
                     FROM learned_rules
                     WHERE state != 'suppressed'",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
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
                ) = row.map_err(|e| format!("Row error: {e}"))?;
                map.insert(
                    name,
                    (
                        domain, alpha, beta, obs_count, last_ev, state, project, created, updated,
                        is_anti, source,
                    ),
                );
            }
            map
        };

        let rules_dir = dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".claude")
            .join("rules")
            .join("learned");

        let mut rules = Vec::new();

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

        if rules_dir.exists() {
            let mut files = Vec::new();
            collect_rule_files(&rules_dir, &mut files);

            for (name, file_path) in files {
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
        let file_path: Option<String> = {
            let conn = self.conn.lock();
            let fp: Option<String> = conn
                .query_row(
                    "SELECT file_path FROM learned_rules WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .ok();

            conn.execute(
                "UPDATE learned_rules SET beta_param = beta_param + 5.0, file_path = '', state = 'suppressed', updated_at = datetime('now') WHERE name = ?1",
                params![name],
            )
            .ok();
            fp
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
        let rules_dir = dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".claude")
            .join("rules")
            .join("learned");
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
        find_and_delete(&rules_dir, name);
        Ok(())
    }

    pub fn get_learning_status(&self) -> Result<LearningStatus, String> {
        let observation_count = self.get_observation_count()?;
        let unanalyzed_count = self.get_unanalyzed_observation_count()?;
        let rules = self.get_learned_rules()?;
        let runs = self.get_learning_runs(1)?;

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
                "SELECT cwd, tool_name, COUNT(*) as cnt,
                        SUM(CASE WHEN tool_output LIKE '%error%' OR tool_output LIKE '%Error%' THEN 1 ELSE 0 END) as err_cnt
                 FROM observations
                 WHERE created_at < ?1
                 GROUP BY cwd, tool_name",
            )
            .map_err(|e| format!("Summary prepare error: {e}"))?;

        let summary_rows = summary_stmt
            .query_map(params![cutoff_ts], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| format!("Summary query error: {e}"))?;

        // Group by project
        let mut project_summaries: std::collections::HashMap<
            Option<String>,
            (serde_json::Map<String, serde_json::Value>, i64, i64),
        > = std::collections::HashMap::new();
        for row in summary_rows {
            let (project, tool, count, errors) = row.map_err(|e| format!("Summary row: {e}"))?;
            let entry = project_summaries
                .entry(project)
                .or_insert_with(|| (serde_json::Map::new(), 0, 0));
            entry.0.insert(tool, serde_json::Value::from(count));
            entry.1 += errors;
            entry.2 += count;
        }
        drop(summary_stmt);

        let period = Utc::now().format("%Y-%m-%d").to_string();
        for (project, (tool_counts, error_count, total)) in &project_summaries {
            if *total == 0 {
                continue;
            }
            let tc_json = serde_json::Value::Object(tool_counts.clone()).to_string();
            conn.execute(
                "INSERT OR REPLACE INTO observation_summaries (period, project, tool_counts, error_count, total_observations)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![period, project, tc_json, error_count, total],
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
            "INSERT INTO token_hourly (hour, hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count)
             SELECT
                 strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour,
                 hostname,
                 SUM(input_tokens),
                 SUM(output_tokens),
                 SUM(cache_creation_input_tokens),
                 SUM(cache_read_input_tokens),
                 COUNT(*)
             FROM token_snapshots
             WHERE timestamp < ?1
             GROUP BY hour, hostname
             ON CONFLICT(hour, hostname) DO UPDATE SET
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
    pub fn delete_tool_actions_for_session(&self, session_id: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM tool_actions WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .map_err(|e| format!("Delete tool_actions for session: {e}"))?;
        Ok(())
    }

    pub fn store_tool_actions(
        &self,
        actions: &[crate::sessions::ToolAction],
        message_id: &str,
        session_id: &str,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin tool_actions transaction: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO tool_actions (message_id, session_id, tool_name, category, file_path, summary, full_input, full_output, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|e| format!("Prepare store_tool_actions: {e}"))?;

            for action in actions {
                stmt.execute(rusqlite::params![
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
        }

        tx.commit()
            .map_err(|e| format!("Commit tool_actions: {e}"))?;
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
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        // Atomic: insert only if no running run exists for this project
        conn.execute(
            "INSERT INTO optimization_runs (project_path, trigger, memories_scanned, suggestions_created, context_sources, status, started_at)
             SELECT ?1, ?2, 0, 0, '{}', 'running', ?3
             WHERE NOT EXISTS (
                 SELECT 1 FROM optimization_runs WHERE project_path = ?1 AND status = 'running'
             )",
            rusqlite::params![project_path, trigger, now],
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
        limit: i64,
    ) -> Result<Vec<crate::models::OptimizationRun>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, project_path, trigger, memories_scanned, suggestions_created,
                        status, error, started_at, completed_at
                 FROM optimization_runs WHERE project_path = ?1
                 ORDER BY started_at DESC LIMIT ?2",
            )
            .map_err(|e| format!("Failed to prepare optimization runs query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![project_path, limit], |row| {
                Ok(crate::models::OptimizationRun {
                    id: row.get(0)?,
                    project_path: row.get(1)?,
                    trigger: row.get(2)?,
                    memories_scanned: row.get(3)?,
                    suggestions_created: row.get(4)?,
                    status: row.get(5)?,
                    error: row.get(6)?,
                    started_at: row.get(7)?,
                    completed_at: row.get(8)?,
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
        conn.execute(
            "INSERT INTO optimization_suggestions
             (run_id, project_path, action_type, target_file, reasoning, proposed_content, merge_sources, status, created_at, original_content, diff_summary, backup_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9, ?10, ?11)",
            rusqlite::params![run_id, project_path, action_type, target_file, reasoning, proposed_content, merge_sources, now, original_content, diff_summary, backup_data],
        )
        .map_err(|e| format!("Failed to store optimization suggestion: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    /// Get optimization suggestions for a project with pagination, optionally filtered by status.
    #[allow(dead_code)]
    pub fn get_optimization_suggestions(
        &self,
        project_path: &str,
        status_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();

        let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(status) = status_filter {
                (
                    "SELECT id, run_id, project_path, action_type, target_file, reasoning,
                            proposed_content, merge_sources, status, error, resolved_at, created_at,
                            original_content, diff_summary, backup_data, group_id
                     FROM optimization_suggestions WHERE project_path = ?1 AND status = ?2
                     ORDER BY created_at DESC LIMIT ?3 OFFSET ?4"
                        .to_string(),
                    vec![
                        Box::new(project_path.to_string()),
                        Box::new(status.to_string()),
                        Box::new(limit),
                        Box::new(offset),
                    ],
                )
            } else {
                (
                    "SELECT id, run_id, project_path, action_type, target_file, reasoning,
                            proposed_content, merge_sources, status, error, resolved_at, created_at,
                            original_content, diff_summary, backup_data, group_id
                     FROM optimization_suggestions WHERE project_path = ?1
                     ORDER BY created_at DESC LIMIT ?2 OFFSET ?3"
                        .to_string(),
                    vec![
                        Box::new(project_path.to_string()),
                        Box::new(limit),
                        Box::new(offset),
                    ],
                )
            };

        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare suggestions query: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let merge_sources_json: Option<String> = row.get(7)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    action_type: row.get(3)?,
                    target_file: row.get(4)?,
                    reasoning: row.get(5)?,
                    proposed_content: row.get(6)?,
                    merge_sources,
                    status: row.get(8)?,
                    error: row.get(9)?,
                    resolved_at: row.get(10)?,
                    created_at: row.get(11)?,
                    original_content: row.get(12)?,
                    diff_summary: row.get(13)?,
                    backup_data: row.get(14)?,
                    group_id: row.get(15)?,
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
        limit: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, run_id, project_path, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions
                 WHERE project_path = ?1 AND status = 'denied'
                 ORDER BY resolved_at DESC LIMIT ?2",
            )
            .map_err(|e| format!("Failed to prepare denied suggestions query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![project_path, limit], |row| {
                let merge_sources_json: Option<String> = row.get(7)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    action_type: row.get(3)?,
                    target_file: row.get(4)?,
                    reasoning: row.get(5)?,
                    proposed_content: row.get(6)?,
                    merge_sources,
                    status: row.get(8)?,
                    error: row.get(9)?,
                    resolved_at: row.get(10)?,
                    created_at: row.get(11)?,
                    original_content: row.get(12)?,
                    diff_summary: row.get(13)?,
                    backup_data: row.get(14)?,
                    group_id: row.get(15)?,
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
            "SELECT id, run_id, project_path, action_type, target_file, reasoning,
                    proposed_content, merge_sources, status, error, resolved_at, created_at,
                    original_content, diff_summary, backup_data, group_id
             FROM optimization_suggestions WHERE id = ?1",
            rusqlite::params![suggestion_id],
            |row| {
                let merge_sources_json: Option<String> = row.get(7)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    action_type: row.get(3)?,
                    target_file: row.get(4)?,
                    reasoning: row.get(5)?,
                    proposed_content: row.get(6)?,
                    merge_sources,
                    status: row.get(8)?,
                    error: row.get(9)?,
                    resolved_at: row.get(10)?,
                    created_at: row.get(11)?,
                    original_content: row.get(12)?,
                    diff_summary: row.get(13)?,
                    backup_data: row.get(14)?,
                    group_id: row.get(15)?,
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
                "SELECT id, run_id, project_path, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions WHERE run_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare suggestions-for-run query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![run_id], |row| {
                let merge_sources_json: Option<String> = row.get(7)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    action_type: row.get(3)?,
                    target_file: row.get(4)?,
                    reasoning: row.get(5)?,
                    proposed_content: row.get(6)?,
                    merge_sources,
                    status: row.get(8)?,
                    error: row.get(9)?,
                    resolved_at: row.get(10)?,
                    created_at: row.get(11)?,
                    original_content: row.get(12)?,
                    diff_summary: row.get(13)?,
                    backup_data: row.get(14)?,
                    group_id: row.get(15)?,
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
                "SELECT id, run_id, project_path, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions WHERE group_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare group suggestions query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![group_id], |row| {
                let merge_sources_json: Option<String> = row.get(7)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    action_type: row.get(3)?,
                    target_file: row.get(4)?,
                    reasoning: row.get(5)?,
                    proposed_content: row.get(6)?,
                    merge_sources,
                    status: row.get(8)?,
                    error: row.get(9)?,
                    resolved_at: row.get(10)?,
                    created_at: row.get(11)?,
                    original_content: row.get(12)?,
                    diff_summary: row.get(13)?,
                    backup_data: row.get(14)?,
                    group_id: row.get(15)?,
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
        session_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SessionCodeStats>, String> {
        if session_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self.conn.lock();

        let placeholders: Vec<String> = (1..=session_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT session_id, tool_name, full_input
			 FROM tool_actions
			 WHERE category = 'code_change'
			   AND full_input IS NOT NULL
			   AND session_id IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params: Vec<Box<dyn rusqlite::types::ToSql>> = session_ids
            .iter()
            .map(|s| Box::new(s.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut result: std::collections::HashMap<String, SessionCodeStats> =
            std::collections::HashMap::new();

        for row in rows {
            let (session_id, tool_name, full_input) = row.map_err(|e| format!("Row error: {e}"))?;

            if let Some((added, removed, _)) = parse_code_change(&tool_name, &full_input) {
                let entry = result.entry(session_id).or_insert(SessionCodeStats {
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

fn calc_trend(conn: &Connection, bucket: &str) -> Result<String, String> {
    let now = Utc::now();
    let one_hour_ago = (now - TimeDelta::hours(1)).to_rfc3339();
    let two_hours_ago = (now - TimeDelta::hours(2)).to_rfc3339();

    let recent_avg: Option<f64> = conn
        .query_row(
            "SELECT AVG(utilization) FROM usage_snapshots
             WHERE bucket_label = ?1 AND timestamp >= ?2",
            params![bucket, one_hour_ago],
            |row| row.get(0),
        )
        .map_err(|e| format!("Trend query error: {e}"))?;

    let prev_avg: Option<f64> = conn
        .query_row(
            "SELECT AVG(utilization) FROM usage_snapshots
             WHERE bucket_label = ?1 AND timestamp >= ?2 AND timestamp < ?3",
            params![bucket, two_hours_ago, one_hour_ago],
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
