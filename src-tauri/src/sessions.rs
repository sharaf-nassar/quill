use std::collections::HashMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tantivy::collector::{FacetCollector, TopDocs};
use tantivy::query::{BooleanQuery, Occur, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::*;
use tantivy::snippet::SnippetGenerator;
use tantivy::{DateTime, Index, IndexReader, IndexWriter, TantivyDocument, Term};

// ---------------------------------------------------------------------------
// Schema fields wrapper
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SessionSchema {
    pub message_id: Field,
    pub session_id: Field,
    pub content: Field,
    pub role: Field,
    pub project: Field,
    pub host: Field,
    pub timestamp: Field,
    pub git_branch: Field,
    pub tools_used: Field,
    pub files_modified: Field,
    pub schema: Schema,
}

// ---------------------------------------------------------------------------
// Index state -- tracks which files have been indexed and their mtimes
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
pub struct IndexState {
    /// Map of JSONL file path -> last-modified epoch seconds
    pub file_mtimes: HashMap<String, u64>,
}

// ---------------------------------------------------------------------------
// SessionIndex -- main struct that owns the tantivy index
// ---------------------------------------------------------------------------

pub struct SessionIndex {
    pub index: Index,
    pub fields: SessionSchema,
    pub writer: Arc<Mutex<IndexWriter>>,
    pub reader: IndexReader,
    pub index_dir: PathBuf,
    pub state: Mutex<IndexState>,
}

impl SessionIndex {
    /// Open an existing index or create a new one at the given directory.
    pub fn open_or_create(index_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(index_dir)
            .map_err(|e| format!("Failed to create index dir: {e}"))?;

        let (schema, fields) = Self::build_schema();

        let dir = tantivy::directory::MmapDirectory::open(index_dir)
            .map_err(|e| format!("Failed to open MmapDirectory: {e}"))?;

        let index = Index::open_or_create(dir, schema)
            .map_err(|e| format!("Failed to open or create index: {e}"))?;

        let writer: IndexWriter = index
            .writer(50_000_000)
            .map_err(|e| format!("Failed to create IndexWriter: {e}"))?;

        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| format!("Failed to create IndexReader: {e}"))?;

        let state = Self::load_state(index_dir);

        Ok(Self {
            index,
            fields,
            writer: Arc::new(Mutex::new(writer)),
            reader,
            index_dir: index_dir.to_path_buf(),
            state: Mutex::new(state),
        })
    }

    /// Build the tantivy schema with all 10 fields.
    fn build_schema() -> (Schema, SessionSchema) {
        let mut builder = Schema::builder();

        // STRING | STORED fields (untokenized, exact-match, stored)
        let message_id = builder.add_text_field("message_id", STRING | STORED);
        let session_id = builder.add_text_field("session_id", STRING | STORED);
        let role = builder.add_text_field("role", STRING | STORED);
        let git_branch = builder.add_text_field("git_branch", STRING | STORED);

        // TEXT | STORED fields (tokenized, full-text searchable, stored)
        let content = builder.add_text_field("content", TEXT | STORED);
        let tools_used = builder.add_text_field("tools_used", TEXT | STORED);
        let files_modified = builder.add_text_field("files_modified", TEXT | STORED);

        // Facet fields (hierarchical)
        let project = builder.add_facet_field("project", FacetOptions::default());
        let host = builder.add_facet_field("host", FacetOptions::default());

        // Date field (indexed, stored, fast)
        let date_opts = DateOptions::from(INDEXED)
            .set_stored()
            .set_fast()
            .set_precision(DateTimePrecision::Seconds);
        let timestamp = builder.add_date_field("timestamp", date_opts);

        let schema = builder.build();

        let fields = SessionSchema {
            message_id,
            session_id,
            content,
            role,
            project,
            host,
            timestamp,
            git_branch,
            tools_used,
            files_modified,
            schema: schema.clone(),
        };

        (schema, fields)
    }

    /// Load persisted index state from disk (file mtimes tracking).
    fn load_state(index_dir: &Path) -> IndexState {
        let state_path = index_dir.join("index_state.json");
        if state_path.exists() {
            match std::fs::read_to_string(&state_path) {
                Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                Err(_) => IndexState::default(),
            }
        } else {
            IndexState::default()
        }
    }

    /// Save the current index state to disk.
    pub fn save_state(&self) -> Result<(), String> {
        let state_path = self.index_dir.join("index_state.json");
        let state = self.state.lock();
        let json =
            serde_json::to_string_pretty(&*state).map_err(|e| format!("Serialize state: {e}"))?;
        std::fs::write(&state_path, json).map_err(|e| format!("Write state: {e}"))?;
        Ok(())
    }

    /// Get a fresh Searcher from the reader pool.
    pub fn searcher(&self) -> tantivy::Searcher {
        self.reader.searcher()
    }

    /// Extract a human-readable project name from a directory-encoded path.
    /// e.g. "-home-mamba-work-claude-usage" -> "claude-usage"
    pub fn project_display_name(dir_name: &str) -> String {
        dir_name
            .rsplit('-')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(dir_name)
            .to_string()
    }

    /// Index a single extracted message into the tantivy index.
    pub fn index_message(
        &self,
        msg: &ExtractedMessage,
        project_facet: &str,
        host_facet: &str,
    ) -> Result<(), String> {
        let mut doc = TantivyDocument::default();

        doc.add_text(self.fields.message_id, &msg.uuid);
        doc.add_text(self.fields.session_id, &msg.session_id);
        doc.add_text(self.fields.content, &msg.content);
        doc.add_text(self.fields.role, &msg.role);
        doc.add_text(self.fields.git_branch, &msg.git_branch);
        doc.add_text(self.fields.tools_used, msg.tools_used.join(" "));
        doc.add_text(self.fields.files_modified, msg.files_modified.join(" "));

        doc.add_facet(
            self.fields.project,
            Facet::from(&format!("/{project_facet}")),
        );
        doc.add_facet(self.fields.host, Facet::from(&format!("/{host_facet}")));

        // Parse timestamp as RFC3339 -> tantivy DateTime
        let ts = if !msg.timestamp.is_empty() {
            chrono::DateTime::parse_from_rfc3339(&msg.timestamp)
                .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                .unwrap_or(DateTime::from_timestamp_secs(0))
        } else {
            DateTime::from_timestamp_secs(0)
        };
        doc.add_date(self.fields.timestamp, ts);

        let writer = self.writer.lock();
        writer
            .add_document(doc)
            .map_err(|e| format!("Add document: {e}"))?;

        Ok(())
    }

    /// Scan ~/.claude/projects/*/*.jsonl and index new/modified files.
    /// Returns the number of newly indexed messages.
    pub fn startup_scan(&self, app_handle: &tauri::AppHandle) -> Result<usize, String> {
        use tauri::Emitter;

        let projects_dir = dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".claude")
            .join("projects");

        if !projects_dir.exists() {
            log::info!("No ~/.claude/projects directory found, skipping scan");
            return Ok(0);
        }

        let mut total_indexed = 0usize;
        let mut state = self.state.lock();

        // Collect all JSONL files and their mtimes
        let project_entries: Vec<_> = std::fs::read_dir(&projects_dir)
            .map_err(|e| format!("Read projects dir: {e}"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        // Detect hostname from system
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .or_else(|_| std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()))
            .unwrap_or_else(|_| "unknown".to_string());

        for project_entry in &project_entries {
            let project_dir = project_entry.path();
            let project_dir_name = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let project_name = Self::project_display_name(project_dir_name);

            let jsonl_files: Vec<_> = std::fs::read_dir(&project_dir)
                .map_err(|e| format!("Read project dir: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
                .collect();

            for entry in &jsonl_files {
                let file_path = entry.path();
                let file_key = file_path.to_string_lossy().to_string();

                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                let known_mtime = state.file_mtimes.get(&file_key).copied();

                if known_mtime == Some(mtime) {
                    // File hasn't changed since last index
                    continue;
                }

                // If file was previously indexed but modified, delete old docs
                if known_mtime.is_some() {
                    // Extract session_id from filename (UUID.jsonl)
                    if let Some(session_id) = file_path.file_stem().and_then(|s| s.to_str()) {
                        let term = Term::from_field_text(self.fields.session_id, session_id);
                        let writer = self.writer.lock();
                        writer.delete_term(term);
                    }
                }

                // Index messages from this file
                let messages = extract_messages_from_jsonl(&file_path);
                for msg in &messages {
                    if let Err(e) = self.index_message(msg, &project_name, &hostname) {
                        log::warn!("Failed to index message: {e}");
                    }
                }

                total_indexed += messages.len();
                state.file_mtimes.insert(file_key, mtime);
            }
        }

        // Commit all changes
        if total_indexed > 0 {
            let mut writer = self.writer.lock();
            writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        }

        // Must drop state lock before save_state which acquires it
        drop(state);

        self.save_state()?;

        log::info!("Session index scan complete: {total_indexed} messages indexed");
        let _ = app_handle.emit("sessions-index-updated", total_indexed);

        Ok(total_indexed)
    }

    // -------------------------------------------------------------------
    // Search
    // -------------------------------------------------------------------

    /// Search the index with a query string and optional filters.
    pub fn search(&self, filters: &SearchFilters) -> Result<SearchResults, String> {
        let searcher = self.searcher();
        let f = &self.fields;

        // Build query parser targeting content, tools_used, files_modified
        let mut parser =
            QueryParser::for_index(&self.index, vec![f.content, f.tools_used, f.files_modified]);
        parser.set_conjunction_by_default();

        let (text_query, _errors) = parser.parse_query_lenient(&filters.query);

        // Combine with filter clauses via BooleanQuery
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> =
            vec![(Occur::Must, text_query)];

        // Project facet filter
        if let Some(ref proj) = filters.project {
            let facet = Facet::from(&format!("/{proj}"));
            let term = Term::from_facet(f.project, &facet);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Host facet filter
        if let Some(ref host) = filters.host {
            let facet = Facet::from(&format!("/{host}"));
            let term = Term::from_facet(f.host, &facet);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Role filter
        if let Some(ref role) = filters.role {
            let term = Term::from_field_text(f.role, role);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Git branch filter
        if let Some(ref branch) = filters.git_branch {
            let term = Term::from_field_text(f.git_branch, branch);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Date range filter
        if filters.date_from.is_some() || filters.date_to.is_some() {
            let lower = match &filters.date_from {
                Some(from_str) => {
                    let dt = chrono::DateTime::parse_from_rfc3339(from_str)
                        .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                        .unwrap_or(DateTime::MIN);
                    Bound::Included(Term::from_field_date(f.timestamp, dt))
                }
                None => Bound::Unbounded,
            };
            let upper = match &filters.date_to {
                Some(to_str) => {
                    let dt = chrono::DateTime::parse_from_rfc3339(to_str)
                        .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                        .unwrap_or(DateTime::MAX);
                    Bound::Included(Term::from_field_date(f.timestamp, dt))
                }
                None => Bound::Unbounded,
            };
            clauses.push((Occur::Must, Box::new(RangeQuery::new(lower, upper))));
        }

        let combined = BooleanQuery::new(clauses);
        let limit = filters.limit.unwrap_or(20) as usize;
        let offset = filters.offset.unwrap_or(0) as usize;

        let top_docs = searcher
            .search(&combined, &TopDocs::with_limit(limit).and_offset(offset))
            .map_err(|e| format!("Search error: {e}"))?;

        // Snippet generator for content field
        let snippet_gen = SnippetGenerator::create(&searcher, &combined, f.content)
            .map_err(|e| format!("Snippet generator error: {e}"))?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, doc_addr) in &top_docs {
            let doc: TantivyDocument = searcher
                .doc(*doc_addr)
                .map_err(|e| format!("Doc retrieval: {e}"))?;

            let snippet = snippet_gen.snippet_from_doc(&doc);
            // Convert <b>...</b> to <mark>...</mark>
            let snippet_html = snippet
                .to_html()
                .replace("<b>", "<mark>")
                .replace("</b>", "</mark>");

            let get_text = |field: Field| -> String {
                doc.get_first(field)
                    .and_then(|v| v.as_value().as_str().map(|s| s.to_string()))
                    .unwrap_or_default()
            };

            let get_facet_str = |field: Field| -> String {
                doc.get_first(field)
                    .and_then(|v| {
                        v.as_value().as_facet().map(|f| {
                            // Strip leading "/"
                            f.strip_prefix('/').unwrap_or(f).to_string()
                        })
                    })
                    .unwrap_or_default()
            };

            let timestamp = doc
                .get_first(f.timestamp)
                .and_then(|v| v.as_value().as_datetime())
                .map(|dt| {
                    chrono::DateTime::from_timestamp(dt.into_timestamp_secs(), 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            hits.push(SearchHit {
                message_id: get_text(f.message_id),
                session_id: get_text(f.session_id),
                content: get_text(f.content),
                snippet: snippet_html,
                role: get_text(f.role),
                project: get_facet_str(f.project),
                host: get_facet_str(f.host),
                timestamp,
                git_branch: get_text(f.git_branch),
                tools_used: get_text(f.tools_used),
                files_modified: get_text(f.files_modified),
                score: *score,
            });
        }

        Ok(SearchResults {
            hits,
            total: top_docs.len() as u64,
        })
    }

    // -------------------------------------------------------------------
    // Facets
    // -------------------------------------------------------------------

    /// Collect distinct project and host facets from the index.
    pub fn get_facets(&self) -> Result<SearchFacets, String> {
        let searcher = self.searcher();

        let mut project_collector = FacetCollector::for_field("project");
        project_collector.add_facet(Facet::root());

        let mut host_collector = FacetCollector::for_field("host");
        host_collector.add_facet(Facet::root());

        let (project_counts, host_counts) = searcher
            .search(
                &tantivy::query::AllQuery,
                &(project_collector, host_collector),
            )
            .map_err(|e| format!("Facet collection error: {e}"))?;

        let projects = project_counts
            .get("/")
            .map(|(facet, count)| FacetCount {
                label: facet
                    .to_string()
                    .strip_prefix('/')
                    .unwrap_or(&facet.to_string())
                    .to_string(),
                count,
            })
            .collect();

        let hosts = host_counts
            .get("/")
            .map(|(facet, count)| FacetCount {
                label: facet
                    .to_string()
                    .strip_prefix('/')
                    .unwrap_or(&facet.to_string())
                    .to_string(),
                count,
            })
            .collect();

        Ok(SearchFacets { projects, hosts })
    }

    // -------------------------------------------------------------------
    // Context -- surrounding messages for a search hit
    // -------------------------------------------------------------------

    /// Find the JSONL file for a session and return a window of messages
    /// around the target message.
    pub fn get_context(
        &self,
        session_id: &str,
        message_id: &str,
        window: usize,
    ) -> Result<SessionContext, String> {
        let projects_dir = dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".claude")
            .join("projects");

        // Find the JSONL file matching this session_id
        let mut jsonl_path: Option<PathBuf> = None;
        if projects_dir.exists() {
            for project_entry in std::fs::read_dir(&projects_dir)
                .map_err(|e| format!("Read projects: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
            {
                let candidate = project_entry.path().join(format!("{session_id}.jsonl"));
                if candidate.exists() {
                    jsonl_path = Some(candidate);
                    break;
                }
            }
        }

        let path =
            jsonl_path.ok_or_else(|| format!("JSONL file not found for session {session_id}"))?;

        let messages = extract_messages_from_jsonl(&path);

        // Find the index of the target message
        let target_idx = messages
            .iter()
            .position(|m| m.uuid == message_id)
            .unwrap_or(0);

        let start = target_idx.saturating_sub(window);
        let end = (target_idx + window + 1).min(messages.len());

        let context_messages: Vec<ContextMessage> = messages[start..end]
            .iter()
            .map(|m| ContextMessage {
                uuid: m.uuid.clone(),
                role: m.role.clone(),
                content: m.content.clone(),
                timestamp: m.timestamp.clone(),
                is_target: m.uuid == message_id,
            })
            .collect();

        Ok(SessionContext {
            session_id: session_id.to_string(),
            messages: context_messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Search result types (serializable for frontend)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct SearchHit {
    pub message_id: String,
    pub session_id: String,
    pub content: String,
    pub snippet: String,
    pub role: String,
    pub project: String,
    pub host: String,
    pub timestamp: String,
    pub git_branch: String,
    pub tools_used: String,
    pub files_modified: String,
    pub score: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total: u64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SearchFilters {
    pub query: String,
    pub project: Option<String>,
    pub host: Option<String>,
    pub role: Option<String>,
    pub git_branch: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Serialize, Clone, Debug)]
pub struct FacetCount {
    pub label: String,
    pub count: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchFacets {
    pub projects: Vec<FacetCount>,
    pub hosts: Vec<FacetCount>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ContextMessage {
    pub uuid: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub is_target: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionContext {
    pub session_id: String,
    pub messages: Vec<ContextMessage>,
}

// ---------------------------------------------------------------------------
// Extracted message -- intermediate struct from JSONL parsing
// ---------------------------------------------------------------------------

pub struct ExtractedMessage {
    pub uuid: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub git_branch: String,
    pub tools_used: Vec<String>,
    pub files_modified: Vec<String>,
}

// ---------------------------------------------------------------------------
// JSONL parsing
// ---------------------------------------------------------------------------

/// Extract indexable messages from a Claude Code JSONL session file.
/// Only "user" and "assistant" type messages are extracted.
/// isMeta messages and messages with empty content are skipped.
pub fn extract_messages_from_jsonl(path: &Path) -> Vec<ExtractedMessage> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read JSONL {}: {e}", path.display());
            return Vec::new();
        }
    };

    let mut messages = Vec::new();

    for line in contents.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let obj: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "user" && msg_type != "assistant" {
            continue;
        }

        // Skip isMeta messages
        if obj.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }

        let uuid = obj
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let session_id = obj
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let git_branch = obj
            .get("gitBranch")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let message = match obj.get("message") {
            Some(m) => m,
            None => continue,
        };

        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(msg_type)
            .to_string();

        let content_val = message.get("content");

        let mut text_parts: Vec<String> = Vec::new();
        let mut tools_used: Vec<String> = Vec::new();
        let mut files_modified: Vec<String> = Vec::new();

        match content_val {
            // Content is a plain string
            Some(serde_json::Value::String(s)) => {
                text_parts.push(s.clone());
            }
            // Content is an array of blocks
            Some(serde_json::Value::Array(blocks)) => {
                for block in blocks {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(text.to_string());
                            }
                        }
                        "tool_use" => {
                            // Extract tool name
                            if let Some(name) = block.get("name").and_then(|v| v.as_str()) {
                                tools_used.push(name.to_string());
                            }
                            // Extract file paths from input
                            if let Some(input) = block.get("input").and_then(|v| v.as_object()) {
                                for key in ["file_path", "path", "pattern"] {
                                    if let Some(val) = input.get(key).and_then(|v| v.as_str())
                                        && !val.is_empty()
                                    {
                                        files_modified.push(val.to_string());
                                    }
                                }
                            }
                        }
                        // Skip thinking, tool_result, image blocks
                        "thinking" | "tool_result" | "image" => {}
                        _ => {}
                    }
                }
            }
            _ => continue,
        }

        let content = text_parts.join("\n");
        if content.trim().is_empty() && tools_used.is_empty() {
            continue;
        }

        messages.push(ExtractedMessage {
            uuid,
            session_id,
            role,
            content,
            timestamp,
            git_branch,
            tools_used,
            files_modified,
        });
    }

    messages
}
