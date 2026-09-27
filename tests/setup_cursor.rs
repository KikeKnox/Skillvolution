//! `skillvolution setup` for Cursor: project scope runs in-process (like
//! `tests/setup.rs`/`tests/setup_remove.rs`); global scope and the `hook` CLI end-to-end
//! run the built binary as a subprocess (like `tests/setup_global.rs`), since HOME and
//! stdin are process-global/process-local inputs an in-process call can't isolate.

use skillvolution::setup;

use clap::Parser;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    setup: setup::SetupArgs,
}

/// A directory with spaces (covering shell quoting) containing a file literally named
/// `skillvolution`: hook ownership (see `owned_command` in `src/setup/hooks.rs`) matches on
/// that exact file name, so a rerun must recognize its own previous hook entries.
const BIN_DIR: &str = "bin dir with spaces";
const BIN_NAME: &str = "skillvolution";

fn run(argv: Vec<String>) -> anyhow::Result<()> {
    setup::run(Cli::parse_from(argv).setup)
}

fn write_bin(project: &Path) -> PathBuf {
    // Two joins, not one string with an embedded `/` (which Windows keeps
    // literally instead of normalizing to `\` the way the app's own path
    // handling does).
    let bin = project.join(BIN_DIR).join(BIN_NAME);
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, "test binary").unwrap();
    bin
}

fn install_project(project: &Path, bin: &Path, db: &Path) -> anyhow::Result<()> {
    run(vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        "cursor".to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ])
}

fn dry_run_install_project(project: &Path, bin: &Path, db: &Path) -> anyhow::Result<()> {
    run(vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        "cursor".to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
        "--dry-run".to_owned(),
    ])
}

fn remove_project(project: &Path, db: &Path, dry_run: bool) -> anyhow::Result<()> {
    let mut argv = vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--remove".to_owned(),
        "--client".to_owned(),
        "cursor".to_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ];
    if dry_run {
        argv.push("--dry-run".to_owned());
    }
    run(argv)
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The exact pretty-printed JSON text `setup` itself writes (see `common::json_text`), so
/// a file seeded with it round-trips byte-for-byte if `setup --remove` touches nothing in
/// it that wasn't ours.
fn pretty_json(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap() + "\n"
}

#[test]
fn installs_cursor_project_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data").join("vault.sqlite3");

    install_project(p, &bin, &db).unwrap();

    let skill = fs::read_to_string(p.join(".cursor/skills/evolution/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: evolution\n"));

    let mcp = read_json(p.join(".cursor/mcp.json"));
    let server = &mcp["mcpServers"]["skillvolution"];
    assert_eq!(server["type"], "stdio");
    assert_eq!(server["command"], bin.to_str().unwrap());
    let args = server["args"].as_array().unwrap();
    assert_eq!(args[0], "--db");
    assert_eq!(args[2], "serve");
    assert_eq!(args[3], "--project");

    let hooks = read_json(p.join(".cursor/hooks.json"));
    assert_eq!(hooks["version"], 1);
    for event in ["sessionStart", "postToolUse", "stop", "sessionEnd"] {
        let entries = hooks["hooks"][event].as_array().unwrap();
        assert_eq!(entries.len(), 1, "{event}");
        assert!(
            entries[0]["command"]
                .as_str()
                .unwrap()
                .contains("skillvolution")
        );
        assert_eq!(entries[0]["timeout"], 10);
    }
    let session_cmd = hooks["hooks"]["sessionStart"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        session_cmd.contains("hook session-start --client cursor --project"),
        "{session_cmd}"
    );
    let tool_cmd = hooks["hooks"]["postToolUse"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        tool_cmd.ends_with("hook tool-use --client cursor"),
        "{tool_cmd}"
    );
    let stop_cmd = hooks["hooks"]["stop"][0]["command"].as_str().unwrap();
    assert!(
        stop_cmd.ends_with("hook stop --client cursor"),
        "{stop_cmd}"
    );
    let end_cmd = hooks["hooks"]["sessionEnd"][0]["command"].as_str().unwrap();
    assert!(end_cmd.ends_with("hook session-end"), "{end_cmd}");
    // postToolUse carries a tool-name matcher; the others don't.
    assert!(hooks["hooks"]["postToolUse"][0]["matcher"].is_string());
    assert!(hooks["hooks"]["stop"][0].get("matcher").is_none());

    let permissions = read_json(p.join(".cursor/cli.json"));
    assert_eq!(
        permissions["permissions"]["allow"],
        json!(["Mcp(skillvolution:*)"])
    );

    let rule = fs::read_to_string(p.join(".cursor/rules/skillvolution.mdc")).unwrap();
    assert!(rule.starts_with("---\n"));
    assert!(rule.contains("alwaysApply: true"));
    assert!(rule.contains("skillvolution-managed:cursor-rule:"));
    assert!(rule.contains("`evolution` skill"));
}

#[test]
fn cursor_project_setup_preserves_existing_config_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data").join("vault.sqlite3");
    fs::create_dir_all(p.join(".cursor/rules")).unwrap();

    fs::write(
        p.join(".cursor/mcp.json"),
        "{\"mcpServers\":{\"other\":{\"command\":\"npx\",\"args\":[\"-y\",\"x\"]}}}",
    )
    .unwrap();
    fs::write(
        p.join(".cursor/hooks.json"),
        "{\"hooks\":{\"workspaceOpen\":[{\"command\":\"echo mine\"}]}}",
    )
    .unwrap();
    fs::write(
        p.join(".cursor/cli.json"),
        "{\"permissions\":{\"allow\":[\"Shell(git)\"]},\"other\":1}",
    )
    .unwrap();

    install_project(p, &bin, &db).unwrap();

    let mcp = read_json(p.join(".cursor/mcp.json"));
    assert_eq!(mcp["mcpServers"]["other"]["command"], "npx");
    let hooks = read_json(p.join(".cursor/hooks.json"));
    assert_eq!(
        hooks["hooks"]["workspaceOpen"],
        json!([{"command": "echo mine"}])
    );
    let permissions = read_json(p.join(".cursor/cli.json"));
    assert_eq!(permissions["permissions"]["allow"][0], "Shell(git)");
    assert_eq!(permissions["other"], 1);

    let before = [
        fs::read(p.join(".cursor/mcp.json")).unwrap(),
        fs::read(p.join(".cursor/hooks.json")).unwrap(),
        fs::read(p.join(".cursor/cli.json")).unwrap(),
        fs::read(p.join(".cursor/rules/skillvolution.mdc")).unwrap(),
    ];
    install_project(p, &bin, &db).unwrap();
    assert_eq!(fs::read(p.join(".cursor/mcp.json")).unwrap(), before[0]);
    assert_eq!(fs::read(p.join(".cursor/hooks.json")).unwrap(), before[1]);
    assert_eq!(fs::read(p.join(".cursor/cli.json")).unwrap(), before[2]);
    assert_eq!(
        fs::read(p.join(".cursor/rules/skillvolution.mdc")).unwrap(),
        before[3]
    );
    for name in ["mcp.json", "hooks.json", "cli.json"] {
        assert!(
            !p.join(format!(".cursor/{name}.skillvolution.bak.1"))
                .exists(),
            "second run must not create another backup for {name}"
        );
    }
}

#[test]
fn cursor_project_remove_restores_foreign_content_and_deletes_owned_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data").join("vault.sqlite3");
    fs::create_dir_all(p.join(".cursor")).unwrap();

    let mcp = pretty_json(json!({"mcpServers": {"other": {"command": "npx"}}}));
    fs::write(p.join(".cursor/mcp.json"), &mcp).unwrap();
    // `version` is required by Cursor's own hooks.json schema, so a real pre-existing
    // file always carries it; setup only adds it when absent, and never removes it (it's
    // file-format metadata, not something attributable to us or the user either way).
    let hooks =
        pretty_json(json!({"version": 1, "hooks": {"workspaceOpen": [{"command": "echo mine"}]}}));
    fs::write(p.join(".cursor/hooks.json"), &hooks).unwrap();
    let permissions = pretty_json(json!({"permissions": {"allow": ["Shell(git)"]}}));
    fs::write(p.join(".cursor/cli.json"), &permissions).unwrap();

    install_project(p, &bin, &db).unwrap();
    remove_project(p, &db, false).unwrap();

    assert_eq!(fs::read_to_string(p.join(".cursor/mcp.json")).unwrap(), mcp);
    assert_eq!(
        fs::read_to_string(p.join(".cursor/hooks.json")).unwrap(),
        hooks
    );
    assert_eq!(
        fs::read_to_string(p.join(".cursor/cli.json")).unwrap(),
        permissions
    );
    for path in [
        ".cursor/skills/evolution/SKILL.md",
        ".cursor/rules/skillvolution.mdc",
    ] {
        assert!(!p.join(path).exists(), "{path} should be gone");
    }
}

#[test]
fn cursor_dry_run_install_and_remove_write_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data").join("vault.sqlite3");

    dry_run_install_project(p, &bin, &db).unwrap();
    assert!(!db.exists(), "dry-run must never create the vault database");
    assert!(!p.join(".cursor").exists());

    install_project(p, &bin, &db).unwrap();
    let mcp_before = fs::read_to_string(p.join(".cursor/mcp.json")).unwrap();
    let rule_before = fs::read_to_string(p.join(".cursor/rules/skillvolution.mdc")).unwrap();

    remove_project(p, &db, true).unwrap();
    assert_eq!(
        fs::read_to_string(p.join(".cursor/mcp.json")).unwrap(),
        mcp_before,
        "a dry-run remove must not write anything"
    );
    assert_eq!(
        fs::read_to_string(p.join(".cursor/rules/skillvolution.mdc")).unwrap(),
        rule_before
    );
}

// --- global scope (subprocess: HOME is process-global) ---------------------

struct GlobalEnv {
    home: tempfile::TempDir,
    empty_path: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    bin: PathBuf,
    db: PathBuf,
}

impl GlobalEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let empty_path = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let bin = workspace.path().join("skillvolution");
        fs::write(&bin, "test binary").unwrap();
        let db = workspace.path().join("vault.sqlite3");
        Self {
            home,
            empty_path,
            _workspace: workspace,
            bin,
            db,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        cmd.arg("--db")
            .arg(&self.db)
            .arg("setup")
            .arg("--client")
            .arg("cursor")
            .arg("--bin")
            .arg(&self.bin)
            .env("HOME", self.home.path())
            // Isolate %APPDATA% (Devin's config dir on Windows) from the host.
            .env("APPDATA", self.home.path().join("AppData"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", self.empty_path.path())
            .current_dir(self.home.path());
        cmd
    }

    fn cursor_dir(&self) -> PathBuf {
        self.home.path().join(".cursor")
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn global_cursor_writes_expected_files_without_a_rule_file() {
    let env = GlobalEnv::new();

    let output = env.command().output().unwrap();
    assert_success(&output);

    assert!(env.cursor_dir().join("skills/evolution/SKILL.md").exists());
    let mcp = read_json(env.cursor_dir().join("mcp.json"));
    let server = &mcp["mcpServers"]["skillvolution"];
    assert_eq!(server["command"], env.bin.to_str().unwrap());
    assert!(
        !server["args"]
            .as_array()
            .unwrap()
            .contains(&json!("--project"))
    );

    let hooks = read_json(env.cursor_dir().join("hooks.json"));
    let session_cmd = hooks["hooks"]["sessionStart"][0]["command"]
        .as_str()
        .unwrap();
    assert!(session_cmd.contains("hook session-start --client cursor"));
    assert!(!session_cmd.contains("--project"), "{session_cmd}");

    // Global permissions file is cli-config.json, not cli.json.
    let permissions = read_json(env.cursor_dir().join("cli-config.json"));
    assert_eq!(
        permissions["permissions"]["allow"],
        json!(["Mcp(skillvolution:*)"])
    );
    assert!(!env.cursor_dir().join("cli.json").exists());

    // No global rules FILE: Cursor's user rules live in app settings, not a path setup
    // writes to.
    assert!(!env.cursor_dir().join("rules").exists());
}

// --- hook CLI end-to-end (subprocess: stdin is process-local) ---------------

/// Runs `hook <args>` with `input` on stdin; stdout/stderr are captured.
fn run_hook(db: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(db)
        .arg("hook")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn cli_hook_cursor_tool_use_then_stop_blocks_once_then_not_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");

    // A realistic Cursor postToolUse payload: Cursor's own docs name the session id
    // field `conversation_id`, not `session_id` (unlike Claude Code/Devin).
    let tool_use_input = json!({
        "conversation_id": "conv-1",
        "hook_event_name": "postToolUse",
        "tool_name": "Write",
    })
    .to_string();
    let output = run_hook(&db, &["tool-use", "--client", "cursor"], &tool_use_input);
    assert_success(&output);
    assert!(output.stdout.is_empty());

    let stop_input = json!({"conversation_id": "conv-1", "status": "completed"}).to_string();
    let blocked = run_hook(&db, &["stop", "--client", "cursor"], &stop_input);
    assert_success(&blocked);
    let decision: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert!(
        decision["followup_message"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
        "{decision}"
    );

    let clean = run_hook(&db, &["stop", "--client", "cursor"], &stop_input);
    assert_success(&clean);
    assert!(clean.stdout.is_empty(), "{:?}", clean.stdout);
}

#[test]
fn cli_hook_cursor_session_start_wraps_context_in_additional_context() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let output = run_hook(&db, &["session-start", "--client", "cursor"], "");
    assert_success(&output);
    let decision: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        decision["additional_context"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
        "{decision}"
    );
}

#[test]
fn cli_hook_cursor_review_tool_marks_the_span_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");

    let work = json!({"conversation_id": "s1", "tool_name": "Write"}).to_string();
    run_hook(&db, &["tool-use", "--client", "cursor"], &work);
    // Cursor's MCP tool naming (unverified exact form; suffix-matched either way).
    let review = json!({
        "conversation_id": "s1",
        "tool_name": "MCP:skillvolution_report_skill_outcome",
    })
    .to_string();
    run_hook(&db, &["tool-use", "--client", "cursor"], &review);

    let stop_input = json!({"conversation_id": "s1"}).to_string();
    let output = run_hook(&db, &["stop", "--client", "cursor"], &stop_input);
    assert_success(&output);
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
}
