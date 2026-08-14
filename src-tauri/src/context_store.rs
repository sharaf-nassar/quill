//! Loopback-only HTTP access to the Python MCP working-context store.
//!
//! Threat model: Quill's main HTTP server intentionally listens on
//! `0.0.0.0`, so these routes never join that router. They bind only
//! `127.0.0.1`, require the per-install secret, cap bodies and responses, and
//! constrain command execution to configured roots. Python and Rust writers
//! share the schema below, use WAL, wait up to 30 seconds for the other
//! writer, and replace a source plus its chunks in one transaction.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Response, StatusCode, header},
    routing::post,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{io::AsyncReadExt, net::TcpListener, process::Command, task::JoinHandle};

pub(crate) const MAX_HTTP_REQUEST_BYTES: usize = 5 * 1024 * 1024 + 64 * 1024;
const MAX_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_INDEX_BYTES: usize = 5 * 1024 * 1024;
const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const CHUNK_TARGET_BYTES: usize = 8192;
const CHUNK_OVERLAP_LINES: usize = 4;

#[derive(Clone)]
pub(crate) struct ContextServerConfig {
    pub enabled: bool,
    pub port: u16,
    pub db_path: PathBuf,
    pub secret: String,
    pub allowed_roots: Vec<PathBuf>,
    pub execute_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub(crate) struct ContextServerHandle {
    pub addr: std::net::SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for ContextServerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct ContextState {
    db_path: PathBuf,
    secret: String,
    allowed_roots: Vec<PathBuf>,
    execute_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub(crate) async fn spawn_context_server(
    config: ContextServerConfig,
) -> Result<Option<ContextServerHandle>, String> {
    if !config.enabled {
        return Ok(None);
    }
    if let Some(parent) = config.db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Create context directory: {error}"))?;
    }
    open_store(&config.db_path)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.port))
        .await
        .map_err(|error| format!("Bind context HTTP server: {error}"))?;
    let addr = listener.local_addr().map_err(|error| error.to_string())?;
    let state = Arc::new(ContextState {
        db_path: config.db_path,
        secret: config.secret,
        allowed_roots: config.allowed_roots,
        execute_enabled: config.execute_enabled,
    });
    let app = context_router(state);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            log::error!("Context HTTP server error: {error}");
        }
    });
    Ok(Some(ContextServerHandle { addr, task }))
}

fn context_router(state: Arc<ContextState>) -> Router {
    Router::new()
        .route("/api/v1/context/index", post(index_handler))
        .route("/api/v1/context/fetch", post(fetch_handler))
        .route("/api/v1/context/execute", post(execute_handler))
        .route("/api/v1/context/search", post(search_handler))
        .route("/api/v1/context/source", post(source_handler))
        .route("/api/v1/context/stats", post(stats_handler))
        .route("/api/v1/context/purge", post(purge_handler))
        .layer(DefaultBodyLimit::max(MAX_HTTP_REQUEST_BYTES))
        .with_state(state)
}

fn authorized(headers: &HeaderMap, secret: &str) -> bool {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    token.as_bytes().ct_eq(secret.as_bytes()).into()
}

fn bounded_json(status: StatusCode, value: Value) -> Response<Body> {
    let body =
        serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"error\":\"encode failed\"}".to_vec());
    let (status, body) = if body.len() <= MAX_HTTP_RESPONSE_BYTES {
        (status, body)
    } else {
        (
            StatusCode::INSUFFICIENT_STORAGE,
            b"{\"error\":\"response exceeds context API limit\"}".to_vec(),
        )
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

fn unauthorized() -> Response<Body> {
    bounded_json(StatusCode::UNAUTHORIZED, json!({"error": "Unauthorized"}))
}

fn bad_request(error: impl std::fmt::Display) -> Response<Body> {
    bounded_json(StatusCode::BAD_REQUEST, json!({"error": error.to_string()}))
}

fn internal(error: impl std::fmt::Display) -> Response<Body> {
    log::error!("Context HTTP operation failed: {error}");
    bounded_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": "Internal server error"}),
    )
}

fn open_store(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|error| format!("Open context store: {error}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| format!("Enable context foreign keys: {error}"))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| format!("Enable context WAL: {error}"))?;
    conn.busy_timeout(Duration::from_secs(30))
        .map_err(|error| format!("Set context busy timeout: {error}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL, origin TEXT, file_path TEXT, url TEXT,
            content_hash TEXT, content_bytes INTEGER NOT NULL DEFAULT 0,
            chunk_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL, metadata_json TEXT
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL, title TEXT NOT NULL, content TEXT NOT NULL,
            content_type TEXT NOT NULL, start_line INTEGER, end_line INTEGER,
            byte_length INTEGER NOT NULL, created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_id, chunk_index);
        CREATE INDEX IF NOT EXISTS idx_sources_updated ON sources(updated_at DESC);
        CREATE TABLE IF NOT EXISTS executions (
            id INTEGER PRIMARY KEY AUTOINCREMENT, command TEXT NOT NULL, cwd TEXT NOT NULL,
            exit_code INTEGER, timed_out INTEGER NOT NULL DEFAULT 0,
            duration_ms INTEGER NOT NULL, stdout_bytes INTEGER NOT NULL DEFAULT 0,
            stderr_bytes INTEGER NOT NULL DEFAULT 0,
            stdout_truncated INTEGER NOT NULL DEFAULT 0,
            stderr_truncated INTEGER NOT NULL DEFAULT 0,
            output_source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS fetch_cache (
            url TEXT PRIMARY KEY, source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL,
            label TEXT NOT NULL, content_type TEXT, status_code INTEGER, etag TEXT,
            last_modified TEXT, fetched_at TEXT NOT NULL, content_hash TEXT
        );",
    )
    .map_err(|error| format!("Initialize context schema: {error}"))?;
    let _ = conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            title, content, source_id UNINDEXED, content_type UNINDEXED,
            tokenize='porter unicode61'
        );",
    );
    Ok(conn)
}

fn has_fts(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chunks_fts')",
        [],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn preview(text: &str, limit: usize) -> Value {
    let bytes = text.as_bytes();
    let end = bytes.len().min(limit);
    let text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    json!({"text": text, "truncated": bytes.len() > limit})
}

fn content_type(text: &str, requested: &str) -> String {
    if requested != "auto" {
        return requested.to_string();
    }
    if [
        "```",
        "def ",
        "class ",
        "function ",
        "import ",
        "const ",
        "let ",
    ]
    .iter()
    .any(|marker| text.contains(marker))
    {
        "code".into()
    } else {
        "prose".into()
    }
}

#[derive(Debug)]
struct Chunk {
    index: usize,
    title: String,
    content: String,
    content_type: String,
    start_line: usize,
    end_line: usize,
}

fn chunk_text(text: &str, requested_type: &str) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return vec![];
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        let mut size = 0;
        while end < lines.len() {
            let line_bytes = lines[end].len() + 1;
            if end > start && size + line_bytes > CHUNK_TARGET_BYTES {
                break;
            }
            size += line_bytes;
            end += 1;
            if size >= CHUNK_TARGET_BYTES {
                break;
            }
        }
        let content = lines[start..end].join("\n").trim().to_string();
        if !content.is_empty() {
            let title = lines[start..end]
                .iter()
                .map(|line| line.trim())
                .find(|line| !line.is_empty())
                .unwrap_or("Untitled")
                .chars()
                .take(120)
                .collect();
            chunks.push(Chunk {
                index: chunks.len(),
                title,
                content_type: content_type(&content, requested_type),
                content,
                start_line: start + 1,
                end_line: end,
            });
        }
        if end >= lines.len() {
            break;
        }
        start = (end.saturating_sub(CHUNK_OVERLAP_LINES)).max(start + 1);
    }
    chunks
}

fn delete_sources(conn: &Connection, ids: &[i64]) -> Result<(), String> {
    for id in ids {
        if has_fts(conn) {
            conn.execute(
                "DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE source_id=?1)",
                [id],
            )
            .map_err(|error| error.to_string())?;
        }
        conn.execute("DELETE FROM chunks WHERE source_id=?1", [id])
            .map_err(|error| error.to_string())?;
        conn.execute("DELETE FROM sources WHERE id=?1", [id])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

struct NewSource<'a> {
    label: &'a str,
    kind: &'a str,
    origin: &'a str,
    file_path: Option<&'a str>,
    url: Option<&'a str>,
    content: &'a str,
    content_type: &'a str,
    metadata: Value,
}

fn insert_source(path: &Path, source: NewSource<'_>) -> Result<Value, String> {
    let mut conn = open_store(path)?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let old: Vec<i64> = {
        let mut stmt = tx
            .prepare("SELECT id FROM sources WHERE label=?1")
            .map_err(|e| e.to_string())?;
        stmt.query_map([source.label], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?
    };
    delete_sources(&tx, &old)?;
    let chunks = chunk_text(source.content, source.content_type);
    let timestamp = now();
    tx.execute(
        "INSERT INTO sources(label,kind,origin,file_path,url,content_hash,content_bytes,
         chunk_count,created_at,updated_at,metadata_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,?10)",
        params![
            source.label,
            source.kind,
            source.origin,
            source.file_path,
            source.url,
            sha256(source.content),
            source.content.len() as i64,
            chunks.len() as i64,
            timestamp,
            source.metadata.to_string()
        ],
    )
    .map_err(|e| e.to_string())?;
    let source_id = tx.last_insert_rowid();
    let mut inventory = Vec::new();
    for chunk in &chunks {
        tx.execute(
            "INSERT INTO chunks(source_id,chunk_index,title,content,content_type,start_line,
             end_line,byte_length,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                source_id,
                chunk.index as i64,
                chunk.title,
                chunk.content,
                chunk.content_type,
                chunk.start_line as i64,
                chunk.end_line as i64,
                chunk.content.len() as i64,
                timestamp
            ],
        )
        .map_err(|e| e.to_string())?;
        let chunk_id = tx.last_insert_rowid();
        if has_fts(&tx) {
            tx.execute(
                "INSERT INTO chunks_fts(rowid,title,content,source_id,content_type)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    chunk_id,
                    chunk.title,
                    chunk.content,
                    source_id,
                    chunk.content_type
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        if inventory.len() < 5 {
            inventory.push(json!({
                "chunk_ref": format!("chunk:{chunk_id}"), "index": chunk.index,
                "title": chunk.title, "content_type": chunk.content_type,
                "bytes": chunk.content.len(), "lines": [chunk.start_line, chunk.end_line],
                "preview": preview(&chunk.content, 400)["text"]
            }));
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({
        "source_id": source_id, "source_ref": format!("source:{source_id}"),
        "label": source.label, "kind": source.kind, "content_bytes": source.content.len(),
        "chunk_count": chunks.len(), "content_hash": sha256(source.content), "chunks": inventory
    }))
}

fn parse_ref(value: Option<&str>, prefix: &str) -> Option<i64> {
    value?
        .trim()
        .strip_prefix(&format!("{prefix}:"))
        .unwrap_or(value?.trim())
        .parse()
        .ok()
}

fn tokens(query: &str) -> Vec<String> {
    regex::Regex::new(r"[\w./:-]+")
        .unwrap()
        .find_iter(query)
        .map(|m| m.as_str().to_lowercase())
        .filter(|word| word.trim().len() > 1)
        .collect()
}

fn like_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn snippet(content: &str, query: &str, limit: usize) -> String {
    let lower = content.to_lowercase();
    let pos = tokens(query)
        .iter()
        .filter_map(|term| lower.find(term))
        .min()
        .unwrap_or(0);
    let mut start = pos.saturating_sub(limit / 3);
    while start > 0 && !content.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + limit).min(content.len());
    while end > start && !content.is_char_boundary(end) {
        end -= 1;
    }
    start = end.saturating_sub(limit);
    let mut out = content
        .get(start..end)
        .unwrap_or(content)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if start > 0 {
        out.insert_str(0, "...");
    }
    if end < content.len() {
        out.push_str("...");
    }
    out
}

fn search_store(
    path: &Path,
    query: &str,
    limit: usize,
    source: Option<&str>,
) -> Result<Value, String> {
    let conn = open_store(path)?;
    let limit = limit.clamp(1, 20);
    let words = tokens(query);
    let match_query = words
        .iter()
        .take(12)
        .map(|word| {
            format!(
                "\"{}\"",
                regex::Regex::new(r"[^\w]")
                    .unwrap()
                    .replace_all(word, " ")
                    .trim()
            )
        })
        .filter(|word| word != "\"\"")
        .collect::<Vec<_>>()
        .join(" ");
    let mut rows: Vec<(i64, i64, String, String, String, String, i64)> = Vec::new();
    let mut fts_used = false;
    let source_id = parse_ref(source, "source");
    if has_fts(&conn) && !match_query.is_empty() {
        let (clause, mut values) = if let Some(id) = source_id {
            (
                " AND s.id=?".to_string(),
                vec![SqlValue::Text(match_query.clone()), SqlValue::Integer(id)],
            )
        } else if let Some(label) = source {
            (
                " AND s.label LIKE ? ESCAPE '\\'".to_string(),
                vec![
                    SqlValue::Text(match_query.clone()),
                    SqlValue::Text(format!("%{}%", like_escape(label))),
                ],
            )
        } else {
            (String::new(), vec![SqlValue::Text(match_query.clone())])
        };
        values.push(SqlValue::Integer(limit as i64));
        let sql = format!("SELECT c.id,c.source_id,s.label,c.title,c.content,c.content_type,c.byte_length
            FROM chunks_fts JOIN chunks c ON c.id=chunks_fts.rowid JOIN sources s ON s.id=c.source_id
            WHERE chunks_fts MATCH ?{clause} ORDER BY bm25(chunks_fts) LIMIT ?");
        if let Ok(mut stmt) = conn.prepare(&sql)
            && let Ok(mapped) = stmt.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
        {
            fts_used = true;
            rows = mapped.filter_map(Result::ok).collect();
        }
    }
    if rows.is_empty() {
        let mut where_sql = "1=1".to_string();
        let mut values = Vec::new();
        for word in words.iter().take(8) {
            where_sql.push_str(
                " AND (LOWER(c.title) LIKE ? ESCAPE '\\' OR LOWER(c.content) LIKE ? ESCAPE '\\')",
            );
            let pattern = SqlValue::Text(format!("%{}%", like_escape(word)));
            values.extend([pattern.clone(), pattern]);
        }
        if let Some(id) = source_id {
            where_sql.push_str(" AND s.id=?");
            values.push(SqlValue::Integer(id));
        } else if let Some(label) = source {
            where_sql.push_str(" AND s.label LIKE ? ESCAPE '\\'");
            values.push(SqlValue::Text(format!("%{}%", like_escape(label))));
        }
        values.push(SqlValue::Integer((limit * 3) as i64));
        let sql = format!(
            "SELECT c.id,c.source_id,s.label,c.title,c.content,c.content_type,c.byte_length
            FROM chunks c JOIN sources s ON s.id=c.source_id WHERE {where_sql}
            ORDER BY s.updated_at DESC,c.chunk_index LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        rows = stmt
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows.sort_by_key(|row| {
            std::cmp::Reverse(
                words
                    .iter()
                    .map(|word| {
                        row.3.to_lowercase().matches(word).count()
                            + row.4.to_lowercase().matches(word).count()
                    })
                    .sum::<usize>(),
            )
        });
        rows.truncate(limit);
    }
    Ok(
        json!({"query": query, "fts_used": fts_used, "results": rows.into_iter().map(|row| json!({
        "source_ref": format!("source:{}", row.1), "chunk_ref": format!("chunk:{}", row.0),
        "source": row.2, "title": row.3, "content_type": row.5, "bytes": row.6,
        "snippet": snippet(&row.4, query, 700)
    })).collect::<Vec<_>>() }),
    )
}

fn chunk_inventory(conn: &Connection, source_id: i64, limit: usize) -> Result<Vec<Value>, String> {
    let mut stmt = conn.prepare("SELECT id,chunk_index,title,content,content_type,byte_length,start_line,end_line FROM chunks WHERE source_id=?1 ORDER BY chunk_index LIMIT ?2").map_err(|e| e.to_string())?;
    stmt.query_map(params![source_id, limit.clamp(1, 100) as i64], |row| {
        let content: String = row.get(3)?;
        Ok(json!({"chunk_ref": format!("chunk:{}", row.get::<_,i64>(0)?), "index": row.get::<_,i64>(1)?, "title": row.get::<_,String>(2)?, "content_type": row.get::<_,String>(4)?, "bytes": row.get::<_,i64>(5)?, "lines": [row.get::<_,i64>(6)?,row.get::<_,i64>(7)?], "preview": preview(&content,400)["text"]}))
    }).map_err(|e| e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e| e.to_string())
}

fn source_store(path: &Path, request: &SourceRequest) -> Result<Value, String> {
    let conn = open_store(path)?;
    if let Some(chunk_id) = parse_ref(request.chunk_ref.as_deref(), "chunk") {
        let row = conn.query_row("SELECT c.id,c.source_id,s.label,s.kind,c.title,c.content,c.content_type,c.byte_length FROM chunks c JOIN sources s ON s.id=c.source_id WHERE c.id=?1", [chunk_id], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,i64>(7)?))).optional().map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(
                json!({"error": format!("chunk not found: {}", request.chunk_ref.as_deref().unwrap_or(""))}),
            );
        };
        let shown = preview(
            &row.5,
            if request.include_content {
                16 * 1024
            } else {
                1200
            },
        );
        return Ok(
            json!({"chunk_ref":format!("chunk:{}",row.0),"source_ref":format!("source:{}",row.1),"source":row.2,"title":row.4,"content_type":row.6,"bytes":row.7,"content":if request.include_content{Some(shown.clone())}else{None},"preview":if request.include_content{None}else{Some(shown)}}),
        );
    }
    let source_id = parse_ref(request.source_ref.as_deref(), "source");
    let row = if let Some(id)=source_id {
        conn.query_row("SELECT id,label,kind,origin,file_path,url,content_bytes,chunk_count,updated_at FROM sources WHERE id=?1 ORDER BY updated_at DESC LIMIT 1",[id],map_source_row).optional()
    } else if let Some(label)=&request.source {
        conn.query_row("SELECT id,label,kind,origin,file_path,url,content_bytes,chunk_count,updated_at FROM sources WHERE label LIKE ?1 ESCAPE '\\' ORDER BY updated_at DESC LIMIT 1",[format!("%{}%",like_escape(label))],map_source_row).optional()
    } else {
        conn.query_row("SELECT id,label,kind,origin,file_path,url,content_bytes,chunk_count,updated_at FROM sources ORDER BY updated_at DESC LIMIT 1",[],map_source_row).optional()
    }.map_err(|e|e.to_string())?;
    let Some(row) = row else {
        return Ok(json!({"error":"source not found"}));
    };
    Ok(
        json!({"source_ref":format!("source:{}",row.0),"label":row.1,"kind":row.2,"origin":row.3,"file_path":row.4,"url":row.5,"content_bytes":row.6,"chunk_count":row.7,"updated_at":row.8,"chunks":chunk_inventory(&conn,row.0,request.limit)?}),
    )
}

type SourceRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
);
fn map_source_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn stats_store(path: &Path) -> Result<Value, String> {
    let conn = open_store(path)?;
    Ok(
        json!({"db_path":path.to_string_lossy(),"fts_available":has_fts(&conn),"sources":conn.query_row("SELECT COUNT(*) FROM sources",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?,"chunks":conn.query_row("SELECT COUNT(*) FROM chunks",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?,"executions":conn.query_row("SELECT COUNT(*) FROM executions",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?,"fetch_cache_entries":conn.query_row("SELECT COUNT(*) FROM fetch_cache",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?,"indexed_bytes":conn.query_row("SELECT COALESCE(SUM(content_bytes),0) FROM sources",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?}),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexRequest {
    content: Option<String>,
    #[serde(alias = "file_path")]
    file_path: Option<String>,
    cwd: Option<String>,
    source: Option<String>,
    #[serde(default = "auto", alias = "content_type")]
    content_type: String,
    #[serde(default = "max_index", alias = "max_bytes")]
    max_bytes: usize,
}
fn auto() -> String {
    "auto".into()
}
fn max_index() -> usize {
    MAX_INDEX_BYTES
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequest {
    query: String,
    source: Option<String>,
    #[serde(default = "five")]
    limit: usize,
}
fn five() -> usize {
    5
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceRequest {
    #[serde(alias = "source_ref")]
    source_ref: Option<String>,
    #[serde(alias = "chunk_ref")]
    chunk_ref: Option<String>,
    source: Option<String>,
    #[serde(default, alias = "include_content")]
    include_content: bool,
    #[serde(default = "twenty")]
    limit: usize,
}
fn twenty() -> usize {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeRequest {
    #[serde(default)]
    confirm: bool,
    #[serde(alias = "source_ref")]
    source_ref: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchRequest {
    url: String,
    source: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default = "max_fetch", alias = "max_bytes")]
    max_bytes: usize,
}
fn max_fetch() -> usize {
    MAX_FETCH_BYTES
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest {
    command: String,
    cwd: Option<String>,
    #[serde(default = "timeout", alias = "timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "max_output", alias = "max_output_bytes")]
    max_output_bytes: usize,
    #[serde(default = "yes", alias = "index_output")]
    index_output: bool,
}
fn timeout() -> u64 {
    30_000
}
fn max_output() -> usize {
    MAX_OUTPUT_BYTES
}
fn yes() -> bool {
    true
}

async fn index_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(req): Json<IndexRequest>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    if req.content.is_some() == req.file_path.is_some() {
        return bad_request("provide exactly one of content or file_path");
    }
    let limit = req.max_bytes.clamp(1024, MAX_INDEX_BYTES);
    let (mut text, kind, file_path, label, input_bytes) = if let Some(content) = req.content {
        let bytes = content.as_bytes();
        let kept = String::from_utf8_lossy(&bytes[..bytes.len().min(limit)]).into_owned();
        let label = req
            .source
            .unwrap_or_else(|| format!("content:{}", &sha256(&kept)[..12]));
        (kept, "content", None, label, bytes.len())
    } else {
        let file = req.file_path.unwrap();
        let cwd = match resolve_cwd(req.cwd.as_deref(), &state.allowed_roots) {
            Ok(p) => p,
            Err(e) => return bad_request(e),
        };
        let path = match resolve_file(&file, &cwd) {
            Ok(p) => p,
            Err(e) => return bad_request(e),
        };
        let bytes = match std::fs::read(&path) {
            Ok(v) => v,
            Err(e) => return bad_request(e),
        };
        let kept = String::from_utf8_lossy(&bytes[..bytes.len().min(limit)]).into_owned();
        let label = req
            .source
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        (
            kept,
            "file",
            Some(path.to_string_lossy().into_owned()),
            label,
            bytes.len(),
        )
    };
    let truncated = input_bytes > limit;
    if truncated {
        text.push_str("\n\n[truncated at Quill indexing cap]")
    }
    match insert_source(
        &state.db_path,
        NewSource {
            label: &label,
            kind,
            origin: "quill_index_context",
            file_path: file_path.as_deref(),
            url: None,
            content: &text,
            content_type: &req.content_type,
            metadata: json!({"truncated":truncated}),
        },
    ) {
        Ok(indexed) => bounded_json(
            StatusCode::OK,
            json!({"indexed":indexed,"truncated":truncated}),
        ),
        Err(e) => internal(e),
    }
}
async fn search_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(req): Json<SearchRequest>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    match search_store(&state.db_path, &req.query, req.limit, req.source.as_deref()) {
        Ok(v) => bounded_json(StatusCode::OK, v),
        Err(e) => internal(e),
    }
}
async fn source_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(req): Json<SourceRequest>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    match source_store(&state.db_path, &req) {
        Ok(v) => bounded_json(StatusCode::OK, v),
        Err(e) => internal(e),
    }
}
async fn stats_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(_): Json<Value>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    match stats_store(&state.db_path) {
        Ok(v) => bounded_json(StatusCode::OK, v),
        Err(e) => internal(e),
    }
}
async fn purge_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(req): Json<PurgeRequest>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    if !req.confirm {
        return bounded_json(
            StatusCode::OK,
            json!({"purged":false,"message":"Pass confirm=true to purge context data."}),
        );
    }
    let conn = match open_store(&state.db_path) {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    if let Some(reference) = req.source_ref {
        let Some(id) = parse_ref(Some(&reference), "source") else {
            return bad_request("invalid source_ref");
        };
        if let Err(e) = delete_sources(&conn, &[id]) {
            return internal(e);
        }
        let _ = conn.execute("DELETE FROM fetch_cache WHERE source_id=?1", [id]);
        return bounded_json(
            StatusCode::OK,
            json!({"purged":true,"scope":format!("source:{id}")}),
        );
    }
    let prior = stats_store(&state.db_path).unwrap_or(json!({}));
    let result = (|| -> Result<(), String> {
        if has_fts(&conn) {
            conn.execute("DELETE FROM chunks_fts", [])
                .map_err(|e| e.to_string())?;
        }
        conn.execute_batch("DELETE FROM fetch_cache;DELETE FROM executions;DELETE FROM chunks;DELETE FROM sources;").map_err(|e|e.to_string())?;
        Ok(())
    })();
    match result {
        Ok(()) => bounded_json(
            StatusCode::OK,
            json!({"purged":true,"scope":"all","previous_counts":prior,"removed_files":[]}),
        ),
        Err(e) => internal(e),
    }
}

async fn fetch_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(req): Json<FetchRequest>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    let max = req.max_bytes.clamp(1024, MAX_FETCH_BYTES);
    if !req.force
        && let Ok(conn) = open_store(&state.db_path)
        && let Ok(Some((id, label, chunks, fetched_at))) = conn
            .query_row(
                "SELECT s.id,s.label,s.chunk_count,f.fetched_at
                 FROM fetch_cache f JOIN sources s ON s.id=f.source_id
                 WHERE f.url=?1 AND julianday('now')-julianday(f.fetched_at)<1.0",
                [&req.url],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
    {
        return bounded_json(
            StatusCode::OK,
            json!({"cached":true,"source_ref":format!("source:{id}"),"label":label,"chunk_count":chunks,"fetched_at":fetched_at}),
        );
    }
    let fetched = match crate::fetcher::fetch_context_url(&req.url, max).await {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    let label = req.source.unwrap_or_else(|| req.url.clone());
    let mut text = String::from_utf8_lossy(&fetched.body).into_owned();
    if fetched.truncated {
        text.push_str("\n\n[truncated at Quill fetch cap]")
    }
    let indexed = match insert_source(
        &state.db_path,
        NewSource {
            label: &label,
            kind: "fetch",
            origin: "quill_fetch_and_index",
            file_path: None,
            url: Some(&req.url),
            content: &text,
            content_type: "text",
            metadata: json!({"truncated":fetched.truncated,"status_code":fetched.status,"content_type":fetched.content_type,"final_url":fetched.final_url}),
        },
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if let Ok(conn) = open_store(&state.db_path) {
        let _=conn.execute("INSERT INTO fetch_cache(url,source_id,label,content_type,status_code,etag,last_modified,fetched_at,content_hash)VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)ON CONFLICT(url)DO UPDATE SET source_id=excluded.source_id,label=excluded.label,content_type=excluded.content_type,status_code=excluded.status_code,etag=excluded.etag,last_modified=excluded.last_modified,fetched_at=excluded.fetched_at,content_hash=excluded.content_hash",params![req.url,indexed["source_id"].as_i64(),label,fetched.content_type,fetched.status,fetched.etag,fetched.last_modified,now(),indexed["content_hash"].as_str()]);
    }
    bounded_json(
        StatusCode::OK,
        json!({"cached":false,"indexed":indexed,"preview":preview(&text,3000)}),
    )
}

fn resolve_cwd(cwd: Option<&str>, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let raw = PathBuf::from(cwd.ok_or("cwd is required")?);
    let path = raw
        .canonicalize()
        .map_err(|_| "cwd does not exist or is not a directory".to_string())?;
    if !path.is_dir() {
        return Err("cwd does not exist or is not a directory".into());
    }
    let allowed = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| path == root || path.starts_with(&root));
    if !allowed {
        return Err("cwd is outside configured context roots".into());
    }
    Ok(path)
}
fn resolve_file(file: &str, cwd: &Path) -> Result<PathBuf, String> {
    let raw = PathBuf::from(file);
    let path = if raw.is_absolute() {
        raw
    } else {
        cwd.join(raw)
    }
    .canonicalize()
    .map_err(|_| "file_path does not exist or is not a file".to_string())?;
    if !path.is_file() || !path.starts_with(cwd) {
        return Err("file_path must be a file under cwd".into());
    }
    Ok(path)
}
fn validate_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("command must not be empty".into());
    }
    let patterns = [
        r"(?is)\brm\s+[^;&|]*(-rf|-fr|--recursive\s+--force|--force\s+--recursive)[^;&|]*(\s/(\s|$)|\s/\*|\s~(\s|/|$)|\s\$HOME(\s|/|$)|--no-preserve-root)",
        r"(?is)(^|[;&|])\s*(sudo\s+)?(shutdown|reboot|halt|poweroff)\b",
        r"(?is)\b(curl|wget)\b[^;&]*\|\s*(sudo\s+)?(sh|bash|zsh|fish|python|perl|ruby)\b",
        r":\s*\(\s*\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:",
        r"(?is)(^|[;&|])\s*(sudo\s+)?mkfs(\.\w+)?\b",
        r"(?is)\bdd\s+[^;&|]*\bof=/dev/",
        r"(?is)(^|[;&|])\s*(cd|pushd|popd)\b",
    ];
    if patterns
        .iter()
        .any(|p| regex::Regex::new(p).unwrap().is_match(command))
    {
        return Err("unsafe command rejected".into());
    }
    Ok(())
}
async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Result<(String, usize, bool), String> {
    let mut kept = Vec::new();
    let mut total = 0;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count;
        if kept.len() < limit {
            kept.extend_from_slice(&buffer[..count.min(limit - kept.len())]);
        }
    }
    Ok((
        String::from_utf8_lossy(&kept).into_owned(),
        total,
        total > kept.len(),
    ))
}
async fn execute_handler(
    State(state): State<Arc<ContextState>>,
    headers: HeaderMap,
    Json(req): Json<ExecuteRequest>,
) -> Response<Body> {
    if !authorized(&headers, &state.secret) {
        return unauthorized();
    }
    if !(state.execute_enabled)() {
        return bounded_json(
            StatusCode::FORBIDDEN,
            json!({"error":"context preservation is disabled"}),
        );
    }
    if let Err(e) = validate_command(&req.command) {
        return bad_request(e);
    }
    let cwd = match resolve_cwd(req.cwd.as_deref(), &state.allowed_roots) {
        Ok(p) => p,
        Err(e) => return bad_request(e),
    };
    let cap = req.max_output_bytes.clamp(1024, MAX_OUTPUT_BYTES);
    let started = Instant::now();
    let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        command.arg("/C");
    } else {
        command.arg("-c");
    }
    command
        .arg(&req.command)
        .current_dir(&cwd)
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in [
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TERM", "TMPDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("QUILL_CONTEXT", "1");
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return bad_request(e),
    };
    let stdout = tokio::spawn(read_bounded(child.stdout.take().unwrap(), cap));
    let stderr = tokio::spawn(read_bounded(child.stderr.take().unwrap(), cap));
    let (status, timed_out) = match tokio::time::timeout(
        Duration::from_millis(req.timeout_ms.clamp(100, 120_000)),
        child.wait(),
    )
    .await
    {
        Ok(Ok(s)) => (Some(s), false),
        Ok(Err(e)) => return internal(e),
        Err(_) => {
            #[cfg(unix)]
            if let Some(id) = child.id() {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(id as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            let _ = child.kill().await;
            (child.wait().await.ok(), true)
        }
    };
    let (stdout_text, stdout_bytes, stdout_truncated) = match stdout.await {
        Ok(Ok(v)) => v,
        _ => return internal("stdout reader failed"),
    };
    let (stderr_text, stderr_bytes, stderr_truncated) = match stderr.await {
        Ok(Ok(v)) => v,
        _ => return internal("stderr reader failed"),
    };
    let duration = started.elapsed().as_millis() as i64;
    let mut output_source = None;
    if req.index_output
        && (stdout_truncated
            || stderr_truncated
            || stdout_text.len() + stderr_text.len() > 12 * 1024)
    {
        let text = format!(
            "$ {}\ncwd: {}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
            req.command,
            cwd.display(),
            stdout_text,
            stderr_text
        );
        output_source = insert_source(
            &state.db_path,
            NewSource {
                label: &format!("execute:{}", &sha256(&(req.command.clone() + &now()))[..12]),
                kind: "execution",
                origin: "quill_execute",
                file_path: None,
                url: None,
                content: &text,
                content_type: "text",
                metadata: json!({"cwd":cwd}),
            },
        )
        .ok();
    }
    if let Ok(conn) = open_store(&state.db_path) {
        let _=conn.execute("INSERT INTO executions(command,cwd,exit_code,timed_out,duration_ms,stdout_bytes,stderr_bytes,stdout_truncated,stderr_truncated,output_source_id,created_at)VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![req.command,cwd.to_string_lossy(),status.and_then(|s|s.code()),timed_out as i64,duration,stdout_bytes as i64,stderr_bytes as i64,stdout_truncated as i64,stderr_truncated as i64,output_source.as_ref().and_then(|v|v["source_id"].as_i64()),now()]);
    }
    bounded_json(
        StatusCode::OK,
        json!({"command":req.command,"cwd":cwd,"exitCode":status.and_then(|s|s.code()),"timedOut":timed_out,"durationMs":duration,"stdout":stdout_text,"stderr":stderr_text,"stdoutBytes":stdout_bytes,"stderrBytes":stderr_bytes,"stdoutTruncated":stdout_truncated,"stderrTruncated":stderr_truncated,"outputSource":output_source}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    async fn api(
        enabled: bool,
        execute_enabled: Arc<AtomicBool>,
    ) -> (tempfile::TempDir, Option<ContextServerHandle>) {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let handle = spawn_context_server(ContextServerConfig {
            enabled,
            port: 0,
            db_path: temp.path().join("context.db"),
            secret: "test-secret".into(),
            allowed_roots: vec![work],
            execute_enabled: Arc::new(move || execute_enabled.load(Ordering::Relaxed)),
        })
        .await
        .unwrap();
        (temp, handle)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    async fn post(handle: &ContextServerHandle, path: &str, body: Value) -> reqwest::Response {
        client()
            .post(format!("http://{}{path}", handle.addr))
            .bearer_auth("test-secret")
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    // @lat: [[context-http-api-tests#Loopback listener and mount gate]]
    #[tokio::test]
    async fn listener_is_loopback_only_and_absent_when_disabled() {
        let flag = Arc::new(AtomicBool::new(false));
        let (_temp, absent) = api(false, Arc::clone(&flag)).await;
        assert!(absent.is_none());

        let (_temp, handle) = api(true, flag).await;
        let handle = handle.unwrap();
        assert_eq!(
            handle.addr.ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );

        let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        probe.connect("8.8.8.8:80").unwrap();
        let host_ip = probe.local_addr().unwrap().ip();
        if !host_ip.is_loopback() {
            assert!(
                tokio::net::TcpStream::connect((host_ip, handle.addr.port()))
                    .await
                    .is_err()
            );
        }
    }

    // @lat: [[context-http-api-tests#Authentication and size bounds]]
    #[tokio::test]
    async fn auth_and_request_response_bounds_are_enforced() {
        let execute_enabled = Arc::new(AtomicBool::new(false));
        let (temp, handle) = api(true, Arc::clone(&execute_enabled)).await;
        let handle = handle.unwrap();
        let response = client()
            .post(format!("http://{}/api/v1/context/stats", handle.addr))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        let response = post(
            &handle,
            "/api/v1/context/index",
            json!({"content": "x".repeat(MAX_HTTP_REQUEST_BYTES), "source": "too-big"}),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

        let response = post(
            &handle,
            "/api/v1/context/fetch",
            json!({"url": "http://127.0.0.1/private"}),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        execute_enabled.store(true, Ordering::Relaxed);
        let response = post(
            &handle,
            "/api/v1/context/execute",
            json!({
                "command": "head -c 200000 /dev/zero",
                "cwd": temp.path().join("work"),
                "maxOutputBytes": 200000
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::INSUFFICIENT_STORAGE);
    }

    // @lat: [[context-http-api-tests#Execute permission scope and cap]]
    #[tokio::test]
    async fn execute_is_permission_gated_scoped_and_capped() {
        let flag = Arc::new(AtomicBool::new(false));
        let (temp, handle) = api(true, Arc::clone(&flag)).await;
        let handle = handle.unwrap();
        let work = temp.path().join("work");
        let response = post(
            &handle,
            "/api/v1/context/execute",
            json!({"command": "pwd", "cwd": work}),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        flag.store(true, Ordering::Relaxed);
        let response = post(
            &handle,
            "/api/v1/context/execute",
            json!({"command": "pwd", "cwd": temp.path()}),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        let response = post(
            &handle,
            "/api/v1/context/execute",
            json!({
                "command": "head -c 4096 /dev/zero | tr '\\0' x",
                "cwd": work,
                "maxOutputBytes": 1024
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["stdoutBytes"], 4096);
        assert_eq!(body["stdout"].as_str().unwrap().len(), 1024);
        assert_eq!(body["stdoutTruncated"], true);
    }

    // @lat: [[context-http-api-tests#Shared-store Python parity]]
    #[tokio::test]
    async fn search_source_and_stats_match_python_refs_on_shared_store() {
        let (temp, handle) = api(true, Arc::new(AtomicBool::new(false))).await;
        let handle = handle.unwrap();
        let indexed: Value = post(
            &handle,
            "/api/v1/context/index",
            json!({"content": "alpha needle omega", "source": "fixture"}),
        )
        .await
        .json()
        .await
        .unwrap();
        let source_ref = indexed["indexed"]["source_ref"].as_str().unwrap();

        let rust_search: Value = post(
            &handle,
            "/api/v1/context/search",
            json!({"query": "needle", "limit": 5}),
        )
        .await
        .json()
        .await
        .unwrap();
        let rust_source: Value = post(
            &handle,
            "/api/v1/context/source",
            json!({"sourceRef": source_ref}),
        )
        .await
        .json()
        .await
        .unwrap();
        let rust_stats: Value = post(&handle, "/api/v1/context/stats", json!({}))
            .await
            .json()
            .await
            .unwrap();

        let script = r#"
import json, pathlib, sys, types
class M:
    def tool(self, **kwargs): return lambda fn: fn
server = types.ModuleType('server'); server.mcp = M(); sys.modules['server'] = server
sys.path.insert(0, sys.argv[2])
from tools import context as c
c.CONTEXT_DB = pathlib.Path(sys.argv[1]); c.CONTEXT_DIR = c.CONTEXT_DB.parent
c._db_conn = None; c._fts_available = None
search = c._search_context('needle', 5)
source = c.quill_get_context_source(source_ref='source:1')
stats = c._context_stats()
print(json.dumps({
  'search_refs': [[x['source_ref'], x['chunk_ref']] for x in search['results']],
  'source_ref': source['source_ref'],
  'chunk_refs': [x['chunk_ref'] for x in source['chunks']],
  'counts': [stats['sources'], stats['chunks'], stats['executions'], stats['fetch_cache_entries'], stats['indexed_bytes']],
}))
"#;
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(temp.path().join("context.db"))
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/claude-integration/mcp"
            ))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let python: Value = serde_json::from_slice(&output.stdout).unwrap();
        let rust_refs: Vec<Value> = rust_search["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| json!([row["source_ref"], row["chunk_ref"]]))
            .collect();
        assert_eq!(python["search_refs"], json!(rust_refs));
        assert_eq!(python["source_ref"], rust_source["source_ref"]);
        assert_eq!(
            python["chunk_refs"],
            json!(
                rust_source["chunks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["chunk_ref"].clone())
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(
            python["counts"],
            json!([
                rust_stats["sources"],
                rust_stats["chunks"],
                rust_stats["executions"],
                rust_stats["fetch_cache_entries"],
                rust_stats["indexed_bytes"]
            ])
        );
    }
}
