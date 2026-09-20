use super::*;

const CHECKPOINT_KIND: &str = "checkpoint";

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    path.with_file_name(format!(
        "{}.{suffix}",
        path.file_name().unwrap().to_string_lossy()
    ))
}

impl SessionIndex {
    /// Recover an interrupted two-rename migration before opening any mmap handles.
    pub(super) fn recover_migration(path: &Path) -> Result<(), String> {
        let backup = sibling(path, "v9-backup");
        if backup.exists() {
            let valid = Index::open_in_dir(path).is_ok_and(|index| {
                index.schema().get_field("source_key").is_ok() && index.reader().is_ok()
            });
            if valid {
                std::fs::remove_dir_all(&backup).map_err(|e| e.to_string())?;
            } else {
                if path.exists() {
                    std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
                }
                std::fs::rename(&backup, path).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub(super) fn migrate_legacy(path: &Path, heap: usize) -> Result<(), String> {
        let staged = sibling(path, "v10-staged");
        let backup = sibling(path, "v9-backup");
        if staged.exists() {
            std::fs::remove_dir_all(&staged).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
        {
            let old = Index::open_in_dir(path).map_err(|e| e.to_string())?;
            let reader = old.reader().map_err(|e| e.to_string())?;
            let searcher = reader.searcher();
            let old_schema = old.schema();
            let (schema, fields) = Self::build_schema();
            let new = Index::create_in_dir(&staged, schema.clone()).map_err(|e| e.to_string())?;
            let mut writer = new
                .writer::<TantivyDocument>(heap)
                .map_err(|e| e.to_string())?;
            // Legacy documents have no trustworthy source/remote discriminator.
            // Retain them as unattributed, never grant a sidecar deletion authority.
            for (segment_ord, segment) in searcher.segment_readers().iter().enumerate() {
                // Copy stored content in bounded batches. Recover the one nonstored
                // identity field from postings; do not parse retained source files.
                let path_field = old_schema.get_field("project_path").ok();
                let mut start = 0;
                while start < segment.max_doc() {
                    let mut end = start;
                    let mut bytes = 0;
                    let mut docs = HashMap::new();
                    for id in start..(start + 256).min(segment.max_doc()) {
                        end = id + 1;
                        if segment.is_deleted(id) {
                            continue;
                        }
                        let old_doc: TantivyDocument = searcher
                            .doc(tantivy::DocAddress::new(segment_ord as u32, id))
                            .map_err(|e| e.to_string())?;
                        let mut doc = TantivyDocument::default();
                        for (field, value) in old_doc.field_values() {
                            if let Ok(new_field) =
                                schema.get_field(old_schema.get_field_name(field))
                            {
                                doc.add_field_value(new_field, value);
                            }
                        }
                        if let Some(provider) = doc
                            .get_first(fields.provider)
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                        {
                            doc.add_facet(
                                fields.provider_facet,
                                Facet::from(&format!("/{provider}")),
                            );
                        }
                        doc.add_text(fields.source_key, "legacy:unattributed");
                        bytes += doc
                            .field_values()
                            .map(|(_, v)| v.as_str().map_or(0, str::len))
                            .sum::<usize>();
                        docs.insert(id, doc);
                        if bytes >= 8 * 1024 * 1024 {
                            break;
                        }
                    }
                    // ponytail: scan cwd terms per bounded batch; an external merge
                    // join is warranted only if migration profiling finds high cwd cardinality.
                    if let Some(path_field) = path_field {
                        use tantivy::{DocSet, TERMINATED};
                        let inverted = segment
                            .inverted_index(path_field)
                            .map_err(|e| e.to_string())?;
                        let mut terms = inverted.terms().stream().map_err(|e| e.to_string())?;
                        while terms.advance() {
                            let text =
                                std::str::from_utf8(terms.key()).map_err(|e| e.to_string())?;
                            let mut postings = inverted
                                .read_postings_from_terminfo(
                                    terms.value(),
                                    IndexRecordOption::Basic,
                                )
                                .map_err(|e| e.to_string())?;
                            // Seek is forward-only; this term may begin after the batch.
                            let mut id = postings.doc();
                            if id < start {
                                id = postings.seek(start);
                            }
                            while id != TERMINATED && id < end {
                                if let Some(doc) = docs.get_mut(&id) {
                                    doc.add_text(fields.project_path, text);
                                }
                                id = postings.advance();
                            }
                        }
                    }
                    for doc in docs.into_values() {
                        writer.add_document(doc).map_err(|e| e.to_string())?;
                    }
                    start = end;
                }
            }
            writer.commit().map_err(|e| e.to_string())?;
            writer.wait_merging_threads().map_err(|e| e.to_string())?;
            let check = new.reader().map_err(|e| e.to_string())?;
            if check.searcher().num_docs() != searcher.num_docs() {
                return Err("Migration document count mismatch".into());
            }
            std::fs::write(
                staged.join("schema_version.txt"),
                Self::SCHEMA_VERSION.to_string(),
            )
            .map_err(|e| e.to_string())?;
            // Rejections are only an optimization; ownership is recovered from commits.
            let state = Self::load_state(path);
            std::fs::write(
                staged.join("index_state.json"),
                serde_json::to_vec(&state).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        } // all old/new mmap and writer handles dropped before directory renames
        std::fs::rename(path, &backup).map_err(|e| e.to_string())?;
        if let Err(error) = std::fs::rename(&staged, path) {
            std::fs::rename(&backup, path)
                .map_err(|restore| format!("{error}; restore: {restore}"))?;
            return Err(error.to_string());
        }
        Self::recover_migration(path)
    }

    pub(super) fn recover_committed_sources(&self) -> Result<(), String> {
        let searcher = self.searcher();
        let mut sources = HashMap::new();
        // Only one small checkpoint document per source, atomically committed with
        // its messages. No O(corpus) state serialization on a single-source commit.
        for segment in searcher.segment_readers() {
            use tantivy::{DocSet, TERMINATED};
            let inverted = segment
                .inverted_index(self.fields.document_kind)
                .map_err(|e| e.to_string())?;
            let term = Term::from_field_text(self.fields.document_kind, CHECKPOINT_KIND);
            if let Some(mut postings) = inverted
                .read_postings(&term, IndexRecordOption::Basic)
                .map_err(|e| e.to_string())?
            {
                let store = segment.get_store_reader(1).map_err(|e| e.to_string())?;
                while postings.doc() != TERMINATED {
                    let id = postings.doc();
                    if !segment.is_deleted(id) {
                        let doc: TantivyDocument = store.get(id).map_err(|e| e.to_string())?;
                        let key = doc
                            .get_first(self.fields.source_key)
                            .and_then(|v| v.as_str())
                            .ok_or("Missing committed source key")?;
                        let state = doc
                            .get_first(self.fields.source_state)
                            .and_then(|v| v.as_str())
                            .ok_or("Missing committed source state")?;
                        sources.insert(
                            key.to_owned(),
                            serde_json::from_str(state)
                                .map_err(|e| format!("Invalid committed source state: {e}"))?,
                        );
                    }
                    postings.advance();
                }
            }
        }
        self.state.lock().unwrap().sources = sources;
        Ok(())
    }

    pub(super) fn checkpoint_document(
        &self,
        key: &str,
        state: &IndexedSource,
    ) -> Result<TantivyDocument, String> {
        let mut doc = TantivyDocument::default();
        doc.add_text(self.fields.source_key, key);
        doc.add_text(self.fields.document_kind, CHECKPOINT_KIND);
        doc.add_text(
            self.fields.source_state,
            serde_json::to_string(state).map_err(|e| e.to_string())?,
        );
        Ok(doc)
    }

    pub(super) fn delete_source_docs(&self, writer: &IndexWriter, key: &str) {
        writer.delete_term(Term::from_field_text(self.fields.source_key, key));
    }

    pub(super) fn remote_source_key(
        provider: IntegrationProvider,
        host: &str,
        session: &str,
    ) -> String {
        format!(
            "remote:{}",
            serde_json::json!([provider.as_str(), host, session])
        )
    }
}

pub(super) fn exclude_checkpoints(
    fields: &SessionSchema,
) -> (Occur, Box<dyn tantivy::query::Query>) {
    (
        Occur::MustNot,
        Box::new(TermQuery::new(
            Term::from_field_text(fields.document_kind, CHECKPOINT_KIND),
            IndexRecordOption::Basic,
        )),
    )
}
