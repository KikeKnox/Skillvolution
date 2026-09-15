use skillvolution::vault::Vault;

#[test]
fn drafts_lists_only_unpublished_revisions_in_stable_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    assert!(vault.drafts().unwrap().is_empty());
    vault.propose("beta", "B", "C", "E", 0).unwrap();
    vault.propose("alpha", "A", "C", "E", 0).unwrap();
    vault.propose("alpha", "A2", "C", "E", 0).unwrap();
    vault.publish("alpha", 1).unwrap();
    let drafts = vault.drafts().unwrap();
    assert_eq!(drafts.len(), 2);
    assert_eq!((&*drafts[0].id, drafts[0].version), ("alpha", 2));
    assert_eq!(drafts[1].id, "beta");
    assert!(drafts.iter().all(|r| !r.published));
}

#[test]
fn read_and_publish_validate_identifiers_before_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    for id in ["Upper", "-bad", "bad--id", ""] {
        let error = vault.get(id, None).unwrap_err().to_string();
        assert!(error.contains("id must"), "{error}");
        assert!(vault.inspect(id, 1).unwrap_err().to_string().contains("id must"));
        assert!(vault.publish(id, 1).unwrap_err().to_string().contains("id must"));
    }
    for version in [-1, 0] {
        assert!(vault.get("valid", Some(version)).unwrap_err().to_string().contains("version must"));
        assert!(vault.inspect("valid", version).unwrap_err().to_string().contains("version must"));
        assert!(vault.publish("valid", version).unwrap_err().to_string().contains("version must"));
    }
}

#[test]
fn search_is_body_free_latest_published_metadata_with_exact_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    for id in ["alpha", "beta", "gamma"] {
        vault.propose(id, "Useful tests", "SECRET BODY", "SECRET EVIDENCE", 0).unwrap();
        vault.publish(id, 1).unwrap();
    }
    vault.propose("alpha", "Latest tests", "New body", "Evidence", 1).unwrap();
    vault.publish("alpha", 2).unwrap();
    vault.propose("alpha", "Unpublished", "Draft", "Evidence", 2).unwrap();
    vault.propose("hidden", "Hidden tests", "Draft", "Evidence", 0).unwrap();
    let page = vault.search("", 2, 0).unwrap();
    assert_eq!(page.total, 3);
    assert!(page.has_more);
    assert_eq!(page.skills.len(), 2);
    assert_eq!(page.skills[0].id, "alpha");
    assert_eq!(page.skills[0].version, 2);
    let json = serde_json::to_value(&page).unwrap();
    assert_eq!(json["skills"][0].as_object().unwrap().len(), 3);
    assert!(!json.to_string().contains("SECRET"));
    let page = vault.search("TESTS", 2, 2).unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.skills[0].id, "gamma");
    assert!(!page.has_more);
    let beyond = vault.search("", 2, 99).unwrap();
    assert_eq!(beyond.total, 3);
    assert!(beyond.skills.is_empty());
    assert!(!beyond.has_more);
    assert_eq!(vault.search("Latest", 20, 0).unwrap().total, 1);
    assert_eq!(vault.search("SECRET", 20, 0).unwrap().total, 0);
    for query in ["*", r#"""#, "OR", "%", "_", "' OR 1=1 --", "NEAR("] {
        assert_eq!(vault.search(query, 20, 0).unwrap().total, 0);
    }
    assert!(vault.search("", 0, 0).is_err());
    assert!(vault.search("", 101, 0).is_err());
    assert!(vault.search("", 1, -1).is_err());
    assert!(vault.search(&"a".repeat(513), 1, 0).is_err());
}

#[test]
fn stale_bases_never_overwrite_a_publication() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    assert!(vault.propose("skill", "D", "C", "E", 9).is_err());
    vault.propose("skill", "First", "C", "E", 0).unwrap();
    vault.propose("skill", "Competing", "C", "E", 0).unwrap();
    vault.publish("skill", 1).unwrap();
    assert!(vault.publish("skill", 2).is_err());
    assert!(!vault.inspect("skill", 2).unwrap().published);
    assert_eq!(vault.get("skill", None).unwrap().version, 1);
    assert!(vault.propose("skill", "Stale", "C", "E", 0).is_err());
    assert!(vault.publish("skill", 1).is_err());
    let revision = vault.propose("skill", "Fresh", "C", "E", 1).unwrap();
    assert_eq!(revision.version, 3);
    vault.publish("skill", 3).unwrap();
    assert!(vault.publish("skill", 2).is_err());
}

#[test]
fn publication_exposes_only_published_revisions_and_keeps_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    vault.propose("rust-tests", "First", "First body", "First evidence", 0).unwrap();
    assert!(vault.get("rust-tests", None).is_err());
    assert!(vault.get("rust-tests", Some(1)).is_err());
    vault.publish("rust-tests", 1).unwrap();
    assert!(vault.get("rust-tests", None).unwrap().published);
    vault.propose("rust-tests", "Second", "Second body", "Second evidence", 1).unwrap();
    assert_eq!(vault.get("rust-tests", None).unwrap().version, 1);
    assert!(vault.get("rust-tests", Some(2)).is_err());
    vault.publish("rust-tests", 2).unwrap();
    drop(vault);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.get("rust-tests", None).unwrap().version, 2);
    assert_eq!(vault.get("rust-tests", Some(1)).unwrap().content, "First body");
    assert!(vault.get("rust-tests", Some(99)).is_err());
}

#[test]
fn rejects_invalid_proposals_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    for id in ["", "Upper", "two--parts", "-leading", "trailing-", "has_space", "has space", "é", "sql';--", &"a".repeat(65)] {
        assert!(vault.propose(id, "Description", "Content", "Evidence", 0).is_err(), "accepted {id:?}");
    }
    for description in ["", "  ", "two\nlines", "two\rlines", "tab\tline", "nul\0line", "two\u{2028}lines", &"d".repeat(281)] {
        assert!(vault.propose("valid-id", description, "Content", "Evidence", 0).is_err());
    }
    for content in ["", " \n", "nul\0body", &"x".repeat(65_537)] {
        assert!(vault.propose("valid-id", "Description", content, "Evidence", 0).is_err());
    }
    for evidence in ["", " \n", "nul\0evidence", &"x".repeat(16_385)] {
        assert!(vault.propose("valid-id", "Description", "Content", evidence, 0).is_err());
    }
    assert!(vault.propose("valid-id", "Description", "Content", "Evidence", -1).is_err());
    let valid = vault.propose("valid-id", "Description", "Content", "Evidence", 0).unwrap();
    assert_eq!(valid.version, 1);
}

#[test]
fn proposals_are_immutable_persistent_draft_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    let first = vault
        .propose(
            "rust-tests",
            "Run tests",
            "Run cargo test",
            "Passed locally",
            0,
        )
        .unwrap();
    let second = vault
        .propose(
            "rust-tests",
            "Run better tests",
            "Run cargo test --all-targets",
            "Passed twice",
            0,
        )
        .unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
    assert!(!first.published);
    assert_eq!(first.expected_version, 0);
    drop(vault);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.inspect("rust-tests", 1).unwrap(), first);
    assert_eq!(vault.inspect("rust-tests", 2).unwrap(), second);
}

#[test]
fn initializes_persistent_database_idempotently_with_wal_and_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/skills.db");
    let vault = Vault::open(&path).unwrap();
    assert!(path.is_file());
    drop(vault);
    Vault::open(&path).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}
