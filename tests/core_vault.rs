#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::{Proposal, Vault};
use support::Draft;

fn open() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    (dir, vault)
}

// --- validation --------------------------------------------------------

#[test]
fn get_inspect_and_publish_return_not_found_for_unknown_or_malformed_ids() {
    let (_dir, mut vault) = open();
    for id in ["missing", "Upper", ""] {
        assert!(vault.get(id, None, None).is_err());
        assert!(vault.inspect(id, 1).is_err());
        assert!(vault.publish(id, 1).is_err());
    }
    assert!(vault.get("missing", Some(-1), None).is_err());
    assert!(vault.inspect("missing", 0).is_err());
}

#[test]
fn rejects_invalid_proposals_before_writing() {
    let (_dir, mut vault) = open();
    let base = |id, description, content, evidence, expected_version| Proposal {
        id,
        description,
        tags: &[],
        content,
        evidence,
        expected_version,
        scope: None,
    };
    let long_id = "a".repeat(65);
    for id in ["", "Upper", long_id.as_str()] {
        let proposal = base(id, "Use when testing", "Content body", "Evidence text", 0);
        assert!(vault.propose(&proposal).is_err(), "accepted id {id:?}");
    }
    let long_description = "d".repeat(281);
    for description in ["", long_description.as_str(), "two\nlines"] {
        let proposal = base("valid-id", description, "Content body", "Evidence text", 0);
        assert!(
            vault.propose(&proposal).is_err(),
            "accepted description {description:?}"
        );
    }
    let long_content = "x".repeat(65_537);
    for content in ["", long_content.as_str(), "nul\0body"] {
        let proposal = base("valid-id", "Use when testing", content, "Evidence text", 0);
        assert!(
            vault.propose(&proposal).is_err(),
            "accepted content {content:?}"
        );
    }
    let long_evidence = "x".repeat(16_385);
    for evidence in ["", long_evidence.as_str(), "nul\0evidence"] {
        let proposal = base("valid-id", "Use when testing", "Content body", evidence, 0);
        assert!(
            vault.propose(&proposal).is_err(),
            "accepted evidence {evidence:?}"
        );
    }
    assert!(
        vault
            .propose(&base(
                "valid-id",
                "Use when testing",
                "Content body",
                "Evidence text",
                -1
            ))
            .is_err()
    );
    let valid = Draft::new("valid-id").propose(&mut vault);
    assert_eq!(valid.version, 1);
}

#[test]
fn rejects_invalid_tags_and_keeps_valid_ones_in_order() {
    let (_dir, mut vault) = open();
    let too_many = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    assert!(
        vault
            .propose(&Draft::new("skill-a").tags(&too_many).proposal())
            .is_err()
    );
    assert!(
        vault
            .propose(&Draft::new("skill-b").tags(&["Bad_Tag"]).proposal())
            .is_err()
    );
    let long_tag = "a".repeat(33);
    assert!(
        vault
            .propose(&Draft::new("skill-c").tags(&[long_tag.as_str()]).proposal())
            .is_err()
    );
    let revision = vault
        .propose(&Draft::new("skill-d").tags(&["rust", "sqlite"]).proposal())
        .unwrap();
    assert_eq!(revision.tags, vec!["rust", "sqlite"]);
}

// --- lifecycle -----------------------------------------------------------

#[test]
fn drafts_lists_only_unpublished_revisions_in_stable_order() {
    let (_dir, mut vault) = open();
    assert!(vault.drafts().unwrap().is_empty());
    Draft::new("beta").propose(&mut vault);
    Draft::new("alpha").publish(&mut vault);
    Draft::new("alpha").expected_version(1).propose(&mut vault);
    let drafts = vault.drafts().unwrap();
    assert_eq!(drafts.len(), 2);
    assert_eq!((&*drafts[0].id, drafts[0].version), ("alpha", 2));
    assert_eq!(drafts[1].id, "beta");
    assert!(drafts.iter().all(|r| r.status == "draft"));
}

#[test]
fn proposals_are_immutable_persistent_draft_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    let first = Draft::new("rust-tests")
        .description("Run tests")
        .content("Run cargo test")
        .evidence("Passed locally")
        .propose(&mut vault);
    let second = Draft::new("rust-tests")
        .description("Run better tests")
        .content("Run cargo test --all-targets")
        .evidence("Passed twice")
        .propose(&mut vault);
    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
    assert_eq!(first.status, "draft");
    assert_eq!(first.expected_version, 0);
    drop(vault);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.inspect("rust-tests", 1).unwrap(), first);
    assert_eq!(vault.inspect("rust-tests", 2).unwrap(), second);
}

#[test]
fn publication_exposes_only_published_revisions_and_keeps_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    Draft::new("rust-tests")
        .description("First")
        .content("First body")
        .evidence("First evidence")
        .propose(&mut vault);
    assert!(vault.get("rust-tests", None, None).is_err());
    assert!(vault.get("rust-tests", Some(1), None).is_err());
    vault.publish("rust-tests", 1).unwrap();
    assert!(vault.get("rust-tests", None, None).is_ok());
    Draft::new("rust-tests")
        .expected_version(1)
        .description("Second")
        .content("Second body")
        .evidence("Second evidence")
        .propose(&mut vault);
    assert_eq!(vault.get("rust-tests", None, None).unwrap().version, 1);
    assert!(vault.get("rust-tests", Some(2), None).is_err());
    vault.publish("rust-tests", 2).unwrap();
    drop(vault);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.get("rust-tests", None, None).unwrap().version, 2);
    assert_eq!(
        vault.get("rust-tests", Some(1), None).unwrap().content,
        "First body"
    );
    assert!(vault.get("rust-tests", Some(99), None).is_err());
}

#[test]
fn stale_bases_never_overwrite_a_publication() {
    let (_dir, mut vault) = open();
    assert!(
        vault
            .propose(&Draft::new("skill").expected_version(9).proposal())
            .is_err()
    );
    let first = Draft::new("skill").description("First").propose(&mut vault);
    let competing = Draft::new("skill")
        .description("Competing")
        .propose(&mut vault);
    vault.publish("skill", first.version).unwrap();
    assert!(vault.publish("skill", competing.version).is_err());
    assert_eq!(
        vault.get("skill", None, None).unwrap().version,
        first.version
    );
    assert!(
        vault
            .propose(&Draft::new("skill").description("Stale").proposal())
            .is_err()
    );
    assert!(vault.publish("skill", first.version).is_err());
    let fresh = Draft::new("skill")
        .expected_version(1)
        .description("Fresh")
        .propose(&mut vault);
    assert_eq!(fresh.version, 3);
    vault.publish("skill", fresh.version).unwrap();
    assert!(vault.publish("skill", competing.version).is_err());
}

#[test]
fn publishing_supersedes_only_drafts_sharing_its_base() {
    let (_dir, mut vault) = open();
    let sibling = Draft::new("skill")
        .description("Sibling draft")
        .propose(&mut vault);
    let winner = Draft::new("skill")
        .description("Winning draft")
        .propose(&mut vault);
    vault.publish("skill", winner.version).unwrap();
    let rebased = Draft::new("skill")
        .expected_version(winner.version)
        .description("Rebased draft")
        .propose(&mut vault);
    let superseded = vault.inspect("skill", sibling.version).unwrap();
    assert_eq!(superseded.status, "superseded");
    assert!(superseded.review_note.unwrap().contains("superseded"));
    assert!(superseded.reviewed_at.is_some());
    assert_eq!(
        vault.inspect("skill", rebased.version).unwrap().status,
        "draft"
    );
}

#[test]
fn reject_sets_status_and_note_and_blocks_publish() {
    let (_dir, mut vault) = open();
    let draft = Draft::new("skill").propose(&mut vault);
    let version = vault
        .reject("skill", draft.version, Some("not reusable"))
        .unwrap();
    let rejected = vault.inspect("skill", version).unwrap();
    assert_eq!(rejected.status, "rejected");
    assert_eq!(rejected.review_note.as_deref(), Some("not reusable"));
    assert!(rejected.reviewed_at.is_some());
    assert!(vault.publish("skill", draft.version).is_err());
}

#[test]
fn cannot_publish_rejected_or_superseded_revisions() {
    let (_dir, mut vault) = open();
    let rejected = Draft::new("skill").propose(&mut vault);
    vault.reject("skill", rejected.version, None).unwrap();
    let error = vault
        .publish("skill", rejected.version)
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected"), "{error}");

    let sibling = Draft::new("other").propose(&mut vault);
    let winner = Draft::new("other").propose(&mut vault);
    vault.publish("other", winner.version).unwrap();
    let error = vault
        .publish("other", sibling.version)
        .unwrap_err()
        .to_string();
    assert!(error.contains("superseded"), "{error}");
}

#[test]
fn deprecate_hides_from_search_but_get_still_works() {
    let (_dir, mut vault) = open();
    Draft::new("skill").publish(&mut vault);
    vault.set_deprecated("skill", true).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 0);
    assert!(vault.get("skill", None, None).is_ok());
    vault.set_deprecated("skill", false).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 1);
    assert!(vault.set_deprecated("missing", true).is_err());
}

#[test]
fn diff_shows_unified_diff_against_base_and_empty_for_new_skill() {
    let (_dir, mut vault) = open();
    let first = Draft::new("skill")
        .content("line one\nline two\n")
        .propose(&mut vault);
    let new_diff = vault.diff("skill", first.version).unwrap();
    assert!(new_diff.contains("+line one"), "{new_diff}");
    assert!(new_diff.contains("+line two"), "{new_diff}");
    vault.publish("skill", first.version).unwrap();
    let second = Draft::new("skill")
        .expected_version(1)
        .content("line one\nline three\n")
        .propose(&mut vault);
    let diff = vault.diff("skill", second.version).unwrap();
    assert!(diff.contains("-line two"), "{diff}");
    assert!(diff.contains("+line three"), "{diff}");
}

// --- scope -----------------------------------------------------------

#[test]
fn global_skill_is_visible_from_every_project() {
    let (_dir, mut vault) = open();
    Draft::new("global-skill").publish(&mut vault);
    for project in [None, Some("proja"), Some("projb")] {
        assert_eq!(
            vault.search("", project, 20, 0).unwrap().total,
            1,
            "project {project:?}"
        );
        assert!(vault.get("global-skill", None, project).is_ok());
    }
}

#[test]
fn project_scoped_skill_is_visible_only_to_its_project() {
    let (_dir, mut vault) = open();
    Draft::new("proj-skill").scope("proja").publish(&mut vault);
    assert_eq!(vault.search("", Some("proja"), 20, 0).unwrap().total, 1);
    assert_eq!(vault.search("", Some("projb"), 20, 0).unwrap().total, 0);
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 0);
    assert!(vault.get("proj-skill", None, Some("proja")).is_ok());
    assert!(vault.get("proj-skill", None, Some("projb")).is_err());
    assert!(vault.get("proj-skill", None, None).is_err());
}

#[test]
fn scope_cannot_change_after_the_first_proposal() {
    let (_dir, mut vault) = open();
    Draft::new("skill").scope("proja").propose(&mut vault);
    let error = vault
        .propose(&Draft::new("skill").scope("projb").proposal())
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists with scope"), "{error}");
    let error = vault
        .propose(&Draft::new("skill").proposal())
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists with scope"), "{error}");
    let matching = vault
        .propose(&Draft::new("skill").scope("proja").proposal())
        .unwrap();
    assert_eq!(matching.version, 2);
}

#[test]
fn id_collision_across_projects_errors_clearly() {
    let (_dir, mut vault) = open();
    Draft::new("shared-id").scope("proja").propose(&mut vault);
    let error = vault
        .propose(&Draft::new("shared-id").scope("projb").proposal())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("shared-id") && error.contains("scope"),
        "{error}"
    );
}

// --- search ------------------------------------------------------------

#[test]
fn multi_word_queries_match_when_words_are_not_adjacent() {
    let (_dir, mut vault) = open();
    Draft::new("network-retry")
        .description("Handles timeout errors")
        .content("retry the flaky network call")
        .publish(&mut vault);
    assert_eq!(vault.search("timeout flaky", None, 20, 0).unwrap().total, 1);
}

#[test]
fn body_content_words_are_searchable() {
    let (_dir, mut vault) = open();
    Draft::new("skill")
        .description("Something else entirely")
        .content("mentions xylophone somewhere in the body")
        .publish(&mut vault);
    assert_eq!(vault.search("xylophone", None, 20, 0).unwrap().total, 1);
}

#[test]
fn ranking_prefers_id_and_description_matches_over_content_only_matches() {
    let (_dir, mut vault) = open();
    Draft::new("widget-cache")
        .description("Widget caching guide")
        .content("unrelated body")
        .publish(&mut vault);
    Draft::new("other-skill")
        .description("Unrelated description")
        .content("uses a widget somewhere")
        .publish(&mut vault);
    let page = vault.search("widget", None, 20, 0).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.skills[0].id, "widget-cache");
}

#[test]
fn search_tokenizer_ignores_diacritics_prefix_and_case() {
    let (_dir, mut vault) = open();
    Draft::new("coffee-skill")
        .description("Como preparar un buen café")
        .publish(&mut vault);
    Draft::new("release-skill")
        .description("Deployment checklist")
        .publish(&mut vault);
    Draft::new("tests-skill")
        .description("Useful Tests")
        .publish(&mut vault);
    assert_eq!(vault.search("cafe", None, 20, 0).unwrap().total, 1);
    assert_eq!(vault.search("deploy", None, 20, 0).unwrap().total, 1);
    assert_eq!(vault.search("TESTS", None, 20, 0).unwrap().total, 1);
}

#[test]
fn fts_special_characters_never_error() {
    let (_dir, mut vault) = open();
    Draft::new("skill").publish(&mut vault);
    for query in ["*", "\"", "OR", "NEAR(", "' OR 1=1 --", "AND", "-", "((("] {
        assert!(
            vault.search(query, None, 20, 0).is_ok(),
            "query {query:?} errored"
        );
    }
}

#[test]
fn empty_query_lists_the_full_catalog_ordered_by_score_then_id() {
    let (_dir, mut vault) = open();
    Draft::new("beta").publish(&mut vault);
    Draft::new("alpha").publish(&mut vault);
    let page = vault.search("", None, 20, 0).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(
        page.skills
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
}

#[test]
fn pagination_reports_total_and_has_more_correctly() {
    let (_dir, mut vault) = open();
    for id in ["a", "b", "c"] {
        Draft::new(id).publish(&mut vault);
    }
    let page = vault.search("", None, 2, 0).unwrap();
    assert_eq!(page.total, 3);
    assert!(page.has_more);
    assert_eq!(page.skills.len(), 2);
    let page = vault.search("", None, 2, 2).unwrap();
    assert_eq!(page.total, 3);
    assert!(!page.has_more);
    assert_eq!(page.skills.len(), 1);
    let page = vault.search("", None, 2, 99).unwrap();
    assert_eq!(page.total, 3);
    assert!(page.skills.is_empty());
    assert!(!page.has_more);
}

#[test]
fn search_never_returns_drafts_or_bodies_and_only_the_latest_published_version() {
    let (_dir, mut vault) = open();
    let v1 = Draft::new("skill")
        .content("SECRET body v1")
        .publish(&mut vault);
    Draft::new("skill")
        .expected_version(1)
        .content("SECRET body v2")
        .propose(&mut vault);
    Draft::new("hidden-skill")
        .content("never published")
        .propose(&mut vault);
    let page = vault.search("", None, 20, 0).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.skills[0].id, "skill");
    assert_eq!(page.skills[0].version, v1);
    let json = serde_json::to_value(&page).unwrap();
    assert!(!json.to_string().contains("SECRET"));
    assert!(!json.to_string().contains("never published"));
    let fields: Vec<String> = json["skills"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert!(!fields.contains(&"content".to_string()));
}

#[test]
fn search_validates_limit_offset_and_query_length() {
    let (_dir, vault) = open();
    assert!(vault.search("", None, 0, 0).is_err());
    assert!(vault.search("", None, 101, 0).is_err());
    assert!(vault.search("", None, 1, -1).is_err());
    assert!(vault.search(&"a".repeat(513), None, 1, 0).is_err());
}

// --- outcomes ------------------------------------------------------------

#[test]
fn outcomes_are_recorded_only_for_published_revisions() {
    let (_dir, mut vault) = open();
    let draft = Draft::new("skill").propose(&mut vault);
    assert!(
        vault
            .record_outcome("skill", draft.version, "helped", "note", None)
            .is_err()
    );
    vault.publish("skill", draft.version).unwrap();
    assert!(
        vault
            .record_outcome("skill", draft.version, "helped", "note", None)
            .is_ok()
    );
}

#[test]
fn record_outcome_rejects_invalid_result() {
    let (_dir, mut vault) = open();
    let published = Draft::new("skill").publish(&mut vault);
    assert!(
        vault
            .record_outcome("skill", published, "bogus", "note", None)
            .is_err()
    );
}

#[test]
fn search_metadata_counts_reset_on_new_published_version_and_support_failing_filter() {
    let (_dir, mut vault) = open();
    let v1 = Draft::new("skill").publish(&mut vault);
    vault
        .record_outcome("skill", v1, "helped", "n", None)
        .unwrap();
    vault
        .record_outcome("skill", v1, "failed", "n", None)
        .unwrap();
    let page = vault.search("", None, 20, 0).unwrap();
    assert_eq!((page.skills[0].helped, page.skills[0].failed), (1, 1));
    assert_eq!(vault.outcome_summaries(true).unwrap().len(), 1);
    assert_eq!(vault.outcome_summaries(false).unwrap().len(), 1);

    let v2 = Draft::new("skill").expected_version(1).publish(&mut vault);
    let page = vault.search("", None, 20, 0).unwrap();
    assert_eq!(
        (
            page.skills[0].version,
            page.skills[0].helped,
            page.skills[0].failed
        ),
        (v2, 0, 0)
    );
    assert!(vault.outcome_summaries(true).unwrap().is_empty());
}

#[test]
fn outcomes_returns_the_full_log_for_a_skill_newest_first() {
    let (_dir, mut vault) = open();
    let published = Draft::new("skill").publish(&mut vault);
    vault
        .record_outcome("skill", published, "helped", "first", None)
        .unwrap();
    vault
        .record_outcome("skill", published, "failed", "second", None)
        .unwrap();
    let log = vault.outcomes("skill").unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].note, "second");
    assert_eq!(log[1].note, "first");
}

// --- migration -----------------------------------------------------------

#[test]
fn refuses_to_migrate_a_pre_1_schema_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE revisions (id TEXT PRIMARY KEY);")
        .unwrap();
    drop(conn);
    let error = match Vault::open(&path) {
        Ok(_) => panic!("expected the pre-1 schema to be refused"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("pre-1 schema"), "{error}");
}
