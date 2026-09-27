//! `skillvolution setup` for Codex CLI: `.codex/config.toml` (MCP server entry, edited
//! with `toml_edit`), `.codex/hooks.json` (lifecycle hooks), the native skill, and the
//! shared AGENTS.md block. Project-scope tests run in-process (like `tests/setup.rs`);
//! global-scope and `doctor` tests run the built binary as a subprocess with HOME/
//! CODEX_HOME/PATH pinned (like `tests/setup_global.rs`), since those are process-global.

use skillvolution::setup;

use clap::Parser;
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

fn run(argv: Vec<String>) -> anyhow::Result<()> {
    setup::run(Cli::parse_from(argv).setup)
}

fn write_bin(dir: &Path) -> PathBuf {
    let bin = dir.join("skillvolution");
    fs::write(&bin, "test binary").unwrap();
    bin
}

fn install_project(project: &Path, bin: &Path, db: &Path) -> anyhow::Result<()> {
    run(vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        "codex".to_owned(),
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
        "codex".to_owned(),
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
        "codex".to_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ];
    if dry_run {
        argv.push("--dry-run".to_owned());
    }
    run(argv)
}

/// The event names every hook entry `changes` writes, and (for the flag-tracked ones)
/// a substring their command must carry.
const HOOK_EVENTS: [(&str, &str); 5] = [
    ("SessionStart", "hook session-start --client codex"),
    ("PostToolUse", "hook tool-use --client codex"),
    ("Stop", "hook stop --client codex"),
    ("SessionEnd", "hook session-end"),
    ("PermissionRequest", "hook approve"),
];

fn hook_command(hooks: &serde_json::Value, event: &str) -> String {
    hooks["hooks"][event][0]["hooks"][0]["command"]
        .as_str()
        .unwrap_or_else(|| panic!("no command for {event} in {hooks}"))
        .to_owned()
}

#[test]
fn project_install_writes_expected_config_toml_and_hooks_json() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    install_project(p, &bin, &db).unwrap();

    let toml_text = fs::read_to_string(p.join(".codex/config.toml")).unwrap();
    let doc: toml_edit::DocumentMut = toml_text.parse().unwrap();
    let entry = &doc["mcp_servers"]["skillvolution"];
    assert_eq!(entry["command"].as_str().unwrap(), bin.to_str().unwrap());
    let args: Vec<&str> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let key = skillvolution::project::key_from_dir(&fs::canonicalize(p).unwrap()).unwrap();
    assert_eq!(
        args,
        vec!["--db", db.to_str().unwrap(), "serve", "--project", &key]
    );
    // A fresh project entry carries the trust-note comment right above its header.
    assert!(
        toml_text.contains("# Skillvolution: Codex only loads this project's"),
        "{toml_text}"
    );

    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p.join(".codex/hooks.json")).unwrap()).unwrap();
    for (event, substring) in HOOK_EVENTS {
        let command = hook_command(&hooks, event);
        assert!(command.contains(substring), "{event}: {command}");
    }
    let session_start = hook_command(&hooks, "SessionStart");
    assert!(
        session_start.contains(&format!("--project '{key}'")),
        "{session_start}"
    );
    let matcher = hooks["hooks"]["PostToolUse"][0]["matcher"]
        .as_str()
        .unwrap();
    assert_eq!(matcher, "^(apply_patch|mcp__skillvolution__.*)$");
    let approve_matcher = hooks["hooks"]["PermissionRequest"][0]["matcher"]
        .as_str()
        .unwrap();
    assert_eq!(approve_matcher, "^mcp__skillvolution__.*$");

    let skill = fs::read_to_string(p.join(".codex/skills/evolution/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: evolution\n"));

    let agents = fs::read_to_string(p.join("AGENTS.md")).unwrap();
    assert!(agents.contains("<!-- skillvolution:start -->"));
    assert!(agents.contains("<!-- skillvolution:end -->"));
}

#[test]
fn project_rerun_is_idempotent_without_extra_backups() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    install_project(p, &bin, &db).unwrap();
    let toml_before = fs::read(p.join(".codex/config.toml")).unwrap();
    let hooks_before = fs::read(p.join(".codex/hooks.json")).unwrap();

    install_project(p, &bin, &db).unwrap();
    assert_eq!(fs::read(p.join(".codex/config.toml")).unwrap(), toml_before);
    assert_eq!(fs::read(p.join(".codex/hooks.json")).unwrap(), hooks_before);

    for name in ["config.toml", "hooks.json"] {
        let backup = p.join(".codex").join(format!("{name}.skillvolution.bak"));
        assert!(!backup.exists(), "unexpected backup for {name}");
    }

    // No duplicated hook groups: exactly one group per event after the rerun.
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p.join(".codex/hooks.json")).unwrap()).unwrap();
    for (event, _) in HOOK_EVENTS {
        assert_eq!(
            hooks["hooks"][event].as_array().unwrap().len(),
            1,
            "{event}"
        );
    }
}

#[test]
fn project_install_preserves_foreign_toml_and_json_content() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");
    fs::create_dir_all(p.join(".codex")).unwrap();

    let toml_seed = "# my own notes\nmodel = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"other-cmd\"\n\n\
[mcp_servers.skillvolution]\ncommand = \"/old/bin\"\nargs = [\"--db\", \"/old\", \"serve\"]\nenabled = false\nenv = { FOO = \"bar\" }\nstartup_timeout_sec = 30\n";
    fs::write(p.join(".codex/config.toml"), toml_seed).unwrap();

    let hooks_seed = serde_json::json!({"hooks": {
        "Notification": [{"hooks": [{"type": "command", "command": "echo foreign"}]}],
    }});
    fs::write(
        p.join(".codex/hooks.json"),
        serde_json::to_string_pretty(&hooks_seed).unwrap() + "\n",
    )
    .unwrap();

    install_project(p, &bin, &db).unwrap();

    let toml_text = fs::read_to_string(p.join(".codex/config.toml")).unwrap();
    assert!(toml_text.contains("# my own notes"));
    assert!(toml_text.contains("model = \"gpt-5\""));
    let doc: toml_edit::DocumentMut = toml_text.parse().unwrap();
    assert_eq!(
        doc["mcp_servers"]["other"]["command"].as_str().unwrap(),
        "other-cmd"
    );
    let entry = &doc["mcp_servers"]["skillvolution"];
    assert_eq!(entry["command"].as_str().unwrap(), bin.to_str().unwrap());
    assert!(!entry["enabled"].as_bool().unwrap());
    assert_eq!(entry["env"]["FOO"].as_str().unwrap(), "bar");
    assert_eq!(entry["startup_timeout_sec"].as_integer().unwrap(), 30);
    // An entry that already existed keeps whatever decor it had (none here), so a
    // rerun over a hand-written entry does not retroactively inject the trust note.
    assert!(!toml_text.contains("Skillvolution: Codex only loads"));

    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p.join(".codex/hooks.json")).unwrap()).unwrap();
    assert_eq!(
        hooks["hooks"]["Notification"][0]["hooks"][0]["command"],
        "echo foreign"
    );
    assert_eq!(hooks["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
}

#[test]
fn project_remove_restores_foreign_content_and_deletes_owned_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    fs::create_dir_all(p.join(".codex")).unwrap();
    let toml_seed = "[mcp_servers.other]\ncommand = \"other-cmd\"\n";
    fs::write(p.join(".codex/config.toml"), toml_seed).unwrap();
    let agents = "# Team notes\nKeep me.\n".to_owned();
    fs::write(p.join("AGENTS.md"), &agents).unwrap();

    install_project(p, &bin, &db).unwrap();
    remove_project(p, &db, false).unwrap();

    assert_eq!(
        fs::read_to_string(p.join(".codex/config.toml")).unwrap(),
        toml_seed
    );
    assert_eq!(fs::read_to_string(p.join("AGENTS.md")).unwrap(), agents);
    // hooks.json held nothing but our own entries, so it's left behind as an empty
    // object rather than deleted (same convention as Devin's mcp_config.json).
    assert_eq!(
        fs::read_to_string(p.join(".codex/hooks.json")).unwrap(),
        "{}\n"
    );
    assert!(!p.join(".codex/skills/evolution/SKILL.md").exists());
}

#[test]
fn project_remove_when_nothing_installed_creates_no_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let db = p.join("data/vault.sqlite3");

    remove_project(p, &db, false).unwrap();

    assert!(!db.exists(), "remove must never create the vault database");
    assert!(!p.join(".codex").exists());
    assert!(!p.join("AGENTS.md").exists());
}

#[test]
fn project_dry_run_install_and_remove_write_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    dry_run_install_project(p, &bin, &db).unwrap();
    assert!(!db.exists(), "dry-run must never create the vault database");
    assert!(!p.join(".codex").exists());
    assert!(!p.join("AGENTS.md").exists());

    install_project(p, &bin, &db).unwrap();
    let toml_after_install = fs::read_to_string(p.join(".codex/config.toml")).unwrap();

    remove_project(p, &db, true).unwrap();
    assert_eq!(
        fs::read_to_string(p.join(".codex/config.toml")).unwrap(),
        toml_after_install,
        "a dry-run remove must not write anything"
    );
    assert!(p.join(".codex/skills/evolution/SKILL.md").exists());
}

// --- global scope + doctor (process-global HOME/CODEX_HOME/PATH) -----------

struct Env {
    home: tempfile::TempDir,
    codex_home: PathBuf,
    empty_path: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    bin: PathBuf,
    db: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let codex_home = home.path().join(".codex");
        let empty_path = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // Executable, unlike the project-scope helper's fake bin: `doctor`'s
        // configured-binary check requires that.
        let bin = workspace.path().join("skillvolution");
        fs::write(&bin, "test binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let db = workspace.path().join("vault.sqlite3");
        Self {
            home,
            codex_home,
            empty_path,
            _workspace: workspace,
            bin,
            db,
        }
    }

    /// Base command with HOME/PATH pinned to this sandbox and `CODEX_HOME` unset, so
    /// the client falls back to `~/.codex`. `XDG_CONFIG_HOME`/`CLAUDE_CONFIG_DIR` are
    /// pinned too so `doctor`'s pass over every other client never touches this
    /// machine's real OpenCode/Claude Code config.
    fn cli(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        cmd.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("xdg-config"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env("PATH", self.empty_path.path())
            .current_dir(self.home.path());
        cmd
    }

    fn setup(&self) -> Command {
        let mut cmd = self.cli();
        cmd.arg("--db")
            .arg(&self.db)
            .arg("setup")
            .arg("--client")
            .arg("codex")
            .arg("--bin")
            .arg(&self.bin);
        cmd
    }

    fn doctor(&self) -> Command {
        let mut cmd = self.cli();
        cmd.arg("--db").arg(&self.db).arg("doctor");
        cmd
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
fn global_install_writes_to_home_codex_dir() {
    let env = Env::new();
    assert_success(&env.setup().output().unwrap());

    assert!(env.codex_home.join("config.toml").exists());
    assert!(env.codex_home.join("hooks.json").exists());
    assert!(env.codex_home.join("AGENTS.md").exists());
    assert!(env.codex_home.join("skills/evolution/SKILL.md").exists());

    let toml_text = fs::read_to_string(env.codex_home.join("config.toml")).unwrap();
    let doc: toml_edit::DocumentMut = toml_text.parse().unwrap();
    let args = doc["mcp_servers"]["skillvolution"]["args"]
        .as_array()
        .unwrap();
    // Global setup carries no --project.
    assert_eq!(args.len(), 3, "{args:?}");
    // A global entry is always trusted, so it never gets the project trust note.
    assert!(!toml_text.contains("Skillvolution: Codex only loads"));
}

#[test]
fn codex_home_env_var_overrides_the_default_config_dir() {
    let env = Env::new();
    let custom = env.home.path().join("custom-codex-home");
    assert_success(&env.setup().env("CODEX_HOME", &custom).output().unwrap());

    assert!(custom.join("config.toml").exists());
    assert!(!env.codex_home.exists());
}

#[test]
fn doctor_reports_codex_configured_up_to_date_after_setup() {
    let env = Env::new();
    assert_success(&env.setup().output().unwrap());

    let output = env.doctor().output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("[ok] Codex CLI: configured, up to date"),
        "{stdout}"
    );
    assert!(!stdout.contains("[fail]"), "{stdout}");
}

// --- hook CLI end-to-end ----------------------------------------------------

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
fn cli_hook_tool_use_then_stop_blocks_once_then_not_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    skillvolution::vault::Vault::open(&db).unwrap();

    // Codex's PostToolUse payload for its file-editing tool.
    let tool_use_input = serde_json::json!({
        "session_id": "s1",
        "hook_event_name": "PostToolUse",
        "tool_name": "apply_patch",
    })
    .to_string();
    let tool_use_output = run_hook(&db, &["tool-use", "--client", "codex"], &tool_use_input);
    assert_success(&tool_use_output);
    assert!(tool_use_output.stdout.is_empty());

    let stop_input = serde_json::json!({"session_id": "s1", "hook_event_name": "Stop"}).to_string();
    let blocked = run_hook(&db, &["stop", "--client", "codex"], &stop_input);
    assert_success(&blocked);
    let decision: serde_json::Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(decision["decision"], "block");
    assert!(
        decision["reason"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
        "{decision}"
    );

    // The same span was already judged, so a second stop prints nothing.
    let clean = run_hook(&db, &["stop", "--client", "codex"], &stop_input);
    assert_success(&clean);
    assert!(clean.stdout.is_empty());
}

#[test]
fn cli_hook_session_start_wraps_context_for_codex() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    skillvolution::vault::Vault::open(&db).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("hook")
        .arg("session-start")
        .arg("--client")
        .arg("codex")
        .env_remove("CLAUDE_PROJECT_DIR")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(
        value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
    );
}
