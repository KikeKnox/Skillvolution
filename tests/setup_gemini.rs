//! Gemini CLI setup: `.gemini/settings.json` (MCP server entry + hooks), the native
//! skill file, and the shared `GEMINI.md` marker block. Project-scope tests run
//! in-process (like `tests/setup.rs`); global-scope and hook-CLI tests run the built
//! binary as a subprocess (like `tests/setup_global.rs` and `tests/hook.rs`), since HOME
//! and stdin/stdout need real process boundaries.

use skillvolution::{hook, setup, vault::Vault};

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
/// `skillvolution`: hook ownership (see `owned_command` in `src/setup/hooks.rs`) matches
/// on that exact file name, so a rerun must recognize its own previous hook entries.
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

fn install_project(project: &Path, bin: &Path, db: &Path) -> anyhow::Result<()> {
    run(vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        "gemini".to_owned(),
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
        "gemini".to_owned(),
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
        "gemini".to_owned(),
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

// --- project scope ----------------------------------------------------------

#[test]
fn installs_gemini_project_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data with spaces/vault.sqlite3");

    install_project(p, &bin, &db).unwrap();

    let skill = fs::read_to_string(p.join(".gemini/skills/evolution/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: evolution\n"));
    assert!(skill.contains("skillvolution-managed:evolution:"));

    let settings = read_json(p.join(".gemini/settings.json"));
    let server = &settings["mcpServers"]["skillvolution"];
    assert_eq!(server["command"], json!(bin));
    let args = server["args"].as_array().unwrap();
    assert_eq!(args[0], "--db");
    assert_eq!(args[1], json!(db));
    assert_eq!(args[2], "serve");
    assert_eq!(args[3], "--project");
    assert!(args[4].is_string(), "{args:?}");
    assert_eq!(server["trust"], true);

    for event in ["SessionStart", "AfterTool", "AfterAgent", "SessionEnd"] {
        let groups = settings["hooks"][event].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{event}");
        let command = groups[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("skillvolution"), "{event}: {command}");
    }
    let session_cmd = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        session_cmd.contains("hook session-start --client gemini --project"),
        "{session_cmd}"
    );
    let tool_cmd = settings["hooks"]["AfterTool"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        tool_cmd.contains("hook tool-use --client gemini"),
        "{tool_cmd}"
    );
    let stop_cmd = settings["hooks"]["AfterAgent"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(stop_cmd.contains("hook stop --client gemini"), "{stop_cmd}");
    let end_cmd = settings["hooks"]["SessionEnd"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(end_cmd.ends_with("hook session-end"), "{end_cmd}");
    // AfterTool carries a tool-name matcher; lifecycle events don't.
    assert!(settings["hooks"]["AfterTool"][0]["matcher"].is_string());
    assert!(
        settings["hooks"]["SessionStart"][0]
            .get("matcher")
            .is_none()
    );
    assert!(settings["hooks"]["AfterAgent"][0].get("matcher").is_none());

    let gemini_md = fs::read_to_string(p.join("GEMINI.md")).unwrap();
    assert!(gemini_md.contains("<!-- skillvolution:start -->"));
    assert!(gemini_md.contains("<!-- skillvolution:end -->"));
}

#[test]
fn gemini_project_setup_preserves_existing_config_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("vault.sqlite3");

    fs::create_dir_all(p.join(".gemini")).unwrap();
    fs::write(
        p.join(".gemini/settings.json"),
        "{\"theme\":\"dark\",\"mcpServers\":{\"other\":{\"command\":\"npx\",\"trust\":false}}}",
    )
    .unwrap();
    fs::write(p.join("GEMINI.md"), "# Team notes\n").unwrap();

    install_project(p, &bin, &db).unwrap();
    let settings = read_json(p.join(".gemini/settings.json"));
    assert_eq!(settings["theme"], "dark");
    assert_eq!(settings["mcpServers"]["other"]["command"], "npx");
    assert_eq!(settings["mcpServers"]["other"]["trust"], false);
    let gemini_md = fs::read_to_string(p.join("GEMINI.md")).unwrap();
    assert!(gemini_md.starts_with("# Team notes\n"));
    assert!(p.join("GEMINI.md.skillvolution.bak").exists());

    // A rerun must be a byte-for-byte no-op: no duplicate hook entries, no new backup.
    let before = fs::read(p.join(".gemini/settings.json")).unwrap();
    install_project(p, &bin, &db).unwrap();
    assert_eq!(fs::read(p.join(".gemini/settings.json")).unwrap(), before);
    assert!(!p.join(".gemini/settings.json.skillvolution.bak.1").exists());
    let settings_after_rerun = read_json(p.join(".gemini/settings.json"));
    for event in ["SessionStart", "AfterTool", "AfterAgent", "SessionEnd"] {
        assert_eq!(
            settings_after_rerun["hooks"][event]
                .as_array()
                .unwrap()
                .len(),
            1,
            "{event}"
        );
    }
}

#[test]
fn a_user_supplied_trust_false_survives_a_rerun() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("vault.sqlite3");

    install_project(p, &bin, &db).unwrap();
    let settings_path = p.join(".gemini/settings.json");
    let mut settings = read_json(&settings_path);
    settings["mcpServers"]["skillvolution"]["trust"] = json!(false);
    fs::write(&settings_path, pretty_json(settings)).unwrap();

    install_project(p, &bin, &db).unwrap();
    let settings = read_json(&settings_path);
    assert_eq!(settings["mcpServers"]["skillvolution"]["trust"], false);
}

#[test]
fn rejects_gemini_conflicts_before_writing() {
    for (file, text) in [
        (".gemini/settings.json", "not json"),
        (".gemini/settings.json", "{\"mcpServers\":[]}"),
        (
            ".gemini/settings.json",
            "{\"mcpServers\":{\"skillvolution\":false}}",
        ),
        (".gemini/settings.json", "{\"hooks\":\"nope\"}"),
        (
            ".gemini/skills/evolution/SKILL.md",
            "# My own evolution skill",
        ),
        (
            "GEMINI.md",
            "Team notes\n<!-- skillvolution:start -->\nmissing end",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, text).unwrap();
        let bin = write_bin(temp.path());
        let db = temp.path().join("vault.sqlite3");
        let result = install_project(temp.path(), &bin, &db);
        assert!(result.is_err(), "must reject {file}: {text}");
        assert_eq!(fs::read_to_string(&target).unwrap(), text);
        if file != ".gemini/settings.json" {
            assert!(
                !temp.path().join(".gemini/settings.json").exists(),
                "partial write for {file}"
            );
        }
        if file != "GEMINI.md" {
            assert!(
                !temp.path().join("GEMINI.md").exists(),
                "partial write for {file}"
            );
        }
    }
}

#[test]
fn gemini_project_remove_restores_foreign_content_and_deletes_owned_files() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    fs::create_dir_all(p.join(".gemini")).unwrap();
    let settings = pretty_json(json!({
        "mcpServers": {"other": {"command": "npx"}},
        "hooks": {"AfterTool": [{"matcher": "some_tool", "hooks": [{"type": "command", "command": "echo pre"}]}]},
    }));
    fs::write(p.join(".gemini/settings.json"), &settings).unwrap();
    let gemini_md = "# Team notes\nKeep me.\n".to_owned();
    fs::write(p.join("GEMINI.md"), &gemini_md).unwrap();

    install_project(p, &bin, &db).unwrap();
    remove_project(p, &db, false).unwrap();

    assert_eq!(
        fs::read_to_string(p.join(".gemini/settings.json")).unwrap(),
        settings
    );
    assert_eq!(fs::read_to_string(p.join("GEMINI.md")).unwrap(), gemini_md);
    assert!(!p.join(".gemini/skills/evolution/SKILL.md").exists());
}

#[test]
fn gemini_remove_when_nothing_installed_is_a_no_op() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let db = p.join("data/vault.sqlite3");

    remove_project(p, &db, false).unwrap();

    assert!(!db.exists(), "remove must never create the vault database");
    assert!(!p.join(".gemini").exists());
    assert!(!p.join("GEMINI.md").exists());
}

#[test]
fn gemini_dry_run_install_and_remove_write_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let bin = write_bin(p);
    let db = p.join("data/vault.sqlite3");

    dry_run_install_project(p, &bin, &db).unwrap();
    assert!(!db.exists(), "dry-run must never create the vault database");
    assert!(!p.join(".gemini").exists());
    assert!(!p.join("GEMINI.md").exists());

    install_project(p, &bin, &db).unwrap();
    let settings_after_install = fs::read_to_string(p.join(".gemini/settings.json")).unwrap();
    let gemini_md_after_install = fs::read_to_string(p.join("GEMINI.md")).unwrap();

    remove_project(p, &db, true).unwrap();
    assert_eq!(
        fs::read_to_string(p.join(".gemini/settings.json")).unwrap(),
        settings_after_install,
        "a dry-run remove must not write anything"
    );
    assert_eq!(
        fs::read_to_string(p.join("GEMINI.md")).unwrap(),
        gemini_md_after_install
    );
    assert!(p.join(".gemini/skills/evolution/SKILL.md").exists());
}

// --- global scope (subprocess: HOME is process-global) ----------------------

struct GlobalEnv {
    home: tempfile::TempDir,
    xdg_config: PathBuf,
    empty_path: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    bin: PathBuf,
    db: PathBuf,
}

impl GlobalEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let xdg_config = home.path().join("xdg-config");
        let empty_path = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let bin = workspace.path().join("skillvolution");
        fs::write(&bin, "test binary").unwrap();
        let db = workspace.path().join("vault.sqlite3");
        Self {
            home,
            xdg_config,
            empty_path,
            _workspace: workspace,
            bin,
            db,
        }
    }

    /// Base command with HOME/XDG_CONFIG_HOME/CLAUDE_CONFIG_DIR/PATH pinned to this
    /// sandbox (like `tests/setup_global.rs`'s `Env`), so `doctor`'s checks of every
    /// other client never touch this machine's real config files.
    fn base(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        cmd.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("GEMINI_CLI_HOME")
            .env("PATH", self.empty_path.path())
            // Outside any git repo, so the project-install warning never fires by accident.
            .current_dir(self.home.path());
        cmd
    }

    fn command(&self) -> Command {
        let mut cmd = self.base();
        cmd.arg("--db")
            .arg(&self.db)
            .arg("setup")
            .arg("--bin")
            .arg(&self.bin)
            .arg("--client")
            .arg("gemini");
        cmd
    }

    /// `setup` with no `--bin`: it then defaults to its own current executable, the same
    /// default `doctor` assumes when it recomputes what setup would write today — so this
    /// is the only way the two can agree in a test, which runs both as the same built
    /// binary (see `tests/doctor.rs`'s `Env::setup`, the model for this).
    fn setup_default_bin(&self) -> Command {
        let mut cmd = self.base();
        cmd.arg("--db")
            .arg(&self.db)
            .arg("setup")
            .arg("--client")
            .arg("gemini");
        cmd
    }

    fn doctor(&self) -> Command {
        let mut cmd = self.base();
        cmd.arg("--db").arg(&self.db).arg("doctor");
        cmd
    }

    fn gemini_dir(&self) -> PathBuf {
        self.home.path().join(".gemini")
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
fn global_gemini_writes_expected_files() {
    let env = GlobalEnv::new();

    let output = env.command().output().unwrap();
    assert_success(&output);

    let skill = fs::read_to_string(env.gemini_dir().join("skills/evolution/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: evolution\n"));

    let settings: Value =
        serde_json::from_slice(&fs::read(env.gemini_dir().join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        settings["mcpServers"]["skillvolution"],
        json!({
            "command": env.bin,
            "args": ["--db", env.db, "serve"],
            "trust": true,
        })
    );
    let session_cmd = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        session_cmd.contains("hook session-start --client gemini"),
        "{session_cmd}"
    );
    assert!(!session_cmd.contains("--project"), "{session_cmd}");

    let gemini_md = fs::read_to_string(env.gemini_dir().join("GEMINI.md")).unwrap();
    assert!(gemini_md.contains("<!-- skillvolution:start -->"));
}

/// `$GEMINI_CLI_HOME` overrides the home directory `~/.gemini` is computed from (verified
/// against `packages/core/src/utils/paths.ts`'s `homedir()` in the gemini-cli repo, since
/// it isn't in the published docs).
#[test]
fn global_gemini_honors_gemini_cli_home_override() {
    let env = GlobalEnv::new();
    let override_home = tempfile::tempdir().unwrap();

    let output = env
        .command()
        .env("GEMINI_CLI_HOME", override_home.path())
        .output()
        .unwrap();
    assert_success(&output);

    assert!(override_home.path().join(".gemini/settings.json").exists());
    assert!(!env.gemini_dir().join("settings.json").exists());
}

#[test]
fn global_gemini_doctor_reports_configured_up_to_date() {
    let env = GlobalEnv::new();
    assert_success(&env.setup_default_bin().output().unwrap());

    let output = env.doctor().output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Gemini CLI: configured, up to date"),
        "{stdout}"
    );
}

// --- hook CLI end-to-end (subprocess: real stdin/stdout) ---------------------

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

fn after_tool_input(session: &str, tool: &str) -> String {
    json!({"session_id": session, "tool_name": tool}).to_string()
}

fn after_agent_input(session: &str, stop_hook_active: bool) -> String {
    json!({"session_id": session, "stop_hook_active": stop_hook_active}).to_string()
}

#[test]
fn cli_gemini_tool_use_then_stop_denies_once_then_not_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();

    // A realistic Gemini AfterTool payload: the FQN `mcp_{server}_{tool}` naming means
    // `write_file` (a GEMINI_WORK_TOOLS entry) is what a real hook would see verbatim.
    let tool_use = run_hook(
        &db,
        &["tool-use", "--client", "gemini"],
        &after_tool_input("s1", "write_file"),
    );
    assert_success(&tool_use);
    assert!(tool_use.stdout.is_empty());

    let blocked = run_hook(
        &db,
        &["stop", "--client", "gemini"],
        &after_agent_input("s1", false),
    );
    assert_success(&blocked);
    let decision: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(decision["decision"], "deny");
    assert!(
        decision["reason"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
        "{decision}"
    );

    // Every judged stop consumes its span, so an identical second stop (still
    // stop_hook_active: false) sees no new work and stays silent.
    let clean = run_hook(
        &db,
        &["stop", "--client", "gemini"],
        &after_agent_input("s1", false),
    );
    assert_success(&clean);
    assert!(clean.stdout.is_empty());
}

#[test]
fn cli_gemini_stop_hook_active_never_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();

    run_hook(
        &db,
        &["tool-use", "--client", "gemini"],
        &after_tool_input("s1", "replace"),
    );
    let output = run_hook(
        &db,
        &["stop", "--client", "gemini"],
        &after_agent_input("s1", true),
    );
    assert_success(&output);
    assert!(output.stdout.is_empty());
}

#[test]
fn cli_gemini_review_tool_marks_the_span_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();

    run_hook(
        &db,
        &["tool-use", "--client", "gemini"],
        &after_tool_input("s1", "write_file"),
    );
    // The fully-qualified MCP tool name Gemini reports (`mcp_{server}_{tool}`).
    run_hook(
        &db,
        &["tool-use", "--client", "gemini"],
        &after_tool_input("s1", "mcp_skillvolution_report_skill_outcome"),
    );
    let output = run_hook(
        &db,
        &["stop", "--client", "gemini"],
        &after_agent_input("s1", false),
    );
    assert_success(&output);
    assert!(output.stdout.is_empty());
}

#[test]
fn cli_gemini_session_start_wraps_context_in_hook_specific_output() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();

    let output = run_hook(&db, &["session-start", "--client", "gemini"], "");
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(
        value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
    );
}

// Confirms `hook::GEMINI_WORK_TOOLS` still matches the built-in tool names this test file
// relies on (write_file/replace); a rename in `src/hook.rs` should fail this instead of
// silently drifting from the constant the CLI actually dispatches on.
#[test]
fn gemini_work_tools_are_the_documented_file_editing_tool_names() {
    assert_eq!(hook::GEMINI_WORK_TOOLS, ["write_file", "replace"]);
}
