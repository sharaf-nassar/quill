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

use crate::integrations::IntegrationProvider;

// ---------------------------------------------------------------------------
// Schema fields wrapper
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SessionSchema {
    pub provider: Field,
    pub message_id: Field,
    pub session_id: Field,
    pub content: Field,
    pub role: Field,
    pub project: Field,
    pub host: Field,
    pub provider_facet: Field,
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
    const SCHEMA_VERSION: u32 = 5;

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
        let provider = builder.add_text_field("provider", STRING | STORED);
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
        let provider_facet = builder.add_facet_field("provider_facet", FacetOptions::default());

        // Date field (indexed, stored, fast)
        let date_opts = DateOptions::from(INDEXED)
            .set_stored()
            .set_fast()
            .set_precision(DateTimePrecision::Seconds);
        let timestamp = builder.add_date_field("timestamp", date_opts);

        let schema = builder.build();

        let fields = SessionSchema {
            provider,
            message_id,
            session_id,
            content,
            role,
            project,
            host,
            provider_facet,
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

    fn build_index_document(
        &self,
        provider: IntegrationProvider,
        msg: &ExtractedMessage,
        project_facet: &str,
        host_facet: &str,
    ) -> TantivyDocument {
        let mut doc = TantivyDocument::default();

        doc.add_text(self.fields.provider, provider.as_str());
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
        doc.add_facet(
            self.fields.provider_facet,
            Facet::from(&format!("/{}", provider.as_str())),
        );

        // Parse timestamp as RFC3339 -> tantivy DateTime
        let ts = if !msg.timestamp.is_empty() {
            chrono::DateTime::parse_from_rfc3339(&msg.timestamp)
                .map(|dt| DateTime::from_timestamp_secs(dt.timestamp()))
                .unwrap_or(DateTime::from_timestamp_secs(0))
        } else {
            DateTime::from_timestamp_secs(0)
        };
        doc.add_date(self.fields.timestamp, ts);

        doc
    }

    fn add_message_to_writer(
        &self,
        writer: &IndexWriter,
        provider: IntegrationProvider,
        msg: &ExtractedMessage,
        project_facet: &str,
        host_facet: &str,
    ) -> Result<(), String> {
        let doc = self.build_index_document(provider, msg, project_facet, host_facet);
        writer
            .add_document(doc)
            .map_err(|e| format!("Add document: {e}"))?;
        Ok(())
    }

    fn delete_session_docs_with_writer(
        &self,
        writer: &IndexWriter,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let provider_term = Term::from_field_text(self.fields.provider, provider.as_str());
        let session_term = Term::from_field_text(self.fields.session_id, session_id);
        let delete_query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(provider_term, IndexRecordOption::Basic)),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(session_term, IndexRecordOption::Basic)),
            ),
        ]);

        writer
            .delete_query(Box::new(delete_query))
            .map(|_| ())
            .map_err(|e| format!("Delete session docs: {e}"))
    }

    pub(crate) fn replace_session_docs_batch(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        project_facet: &str,
        host_facet: &str,
        messages: &[ExtractedMessage],
    ) -> Result<usize, String> {
        let mut writer = self.writer.lock();
        self.delete_session_docs_with_writer(&writer, provider, session_id)?;
        for msg in messages {
            self.add_message_to_writer(&writer, provider, msg, project_facet, host_facet)?;
        }
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(messages.len())
    }

    pub(crate) fn append_messages_batch(
        &self,
        provider: IntegrationProvider,
        project_facet: &str,
        host_facet: &str,
        messages: &[ExtractedMessage],
    ) -> Result<usize, String> {
        let mut writer = self.writer.lock();
        for msg in messages {
            self.add_message_to_writer(&writer, provider, msg, project_facet, host_facet)?;
        }
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(messages.len())
    }

    fn local_hostname() -> String {
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .or_else(|_| {
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
            .unwrap_or_else(|_| "unknown".to_string())
    }

    fn discover_session_files() -> Result<Vec<DiscoveredSessionFile>, String> {
        let mut files = Self::discover_claude_session_files()?;
        files.extend(Self::discover_codex_session_files()?);
        Ok(files)
    }

    fn discover_claude_session_files() -> Result<Vec<DiscoveredSessionFile>, String> {
        let projects_dir = dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".claude")
            .join("projects");

        if !projects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for project_entry in std::fs::read_dir(&projects_dir)
            .map_err(|e| format!("Read projects dir: {e}"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
        {
            let project_dir = project_entry.path();
            let project_dir_name = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let project_name = Self::project_display_name(project_dir_name);

            for entry in std::fs::read_dir(&project_dir)
                .map_err(|e| format!("Read project dir: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            {
                files.push(DiscoveredSessionFile {
                    provider: IntegrationProvider::Claude,
                    path: entry.path(),
                    default_project: project_name.clone(),
                });
            }
        }

        Ok(files)
    }

    fn discover_codex_session_files() -> Result<Vec<DiscoveredSessionFile>, String> {
        let sessions_dir = dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".codex")
            .join("sessions");

        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&sessions_dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        {
            files.push(DiscoveredSessionFile {
                provider: IntegrationProvider::Codex,
                path: entry.path().to_path_buf(),
                default_project: "unknown".to_string(),
            });
        }

        Ok(files)
    }

    /// Scan Claude and Codex session JSONL files and index new/modified files.
    /// Returns the number of newly indexed messages.
    pub fn startup_scan(
        &self,
        app_handle: &tauri::AppHandle,
        storage: Option<&crate::storage::Storage>,
    ) -> Result<usize, String> {
        use tauri::Emitter;

        let mut total_indexed = 0usize;
        let mut index_changed = false;
        let mut state = self.state.lock();
        let hostname = Self::local_hostname();
        let mut writer = self.writer.lock();

        for discovered in Self::discover_session_files()? {
            let file_key = discovered.path.to_string_lossy().to_string();
            let mtime = std::fs::metadata(&discovered.path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let known_mtime = state.file_mtimes.get(&file_key).copied();
            if known_mtime == Some(mtime) {
                continue;
            }

            let extracted = extract_messages_from_jsonl(discovered.provider, &discovered.path);
            let project_name = extracted
                .project_name
                .clone()
                .filter(|project| !project.is_empty())
                .unwrap_or_else(|| discovered.default_project.clone());

            // Always delete-then-reinsert per session, even on first sight of a
            // file. Hook-driven /sessions/notify may have already indexed this
            // session before the mtime sweep first sees the file; gating delete
            // on known_mtime would let those docs stack up on top of fresh
            // inserts. delete_query is a no-op when no docs match, so this is
            // safe for genuinely new files.
            if !extracted.session_id.is_empty() {
                if let Err(e) = self.delete_session_docs_with_writer(
                    &writer,
                    discovered.provider,
                    &extracted.session_id,
                ) {
                    log::warn!("Failed to delete old session docs: {e}");
                } else {
                    index_changed = true;
                }

                if let Some(storage) = storage {
                    if let Err(e) = storage
                        .delete_tool_actions_for_session(discovered.provider, &extracted.session_id)
                    {
                        log::warn!("Failed to delete old tool_actions: {e}");
                    }
                    if let Err(e) = storage.delete_response_times_for_session(
                        discovered.provider,
                        &extracted.session_id,
                    ) {
                        log::warn!("Failed to delete old response_times: {e}");
                    }
                }
            }

            for msg in &extracted.messages {
                if let Err(e) = self.add_message_to_writer(
                    &writer,
                    discovered.provider,
                    msg,
                    &project_name,
                    &hostname,
                ) {
                    log::warn!("Failed to index message: {e}");
                } else {
                    index_changed = true;
                }
            }

            if let Some(storage) = storage
                && let Err(e) = storage
                    .store_tool_actions_for_messages(discovered.provider, &extracted.messages)
            {
                log::warn!("Failed to store tool actions: {e}");
            }

            if let Some(storage) = storage
                && !extracted.session_id.is_empty()
                && !extracted.messages.is_empty()
            {
                let rt_pairs: Vec<(&str, &str)> = extracted
                    .messages
                    .iter()
                    .map(|msg| (msg.role.as_str(), msg.timestamp.as_str()))
                    .collect();
                if let Err(e) = storage.ingest_response_times(
                    discovered.provider,
                    &extracted.session_id,
                    &rt_pairs,
                ) {
                    log::warn!("Failed to store response times: {e}");
                }
            }

            total_indexed += extracted.messages.len();
            state.file_mtimes.insert(file_key, mtime);
        }

        // Commit all changes
        if index_changed {
            writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        }

        drop(writer);

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
        // Boost concrete artifact fields so they outrank prose noise; equal
        // weighting plus BM25 length-normalization otherwise lets long fields
        // smother short selective ones. display_text is a derived superset of
        // content+code_changes+commands_run+tool_details — kept in the field
        // set with a tiny boost only so SnippetGenerator (which filters terms
        // by field) can still highlight matches against it.
        parser.set_field_boost(f.files_modified, 4.0);
        parser.set_field_boost(f.code_changes, 2.5);
        parser.set_field_boost(f.commands_run, 2.5);
        parser.set_field_boost(f.tool_details, 1.5);
        parser.set_field_boost(f.content, 1.0);
        parser.set_field_boost(f.tools_used, 0.5);
        parser.set_field_boost(f.display_text, 0.1);

        let text_query: Box<dyn tantivy::query::Query> = if query.trim().is_empty() {
            Box::new(tantivy::query::AllQuery)
        } else {
            let (parsed, errors) = parser.parse_query_lenient(query);
            if !errors.is_empty() {
                log::debug!("Session search parse errors for {query:?}: {errors:?}");
            }
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

        if let Some(provider) = filters.provider {
            let term = Term::from_field_text(f.provider, provider.as_str());
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
                provider: get_text(f.provider)
                    .parse()
                    .unwrap_or(IntegrationProvider::Claude),
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

    /// Collect distinct provider, project, and host facets from the index.
    pub fn get_facets(&self) -> Result<SearchFacets, String> {
        let searcher = self.searcher();

        let mut project_collector = FacetCollector::for_field("project");
        project_collector.add_facet(Facet::root());

        let mut host_collector = FacetCollector::for_field("host");
        host_collector.add_facet(Facet::root());

        let mut provider_collector = FacetCollector::for_field("provider_facet");
        provider_collector.add_facet(Facet::root());

        let (project_counts, host_counts, provider_counts) = searcher
            .search(
                &tantivy::query::AllQuery,
                &(project_collector, host_collector, provider_collector),
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

        let providers = provider_counts
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

        Ok(SearchFacets {
            providers,
            projects,
            hosts,
        })
    }

    // -------------------------------------------------------------------
    // Context -- surrounding messages for a search hit
    // -------------------------------------------------------------------

    /// Find the JSONL file for a session and return a window of messages
    /// around the target message.
    pub fn get_context(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        message_id: &str,
        window: usize,
    ) -> Result<SessionContext, String> {
        let path = find_session_path(provider, session_id)?
            .ok_or_else(|| format!("JSONL file not found for session {session_id}"))?;
        let extracted = extract_messages_from_jsonl(provider, &path);
        let project_name = extracted.project_name.unwrap_or_default();
        let messages = extracted.messages;

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
            provider,
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
    pub provider: IntegrationProvider,
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
    pub provider: Option<IntegrationProvider>,
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
    pub providers: Vec<FacetCount>,
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
    pub provider: IntegrationProvider,
    pub session_id: String,
    pub project: String,
    pub messages: Vec<ContextMessage>,
}

struct DiscoveredSessionFile {
    provider: IntegrationProvider,
    path: PathBuf,
    default_project: String,
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

pub struct ExtractedSession {
    pub session_id: String,
    pub project_name: Option<String>,
    pub messages: Vec<ExtractedMessage>,
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

/// Build a human-readable summary for a Claude tool invocation.
/// Returns (category, summary, file_path).
fn build_claude_tool_summary(
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

fn build_codex_function_tool_summary(
    tool_name: &str,
    arguments: &str,
) -> (String, String, Option<String>) {
    let parsed: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let input = parsed.as_object();

    let get_str = |key: &str| -> String {
        input
            .and_then(|obj| obj.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };

    match tool_name {
        "exec_command" => {
            let command = get_str("cmd");
            ("command".to_string(), format!("$ {command}"), None)
        }
        "write_stdin" => {
            let chars = truncate(&get_str("chars"), 120);
            ("command".to_string(), format!("stdin {chars}"), None)
        }
        _ if tool_name.starts_with("mcp__") => {
            let detail = ["query", "text", "path", "uri", "url"]
                .into_iter()
                .map(&get_str)
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let summary = if detail.is_empty() {
                tool_name.to_string()
            } else {
                format!("{tool_name}: {}", truncate(&detail, 120))
            };
            let file_path = ["file_path", "path"]
                .into_iter()
                .map(&get_str)
                .find(|value| !value.is_empty());
            ("tool_detail".to_string(), summary, file_path)
        }
        _ => {
            let detail = ["path", "file_path", "workdir", "query", "text"]
                .into_iter()
                .map(&get_str)
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let summary = if detail.is_empty() {
                tool_name.to_string()
            } else {
                format!("{tool_name}: {}", truncate(&detail, 120))
            };
            let file_path = ["file_path", "path"]
                .into_iter()
                .map(get_str)
                .find(|value| !value.is_empty());
            ("tool_detail".to_string(), summary, file_path)
        }
    }
}

fn build_codex_custom_tool_summary(
    tool_name: &str,
    input: &str,
) -> (String, String, Option<String>) {
    match tool_name {
        "apply_patch" => {
            let files = extract_apply_patch_files(input);
            let file_path = files.first().cloned();
            let summary = if files.is_empty() {
                "Patch".to_string()
            } else {
                format!("Patch {}", truncate(&files.join(", "), 160))
            };
            ("code_change".to_string(), summary, file_path)
        }
        _ => (
            "tool_detail".to_string(),
            format!("{tool_name}: {}", truncate(input, 120)),
            None,
        ),
    }
}

fn extract_apply_patch_files(patch: &str) -> Vec<String> {
    let mut files = Vec::new();

    for line in patch.lines() {
        for prefix in ["*** Update File: ", "*** Add File: ", "*** Delete File: "] {
            if let Some(path) = line.strip_prefix(prefix) {
                files.push(path.to_string());
            }
        }
    }

    files
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
}

fn make_tool_message(
    uuid: String,
    session_id: String,
    git_branch: String,
    action: ToolAction,
) -> ExtractedMessage {
    let mut code_changes = Vec::new();
    let mut commands_run = Vec::new();
    let mut tool_details = Vec::new();

    match action.category.as_str() {
        "code_change" => code_changes.push(action.summary.clone()),
        "command" => commands_run.push(action.summary.clone()),
        _ => tool_details.push(action.summary.clone()),
    }

    let files_modified = action.file_path.clone().into_iter().collect();
    let timestamp = action.timestamp.clone();
    let tool_name = action.tool_name.clone();

    ExtractedMessage {
        uuid,
        session_id,
        role: "assistant".to_string(),
        content: String::new(),
        timestamp: timestamp.clone(),
        git_branch,
        tools_used: vec![tool_name.clone()],
        files_modified,
        code_changes,
        commands_run,
        tool_details,
        tool_actions: vec![action],
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

/// Extract indexable messages from a provider session transcript.
pub fn extract_messages_from_jsonl(provider: IntegrationProvider, path: &Path) -> ExtractedSession {
    match provider {
        IntegrationProvider::Claude => extract_claude_messages_from_jsonl(path),
        IntegrationProvider::Codex => extract_codex_messages_from_jsonl(path),
        IntegrationProvider::MiniMax => extract_claude_messages_from_jsonl(path),
    }
}

/// Extract indexable messages from a Claude Code JSONL session file.
/// Only "user" and "assistant" type messages are extracted.
/// isMeta messages and messages with empty content are skipped.
fn extract_claude_messages_from_jsonl(path: &Path) -> ExtractedSession {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read JSONL {}: {e}", path.display());
            return ExtractedSession {
                session_id: path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string(),
                project_name: path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    .map(SessionIndex::project_display_name),
                messages: Vec::new(),
            };
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
                            let (category, summary, file_path) =
                                build_claude_tool_summary(&name, input);

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

    ExtractedSession {
        session_id: messages
            .first()
            .map(|message| message.session_id.clone())
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(|stem| stem.to_string())
            })
            .unwrap_or_default(),
        project_name: path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .map(SessionIndex::project_display_name),
        messages,
    }
}

fn extract_codex_messages_from_jsonl(path: &Path) -> ExtractedSession {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read JSONL {}: {e}", path.display());
            return ExtractedSession {
                session_id: String::new(),
                project_name: None,
                messages: Vec::new(),
            };
        }
    };

    let mut messages: Vec<ExtractedMessage> = Vec::new();
    let mut tool_use_map: HashMap<String, ToolUseEntry> = HashMap::new();
    let mut session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit('-').next())
        .unwrap_or_default()
        .to_string();
    let mut cwd: Option<String> = None;
    let mut git_branch = String::new();

    for (line_idx, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let obj: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let timestamp = obj
            .get("timestamp")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();

        match obj
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
        {
            "session_meta" => {
                let Some(payload) = obj.get("payload") else {
                    continue;
                };
                if let Some(id) = payload.get("id").and_then(|value| value.as_str()) {
                    session_id = id.to_string();
                }
                cwd = payload
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string());
                git_branch = payload
                    .get("git")
                    .and_then(|value| value.get("branch"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
            }
            "event_msg" => {
                let Some(payload) = obj.get("payload") else {
                    continue;
                };
                let event_type = payload
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let role = match event_type {
                    "user_message" => "user",
                    "agent_message" => "assistant",
                    _ => continue,
                };
                let content = payload
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                if content.trim().is_empty() {
                    continue;
                }
                messages.push(ExtractedMessage {
                    uuid: format!("{session_id}:event:{line_idx}"),
                    session_id: session_id.clone(),
                    role: role.to_string(),
                    content,
                    timestamp,
                    git_branch: git_branch.clone(),
                    tools_used: Vec::new(),
                    files_modified: Vec::new(),
                    code_changes: Vec::new(),
                    commands_run: Vec::new(),
                    tool_details: Vec::new(),
                    tool_actions: Vec::new(),
                });
            }
            "response_item" => {
                let Some(payload) = obj.get("payload") else {
                    continue;
                };
                match payload
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                {
                    "function_call" => {
                        let name = payload
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = payload
                            .get("arguments")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let call_id = payload
                            .get("call_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if name.is_empty() {
                            continue;
                        }

                        let (category, summary, file_path) =
                            build_codex_function_tool_summary(&name, &arguments);
                        let tool_use_id = if call_id.is_empty() {
                            format!("{session_id}:tool:{line_idx}")
                        } else {
                            call_id.clone()
                        };
                        let action = ToolAction {
                            tool_use_id: tool_use_id.clone(),
                            tool_name: name.clone(),
                            category: category.clone(),
                            file_path: file_path.clone(),
                            summary: summary.clone(),
                            full_input: Some(truncate(&arguments, 10240)),
                            full_output: None,
                            timestamp: timestamp.clone(),
                        };
                        let message_idx = messages.len();
                        messages.push(make_tool_message(
                            format!("{session_id}:tool:{line_idx}"),
                            session_id.clone(),
                            git_branch.clone(),
                            action,
                        ));
                        if !call_id.is_empty() {
                            tool_use_map.insert(
                                call_id,
                                ToolUseEntry {
                                    tool_name: name,
                                    category,
                                    file_path,
                                    summary,
                                    full_input: Some(truncate(&arguments, 10240)),
                                    timestamp,
                                    message_idx,
                                },
                            );
                        }
                    }
                    "custom_tool_call" => {
                        let name = payload
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input = payload
                            .get("input")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let call_id = payload
                            .get("call_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if name.is_empty() {
                            continue;
                        }

                        let (category, summary, file_path) =
                            build_codex_custom_tool_summary(&name, &input);
                        let tool_use_id = if call_id.is_empty() {
                            format!("{session_id}:tool:{line_idx}")
                        } else {
                            call_id.clone()
                        };
                        let action = ToolAction {
                            tool_use_id: tool_use_id.clone(),
                            tool_name: name.clone(),
                            category: category.clone(),
                            file_path: file_path.clone(),
                            summary: summary.clone(),
                            full_input: Some(truncate(&input, 10240)),
                            full_output: None,
                            timestamp: timestamp.clone(),
                        };
                        let message_idx = messages.len();
                        messages.push(make_tool_message(
                            format!("{session_id}:tool:{line_idx}"),
                            session_id.clone(),
                            git_branch.clone(),
                            action,
                        ));
                        if !call_id.is_empty() {
                            tool_use_map.insert(
                                call_id,
                                ToolUseEntry {
                                    tool_name: name,
                                    category,
                                    file_path,
                                    summary,
                                    full_input: Some(truncate(&input, 10240)),
                                    timestamp,
                                    message_idx,
                                },
                            );
                        }
                    }
                    "function_call_output" | "custom_tool_call_output" => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if call_id.is_empty() {
                            continue;
                        }
                        let output = payload.get("output").map(|value| {
                            value
                                .as_str()
                                .map(|text| truncate(text, 10240))
                                .unwrap_or_else(|| truncate(&value.to_string(), 10240))
                        });
                        if let Some(entry) = tool_use_map.get(&call_id)
                            && let Some(message) = messages.get_mut(entry.message_idx)
                        {
                            message.timestamp = timestamp.clone();
                            if let Some(action) = message.tool_actions.first_mut() {
                                action.full_output = output.clone();
                            }
                            if entry.category == "command"
                                && let Some(ref output_text) = output
                            {
                                let preview = truncate(output_text, 300);
                                if let Some(command) = message.commands_run.first_mut() {
                                    *command = format!("{command}\n{preview}");
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    ExtractedSession {
        session_id,
        project_name: cwd.as_deref().and_then(project_name_from_cwd),
        messages,
    }
}

fn find_session_path(
    provider: IntegrationProvider,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    match provider {
        IntegrationProvider::Claude => {
            let projects_dir = dirs::home_dir()
                .ok_or("Cannot determine home directory")?
                .join(".claude")
                .join("projects");

            if !projects_dir.exists() {
                return Ok(None);
            }

            for project_entry in std::fs::read_dir(&projects_dir)
                .map_err(|e| format!("Read projects: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
            {
                let candidate = project_entry.path().join(format!("{session_id}.jsonl"));
                if candidate.exists() {
                    return Ok(Some(candidate));
                }
            }

            Ok(None)
        }
        IntegrationProvider::Codex => {
            let sessions_dir = dirs::home_dir()
                .ok_or("Cannot determine home directory")?
                .join(".codex")
                .join("sessions");

            if !sessions_dir.exists() {
                return Ok(None);
            }

            let expected_suffix = format!("{session_id}.jsonl");
            for entry in walkdir::WalkDir::new(&sessions_dir)
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            {
                let file_name = entry.file_name().to_string_lossy();
                if file_name.ends_with(&expected_suffix) {
                    return Ok(Some(entry.path().to_path_buf()));
                }
            }

            Ok(None)
        }
        IntegrationProvider::MiniMax => Ok(None),
    }
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
    provider: IntegrationProvider,
    session_id: String,
    around_message_id: String,
    window: Option<u32>,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SessionContext, String> {
    let idx = state.0.clone();
    let w = window.unwrap_or(5) as usize;
    crate::run_blocking(move || idx.get_context(provider, &session_id, &around_message_id, w))
}

#[tauri::command]
pub async fn get_search_facets(
    state: tauri::State<'_, SessionIndexState>,
) -> Result<SearchFacets, String> {
    let idx = state.0.clone();
    crate::run_blocking(move || idx.get_facets())
}

#[tauri::command]
pub async fn sync_search_index(
    app: tauri::AppHandle,
    state: tauri::State<'_, SessionIndexState>,
) -> Result<usize, String> {
    let idx = state.0.clone();
    let storage = crate::STORAGE.get();
    crate::run_blocking(move || idx.startup_scan(&app, storage))
}
