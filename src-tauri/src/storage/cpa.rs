//! CPA connection and observation writes share one transaction boundary.
use super::Storage;
use crate::integrations::cpa::{BASE_URL_SETTING, MANAGEMENT_KEY_SETTING};
use crate::models::UsageBucket;
use rusqlite::{Connection, Transaction, params};

pub(crate) const STATE_SETTING: &str = "usage.cpa.state";

impl Storage {
    pub(crate) fn save_cpa_connection(&self, base_url: &str, key: &str) -> Result<(), String> {
        save_connection(&mut self.conn.lock().unwrap(), base_url, key)
            .map_err(|error| format!("Save CPA connection: {error}"))
    }

    pub(crate) fn clear_cpa_connection(&self) -> Result<(), String> {
        clear_connection(&mut self.conn.lock().unwrap())
            .map_err(|error| format!("Clear CPA connection: {error}"))
    }

    pub(crate) fn store_cpa_state(
        &self,
        state: &str,
        observations: &[(String, UsageBucket)],
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![STATE_SETTING, state],
        )
        .map_err(|error| format!("Write CPA state: {error}"))?;
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO usage_snapshots
                 (timestamp, provider, bucket_key, bucket_label, utilization,
                  resets_at, source, account_id, account_label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'cpa', ?7, ?8)",
                )
                .map_err(|error| error.to_string())?;
            for (observed_at, bucket) in observations {
                insert
                    .execute(params![
                        observed_at,
                        bucket.provider.as_str(),
                        bucket.key,
                        bucket.label,
                        bucket.utilization,
                        bucket.resets_at,
                        bucket.account_id,
                        bucket.account_label
                    ])
                    .map_err(|error| format!("Write CPA observation: {error}"))?;
            }
        }
        tx.commit()
            .map_err(|error| format!("Commit CPA state: {error}"))
    }
}

fn purge_runtime(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM settings WHERE key LIKE 'usage.cpa.%'", [])?;
    Ok(())
}

fn purge_history(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM usage_snapshots WHERE source = 'cpa'", [])?;
    tx.execute("DELETE FROM usage_hourly WHERE bucket_key LIKE 'cpa/%'", [])?;
    Ok(())
}

fn save_connection(conn: &mut Connection, base_url: &str, key: &str) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [BASE_URL_SETTING],
            |row| row.get(0),
        )
        .optional()?;
    if previous.as_deref() != Some(base_url) {
        purge_history(&tx)?;
    }
    // Reconnect is the explicit recovery boundary for rejected management keys.
    // Same-instance reconnect retains analytics history, never retry suppression.
    purge_runtime(&tx)?;
    for (setting, value) in [(BASE_URL_SETTING, base_url), (MANAGEMENT_KEY_SETTING, key)] {
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![setting, value],
        )?;
    }
    tx.commit()
}

fn clear_connection(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM settings WHERE key IN (?1, ?2)",
        params![BASE_URL_SETTING, MANAGEMENT_KEY_SETTING],
    )?;
    purge_runtime(&tx)?;
    purge_history(&tx)?;
    tx.commit()
}

use rusqlite::OptionalExtension;

#[cfg(test)]
pub(crate) fn test_storage() -> Storage {
    test_storage_at(std::path::Path::new(":memory:"))
}

#[cfg(test)]
pub(crate) fn test_storage_at(path: &std::path::Path) -> Storage {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE IF NOT EXISTS usage_snapshots (id INTEGER PRIMARY KEY, timestamp TEXT, provider TEXT,
        bucket_key TEXT, bucket_label TEXT, utilization REAL, resets_at TEXT, source TEXT,
        account_id TEXT, account_label TEXT);
        CREATE TABLE IF NOT EXISTS usage_hourly (bucket_key TEXT);"
    )
    .unwrap();
    Storage {
        conn: std::sync::Mutex::new(conn),
        db_path: Default::default(),
        model_usage_overview_cache: Default::default(),
        context_savings_analytics_cache: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
            CREATE TABLE usage_snapshots (source TEXT);
            CREATE TABLE usage_hourly (bucket_key TEXT);
            INSERT INTO usage_snapshots VALUES ('cpa'), ('direct');
            INSERT INTO usage_hourly VALUES ('cpa/a/five_hour'), ('five_hour');
            INSERT INTO settings VALUES ('integration.cpa.base_url', 'http://localhost:8317'),
             ('integration.cpa.management_key', 'old'), ('usage.cpa.state', '{}');",
        )
        .unwrap();
        conn
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Atomic lifecycle storage]]
    #[test]
    fn cpa_lifecycle_transactions_rollback_and_isolate_sources() {
        let mut conn = fixture();
        conn.execute_batch("CREATE TRIGGER reject_key BEFORE INSERT ON settings
            WHEN NEW.key = 'integration.cpa.management_key' BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
        assert!(save_connection(&mut conn, "http://localhost:8318", "new").is_err());
        let url: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [BASE_URL_SETTING],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(url, "http://localhost:8317");
        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute_batch("DROP TRIGGER reject_key;").unwrap();
        save_connection(&mut conn, "http://localhost:8317", "new").unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        save_connection(&mut conn, "http://localhost:8318", "next").unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        conn.execute_batch(
            "INSERT INTO usage_snapshots VALUES ('cpa');
            CREATE TRIGGER reject_delete BEFORE DELETE ON usage_snapshots
            WHEN OLD.source = 'cpa' BEGIN SELECT RAISE(FAIL, 'injected'); END;",
        )
        .unwrap();
        assert!(clear_connection(&mut conn).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM settings", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute_batch("DROP TRIGGER reject_delete;").unwrap();
        clear_connection(&mut conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM settings", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
