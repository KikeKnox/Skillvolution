//! Tests for `Vault::purge`, `export_json`/`import_json`, and `backup`
//! (`src/vault/transfer.rs`), plus the corresponding CLI commands.

#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::Vault;
use std::process::{Command, Output};
use support::Draft;

fn open() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    (dir, vault)
}

/// Drops `exported_at` before comparing two export documents, since it's the
/// one field a re-export is never expected to reproduce.
fn without_exported_at(json: &str) -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
    value["exported_at"] = serde_json::Value::Null;
    value
}

// --- purge ---------------------------------------------------------------

#[test]
fn purge_unknown_skill_or_version_errors() {
    let (_dir, mut vault) = open();
    assert!(
        vault
            .purge("missing", None)
            .unwrap_err()
            .to_string()
            .contains("unknown skill: missing")
    );
    Draft::new("skill-known").propose(&mut vault);
    assert!(vault.purge("skill-known", Some(2)).is_err());
}

#[test]
fn purge_single_version_leaves_earlier_versions_searchable() {
    let (_dir, mut vault) = open();
    Draft::new("skill-a").propose(&mut vault);
    Draft::new("skill-a")
        .content("uniquewordonlyinversiontwo")
        .expected_version(1)
        .propose(&mut vault);

    let report = vault.purge("skill-a", Some(2)).unwrap();
    assert_eq!(report.revisions, 1);
    assert_eq!(report.outcomes, 0);
    assert!(!report.skill_removed);

    let view = vault.get("skill-a", None, None).unwrap();
    assert_eq!(view.version, 1);
    assert!(vault.inspect("skill-a", 2).is_err());
    assert_eq!(
        vault
            .search("uniquewordonlyinversiontwo", None, 20, 0)
            .unwrap()
            .total,
        0
    );
}

#[test]
fn purge_whole_skill_removes_search_get_and_outcomes() {
    let (_dir, mut vault) = open();
    Draft::new("skill-b").propose(&mut vault);
    vault
        .record_outcome("skill-b", 1, "helped", "note", None)
        .unwrap();

    let report = vault.purge("skill-b", None).unwrap();
    assert!(report.skill_removed);
    assert_eq!(report.revisions, 1);
    assert_eq!(report.outcomes, 1);

    assert!(vault.get("skill-b", None, None).is_err());
    assert!(vault.outcomes("skill-b").is_err());
    assert_eq!(vault.search("skill-b", None, 20, 0).unwrap().total, 0);
}

#[test]
fn purge_and_vacuum_erase_the_purged_content_from_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let marker = "sentinelmarkerthatshouldbeerased987654321";
    {
        let mut vault = Vault::open(&db).unwrap();
        Draft::new("secret-skill")
            .content(marker)
            .propose(&mut vault);
        vault.purge("secret-skill", None).unwrap();
    }
    let bytes = std::fs::read(&db).unwrap();
    assert!(
        !contains(&bytes, marker.as_bytes()),
        "marker survived in the main file"
    );
    let wal_path = std::path::PathBuf::from(format!("{}-wal", db.display()));
    if wal_path.exists() {
        let wal_bytes = std::fs::read(&wal_path).unwrap();
        assert!(
            !contains(&wal_bytes, marker.as_bytes()),
            "marker survived in the WAL"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn diff_handles_a_purged_base_revision() {
    let (_dir, mut vault) = open();
    Draft::new("skill-c").propose(&mut vault);
    Draft::new("skill-c")
        .content("second body")
        .expected_version(1)
        .propose(&mut vault);

    vault.purge("skill-c", Some(1)).unwrap();

    let diff = vault.diff("skill-c", 2).unwrap();
    assert!(diff.contains("purged"), "{diff}");
    assert!(vault.inspect("skill-c", 2).is_ok());
}

// --- export / import -------------------------------------------------------

#[test]
fn export_then_import_into_a_fresh_vault_reproduces_the_export() {
    let (_dir, mut source) = open();
    Draft::new("skill-d").propose(&mut source);
    Draft::new("skill-d")
        .content("v2 body")
        .expected_version(1)
        .propose(&mut source);
    source
        .record_outcome("skill-d", 1, "helped", "note", None)
        .unwrap();

    let exported = source.export_json().unwrap();

    let (_dir2, mut target) = open();
    let report = target.import_json(&exported).unwrap();
    assert_eq!(report.revisions_imported, 2);
    assert_eq!(report.revisions_skipped, 0);
    assert_eq!(report.outcomes_imported, 1);
    assert_eq!(report.outcomes_skipped, 0);

    let reexported = target.export_json().unwrap();
    assert_eq!(
        without_exported_at(&exported),
        without_exported_at(&reexported)
    );
}

#[test]
fn importing_the_same_document_twice_is_idempotent() {
    let (_dir, mut vault) = open();
    Draft::new("skill-e").propose(&mut vault);
    vault
        .record_outcome("skill-e", 1, "helped", "note", None)
        .unwrap();
    let exported = vault.export_json().unwrap();

    let (_dir2, mut target) = open();
    target.import_json(&exported).unwrap();
    let report = target.import_json(&exported).unwrap();
    assert_eq!(report.revisions_imported, 0);
    assert_eq!(report.revisions_skipped, 1);
    assert_eq!(report.outcomes_imported, 0);
    assert_eq!(report.outcomes_skipped, 1);
}

#[test]
fn imported_skills_are_searchable() {
    let (_dir, mut source) = open();
    Draft::new("skill-i")
        .content("uniquesearchterm")
        .propose(&mut source);
    let exported = source.export_json().unwrap();

    let (_dir2, mut target) = open();
    target.import_json(&exported).unwrap();
    assert_eq!(
        target
            .search("uniquesearchterm", None, 20, 0)
            .unwrap()
            .total,
        1
    );
}

#[test]
fn conflicting_revision_aborts_the_whole_import_with_no_partial_writes() {
    let (_dir, mut vault) = open();
    Draft::new("skill-f").propose(&mut vault);

    // skill-h is new and valid, listed first; skill-f's revision conflicts
    // with the one already in the vault. If the transaction rolled back
    // correctly, skill-h's insert (earlier in the same transaction) must not
    // have stuck either.
    let document = serde_json::json!({
        "format": "skillvolution-export",
        "version": 1,
        "exported_at": "2024-01-01T00:00:00Z",
        "skills": [
            {
                "id": "skill-h",
                "scope": null,
                "deprecated": false,
                "revisions": [{
                    "id": "skill-h", "version": 1,
                    "description": "Use when working with skill-h",
                    "tags": [],
                    "content": support::sectioned("Body for skill-h"),
                    "evidence": "Observed / Tried / Result",
                    "expected_version": 0,
                    "status": "published",
                    "created_at": "2024-01-01T00:00:00Z",
                    "reviewed_at": "2024-01-01T00:00:00Z",
                    "review_note": "keep global: reason"
                }],
                "outcomes": []
            },
            {
                "id": "skill-f",
                "scope": null,
                "deprecated": false,
                "revisions": [{
                    "id": "skill-f", "version": 1,
                    "description": "a different description entirely",
                    "tags": [],
                    "content": support::sectioned("different body"),
                    "evidence": "Observed / Tried / Result",
                    "expected_version": 0,
                    "status": "published",
                    "created_at": "2024-01-01T00:00:00Z",
                    "reviewed_at": "2024-01-01T00:00:00Z",
                    "review_note": "keep global: reason"
                }],
                "outcomes": []
            }
        ]
    });

    let error = vault
        .import_json(&document.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("skill-f"), "{error}");
    assert!(error.contains("v1"), "{error}");
    assert!(vault.get("skill-h", None, None).is_err());
}

#[test]
fn import_refuses_a_revision_with_a_secret_shaped_value() {
    let (_dir, mut vault) = open();
    let secret_content = support::sectioned("key AKIAA1b2C3d4E5f6G7h8 appears here");
    let document = serde_json::json!({
        "format": "skillvolution-export",
        "version": 1,
        "exported_at": "2024-01-01T00:00:00Z",
        "skills": [{
            "id": "skill-j",
            "scope": null,
            "deprecated": false,
            "revisions": [{
                "id": "skill-j", "version": 1,
                "description": "Use when working with skill-j",
                "tags": [],
                "content": secret_content,
                "evidence": "Observed / Tried / Result",
                "expected_version": 0,
                "status": "published",
                "created_at": "2024-01-01T00:00:00Z",
                "reviewed_at": "2024-01-01T00:00:00Z",
                "review_note": "keep global: reason"
            }],
            "outcomes": []
        }]
    });

    let error = vault
        .import_json(&document.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("credentials"), "{error}");
    assert!(vault.get("skill-j", None, None).is_err());
}

#[test]
fn import_rejects_a_skill_that_exists_with_a_different_scope() {
    let (_dir, mut vault) = open();
    Draft::new("skill-k").scope("project-a").propose(&mut vault);
    let document = serde_json::json!({
        "format": "skillvolution-export",
        "version": 1,
        "exported_at": "2024-01-01T00:00:00Z",
        "skills": [{
            "id": "skill-k",
            "scope": "project-b",
            "deprecated": false,
            "revisions": [],
            "outcomes": []
        }]
    });
    let error = vault
        .import_json(&document.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("skill-k"), "{error}");
}

#[test]
fn import_rejects_revisions_or_outcomes_filed_under_another_skill() {
    let (_dir, mut vault) = open();
    Draft::new("victim").scope("project-a").propose(&mut vault);
    let revision = serde_json::json!({
        "id": "victim", "version": 2,
        "description": "Use when working with victim",
        "tags": [],
        "content": support::sectioned("smuggled body"),
        "evidence": "Observed / Tried / Result",
        "expected_version": 1,
        "status": "published",
        "created_at": "2024-01-01T00:00:00Z",
        "reviewed_at": "2024-01-01T00:00:00Z",
        "review_note": "keep global: reason"
    });
    let outcome = serde_json::json!({
        "id": "victim", "version": 1, "result": "failed", "note": "smuggled",
        "project": null, "created_at": "2024-01-01T00:00:00Z"
    });

    for (revisions, outcomes) in [(vec![revision], vec![]), (vec![], vec![outcome])] {
        let document = serde_json::json!({
            "format": "skillvolution-export",
            "version": 1,
            "exported_at": "2024-01-01T00:00:00Z",
            "skills": [{
                "id": "outer",
                "scope": null,
                "deprecated": false,
                "revisions": revisions,
                "outcomes": outcomes
            }]
        });
        let error = vault
            .import_json(&document.to_string())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("victim") && error.contains("outer"),
            "{error}"
        );
        assert!(vault.inspect("victim", 2).is_err());
        assert!(vault.outcomes("victim").unwrap().is_empty());
        assert!(vault.inspect("outer", 1).is_err());
    }
}

#[test]
fn import_rejects_an_unsupported_document_version() {
    let (_dir, mut vault) = open();
    let document = serde_json::json!({
        "format": "skillvolution-export",
        "version": 2,
        "exported_at": "2024-01-01T00:00:00Z",
        "skills": []
    });
    assert!(vault.import_json(&document.to_string()).is_err());
}

// --- backup ----------------------------------------------------------------

#[test]
fn backup_produces_an_openable_copy_with_the_same_content() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut source = Vault::open(&db).unwrap();
    Draft::new("skill-l").propose(&mut source);

    let backup_path = dir.path().join("backup.db");
    source.backup(&backup_path).unwrap();

    let backup = Vault::open(&backup_path).unwrap();
    assert_eq!(
        without_exported_at(&source.export_json().unwrap()),
        without_exported_at(&backup.export_json().unwrap())
    );

    let error = source.backup(&backup_path).unwrap_err().to_string();
    assert!(error.contains("already exists"), "{error}");
}

// --- CLI ---------------------------------------------------------------

fn run(db: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_purge_deletes_a_revision_and_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("cli-purge").propose(&mut vault);
    Draft::new("cli-purge")
        .content("second")
        .expected_version(1)
        .propose(&mut vault);
    drop(vault);

    let purged = run(&db, &["purge", "cli-purge", "--version", "1"]);
    assert_success(&purged);
    let stdout = String::from_utf8_lossy(&purged.stdout);
    assert!(
        stdout.contains("Purged cli-purge: 1 revision(s), 0 outcome(s)."),
        "{stdout}"
    );

    let vault = Vault::open(&db).unwrap();
    assert!(vault.inspect("cli-purge", 1).is_err());
    assert!(vault.inspect("cli-purge", 2).is_ok());
}

#[test]
fn cli_export_then_import_roundtrips_through_files() {
    let dir = tempfile::tempdir().unwrap();
    let source_db = dir.path().join("source.db");
    let mut vault = Vault::open(&source_db).unwrap();
    Draft::new("cli-export").propose(&mut vault);
    drop(vault);

    let export_path = dir.path().join("export.json");
    let exported = run(
        &source_db,
        &["export", "--output", export_path.to_str().unwrap()],
    );
    assert_success(&exported);
    assert!(export_path.exists());

    let target_db = dir.path().join("target.db");
    let imported = run(&target_db, &["import", export_path.to_str().unwrap()]);
    assert_success(&imported);
    let report: serde_json::Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(report["revisions_imported"], 1);

    let target = Vault::open(&target_db).unwrap();
    assert!(target.get("cli-export", None, None).is_ok());
}
