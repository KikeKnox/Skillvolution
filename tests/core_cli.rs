#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::Vault;
use std::process::{Command, Output};
use support::Draft;

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
fn drafts_and_show_and_diff_expose_pending_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("example")
        .description("Use when testing")
        .propose(&mut vault);
    drop(vault);

    let listed = run(&db, &["drafts", "--json"]);
    assert_success(&listed);
    let drafts: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(drafts[0]["id"], "example");
    assert_eq!(drafts[0]["version"], 1);
    assert_eq!(drafts[0]["status"], "draft");

    let table = run(&db, &["drafts"]);
    assert_success(&table);
    assert!(String::from_utf8_lossy(&table.stdout).contains("example"));

    let shown = run(&db, &["show", "example", "--version", "1"]);
    assert_success(&shown);
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["evidence"], "Observed / Tried / Result");
    assert_eq!(shown["status"], "draft");

    let diff = run(&db, &["diff", "example", "--version", "1"]);
    assert_success(&diff);
    assert!(String::from_utf8_lossy(&diff.stdout).contains("+description"));
}

#[test]
fn publish_reject_deprecate_undeprecate_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let vault = Vault::open(&db).unwrap();
    drop(vault);

    run(&db, &["init"]);
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("published-skill").propose(&mut vault);
    Draft::new("rejected-skill").propose(&mut vault);
    drop(vault);

    let published = run(&db, &["publish", "published-skill", "--version", "1"]);
    assert_success(&published);
    let vault = Vault::open(&db).unwrap();
    assert!(vault.get("published-skill", None, None).is_ok());
    drop(vault);

    let rejected = run(
        &db,
        &[
            "reject",
            "rejected-skill",
            "--version",
            "1",
            "--note",
            "not reusable",
        ],
    );
    assert_success(&rejected);
    let vault = Vault::open(&db).unwrap();
    assert_eq!(
        vault.inspect("rejected-skill", 1).unwrap().status,
        "rejected"
    );
    drop(vault);

    let deprecated = run(&db, &["deprecate", "published-skill"]);
    assert_success(&deprecated);
    let vault = Vault::open(&db).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 0);
    drop(vault);

    let undeprecated = run(&db, &["undeprecate", "published-skill"]);
    assert_success(&undeprecated);
    let vault = Vault::open(&db).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 1);
}

#[test]
fn outcomes_table_json_and_failing_filter() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    let helped = Draft::new("helped-skill").publish(&mut vault);
    let failed = Draft::new("failed-skill").publish(&mut vault);
    vault
        .record_outcome(
            "helped-skill",
            helped.version,
            "helped",
            "worked",
            None,
            None,
        )
        .unwrap();
    vault
        .record_outcome(
            "failed-skill",
            failed.version,
            "failed",
            "broke",
            None,
            None,
        )
        .unwrap();
    drop(vault);

    let summary = run(&db, &["outcomes", "--json"]);
    assert_success(&summary);
    let summary: serde_json::Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(summary.as_array().unwrap().len(), 2);

    let failing = run(&db, &["outcomes", "--failing", "--json"]);
    assert_success(&failing);
    let failing: serde_json::Value = serde_json::from_slice(&failing.stdout).unwrap();
    assert_eq!(failing.as_array().unwrap().len(), 1);
    assert_eq!(failing[0]["id"], "failed-skill");

    let log = run(&db, &["outcomes", "failed-skill", "--json"]);
    assert_success(&log);
    let log: serde_json::Value = serde_json::from_slice(&log.stdout).unwrap();
    assert_eq!(log[0]["note"], "broke");

    let table = run(&db, &["outcomes"]);
    assert_success(&table);
    assert!(String::from_utf8_lossy(&table.stdout).contains("helped-skill"));
}

#[test]
fn cli_error_cases_produce_stderr_and_no_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("example").publish(&mut vault);
    drop(vault);

    for args in [
        vec!["show", "example"],
        vec!["publish", "example"],
        vec!["show", "missing", "--version", "1"],
        vec!["publish", "example", "--version", "1"],
        vec!["reject", "example", "--version", "1"],
        vec!["deprecate", "missing-skill"],
        vec!["outcomes", "example", "--failing"],
        vec!["unknown"],
    ] {
        let result = run(&db, &args);
        assert!(!result.status.success(), "expected failure for {args:?}");
        assert!(result.stdout.is_empty(), "unexpected stdout for {args:?}");
        assert!(!result.stderr.is_empty(), "expected stderr for {args:?}");
    }
}

#[test]
fn help_lists_commands_without_opening_database() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .env("HOME", dir.path())
        .env_remove("XDG_DATA_HOME")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "setup",
        "init",
        "serve",
        "drafts",
        "show",
        "diff",
        "publish",
        "reject",
        "deprecate",
        "undeprecate",
        "outcomes",
        "hook",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
    assert!(!dir.path().join(".local").exists());
}

#[test]
fn default_database_respects_xdg_then_home() {
    for use_xdg in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        command
            .arg("init")
            .env("HOME", dir.path())
            .env_remove("XDG_DATA_HOME");
        let base = if use_xdg {
            let base = dir.path().join("xdg data");
            command.env("XDG_DATA_HOME", &base);
            base
        } else {
            dir.path().join(".local/share")
        };
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(base.join("skillvolution/skills.db").exists());
    }
}

#[test]
fn init_is_idempotent_with_global_database_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("space directory/skills.db");
    for _ in 0..2 {
        let result = run(&db, &["init"]);
        assert_success(&result);
        assert!(result.stderr.is_empty());
    }
    assert!(db.exists());
}
