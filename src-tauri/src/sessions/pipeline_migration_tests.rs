use super::*;

fn legacy_index(path: &Path) {
    legacy_index_with_projects(
        path,
        &[
            ("localneedle", "/exact/project"),
            ("remoteneedle", "/exact/project"),
        ],
    );
}

fn legacy_index_with_projects(path: &Path, documents: &[(&str, &str)]) {
    std::fs::create_dir_all(path).unwrap();
    let (schema, _) = SessionIndex::build_schema();
    let mut json = serde_json::to_value(&schema).unwrap();
    let fields = json.as_array_mut().unwrap();
    fields.retain(|field| {
        !matches!(
            field["name"].as_str(),
            Some("source_key" | "source_state" | "document_kind")
        )
    });
    let schema: Schema = serde_json::from_value(json).unwrap();
    let index = Index::create_in_dir(path, schema.clone()).unwrap();
    let mut writer = index
        .writer::<TantivyDocument>(SessionIndex::TEST_WRITER_HEAP_BYTES)
        .unwrap();
    for &(content, project_path) in documents {
        let mut doc = TantivyDocument::default();
        for (name, value) in [
            ("provider", "claude"),
            ("session_id", "shared"),
            ("message_id", "message"),
            ("role", "user"),
            ("content", content),
            ("display_text", content),
            ("project_path", project_path),
        ] {
            doc.add_text(schema.get_field(name).unwrap(), value);
        }
        doc.add_facet(
            schema.get_field("provider_facet").unwrap(),
            Facet::from("/claude"),
        );
        doc.add_facet(
            schema.get_field("project").unwrap(),
            Facet::from("/project"),
        );
        doc.add_facet(schema.get_field("host").unwrap(), Facet::from("/host"));
        writer.add_document(doc).unwrap();
    }
    writer.commit().unwrap();
    std::fs::write(path.join("schema_version.txt"), "9").unwrap();
}

fn assert_preserved(path: &Path) {
    let index = SessionIndex::open_or_create_for_tests(path).unwrap();
    let results = index
        .search(
            "",
            &SearchFilters {
                project: Some("/exact/project".into()),
                ..Default::default()
            },
            "relevance",
            0,
            10,
        )
        .unwrap();
    assert_eq!(
        results.total_hits, 2,
        "migration retains nonstored exact cwd and both unowned documents"
    );
    assert!(
        results
            .hits
            .iter()
            .all(|hit| hit.source_key.as_deref() == Some("legacy:unattributed"))
    );
    assert_eq!(index.get_facets().unwrap().providers[0].count, 2);
    assert!(
        index.state.lock().unwrap().sources.is_empty(),
        "v9 cannot prove local versus pushed ownership"
    );
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Preservation First Migration]]
#[test]
fn pipeline_migration_preserves_legacy_remote_and_exact_project_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    legacy_index(&path);
    assert_preserved(&path);
    assert_preserved(&path);
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Preservation First Migration]]
#[test]
fn pipeline_migration_preserves_sparse_project_postings_across_batches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    let projects = [
        ("earlyneedle", "/early"),
        ("middleneedle", "/middle"),
        ("lateneedle", "/late"),
    ];
    let documents = projects
        .iter()
        .flat_map(|&document| (0..257).map(move |_| document))
        .collect::<Vec<_>>();
    legacy_index_with_projects(&path, &documents);
    for _ in 0..2 {
        let index = SessionIndex::open_or_create_for_tests(&path).unwrap();
        assert_eq!(index.searcher().num_docs(), 771);
        for (content, project) in projects {
            let results = index
                .search(
                    "",
                    &SearchFilters {
                        project: Some(project.into()),
                        ..Default::default()
                    },
                    "relevance",
                    0,
                    100,
                )
                .unwrap();
            assert_eq!(results.total_hits, 257, "lost cwd postings for {project}");
            assert!(results.hits.iter().all(|hit| hit.content == content));
        }
    }
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Preservation First Migration]]
#[test]
fn pipeline_migration_recovers_before_between_and_after_renames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    let staged = dir.path().join("index.v10-staged");
    let backup = dir.path().join("index.v9-backup");
    legacy_index(&path);
    // Failed staging never removes the live old index.
    std::fs::write(&staged, "interrupted staging").unwrap();
    assert!(SessionIndex::open_or_create_for_tests(&path).is_err());
    assert_eq!(
        Index::open_in_dir(&path)
            .unwrap()
            .reader()
            .unwrap()
            .searcher()
            .num_docs(),
        2
    );
    std::fs::remove_file(&staged).unwrap();
    // Process exits between old->backup and staged->live.
    std::fs::rename(&path, &backup).unwrap();
    assert_preserved(&path);
    assert!(!backup.exists());
    // Process exits after staged->live, before backup cleanup.
    legacy_index(&backup);
    assert_preserved(&path);
    assert!(!backup.exists());
}
