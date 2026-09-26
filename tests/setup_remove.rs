//! `skillvolution setup --remove` and `setup --dry-run`. Project-scope tests run
//! in-process (like `tests/setup.rs`); the one global-scope test runs the built binary as
//! a subprocess with a fake `claude` on PATH (like `tests/setup_global.rs`), since HOME
//! and PATH are process-global.

use skillvolution::setup;

use clap::Parser;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    setup: setup::SetupArgs,
}

/// A directory with spaces (covering shell quoting) containing a file literally named
/// `skillvolution`: hook ownership (see `owned_command` in `src/setup/hooks.rs`) matches
/// on that exact file name, so a rerun (here, a remove) must recognize it.
const BIN_NAME: &str = "bin dir with spaces/skillvolution";

fn run(argv: Vec<String>) -> anyhow::Result<()> {
    setup::run(Cli::parse_from(argv).setup)
}

fn write_bin(project: &Path) -> PathBuf {
    let bin = project.join(BIN_NAME);
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, "test binary").unwrap();
    bin
}

fn install_project(project: &Path, bin: &Path, db: &Path, client: &str) -> anyhow::Result<()> {
    run(vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        client.to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ])
}

fn dry_run_install_project(
    project: &Path,
    bin: &Path,
    db: &Path,
    client: &str,
) -> anyhow::Result<()> {
    run(vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        client.to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
        "--dry-run".to_owned(),
    ])
}

fn remove_project(
    project: &Path,
    db: &Path,
    client: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let mut argv = vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--remove".to_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ];
    if let Some(client) = client {
        argv.push("--client".to_owned());
        argv.push(client.to_owned());
    }
    if dry_run {
        argv.push("--dry-run".to_owned());
    }
    run(argv)
}

/// The exact pretty-printed JSON text `setup` itself writes (see `common::json_text`), so
/// a file seeded with it round-trips byte-for-byte if `setup --remove` touches nothing in
/// it that wasn't ours.
fn pretty_json(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap() + "\n"
}

#[test]
fn project_remove_restores_foreign_content_and_deletes_owned_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    // Foreign content already in place before Skillvolution ever ran here. The
    // Agent/Task and run_subagent/read_subagent grants are pre-seeded because setup
    // deliberately never removes those (the user may have granted them independently),
    // so a file that starts without them would gain them permanently on install and
    // could never come back byte-identical.
    fs::create_dir_all(p.join(".claude")).unwrap();
    let claude_settings = pretty_json(json!({
        "model": "existing",
        "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo pre", "timeout": 5}]}]},
        "permissions": {"allow": ["Bash(git status)", "Agent", "Task"], "deny": ["Bash(rm)"]},
    }));
    fs::write(p.join(".claude/settings.local.json"), &claude_settings).unwrap();
    let mcp = pretty_json(json!({"mcpServers": {"other": {"command": "npx"}}}));
    fs::write(p.join(".mcp.json"), &mcp).unwrap();

    let opencode = pretty_json(json!({
        "model": "existing",
        "permission": {"edit": "deny", "task": "ask"},
    }));
    fs::write(p.join("opencode.json"), &opencode).unwrap();
    let agents = "# Team notes\nKeep me.\n".to_owned();
    fs::write(p.join("AGENTS.md"), &agents).unwrap();

    fs::create_dir_all(p.join(".devin")).unwrap();
    let devin_mcp = pretty_json(json!({"mcpServers": {"other": {"command": "npx"}}}));
    fs::write(p.join(".devin/mcp_config.json"), &devin_mcp).unwrap();
    let devin_config = pretty_json(json!({
        "model": "existing",
        "permissions": {"allow": ["exec", "run_subagent", "read_subagent"], "deny": ["exec(sudo *)"]},
        "hooks": {"PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo pre"}]}]},
    }));
    fs::write(p.join(".devin/config.json"), &devin_config).unwrap();

    install_project(p, &bin, &db, "all").unwrap();
    remove_project(p, &db, None, false).unwrap();

    assert_eq!(
        fs::read_to_string(p.join(".claude/settings.local.json")).unwrap(),
        claude_settings
    );
    assert_eq!(fs::read_to_string(p.join(".mcp.json")).unwrap(), mcp);
    assert_eq!(
        fs::read_to_string(p.join("opencode.json")).unwrap(),
        opencode
    );
    assert_eq!(fs::read_to_string(p.join("AGENTS.md")).unwrap(), agents);
    assert_eq!(
        fs::read_to_string(p.join(".devin/mcp_config.json")).unwrap(),
        devin_mcp
    );
    assert_eq!(
        fs::read_to_string(p.join(".devin/config.json")).unwrap(),
        devin_config
    );

    for path in [
        ".claude/skills/evolution/SKILL.md",
        ".opencode/skills/evolution/SKILL.md",
        ".opencode/plugins/skillvolution.js",
        ".devin/skills/evolution/SKILL.md",
    ] {
        assert!(!p.join(path).exists(), "{path} should be gone");
    }
}

#[test]
fn remove_when_nothing_installed_prints_nothing_to_remove_and_creates_no_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let db = p.join("data/vault.sqlite3");

    remove_project(p, &db, None, false).unwrap();

    assert!(!db.exists(), "remove must never create the vault database");
    assert!(!p.join(".claude").exists());
    assert!(!p.join(".opencode").exists());
    assert!(!p.join(".devin").exists());
    assert!(!p.join("AGENTS.md").exists());
}

#[test]
fn dry_run_install_writes_nothing_and_dry_run_remove_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    dry_run_install_project(p, &bin, &db, "opencode").unwrap();
    assert!(!db.exists(), "dry-run must never create the vault database");
    assert!(!p.join("opencode.json").exists());
    assert!(!p.join("AGENTS.md").exists());
    assert!(!p.join(".opencode").exists());

    install_project(p, &bin, &db, "opencode").unwrap();
    let opencode_after_install = fs::read_to_string(p.join("opencode.json")).unwrap();
    let agents_after_install = fs::read_to_string(p.join("AGENTS.md")).unwrap();

    remove_project(p, &db, None, true).unwrap();
    assert_eq!(
        fs::read_to_string(p.join("opencode.json")).unwrap(),
        opencode_after_install,
        "a dry-run remove must not write anything"
    );
    assert_eq!(
        fs::read_to_string(p.join("AGENTS.md")).unwrap(),
        agents_after_install
    );
    assert!(p.join(".opencode/skills/evolution/SKILL.md").exists());
}

/// Locks `dir` down to read+execute only, the same technique `tests/setup.rs` uses to
/// make a write inside it fail; returns `false` (skipping the test) if this environment
/// still allows writing there anyway (e.g. running as root).
#[cfg(unix)]
fn make_read_only(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.join("probe");
    if fs::write(&probe, "").is_ok() {
        fs::remove_file(&probe).unwrap();
        return false;
    }
    true
}

#[cfg(unix)]
#[test]
fn rollback_restores_earlier_edits_on_a_failing_write() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    install_project(p, &bin, &db, "all").unwrap();
    let claude_settings_before = fs::read_to_string(p.join(".claude/settings.local.json")).unwrap();
    let opencode_before = fs::read_to_string(p.join("opencode.json")).unwrap();

    // Devin is processed last (`ClientKind::ALL` order), so Claude Code's and
    // OpenCode's edits are already applied by the time Devin's `config.json` rewrite
    // needs to create its replacement temp file in `.devin` and can't.
    if !make_read_only(&p.join(".devin")) {
        return;
    }

    let err = remove_project(p, &db, None, false).unwrap_err();
    assert!(format!("{err:#}").contains("rolled back"), "{err:#}");

    assert_eq!(
        fs::read_to_string(p.join(".claude/settings.local.json")).unwrap(),
        claude_settings_before,
        "Claude Code's edit must be rolled back"
    );
    assert_eq!(
        fs::read_to_string(p.join("opencode.json")).unwrap(),
        opencode_before,
        "OpenCode's edit must be rolled back"
    );
    assert!(
        p.join(".claude/skills/evolution/SKILL.md").exists(),
        "the deleted Claude Code skill file must be restored"
    );
    assert!(
        p.join(".opencode/skills/evolution/SKILL.md").exists(),
        "the deleted OpenCode skill file must be restored"
    );
}

fn write_fake_claude(dir: &Path, log: &Path) {
    fs::create_dir_all(dir).unwrap();
    let claude = dir.join("claude");
    fs::write(
        &claude,
        format!("#!/bin/sh\necho \"$@\" >> {}\nexit 0\n", log.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn global_remove_restores_foreign_content_deletes_owned_files_and_unregisters_mcp() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let bin = workspace.path().join("skillvolution");
    fs::write(&bin, "test binary").unwrap();
    let db = workspace.path().join("vault.sqlite3");
    let log = workspace.path().join("claude.log");
    let bin_dir = workspace.path().join("bin");
    write_fake_claude(&bin_dir, &log);

    let claude_dir = home.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    let claude_settings = pretty_json(json!({
        "model": "existing",
        "permissions": {"allow": ["Agent", "Task"]},
    }));
    fs::write(claude_dir.join("settings.json"), &claude_settings).unwrap();

    let command = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        cmd.arg("--db")
            .arg(&db)
            .arg("setup")
            .args(args)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("xdg-config"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", &bin_dir)
            .current_dir(home.path());
        cmd
    };

    let output = command(&["--client", "claude-code", "--bin", bin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = command(&["--remove", "--client", "claude-code"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        fs::read_to_string(claude_dir.join("settings.json")).unwrap(),
        claude_settings
    );
    assert!(!claude_dir.join("skills/evolution/SKILL.md").exists());

    let log_lines: Vec<String> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        log_lines,
        vec![
            format!(
                "mcp add-json --scope user skillvolution {}",
                json!({"type": "stdio", "command": bin, "args": ["--db", &db, "serve"]})
            ),
            "mcp remove --scope user skillvolution".to_owned(),
        ]
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Removed skillvolution with `claude mcp remove --scope user`."));
}
