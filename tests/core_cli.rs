use std::process::Command;

#[test]
fn human_cli_inspects_and_publishes_exact_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = skillvolution::vault::Vault::open(&db).unwrap();
    vault.propose("example", "Description", "Body", "Evidence", 0).unwrap();
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db").arg(&db).args(args).output().unwrap();
    let listed = run(&["drafts"]);
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    let drafts: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(drafts[0]["version"], 1);
    let shown = run(&["show", "example", "--version", "1"]);
    assert!(shown.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["evidence"], "Evidence");
    assert_eq!(shown["published"], false);
    assert!(vault.get("example", None).is_err());
    let published = run(&["publish", "example", "--version", "1"]);
    assert!(published.status.success());
    assert!(vault.get("example", None).unwrap().published);
    for args in [vec!["show", "example"], vec!["publish", "example"], vec!["show", "missing", "--version", "1"], vec!["publish", "example", "--version", "1"], vec!["unknown"]] {
        let result = run(&args);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
    }
}

#[test]
fn help_includes_setup_without_opening_database() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .env("HOME", dir.path()).env_remove("XDG_DATA_HOME").arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["setup", "init", "drafts", "show", "publish"] {
        assert!(help.contains(command), "missing {command}");
    }
    assert!(!dir.path().join(".local").exists());
}

#[test]
fn default_database_respects_xdg_then_home() {
    for use_xdg in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        command.arg("init").env("HOME", dir.path()).env_remove("XDG_DATA_HOME");
        let base = if use_xdg {
            let base = dir.path().join("xdg data");
            command.env("XDG_DATA_HOME", &base);
            base
        } else {
            dir.path().join(".local/share")
        };
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(base.join("skillvolution/skills.db").exists());
    }
}

#[test]
fn init_is_idempotent_with_global_database_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("space directory/skills.db");
    for _ in 0..2 {
        let result = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
            .arg("--db").arg(&db).arg("init").output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(result.stderr.is_empty());
    }
    assert!(db.exists());
}
