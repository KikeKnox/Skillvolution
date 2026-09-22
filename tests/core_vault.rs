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
fn get_and_inspect_return_not_found_for_unknown_ids() {
    let (_dir, vault) = open();
    for id in ["missing", "unknown-id"] {
        assert!(vault.get(id, None, None).is_err());
        assert!(vault.inspect(id, 1).is_err());
    }
}

#[test]
fn get_inspect_outcomes_and_deprecate_reject_malformed_ids() {
    let (_dir, vault) = open();
    let bad_id = "Upper";
    assert!(
        vault
            .get(bad_id, None, None)
            .unwrap_err()
            .to_string()
            .contains("id must")
    );
    assert!(
        vault
            .inspect(bad_id, 1)
            .unwrap_err()
            .to_string()
            .contains("id must")
    );
    assert!(
        vault
            .outcomes(bad_id)
            .unwrap_err()
            .to_string()
            .contains("id must")
    );
    assert!(
        vault
            .set_deprecated(bad_id, true)
            .unwrap_err()
            .to_string()
            .contains("id must")
    );
}

#[test]
fn get_and_inspect_reject_non_positive_versions() {
    let (_dir, vault) = open();
    assert!(
        vault
            .get("valid-id", Some(0), None)
            .unwrap_err()
            .to_string()
            .contains("version must")
    );
    assert!(
        vault
            .inspect("valid-id", 0)
            .unwrap_err()
            .to_string()
            .contains("version must")
    );
}

#[test]
fn rejects_invalid_proposals_before_writing() {
    let (_dir, mut vault) = open();
    let valid_content = support::sectioned("Content body");
    let base = |id, description, content, evidence, expected_version| Proposal {
        id,
        description,
        tags: &[],
        content,
        evidence,
        expected_version,
        scope: None,
        verdict: "keep global",
        verdict_reason: "Verified and reusable",
        replaces_proven: false,
    };
    let long_id = "a".repeat(65);
    for id in ["", "Upper", long_id.as_str()] {
        let proposal = base(id, "Use when testing", &valid_content, "Evidence text", 0);
        assert!(vault.propose(&proposal).is_err(), "accepted id {id:?}");
    }
    let long_description = "d".repeat(281);
    for description in ["", long_description.as_str(), "two\nlines"] {
        let proposal = base("valid-id", description, &valid_content, "Evidence text", 0);
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
        let proposal = base("valid-id", "Use when testing", &valid_content, evidence, 0);
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
                &valid_content,
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

#[test]
fn content_missing_any_required_section_is_rejected() {
    let (_dir, mut vault) = open();
    let sections = [
        "## When to use\nDo the thing.\n",
        "## Procedure\n1. Do it.\n",
        "## Pitfalls\n- None.\n",
        "## Verification\nCheck it.\n",
    ];
    for skip in 0..sections.len() {
        let content: String = sections
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != skip)
            .map(|(_, s)| *s)
            .collect();
        assert!(
            vault
                .propose(&Draft::new("skill").raw_content(&content).proposal())
                .is_err(),
            "accepted content missing section {skip}: {content:?}"
        );
    }
}

#[test]
fn sections_present_but_out_of_order_are_rejected() {
    let (_dir, mut vault) = open();
    let content = "## Procedure\n1. Do it.\n## When to use\nDo the thing.\n\
                   ## Verification\nCheck it.\n## Pitfalls\n- None.\n";
    assert!(
        vault
            .propose(&Draft::new("skill").raw_content(content).proposal())
            .is_err()
    );
}

#[test]
fn sections_accept_case_variation_whitespace_extra_sections_and_leading_prose() {
    let (_dir, mut vault) = open();
    let content = "Some prose before the first heading.\n\n  ## WHEN TO USE  \n\
                   Do the thing.\n## Rollback\nUndo if needed.\n## Procedure\n1. Do it.\n\
                   ## Pitfalls\n- None.\n## Verification\nCheck it.\n";
    assert!(
        vault
            .propose(&Draft::new("skill").raw_content(content).proposal())
            .is_ok()
    );
}

#[test]
fn requires_a_keep_verdict_and_records_it_as_the_review_note() {
    let (_dir, mut vault) = open();
    let reason = "Reproduced twice and reusable outside this repository";
    let published = Draft::new("skill")
        .verdict_reason(reason)
        .propose(&mut vault);
    assert_eq!(
        published.review_note.as_deref(),
        Some(format!("keep global: {reason}").as_str())
    );
    assert!(published.reviewed_at.is_some());
    let stored = vault.inspect("skill", published.version).unwrap();
    assert_eq!(stored.review_note, published.review_note);
    assert_eq!(stored.reviewed_at, published.reviewed_at);
}

#[test]
fn rejects_a_discard_verdict_and_a_verdict_that_contradicts_the_scope() {
    let (_dir, mut vault) = open();
    let discarded = vault
        .propose(&Draft::new("skill-a").verdict("discard").proposal())
        .unwrap_err()
        .to_string();
    assert!(discarded.contains("discard"), "{discarded}");

    let project_as_global = vault
        .propose(
            &Draft::new("skill-b")
                .scope("proja")
                .verdict("keep global")
                .proposal(),
        )
        .unwrap_err()
        .to_string();
    assert!(
        project_as_global.contains("does not match scope"),
        "{project_as_global}"
    );

    let global_as_project = vault
        .propose(&Draft::new("skill-c").verdict("keep project").proposal())
        .unwrap_err()
        .to_string();
    assert!(
        global_as_project.contains("does not match scope"),
        "{global_as_project}"
    );

    let paraphrased = vault
        .propose(&Draft::new("skill-d").verdict("keep it, global").proposal())
        .unwrap_err()
        .to_string();
    assert!(
        paraphrased.contains("verdict must be exactly"),
        "{paraphrased}"
    );

    // Nothing was written by any of the four.
    assert_eq!(vault.search("", Some("proja"), 20, 0).unwrap().total, 0);
}

#[test]
fn replacing_a_proven_version_requires_an_acknowledgement() {
    let (_dir, mut vault) = open();
    let v1 = Draft::new("proven").publish(&mut vault);
    for note in ["worked once", "worked twice"] {
        vault
            .record_outcome("proven", v1, "helped", note, None)
            .unwrap();
    }
    let replacement = Draft::new("proven").expected_version(v1);
    let refused = vault
        .propose(&replacement.proposal())
        .unwrap_err()
        .to_string();
    assert!(refused.contains("proven"), "{refused}");
    assert!(refused.contains("helped 2"), "{refused}");
    assert_eq!(vault.get("proven", None, None).unwrap().version, v1);

    let acknowledged = vault
        .propose(&replacement.replaces_proven(true).proposal())
        .unwrap();
    assert_eq!(acknowledged.version, 2);

    // Control: a version with as many failures as successes is not proven, so
    // it can be replaced freely.
    let net_zero = Draft::new("unproven").publish(&mut vault);
    for result in ["helped", "failed"] {
        vault
            .record_outcome("unproven", net_zero, result, "note", None)
            .unwrap();
    }
    let free = vault
        .propose(&Draft::new("unproven").expected_version(net_zero).proposal())
        .unwrap();
    assert_eq!(free.version, 2);
}

#[test]
fn rejects_credential_shaped_values_in_content_and_evidence() {
    let (_dir, mut vault) = open();
    let content_secret = "AKIA1234567890ABCDEF";
    let content = support::sectioned(&format!("export AWS_ACCESS_KEY_ID={content_secret}"));
    let error = vault
        .propose(&Draft::new("skill").raw_content(&content).proposal())
        .unwrap_err()
        .to_string();
    assert!(error.contains("AWS access key id"), "{error}");
    assert!(error.contains("line 2"), "{error}");
    assert!(!error.contains(content_secret), "{error}");

    let evidence_secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
    let error = vault
        .propose(
            &Draft::new("skill2")
                .evidence(&format!("Observed token {evidence_secret} in logs"))
                .proposal(),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("GitHub token"), "{error}");
    assert!(error.contains("line 1"), "{error}");
    assert!(!error.contains(evidence_secret), "{error}");
}

#[test]
fn accepts_credential_documentation_without_real_values() {
    let (_dir, mut vault) = open();
    let content = "\
## When to use
Configuring credentials for the GitHub and OpenAI CLIs.
## Procedure
1. `export GITHUB_TOKEN=ghp_<your token>`
2. `api_key=$OPENAI_API_KEY`
3. `Authorization: Bearer $TOKEN`
4. keys look like sk-... or AKIA...
5. `git switch task-1a2b3c4d5e6f7a8b`
## Pitfalls
- Never paste a real token into a skill body.
## Verification
Re-run the command with the placeholder substituted.
";
    let result = vault.propose(&Draft::new("skill").raw_content(content).proposal());
    assert!(result.is_ok(), "{:?}", result.err());
}

// --- lifecycle -----------------------------------------------------------

#[test]
fn proposals_are_immutable_persistent_published_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    let first = Draft::new("rust-tests")
        .description("Run tests")
        .content("Run cargo test")
        .evidence("Passed locally")
        .propose(&mut vault);
    let second = Draft::new("rust-tests")
        .expected_version(1)
        .description("Run better tests")
        .content("Run cargo test --all-targets")
        .evidence("Passed twice")
        .propose(&mut vault);
    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
    assert_eq!(first.status, "published");
    assert_eq!(first.expected_version, 0);
    drop(vault);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.inspect("rust-tests", 1).unwrap(), first);
    assert_eq!(vault.inspect("rust-tests", 2).unwrap(), second);
}

#[test]
fn proposals_publish_immediately_and_keep_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    Draft::new("rust-tests")
        .description("First")
        .content("First body")
        .evidence("First evidence")
        .propose(&mut vault);
    assert!(vault.get("rust-tests", None, None).is_ok());
    Draft::new("rust-tests")
        .expected_version(1)
        .description("Second")
        .content("Second body")
        .evidence("Second evidence")
        .propose(&mut vault);
    assert_eq!(vault.get("rust-tests", None, None).unwrap().version, 2);
    drop(vault);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.get("rust-tests", None, None).unwrap().version, 2);
    assert_eq!(
        vault.get("rust-tests", Some(1), None).unwrap().content,
        support::sectioned("First body")
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
    // A competing proposal on the same base fails: v1 is already published.
    assert!(
        vault
            .propose(&Draft::new("skill").description("Competing").proposal())
            .is_err()
    );
    assert_eq!(
        vault.get("skill", None, None).unwrap().version,
        first.version
    );
    let fresh = Draft::new("skill")
        .expected_version(1)
        .description("Fresh")
        .propose(&mut vault);
    assert_eq!(fresh.version, 2);
}

#[test]
fn publishing_supersedes_legacy_drafts_sharing_its_base() {
    // Databases from before proposals auto-published can still hold drafts;
    // publishing over their shared base marks them superseded.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO skills (id) VALUES ('skill');
         INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version)
         VALUES ('skill', 1, 'Legacy draft', '', 'body', 'evidence', 0);
         INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version)
         VALUES ('skill', 2, 'Rebased draft', '', 'body', 'evidence', 0);",
    )
    .unwrap();
    drop(conn);
    let revision = Draft::new("skill").propose(&mut vault);
    assert_eq!(revision.version, 3);
    let superseded = vault.inspect("skill", 1).unwrap();
    assert_eq!(superseded.status, "superseded");
    assert!(superseded.review_note.unwrap().contains("superseded"));
    assert!(superseded.reviewed_at.is_some());
    assert_eq!(vault.inspect("skill", 2).unwrap().status, "superseded");
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
        .propose(
            &Draft::new("skill")
                .scope("proja")
                .expected_version(1)
                .proposal(),
        )
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
fn a_net_negative_history_demotes_a_stronger_text_match() {
    let (_dir, mut vault) = open();
    let cache = Draft::new("widget-cache")
        .description("Widget caching guide")
        .content("unrelated body")
        .publish(&mut vault);
    Draft::new("other-skill")
        .description("Unrelated description")
        .content("uses a widget somewhere")
        .publish(&mut vault);
    let page = vault.search("widget", None, 20, 0).unwrap();
    assert_eq!(page.skills[0].id, "widget-cache");

    for _ in 0..3 {
        vault
            .record_outcome("widget-cache", cache, "failed", "did not work", None)
            .unwrap();
    }
    let page = vault.search("widget", None, 20, 0).unwrap();
    assert_eq!(
        page.skills
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["other-skill", "widget-cache"],
        "three failures must outweigh a stronger text match"
    );
    assert_eq!(page.total, 2);
}

#[test]
fn equal_helped_and_failed_counts_leave_the_text_ranking_unchanged() {
    let (_dir, mut vault) = open();
    let cache = Draft::new("widget-cache")
        .description("Widget caching guide")
        .content("unrelated body")
        .publish(&mut vault);
    Draft::new("other-skill")
        .description("Unrelated description")
        .content("uses a widget somewhere")
        .publish(&mut vault);
    for result in ["helped", "failed", "helped", "failed", "helped", "failed"] {
        vault
            .record_outcome("widget-cache", cache, result, "n", None)
            .unwrap();
    }
    assert_eq!(
        vault.search("widget", None, 20, 0).unwrap().skills[0].id,
        "widget-cache"
    );
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
fn search_returns_only_the_latest_published_version_and_never_bodies() {
    let (_dir, mut vault) = open();
    Draft::new("skill")
        .content("SECRET body v1")
        .publish(&mut vault);
    let v2 = Draft::new("skill")
        .expected_version(1)
        .content("SECRET body v2")
        .publish(&mut vault);
    let page = vault.search("", None, 20, 0).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.skills[0].id, "skill");
    assert_eq!(page.skills[0].version, v2);
    let json = serde_json::to_value(&page).unwrap();
    assert!(!json.to_string().contains("SECRET"));
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
    assert!(
        vault
            .record_outcome("missing", 1, "helped", "note", None)
            .is_err()
    );
    let published = Draft::new("skill").publish(&mut vault);
    assert!(
        vault
            .record_outcome("skill", published, "helped", "note", None)
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
