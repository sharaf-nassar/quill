use std::collections::HashMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tantivy::collector::{Count, FacetCollector, TopDocs};
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
    pub code_changes: Field,
    pub commands_run: Field,
    pub tool_details: Field,
    pub display_text: Field,
    #[allow(dead_code)]
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
    const SCHEMA_VERSION: u32 = 4;

    /// Open an existing index or create a new one at the given directory.
    pub fn open_or_create(index_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(index_dir)
            .map_err(|e| format!("Failed to create index dir: {e}"))?;

        // Check schema version — rebuild index if schema changed
        let version_path = index_dir.join("schema_version.txt");
        let stored_version: u32 = std::fs::read_to_string(&version_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);

        if stored_version < Self::SCHEMA_VERSION {
            log::info!(
                "Schema version mismatch ({stored_version} < {}), rebuilding index",
                Self::SCHEMA_VERSION
            );
            // Remove entire index directory and recreate it (handles files + subdirectories)
            let _ = std::fs::remove_dir_all(index_dir);
            std::fs::create_dir_all(index_dir)
                .map_err(|e| format!("Failed to recreate index dir: {e}"))?;
        }

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

        let _ = std::fs::write(&version_path, Self::SCHEMA_VERSION.to_string());

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
        let code_changes = builder.add_text_field("code_changes", TEXT | STORED);
        let commands_run = builder.add_text_field("commands_run", TEXT | STORED);
        let tool_details = builder.add_text_field("tool_details", TEXT | STORED);
        let display_text = builder.add_text_field("display_text", TEXT | STORED);

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
            code_changes,
            commands_run,
            tool_details,
            display_text,
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
    ///
    /// Claude Code encodes CWD paths by replacing `/` (and `.`) with `-`, so
    /// `-home-mamba-work-claude-usage` represents `/home/mamba/work/claude-usage`.
    /// The encoding is lossy — a literal hyphen in a directory name is
    /// indistinguishable from a path separator.
    ///
    /// We recover the real path by greedily walking the filesystem: at each
    /// level we try the longest candidate that exists, which correctly
    /// preserves names like `claude-usage` and `nasha-lab`.
    ///
    /// Falls back to the last `-`-delimited segment if the path can't be
    /// resolved (e.g. the directory was deleted).
    pub fn project_display_name(dir_name: &str) -> String {
        // Strip the leading `-` which represents the root `/`
        let remaining = dir_name.strip_prefix('-').unwrap_or(dir_name);
        if remaining.is_empty() {
            return dir_name.to_string();
        }

        let segments: Vec<&str> = remaining.split('-').collect();
        let mut path = std::path::PathBuf::from("/");
        let mut i = 0;

        while i < segments.len() {
            // Greedy: try the longest possible component first
            let mut matched = false;
            for end in (i + 1..=segments.len()).rev() {
                let candidate = segments[i..end].join("-");
                let try_path = path.join(&candidate);
                if try_path.exists() {
                    path = try_path;
                    i = end;
                    matched = true;
                    break;
                }
            }
            if !matched {
                // No filesystem match — append remaining segments as one component
                let rest = segments[i..].join("-");
                path.push(&rest);
                break;
            }
        }

        // Return the last component of the recovered path
        path.file_name()
            .and_then(|n| n.to_str())
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
        doc.add_text(self.fields.code_changes, msg.code_changes.join("\n"));
        doc.add_text(self.fields.commands_run, msg.commands_run.join("\n"));
        doc.add_text(self.fields.tool_details, msg.tool_details.join("\n"));

        // Compose display_text: text content + tool summaries
        let mut display_parts: Vec<String> = Vec::new();
        if !msg.content.is_empty() {
            display_parts.push(truncate(&msg.content, 500));
        }
        for change in &msg.code_changes {
            display_parts.push(change.clone());
        }
        for cmd in &msg.commands_run {
            display_parts.push(cmd.clone());
        }
        for detail in &msg.tool_details {
            display_parts.push(detail.clone());
        }
        let display_text = truncate(&display_parts.join("\n"), 2000);
        doc.add_text(self.fields.display_text, &display_text);

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
    pub fn startup_scan(
        &self,
        app_handle: &tauri::AppHandle,
        storage: Option<&crate::storage::Storage>,
    ) -> Result<usize, String> {
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
            .or_else(|_| {
                // /etc/hostname exists on Linux; on macOS use `hostname` command
                std::fs::read_to_string("/etc/hostname")
                    .map(|s| s.trim().to_string())
                    .or_else(|_| {
                        std::process::Command::new("hostname")
                            .output()
                            .map_err(|e| e.to_string())
                            .and_then(|o| {
                                String::from_utf8(o.stdout)
                                    .map(|s| s.trim().to_string())
                                    .map_err(|e| e.to_string())
                            })
                    })
            })
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

                // If file was previously indexed but modified, delete old docs + tool_actions
                if known_mtime.is_some()
                    && let Some(session_id) = file_path.file_stem().and_then(|s| s.to_str())
                {
                    let term = Term::from_field_text(self.fields.session_id, session_id);
                    let writer = self.writer.lock();
                    writer.delete_term(term);
                    // Also clear old tool_actions to prevent duplicates
                    if let Some(storage) = storage
                        && let Err(e) = storage.delete_tool_actions_for_session(session_id)
                    {
                        log::warn!("Failed to delete old tool_actions: {e}");
                    }
                }

                // Index messages from this file
                let messages = extract_messages_from_jsonl(&file_path);
                for msg in &messages {
                    if let Err(e) = self.index_message(msg, &project_name, &hostname) {
                        log::warn!("Failed to index message: {e}");
                    }
                    // Store tool actions in SQLite
                    if !msg.tool_actions.is_empty()
                        && let Some(storage) = storage
                        && let Err(e) = storage.store_tool_actions(
                            &msg.tool_actions,
                            &msg.uuid,
                            &msg.session_id,
                        )
                    {
                        log::warn!("Failed to store tool actions: {e}");
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
    pub fn search(
        &self,
        query: &str,
        filters: &SearchFilters,
        sort_by: &str,
        page: usize,
        page_size: usize,
    ) -> Result<SearchResults, String> {
        let start = std::time::Instant::now();
        let searcher = self.searcher();
        let f = &self.fields;

        // Build query parser targeting content, tools_used, files_modified, and new fields
        let mut parser = QueryParser::for_index(
            &self.index,
            vec![
                f.content,
                f.tools_used,
                f.files_modified,
                f.code_changes,
                f.commands_run,
                f.tool_details,
                f.display_text,
            ],
        );
        parser.set_conjunction_by_default();

        let text_query: Box<dyn tantivy::query::Query> = if query.trim().is_empty() {
            Box::new(tantivy::query::AllQuery)
        } else {
            let (parsed, _errors) = parser.parse_query_lenient(query);
            parsed
        };

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

        // Session ID filter
        if let Some(ref sid) = filters.session_id {
            let term = Term::from_field_text(f.session_id, sid);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        // Date range filter
        if filters.date_from.is_some() || filters.date_to.is_some() {
            let parse_date = |s: &str| -> Option<DateTime> {
                // Try RFC3339 first, then plain date
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                    .ok()
                    .or_else(|| {
                        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                            .map(|d| {
                                DateTime::from_timestamp_secs(
                                    d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
                                )
                            })
                            .ok()
                    })
            };

            let lower = match &filters.date_from {
                Some(from_str) => {
                    let dt = parse_date(from_str).unwrap_or(DateTime::MIN);
                    Bound::Included(Term::from_field_date(f.timestamp, dt))
                }
                None => Bound::Unbounded,
            };
            let upper = match &filters.date_to {
                Some(to_str) => {
                    let dt = parse_date(to_str).unwrap_or(DateTime::MAX);
                    Bound::Included(Term::from_field_date(f.timestamp, dt))
                }
                None => Bound::Unbounded,
            };
            clauses.push((Occur::Must, Box::new(RangeQuery::new(lower, upper))));
        }

        let combined = BooleanQuery::new(clauses);
        let limit = page_size.min(100);
        let offset = page * page_size;

        let (doc_addresses, total_count): (Vec<(f32, tantivy::DocAddress)>, usize) =
            if sort_by == "recency" {
                let (top_docs, count) = searcher
                    .search(
                        &combined,
                        &(
                            TopDocs::with_limit(limit)
                                .and_offset(offset)
                                .order_by_fast_field::<DateTime>("timestamp", tantivy::Order::Desc),
                            Count,
                        ),
                    )
                    .map_err(|e| format!("Search error: {e}"))?;
                let addrs = top_docs
                    .into_iter()
                    .map(|(_, addr)| (0.0f32, addr))
                    .collect();
                (addrs, count)
            } else {
                let (top_docs, count) = searcher
                    .search(
                        &combined,
                        &(TopDocs::with_limit(limit).and_offset(offset), Count),
                    )
                    .map_err(|e| format!("Search error: {e}"))?;
                (top_docs, count)
            };

        // Snippet generator for display_text field
        let snippet_gen = SnippetGenerator::create(&searcher, &combined, f.display_text)
            .map_err(|e| format!("Snippet generator error: {e}"))?;

        let mut hits = Vec::with_capacity(doc_addresses.len());
        for (score, doc_addr) in &doc_addresses {
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
                code_changes: get_text(f.code_changes),
                commands_run: get_text(f.commands_run),
                tool_details: get_text(f.tool_details),
                score: *score,
            });
        }

        Ok(SearchResults {
            hits,
            total_hits: total_count as u64,
            query_time_ms: start.elapsed().as_millis() as u64,
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
                name: facet
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
                name: facet
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
        let mut project_name = String::new();
        if projects_dir.exists() {
            for project_entry in std::fs::read_dir(&projects_dir)
                .map_err(|e| format!("Read projects: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
            {
                let candidate = project_entry.path().join(format!("{session_id}.jsonl"));
                if candidate.exists() {
                    jsonl_path = Some(candidate);
                    let dir_name = project_entry
                        .file_name()
                        .to_str()
                        .unwrap_or("unknown")
                        .to_string();
                    project_name = Self::project_display_name(&dir_name);
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
            .map(|m| {
                let tool_summary = m
                    .code_changes
                    .iter()
                    .chain(m.commands_run.iter())
                    .chain(m.tool_details.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");

                ContextMessage {
                    message_id: m.uuid.clone(),
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_summary,
                    tools_used: m.tools_used.join(" "),
                    timestamp: m.timestamp.clone(),
                    is_match: m.uuid == message_id,
                }
            })
            .collect();

        Ok(SessionContext {
            session_id: session_id.to_string(),
            project: project_name,
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
    pub code_changes: String,
    pub commands_run: String,
    pub tool_details: String,
    pub score: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total_hits: u64,
    pub query_time_ms: u64,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct SearchFilters {
    pub project: Option<String>,
    pub host: Option<String>,
    pub role: Option<String>,
    pub git_branch: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct FacetCount {
    pub name: String,
    pub count: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchFacets {
    pub projects: Vec<FacetCount>,
    pub hosts: Vec<FacetCount>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ContextMessage {
    pub message_id: String,
    pub role: String,
    pub content: String,
    pub tool_summary: String,
    pub tools_used: String,
    pub timestamp: String,
    pub is_match: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionContext {
    pub session_id: String,
    pub project: String,
    pub messages: Vec<ContextMessage>,
}

// ---------------------------------------------------------------------------
// Extracted message -- intermediate struct from JSONL parsing
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct ToolAction {
    pub tool_use_id: String,
    pub tool_name: String,
    pub category: String, // "code_change", "command", "tool_detail"
    pub file_path: Option<String>,
    pub summary: String,
    pub full_input: Option<String>,  // JSON string, max 10KB
    pub full_output: Option<String>, // JSON string, max 10KB, set later from tool_result
    pub timestamp: String,
}

pub struct ExtractedMessage {
    pub uuid: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub git_branch: String,
    pub tools_used: Vec<String>,
    pub files_modified: Vec<String>,
    // New fields for tool data summaries
    pub code_changes: Vec<String>,
    pub commands_run: Vec<String>,
    pub tool_details: Vec<String>,
    // Tool actions for SQLite storage
    #[allow(dead_code)]
    pub tool_actions: Vec<ToolAction>,
}

// ---------------------------------------------------------------------------
// JSONL parsing
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find the last char boundary at or before max_len to avoid panic on multi-byte UTF-8
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i <= max_len)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("{}... [truncated]", &s[..boundary])
    }
}

/// Build a human-readable summary for a tool invocation.
/// Returns (category, summary, file_path).
fn build_tool_summary(
    tool_name: &str,
    input: Option<&serde_json::Value>,
) -> (String, String, Option<String>) {
    let inp = input.and_then(|v| v.as_object());

    let get_str = |key: &str| -> String {
        inp.and_then(|o| o.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    match tool_name {
        "Edit" => {
            let file_path = get_str("file_path");
            let old = truncate(&get_str("old_string"), 80);
            let new = truncate(&get_str("new_string"), 80);
            let summary = format!("Edit {file_path}: \"{old}\" -> \"{new}\"");
            ("code_change".to_string(), summary, Some(file_path))
        }
        "Write" => {
            let file_path = get_str("file_path");
            let content_preview = truncate(&get_str("content"), 120);
            let summary = format!("Write {file_path}: {content_preview}");
            ("code_change".to_string(), summary, Some(file_path))
        }
        "Bash" => {
            let command = get_str("command");
            let summary = format!("$ {command}");
            ("command".to_string(), summary, None)
        }
        "Read" => {
            let file_path = get_str("file_path");
            let summary = format!("Read {file_path}");
            ("tool_detail".to_string(), summary, Some(file_path))
        }
        "Grep" => {
            let pattern = get_str("pattern");
            let path = get_str("path");
            let glob = get_str("glob");
            let target = if !path.is_empty() { path } else { glob };
            let summary = format!("Grep \"{pattern}\" in {target}");
            ("tool_detail".to_string(), summary, None)
        }
        "Glob" => {
            let pattern = get_str("pattern");
            let summary = format!("Glob \"{pattern}\"");
            ("tool_detail".to_string(), summary, None)
        }
        "Agent" => {
            let prompt = truncate(&get_str("prompt"), 120);
            let summary = format!("Agent: {prompt}");
            ("tool_detail".to_string(), summary, None)
        }
        _ => {
            let summary = tool_name.to_string();
            ("tool_detail".to_string(), summary, None)
        }
    }
}

#[allow(dead_code)]
struct ToolUseEntry {
    tool_name: String,
    category: String,
    file_path: Option<String>,
    summary: String,
    full_input: Option<String>,
    timestamp: String,
    // Index into messages vec where this tool_use appeared
    message_idx: usize,
}

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

    let mut messages: Vec<ExtractedMessage> = Vec::new();
    // Maps tool_use block id -> entry for cross-message correlation
    let mut tool_use_map: HashMap<String, ToolUseEntry> = HashMap::new();

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
        let mut code_changes: Vec<String> = Vec::new();
        let mut commands_run: Vec<String> = Vec::new();
        let mut tool_details_vec: Vec<String> = Vec::new();
        let mut tool_actions: Vec<ToolAction> = Vec::new();

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
                            let tool_id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let input = block.get("input");

                            if !name.is_empty() {
                                tools_used.push(name.clone());
                            }

                            // Extract file paths from input
                            if let Some(inp) = input.and_then(|v| v.as_object()) {
                                for key in ["file_path", "path", "pattern"] {
                                    if let Some(val) = inp.get(key).and_then(|v| v.as_str())
                                        && !val.is_empty()
                                    {
                                        files_modified.push(val.to_string());
                                    }
                                }
                            }

                            // Build summary and categorize
                            let (category, summary, file_path) = build_tool_summary(&name, input);

                            match category.as_str() {
                                "code_change" => code_changes.push(summary.clone()),
                                "command" => commands_run.push(summary.clone()),
                                "tool_detail" => tool_details_vec.push(summary.clone()),
                                _ => {}
                            }

                            // Serialize full input (capped at 10KB)
                            let full_input = input.map(|v| {
                                let s = v.to_string();
                                truncate(&s, 10240)
                            });

                            // Store in map for later correlation with tool_result
                            let action = ToolAction {
                                tool_use_id: tool_id.clone(),
                                tool_name: name.clone(),
                                category: category.clone(),
                                file_path: file_path.clone(),
                                summary: summary.clone(),
                                full_input: full_input.clone(),
                                full_output: None,
                                timestamp: timestamp.clone(),
                            };
                            tool_actions.push(action);

                            if !tool_id.is_empty() {
                                tool_use_map.insert(
                                    tool_id,
                                    ToolUseEntry {
                                        tool_name: name,
                                        category,
                                        file_path,
                                        summary,
                                        full_input,
                                        timestamp: timestamp.clone(),
                                        message_idx: messages.len(),
                                    },
                                );
                            }
                        }
                        "tool_result" => {
                            let tool_use_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            // Capture output content
                            let output_content = block.get("content").map(|v| {
                                let s = match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    serde_json::Value::Array(arr) => arr
                                        .iter()
                                        .filter_map(|item| {
                                            if item.get("type").and_then(|t| t.as_str())
                                                == Some("text")
                                            {
                                                item.get("text")
                                                    .and_then(|t| t.as_str())
                                                    .map(|s| s.to_string())
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    _ => v.to_string(),
                                };
                                truncate(&s, 10240)
                            });

                            // Correlate with the original tool_use
                            if let Some(entry) = tool_use_map.get_mut(&tool_use_id) {
                                // Update the ToolAction in the original message
                                if entry.message_idx < messages.len()
                                    && let Some(action) = messages[entry.message_idx]
                                        .tool_actions
                                        .iter_mut()
                                        .find(|a| a.tool_use_id == tool_use_id)
                                {
                                    action.full_output = output_content.clone();
                                }

                                // For Bash commands, append truncated output to commands_run summary
                                if entry.tool_name == "Bash"
                                    && let Some(ref output) = output_content
                                {
                                    let output_preview = truncate(output, 300);
                                    let enhanced = format!("{}\n{}", entry.summary, output_preview);
                                    // Update the summary in the original message's commands_run
                                    if entry.message_idx < messages.len()
                                        && let Some(cmd) = messages[entry.message_idx]
                                            .commands_run
                                            .iter_mut()
                                            .find(|c: &&mut String| c.starts_with(&entry.summary))
                                    {
                                        *cmd = enhanced;
                                    }
                                }
                            }
                        }
                        // Skip thinking, image blocks
                        "thinking" | "image" => {}
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
            code_changes,
            commands_run,
            tool_details: tool_details_vec,
            tool_actions,
        });
    }

    messages
}

// ---------------------------------------------------------------------------
// Tauri state wrapper and commands
// ---------------------------------------------------------------------------

/// Wrapper for managed Tauri state.
pub struct SessionIndexState(pub Arc<SessionIndex>);

#[tauri::command]
pub async fn search_sessions(
    query: String,
    filters: SearchFilters,
    sort_by: Option<String>,
    page: usize,
    page_size: usize,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SearchResults, String> {
    let idx = state.0.clone();
    let sort = sort_by.unwrap_or_else(|| "relevance".to_string());
    crate::run_blocking(move || idx.search(&query, &filters, &sort, page, page_size))
}

#[tauri::command]
pub async fn get_session_context(
    session_id: String,
    around_message_id: String,
    window: Option<u32>,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SessionContext, String> {
    let idx = state.0.clone();
    let w = window.unwrap_or(5) as usize;
    crate::run_blocking(move || idx.get_context(&session_id, &around_message_id, w))
}

#[tauri::command]
pub async fn get_search_facets(
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SearchFacets, String> {
    let idx = state.0.clone();
    crate::run_blocking(move || idx.get_facets())
}

#[tauri::command]
pub async fn rebuild_search_index(
    app: tauri::AppHandle,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<usize, String> {
    let idx = state.0.clone();
    let storage = crate::STORAGE.get();
    crate::run_blocking(move || idx.startup_scan(&app, storage))
}
