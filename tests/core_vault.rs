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
    // Two projects, since one project's same-day reports collapse into one.
    for project in ["proja", "projb"] {
        vault
            .record_outcome("proven", v1, "helped", "worked", Some(project))
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
    for (result, project) in [("helped", "proja"), ("failed", "projb")] {
        vault
            .record_outcome("unproven", net_zero, result, "note", Some(project))
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

#[test]
fn rejects_credential_shaped_values_in_the_verdict_reason_and_outcome_note() {
    let (_dir, mut vault) = open();
    let token = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
    let error = vault
        .propose(
            &Draft::new("skill")
                .verdict_reason(&format!("Reproduced with {token}"))
                .proposal(),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("verdict_reason"), "{error}");
    assert!(!error.contains(token), "{error}");

    let published = Draft::new("skill").publish(&mut vault);
    let error = vault
        .record_outcome("skill", published, "helped", &format!("used {token}"), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("note"), "{error}");
    assert!(!error.contains(token), "{error}");
    assert!(vault.outcomes("skill").unwrap().is_empty());
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
         INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version, status)
         VALUES ('skill', 1, 'Legacy draft', '', 'body', 'evidence', 0, 'draft');
         INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version, status)
         VALUES ('skill', 2, 'Rebased draft', '', 'body', 'evidence', 0, 'draft');",
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

#[test]
fn latest_version_is_the_highest_version_of_any_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    Draft::new("skill").publish(&mut vault);
    Draft::new("skill").expected_version(1).publish(&mut vault);
    assert_eq!(vault.latest_version("skill").unwrap(), 2);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version, status)
         VALUES ('skill', 3, 'Rejected', '', 'body', 'evidence', 2, 'rejected');",
    )
    .unwrap();
    assert_eq!(vault.latest_version("skill").unwrap(), 3);

    let error = vault.latest_version("missing").unwrap_err().to_string();
    assert!(error.contains("unknown skill: missing"), "{error}");
}

#[test]
fn new_revisions_default_to_published() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let vault = Vault::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO skills (id) VALUES ('skill');
         INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version)
         VALUES ('skill', 1, 'Raw', '', 'body', 'evidence', 0);",
    )
    .unwrap();
    assert_eq!(vault.inspect("skill", 1).unwrap().status, "published");
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

    for project in ["proja", "projb", "projc"] {
        vault
            .record_outcome(
                "widget-cache",
                cache,
                "failed",
                "did not work",
                Some(project),
            )
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
    let results = ["helped", "failed", "helped", "failed", "helped", "failed"];
    for (index, result) in results.into_iter().enumerate() {
        let project = format!("proj{index}");
        vault
            .record_outcome("widget-cache", cache, result, "n", Some(&project))
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

#[test]
fn republishing_replaces_the_search_entry_of_the_previous_version() {
    let (_dir, mut vault) = open();
    Draft::new("skill").content("alpha").publish(&mut vault);
    Draft::new("skill")
        .expected_version(1)
        .content("beta")
        .publish(&mut vault);
    assert_eq!(vault.search("alpha", None, 20, 0).unwrap().total, 0);
    assert_eq!(vault.search("beta", None, 20, 0).unwrap().total, 1);
}

#[test]
fn open_and_search_do_not_wait_for_a_concurrent_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    Draft::new("skill").publish(&mut vault);
    drop(vault);

    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 1);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "open waited {:?} for the writer",
        started.elapsed()
    );
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
        .record_outcome("skill", v1, "helped", "n", Some("proja"))
        .unwrap();
    vault
        .record_outcome("skill", v1, "failed", "n", Some("projb"))
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
        .record_outcome("skill", published, "helped", "first", Some("proja"))
        .unwrap();
    vault
        .record_outcome("skill", published, "failed", "second", Some("projb"))
        .unwrap();
    let log = vault.outcomes("skill").unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].note, "second");
    assert_eq!(log[1].note, "first");
}

#[test]
fn same_day_reports_from_one_project_collapse_into_the_last_one() {
    let (_dir, mut vault) = open();
    let published = Draft::new("skill").publish(&mut vault);
    for (result, note) in [
        ("helped", "first"),
        ("failed", "second"),
        ("helped", "third"),
    ] {
        vault
            .record_outcome("skill", published, result, note, None)
            .unwrap();
    }
    vault
        .record_outcome("skill", published, "failed", "other project", Some("proja"))
        .unwrap();
    let log = vault.outcomes("skill").unwrap();
    assert_eq!(log.len(), 2, "{log:?}");
    let global = log.iter().find(|record| record.project.is_none()).unwrap();
    assert_eq!(
        (global.result.as_str(), global.note.as_str()),
        ("helped", "third")
    );
}

#[test]
fn outcomes_of_an_unknown_skill_is_an_error() {
    let (_dir, vault) = open();
    let error = vault.outcomes("missing").unwrap_err().to_string();
    assert!(error.contains("unknown skill: missing"), "{error}");
}

// --- hook state ------------------------------------------------------------

/// Marks every hook-state row as last touched on `timestamp`, bypassing the
/// triggers that stamp `updated_at` on real writes.
fn backdate_hook_state(path: &std::path::Path, timestamp: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute("UPDATE hook_state SET updated_at = ?1", [timestamp])
        .unwrap();
    conn.execute("UPDATE devin_hook_state SET updated_at = ?1", [timestamp])
        .unwrap();
}

#[test]
fn prune_hook_state_drops_only_sessions_idle_past_the_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let vault = Vault::open(&path).unwrap();
    vault.set_transcript_offset("old", 10).unwrap();
    vault.set_transcript_offset("touched", 20).unwrap();
    vault.mark_devin_work("old-devin").unwrap();
    backdate_hook_state(&path, "2000-01-01T00:00:00Z");
    // A later write refreshes updated_at, so this session survives the prune.
    vault.set_transcript_offset("touched", 30).unwrap();

    vault.prune_hook_state(30).unwrap();

    assert_eq!(vault.transcript_offset("old").unwrap(), 0);
    assert_eq!(vault.transcript_offset("touched").unwrap(), 30);
    assert_eq!(
        vault.take_devin_hook_state("old-devin").unwrap(),
        (false, false)
    );
}

#[test]
fn open_prunes_hook_state_older_than_thirty_days() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let vault = Vault::open(&path).unwrap();
    vault.set_transcript_offset("stale", 10).unwrap();
    vault.mark_devin_work("stale-devin").unwrap();
    vault.mark_devin_review("stale-devin").unwrap();
    backdate_hook_state(&path, "2000-01-01T00:00:00Z");
    drop(vault);

    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.transcript_offset("stale").unwrap(), 0);
    assert_eq!(
        vault.take_devin_hook_state("stale-devin").unwrap(),
        (false, false)
    );
}

// --- migration -----------------------------------------------------------

const APPLICATION_ID: i64 = 0x534B_5631;

/// The version-1 schema as it shipped, before devin_hook_state existed.
const V1_SCHEMA: &str = "
CREATE TABLE skills (
    id TEXT PRIMARY KEY,
    scope TEXT,
    deprecated INTEGER NOT NULL DEFAULT 0 CHECK(deprecated IN (0, 1))
);
CREATE TABLE revisions (
    id TEXT NOT NULL REFERENCES skills(id),
    version INTEGER NOT NULL CHECK(version > 0),
    description TEXT NOT NULL,
    tags TEXT NOT NULL,
    content TEXT NOT NULL,
    evidence TEXT NOT NULL,
    expected_version INTEGER NOT NULL CHECK(expected_version >= 0),
    status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft', 'published', 'rejected', 'superseded')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    reviewed_at TEXT,
    review_note TEXT,
    PRIMARY KEY (id, version)
);
CREATE TABLE outcomes (
    id TEXT NOT NULL,
    version INTEGER NOT NULL,
    result TEXT NOT NULL CHECK(result IN ('helped', 'failed', 'not_applicable')),
    note TEXT NOT NULL,
    project TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    FOREIGN KEY (id, version) REFERENCES revisions(id, version)
);
CREATE INDEX outcomes_by_revision ON outcomes(id, version, result);
CREATE TABLE hook_state (
    session_id TEXT PRIMARY KEY,
    transcript_offset INTEGER NOT NULL
);
CREATE VIRTUAL TABLE skills_fts USING fts5(
    id, description, tags, content,
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE VIEW current_skills AS
SELECT r.id, r.version, r.description, r.tags, r.content, s.scope, s.deprecated,
    (SELECT COUNT(*) FROM outcomes o WHERE o.id = r.id AND o.version = r.version AND o.result = 'helped') AS helped,
    (SELECT COUNT(*) FROM outcomes o WHERE o.id = r.id AND o.version = r.version AND o.result = 'failed') AS failed,
    (SELECT COUNT(*) FROM outcomes o WHERE o.id = r.id AND o.version = r.version AND o.result = 'not_applicable') AS not_applicable
FROM skills s
JOIN revisions r ON r.id = s.id
WHERE r.status = 'published'
    AND r.version = (SELECT MAX(p.version) FROM revisions p WHERE p.id = r.id AND p.status = 'published');
";

/// What version 2 added on top of version 1.
const V2_ADDITIONS: &str = "
CREATE TABLE devin_hook_state (
    session_id TEXT PRIMARY KEY,
    worked INTEGER NOT NULL DEFAULT 0 CHECK(worked IN (0, 1)),
    reviewed INTEGER NOT NULL DEFAULT 0 CHECK(reviewed IN (0, 1))
);
";

/// Builds a database at schema `version` (1 or 2) the way older releases left
/// it: two published revisions, a legacy draft, a search index still holding
/// the superseded v1 text, and a duplicated same-day outcome.
fn legacy_database(version: i64) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(V1_SCHEMA).unwrap();
    if version >= 2 {
        conn.execute_batch(V2_ADDITIONS).unwrap();
        conn.execute_batch("INSERT INTO devin_hook_state VALUES ('devin-session', 1, 0);")
            .unwrap();
    }
    let alpha = support::sectioned("alpha");
    let beta = support::sectioned("beta");
    conn.execute_batch("INSERT INTO skills (id) VALUES ('skill');")
        .unwrap();
    for (revision, content, status) in [
        (1, &alpha, "published"),
        (2, &beta, "published"),
        (3, &beta, "draft"),
    ] {
        conn.execute(
            "INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version, status)
             VALUES ('skill', ?1, 'Use when testing', 'rust', ?2, 'evidence', ?1 - 1, ?3)",
            rusqlite::params![revision, content, status],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO skills_fts (id, description, tags, content) VALUES ('skill', 'Use when testing', 'rust', ?1)",
        [&alpha],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO outcomes (id, version, result, note) VALUES ('skill', 2, 'helped', 'first');
         INSERT INTO outcomes (id, version, result, note) VALUES ('skill', 2, 'failed', 'second');
         INSERT INTO outcomes (id, version, result, note, project) VALUES ('skill', 2, 'helped', 'elsewhere', 'proja');
         INSERT INTO hook_state VALUES ('claude-session', 42);",
    )
    .unwrap();
    conn.pragma_update(None, "user_version", version).unwrap();
    (dir, path)
}

fn pragma(path: &std::path::Path, name: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .pragma_query_value(None, name, |row| row.get(0))
        .unwrap()
}

fn open_error(path: &std::path::Path) -> String {
    match Vault::open(path) {
        Ok(_) => panic!("expected {} to be refused", path.display()),
        Err(error) => error.to_string(),
    }
}

#[test]
fn new_databases_are_stamped_with_the_version_and_application_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    drop(Vault::open(&path).unwrap());
    assert_eq!(pragma(&path, "user_version"), 3);
    assert_eq!(pragma(&path, "application_id"), APPLICATION_ID);
}

#[test]
fn migrates_a_v1_database_to_v3() {
    let (_dir, path) = legacy_database(1);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(pragma(&path, "user_version"), 3);
    assert_eq!(pragma(&path, "application_id"), APPLICATION_ID);
    vault.mark_devin_work("devin-session").unwrap();
    assert_eq!(
        vault.take_devin_hook_state("devin-session").unwrap(),
        (true, false)
    );
    assert_eq!(vault.search("beta", None, 20, 0).unwrap().total, 1);
}

#[test]
fn migrates_a_v2_database_to_v3_preserving_data() {
    let (_dir, path) = legacy_database(2);
    let mut vault = Vault::open(&path).unwrap();
    assert_eq!(pragma(&path, "user_version"), 3);
    assert_eq!(pragma(&path, "application_id"), APPLICATION_ID);

    // Revisions survive; the legacy draft is retired.
    assert_eq!(
        vault.inspect("skill", 1).unwrap().content,
        support::sectioned("alpha")
    );
    assert_eq!(vault.get("skill", None, None).unwrap().version, 2);
    let draft = vault.inspect("skill", 3).unwrap();
    assert_eq!(draft.status, "superseded");
    assert_eq!(
        draft.review_note.as_deref(),
        Some("legacy draft retired by schema v3")
    );

    // The search index is rebuilt from the current published revision.
    assert_eq!(vault.search("alpha", None, 20, 0).unwrap().total, 0);
    assert_eq!(vault.search("beta", None, 20, 0).unwrap().total, 1);

    // Duplicate same-day outcomes keep only the latest report.
    let notes: Vec<String> = vault
        .outcomes("skill")
        .unwrap()
        .into_iter()
        .map(|record| record.note)
        .collect();
    assert_eq!(notes, ["elsewhere", "second"]);

    assert_eq!(vault.transcript_offset("claude-session").unwrap(), 42);
    assert_eq!(
        vault.take_devin_hook_state("devin-session").unwrap(),
        (true, false)
    );

    // Republishing after the migration still replaces the search entry.
    Draft::new("skill")
        .expected_version(2)
        .content("gamma")
        .publish(&mut vault);
    assert_eq!(vault.search("beta", None, 20, 0).unwrap().total, 0);
    assert_eq!(vault.search("gamma", None, 20, 0).unwrap().total, 1);
}

#[test]
fn refuses_a_newer_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    drop(Vault::open(&path).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let error = open_error(&path);
    assert!(error.contains("upgrade skillvolution"), "{error}");
}

#[test]
fn refuses_a_foreign_nonempty_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE notes (body TEXT);")
        .unwrap();
    let error = open_error(&path);
    assert!(error.contains("not a skillvolution database"), "{error}");
}

#[test]
fn refuses_to_migrate_a_pre_1_schema_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE revisions (id TEXT PRIMARY KEY);")
        .unwrap();
    drop(conn);
    let error = open_error(&path);
    assert!(error.contains("pre-1 schema"), "{error}");
}
