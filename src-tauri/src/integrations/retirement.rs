use crate::integrations::deploy::{path_exists, remove_path};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const RETIRED_EVENT_TYPES: [&str; 5] = [
    "capture.event",
    "capture.snapshot",
    "capture.guidance",
    "mcp.continuity",
    "mcp.snapshot",
];

fn open_existing(path: &Path) -> Result<Option<Connection>, String> {
    if !path_exists(path)? {
        return Ok(None);
    }
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map(Some)
    .map_err(|err| format!("Failed to open existing {}: {err}", path.display()))
}

pub(crate) fn purge_continuity_artifacts_at(
    config_dir: &Path,
    usage_db: &Path,
) -> Result<(), String> {
    for script in [
        config_dir.join("scripts/context-capture.cjs"),
        config_dir.join("codex/scripts/context-capture.cjs"),
    ] {
        remove_path(&script)
            .map_err(|err| format!("Failed to remove {}: {err}", script.display()))?;
    }

    let context_root = config_dir.join("context");
    let continuity_dir = context_root.join("continuity");
    remove_path(&continuity_dir).map_err(|err| {
        format!(
            "Failed to remove retired context at {}: {err}",
            continuity_dir.display()
        )
    })?;

    if let Some(mut conn) = open_existing(&context_root.join("context.db"))? {
        let tx = conn
            .transaction()
            .map_err(|err| format!("Failed to begin context retirement: {err}"))?;
        tx.execute_batch(
            "DROP TABLE IF EXISTS continuity_events;
             DROP TABLE IF EXISTS compaction_snapshots;",
        )
        .map_err(|err| format!("Failed to drop retired context tables: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit context retirement: {err}"))?;
    }

    if let Some(mut conn) = open_existing(usage_db)? {
        let tx = conn
            .transaction()
            .map_err(|err| format!("Failed to begin usage retirement: {err}"))?;
        tx.execute(
            "DELETE FROM context_savings_events
             WHERE event_type IN (?1, ?2, ?3, ?4, ?5)",
            RETIRED_EVENT_TYPES,
        )
        .map_err(|err| format!("Failed to delete retired usage events: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit usage retirement: {err}"))?;
    }
    Ok(())
}

pub(crate) fn default_config_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".config/quill"))
        .ok_or_else(|| "Cannot retire session context without a home directory".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn purge_is_idempotent_and_preserves_unrelated_context() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("config");
        let context_dir = config_dir.join("context");
        let usage_db = temp.path().join("usage.db");
        for script in [
            config_dir.join("scripts/context-capture.cjs"),
            config_dir.join("codex/scripts/context-capture.cjs"),
        ] {
            fs::create_dir_all(script.parent().unwrap()).unwrap();
            fs::write(script, "retired").unwrap();
        }
        fs::create_dir_all(context_dir.join("continuity/locks")).unwrap();
        fs::write(context_dir.join("continuity/locks/session.lock"), "lock").unwrap();
        fs::write(context_dir.join("continuity/session.tmp"), "tmp").unwrap();
        fs::write(context_dir.join("working-context.txt"), "keep").unwrap();

        let context_db = context_dir.join("context.db");
        let conn = Connection::open(&context_db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sources (id INTEGER PRIMARY KEY, content TEXT);
             INSERT INTO sources VALUES (1, 'keep');
             CREATE TABLE continuity_events (id INTEGER PRIMARY KEY, summary TEXT);
             INSERT INTO continuity_events VALUES (1, 'drop');
             CREATE TABLE compaction_snapshots (id INTEGER PRIMARY KEY, snapshot TEXT);
             INSERT INTO compaction_snapshots VALUES (1, 'drop');",
        )
        .unwrap();
        drop(conn);

        let conn = Connection::open(&usage_db).unwrap();
        conn.execute_batch(
            "CREATE TABLE context_savings_events (
                 event_type TEXT NOT NULL,
                 payload TEXT NOT NULL
             );
             INSERT INTO context_savings_events VALUES
                 ('capture.event', 'drop'),
                 ('capture.snapshot', 'drop'),
                 ('capture.guidance', 'drop'),
                 ('mcp.continuity', 'drop'),
                 ('mcp.snapshot', 'drop'),
                 ('mcp.index', 'keep'),
                 ('capture.eventual', 'keep');",
        )
        .unwrap();
        drop(conn);

        purge_continuity_artifacts_at(&config_dir, &usage_db).unwrap();
        purge_continuity_artifacts_at(&config_dir, &usage_db).unwrap();

        assert!(!config_dir.join("scripts/context-capture.cjs").exists());
        assert!(
            !config_dir
                .join("codex/scripts/context-capture.cjs")
                .exists()
        );
        assert!(!context_dir.join("continuity").exists());
        assert_eq!(
            fs::read_to_string(context_dir.join("working-context.txt")).unwrap(),
            "keep"
        );

        let conn = Connection::open(&context_db).unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tables, vec!["sources"]);
        assert_eq!(
            conn.query_row("SELECT content FROM sources", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        drop(conn);

        let conn = Connection::open(&usage_db).unwrap();
        let events: Vec<String> = conn
            .prepare("SELECT event_type FROM context_savings_events ORDER BY event_type")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(events, vec!["capture.eventual", "mcp.index"]);

        let absent = temp.path().join("absent.db");
        purge_continuity_artifacts_at(&config_dir, &absent).unwrap();
        assert!(
            !absent.exists(),
            "cleanup must not create a missing database"
        );
    }
}
