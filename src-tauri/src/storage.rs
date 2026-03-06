use std::path::PathBuf;

use chrono::{TimeDelta, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, params};

use crate::models::{
    BucketStats, DataPoint, HostBreakdown, LearnedRule, LearnedRulePayload, LearningRun,
    LearningRunPayload, LearningStatus, ObservationPayload, ProjectBreakdown, SessionBreakdown,
    TokenDataPoint, TokenReportPayload, TokenStats, ToolCount, UsageBucket,
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
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .ok_or("Cannot determine data directory")?;
    let app_dir = data_dir.join("com.claude.usage-widget");
    std::fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app data dir: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o700));
    }

    Ok(app_dir.join("usage.db"))
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
                 LIMIT 50",
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

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
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

    pub fn get_learning_runs(&self, limit: i64) -> Result<Vec<LearningRun>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, created_at
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
        conn.execute(
            "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, last_evidence_at, state, project)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1.0, ?7, 'emerging', ?8)
             ON CONFLICT(name) DO UPDATE SET
                 domain = excluded.domain,
                 alpha = learned_rules.alpha + excluded.alpha,
                 observation_count = learned_rules.observation_count + excluded.observation_count,
                 file_path = excluded.file_path,
                 last_evidence_at = excluded.last_evidence_at,
                 updated_at = datetime('now')",
            params![
                payload.name,
                payload.domain,
                payload.confidence,
                payload.observation_count,
                payload.file_path,
                payload.confidence,
                now,
                payload.project,
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
                    "SELECT name, domain, alpha, beta_param, observation_count, last_evidence_at, state, project, created_at, updated_at
                     FROM learned_rules",
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
                ) = row.map_err(|e| format!("Row error: {e}"))?;
                map.insert(
                    name,
                    (
                        domain, alpha, beta, obs_count, last_ev, state, project, created, updated,
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
            });
        }

        rules.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(rules)
    }

    pub fn delete_learned_rule(&self, name: &str) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }

        // Get file_path from DB before deleting the record
        let file_path: Option<String> = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT file_path FROM learned_rules WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .ok()
        };

        // Delete from table
        let conn = self.conn.lock();
        conn.execute("DELETE FROM learned_rules WHERE name = ?1", params![name])
            .map_err(|e| format!("Delete rule error: {e}"))?;
        drop(conn);

        // Delete file using stored path, or fall back to searching subdirectories
        if let Some(fp) = file_path {
            let path = std::path::Path::new(&fp);
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| format!("Delete file error: {e}"))?;
            }
        } else {
            // Fallback: search recursively
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
        }
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
        conn.execute(
            "DELETE FROM observations WHERE created_at < (
                SELECT COALESCE(MAX(created_at), datetime('now', '-30 days'))
                FROM learning_runs WHERE status = 'completed'
            ) AND created_at < datetime('now', '-7 days')",
            [],
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
