#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::Vault;
use std::{
    fs,
    process::{Command, Output},
};
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
fn drafts_and_show_expose_pending_revisions() {
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
    let table_out = String::from_utf8_lossy(&table.stdout);
    assert!(table_out.contains("example"));
    assert!(table_out.contains("Use when testing"));

    let shown = run(&db, &["show", "example", "--version", "1"]);
    assert_success(&shown);
    let shown_out = String::from_utf8_lossy(&shown.stdout);
    assert!(shown_out.contains("example v1"));
    assert!(shown_out.contains("draft"));
    assert!(shown_out.contains("Observed / Tried / Result"));
    assert!(shown_out.contains("+description"));

    let shown_json = run(&db, &["show", "example", "--version", "1", "--json"]);
    assert_success(&shown_json);
    let shown_json: serde_json::Value = serde_json::from_slice(&shown_json.stdout).unwrap();
    assert_eq!(shown_json["id"], "example");
    assert_eq!(shown_json["status"], "draft");
}

#[test]
fn publish_reject_deprecate_undeprecate_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("published-skill").propose(&mut vault);
    Draft::new("rejected-skill").propose(&mut vault);
    drop(vault);

    let published = run(&db, &["publish", "published-skill", "--version", "1"]);
    assert_success(&published);
    assert!(String::from_utf8_lossy(&published.stdout).contains("Published published-skill v1"));
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
    assert!(String::from_utf8_lossy(&rejected.stdout).contains("Rejected rejected-skill v1"));
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
        .record_outcome("helped-skill", helped, "helped", "worked", None)
        .unwrap();
    vault
        .record_outcome("failed-skill", failed, "failed", "broke", None)
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
        vec!["show", "missing", "--version", "1"],
        vec!["publish", "example", "--version", "1"],
        vec!["reject", "example", "--version", "1"],
        vec!["deprecate", "missing-skill"],
        vec!["outcomes", "example", "--failing"],
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
    for command in ["setup", "serve", "show", "hook"] {
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
            .arg("drafts")
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
fn setup_defaults_bin_project_and_xdg_database() {
    let home = tempfile::tempdir().unwrap();
    let xdg_base = home.path().join("xdg data");
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("my-project");
    fs::create_dir(&project).unwrap();

    let bin = env!("CARGO_BIN_EXE_skillvolution");
    let canonical_bin = fs::canonicalize(bin).unwrap();

    let output = Command::new(bin)
        .arg("setup")
        .arg("--client")
        .arg("claude-code")
        .current_dir(&project)
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", &xdg_base)
        .output()
        .unwrap();
    assert_success(&output);

    let mcp: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join(".mcp.json")).unwrap()).unwrap();
    let server = &mcp["mcpServers"]["skillvolution"];
    // --bin defaulted to this test's own binary, canonicalized.
    assert_eq!(
        server["command"].as_str().unwrap(),
        canonical_bin.to_str().unwrap()
    );
    // --db defaulted to $XDG_DATA_HOME/skillvolution/skills.db.
    let db_path = xdg_base.join("skillvolution/skills.db");
    assert_eq!(
        server["args"][1].as_str().unwrap(),
        db_path.to_str().unwrap()
    );
    assert!(db_path.is_file(), "setup must initialize the database");
    // --project defaulted to the current directory, so the key comes from its name.
    assert_eq!(server["args"][4], "my-project");
}

#[test]
fn setup_honours_global_db_flag() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    let db = dir.path().join("global.db");

    let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("setup")
        .arg("--project")
        .arg(&project)
        .arg("--client")
        .arg("claude-code")
        .output()
        .unwrap();
    assert_success(&output);

    let mcp: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join(".mcp.json")).unwrap()).unwrap();
    let args = mcp["mcpServers"]["skillvolution"]["args"]
        .as_array()
        .unwrap();
    assert_eq!(args[1].as_str().unwrap(), db.to_str().unwrap());
    assert!(db.is_file());
}

#[test]
fn opening_the_database_is_idempotent_with_a_global_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("space directory/skills.db");
    for _ in 0..2 {
        let result = run(&db, &["drafts"]);
        assert_success(&result);
        assert!(result.stderr.is_empty());
    }
    assert!(db.exists());
}
