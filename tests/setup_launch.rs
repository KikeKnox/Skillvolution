use std::process::Command;

fn launch() -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/launch.sh").replace('\\', "/"));
    cmd
}

#[test]
fn launcher_builds_installs_and_configures_disposable_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project with spaces");
    let bin_dir = temp.path().join("bin with spaces");
    let db = temp.path().join("data with spaces/vault.sqlite3");
    std::fs::create_dir(&project).unwrap();
    let mut command = launch();
    command
        .args(["--project"])
        .arg(&project)
        .arg("--bin-dir")
        .arg(&bin_dir)
        .arg("--db")
        .arg(&db)
        .arg("--client")
        .arg("both")
        .env(
            "CARGO_TARGET_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/launcher-test"),
        );
    for _ in 0..2 {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert!(
        bin_dir
            .join(if cfg!(windows) {
                "skillvolution.exe"
            } else {
                "skillvolution"
            })
            .is_file()
    );
    assert!(
        std::fs::read(&db)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    let oc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(oc["mcp"]["skillvolution"]["enabled"], true);
    assert!(project.join(".mcp.json").is_file());
    assert!(!project.join("opencode.json.skillvolution.bak").exists());
    assert!(project.join(".claude/settings.local.json").is_file());
}

#[test]
fn launcher_help_and_invalid_arguments_need_no_cargo() {
    let temp = tempfile::tempdir().unwrap();
    let help = launch()
        .arg("--help")
        .env("CARGO_HOME", temp.path())
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let stdout = String::from_utf8_lossy(&help.stdout);
    for flag in [
        "--project",
        "--client",
        "--bin-dir",
        "--db",
        "--project-key",
        "--install-rust",
    ] {
        assert!(stdout.contains(flag), "missing {flag}");
    }
    for args in [
        vec!["--unknown"],
        vec!["--client", "hermes"],
        vec!["--project"],
        vec!["--help", "--unknown"],
    ] {
        let output = launch()
            .args(args)
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!temp.path().join("target").exists());
    }
}
