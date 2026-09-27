//! `skillvolution doctor`: read-only health checks of the vault and of the
//! client configurations `setup` wrote. Every test sets HOME, XDG_CONFIG_HOME,
//! CLAUDE_CONFIG_DIR and PATH explicitly and runs the built binary as a
//! subprocess, like tests/setup_global.rs does.

// Only needed by tests that require a real, executable fake `claude` on PATH; see
// `write_fake_claude`.
#[cfg(unix)]
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Env {
    home: tempfile::TempDir,
    xdg_config: PathBuf,
    // Devin's global config dir on Windows (`%APPDATA%\devin`); sandboxed here so a
    // `setup --client all`/`devin` run in one test can't leave real files in the host
    // profile for another test (or the host) to trip over.
    appdata: PathBuf,
    empty_path: tempfile::TempDir,
    // Kept alive only so `bin`/`db`, which point inside it, stay valid for the test.
    _workspace: tempfile::TempDir,
    bin: PathBuf,
    db: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let xdg_config = home.path().join("xdg-config");
        let appdata = home.path().join("AppData/Roaming");
        let empty_path = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // Named exactly `skillvolution` (see tests/setup_global.rs) and left
        // executable, since doctor's own configured-binary check requires that.
        let bin = workspace.path().join("skillvolution");
        write_executable(&bin, "#!/bin/sh\nexit 0\n");
        let db = workspace.path().join("vault.sqlite3");
        Self {
            home,
            xdg_config,
            appdata,
            empty_path,
            _workspace: workspace,
            bin,
            db,
        }
    }

    /// Base command with HOME/XDG_CONFIG_HOME/CLAUDE_CONFIG_DIR/APPDATA/PATH pinned to
    /// this sandbox and no `claude` on PATH, outside any git repo so the
    /// legacy-install warning never fires by accident.
    fn cli(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        cmd.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("APPDATA", &self.appdata)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", self.empty_path.path())
            .current_dir(self.home.path());
        cmd
    }

    /// `setup` with no `--bin`: it then defaults to its own current executable,
    /// the same default `doctor` assumes when it computes what setup would
    /// write today — so this is the only way the two can agree in a test,
    /// which runs both as the same built binary.
    fn setup(&self) -> Command {
        let mut cmd = self.cli();
        cmd.arg("--db").arg(&self.db).arg("setup");
        cmd
    }

    /// `setup --bin <bin>`, for tests that need control over the configured
    /// binary path (so it can be deleted without deleting the test binary
    /// itself).
    fn setup_with_bin(&self, bin: &Path) -> Command {
        let mut cmd = self.setup();
        cmd.arg("--bin").arg(bin);
        cmd
    }

    fn doctor(&self) -> Command {
        let mut cmd = self.cli();
        cmd.arg("--db").arg(&self.db).arg("doctor");
        cmd
    }

    // Only used to seed a foreign edit to OpenCode's config in a test that needs a
    // real, executable fake `claude` on PATH; see `write_fake_claude`.
    #[cfg(unix)]
    fn opencode_dir(&self) -> PathBuf {
        self.xdg_config.join("opencode")
    }
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A fake `claude` that succeeds on every call: `mcp add-json` (setup's
/// registration) and `mcp get` (doctor's registration check) alike.
///
/// A `#!/bin/sh` script; unix-only, like every test that calls it. On Windows there's
/// no way to make a text file with a shebang line run as `claude` would (it isn't a
/// valid executable), so exercising a real, successful registration round-trip isn't
/// something this fake can cover there.
#[cfg(unix)]
fn write_fake_claude(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    write_executable(&dir.join("claude"), "#!/bin/sh\nexit 0\n");
    dir.to_owned()
}

// Only used to read back OpenCode's config in a test that needs a real, executable
// fake `claude` on PATH; see `write_fake_claude`.
#[cfg(unix)]
fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Sets `application_id`/`user_version` on a brand new (otherwise empty)
/// database file, bypassing `Vault::open` so doctor sees exactly the schema
/// state a real vault at that version would have without running any
/// migration itself.
fn raw_db_with_version(path: &Path, application_id: i64, user_version: i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.pragma_update(None, "application_id", application_id)
        .unwrap();
    conn.pragma_update(None, "user_version", user_version)
        .unwrap();
}

/// `PRAGMA application_id` of a Skillvolution vault ("SKV1"); doctor.rs's own
/// copy is private, so this mirrors `skillvolution::vault`'s constant.
const APPLICATION_ID: i64 = 0x534B_5631;

#[test]
fn fresh_home_no_clients_no_db_is_healthy_and_creates_no_database() {
    let env = Env::new();

    let output = env.doctor().output().unwrap();
    assert_success(&output);
    assert!(!env.db.exists(), "doctor must never create the database");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("[warn] vault: database: will be created on first use"),
        "{stdout}"
    );
    for name in ["Claude Code", "OpenCode", "Devin CLI"] {
        assert!(
            stdout.contains(&format!("[ok] {name}: not installed")),
            "{stdout}"
        );
    }
}

// Needs a real, executable fake `claude` on PATH so setup's `claude mcp add-json` and
// doctor's `claude mcp get` both actually run; see `write_fake_claude`.
#[cfg(unix)]
#[test]
fn after_setup_all_clients_every_client_is_up_to_date() {
    let env = Env::new();
    let bin_dir = write_fake_claude(&env.home.path().join("bin"));

    assert_success(
        &env.setup()
            .arg("--client")
            .arg("all")
            .env("PATH", &bin_dir)
            .output()
            .unwrap(),
    );

    let output = env.doctor().env("PATH", &bin_dir).output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    for name in ["Claude Code", "OpenCode", "Devin CLI"] {
        assert!(
            stdout.contains(&format!("[ok] {name}: configured, up to date")),
            "{stdout}"
        );
    }
    assert!(stdout.contains("[ok] vault: schema"), "{stdout}");
    assert!(stdout.contains("[ok] vault: search index"), "{stdout}");
    assert!(!stdout.contains("[fail]"), "{stdout}");
}

// Needs a real, executable fake `claude` on PATH; see `write_fake_claude`.
#[cfg(unix)]
#[test]
fn editing_a_configs_mcp_command_warns_only_for_that_client() {
    let env = Env::new();
    let bin_dir = write_fake_claude(&env.home.path().join("bin"));
    assert_success(
        &env.setup()
            .arg("--client")
            .arg("all")
            .env("PATH", &bin_dir)
            .output()
            .unwrap(),
    );

    // Append a bogus extra argument to OpenCode's MCP command: the referenced
    // binary is still the real one (so this must not also fail the configured-
    // binary check), but the file no longer matches what setup would write.
    let config_path = env.opencode_dir().join("opencode.json");
    let mut config = read_json(&config_path);
    config["mcp"]["skillvolution"]["command"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("--extra".to_owned()));
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let output = env.doctor().env("PATH", &bin_dir).output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "[warn] OpenCode: differs from what `skillvolution setup --client opencode` would write"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(&config_path.display().to_string()),
        "{stdout}"
    );
    assert!(
        stdout.contains("[ok] Claude Code: configured, up to date"),
        "{stdout}"
    );
    assert!(!stdout.contains("[fail]"), "{stdout}");
}

#[test]
fn deleting_the_configured_binary_fails_with_exit_code_1() {
    let env = Env::new();
    assert_success(
        &env.setup_with_bin(&env.bin)
            .arg("--client")
            .arg("claude-code")
            .output()
            .unwrap(),
    );
    fs::remove_file(&env.bin).unwrap();

    let output = env.doctor().output().unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(&format!(
            "[fail] Claude Code binary: configured binary {} does not exist; rerun setup",
            env.bin.display()
        )),
        "{stdout}"
    );
}

#[test]
fn database_newer_than_supported_fails() {
    let env = Env::new();
    raw_db_with_version(&env.db, APPLICATION_ID, 99);

    let output = env.doctor().output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[fail] vault: schema"), "{stdout}");
    assert!(stdout.contains("upgrade skillvolution"), "{stdout}");
}

#[test]
fn database_older_than_supported_warns_and_is_left_unmigrated() {
    let env = Env::new();
    raw_db_with_version(&env.db, APPLICATION_ID, 2);

    let output = env.doctor().output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[warn] vault: schema"), "{stdout}");
    assert!(stdout.contains("will be upgraded"), "{stdout}");

    let conn = rusqlite::Connection::open(&env.db).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2, "doctor must never migrate the database");
}

// Needs a real, executable fake `claude` on PATH; see `write_fake_claude`.
#[cfg(unix)]
#[test]
fn json_output_parses_with_healthy_and_checks() {
    let env = Env::new();
    let bin_dir = write_fake_claude(&env.home.path().join("bin"));
    assert_success(
        &env.setup()
            .arg("--client")
            .arg("claude-code")
            .env("PATH", &bin_dir)
            .output()
            .unwrap(),
    );

    let output = env
        .doctor()
        .arg("--json")
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["healthy"], true);
    let checks = report["checks"].as_array().unwrap();
    assert!(!checks.is_empty());
    for check in checks {
        assert!(check["name"].is_string(), "{check}");
        assert!(check["status"].is_string(), "{check}");
        assert!(check["detail"].is_string(), "{check}");
    }
    assert!(
        checks
            .iter()
            .any(|c| c["status"] == "ok" && c["name"] == "Claude Code")
    );
}

/// Publishes one valid skill into `db` directly through the library.
fn publish(db: &Path, id: &str) {
    let mut vault = skillvolution::vault::Vault::open(db).unwrap();
    vault
        .propose(&skillvolution::vault::Proposal {
            id,
            description: "a test skill",
            tags: &[],
            content: "## When to use\nx\n## Procedure\nx\n## Pitfalls\nx\n## Verification\nx",
            evidence: "evidence",
            expected_version: 0,
            scope: None,
            verdict: "keep global",
            verdict_reason: "because",
            replaces_proven: false,
        })
        .unwrap();
}

#[test]
fn a_deprecated_skill_keeps_the_search_index_healthy() {
    let env = Env::new();
    publish(&env.db, "old-skill");
    skillvolution::vault::Vault::open(&env.db)
        .unwrap()
        .set_deprecated("old-skill", true)
        .unwrap();

    let output = env.doctor().output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("[ok] vault: search index: 1 skill(s) indexed, no orphans"),
        "{stdout}"
    );
}

#[test]
fn a_relative_skillvolution_db_matches_what_setup_wrote() {
    let env = Env::new();
    let output = env
        .cli()
        .env("SKILLVOLUTION_DB", "vault.sqlite3")
        .args(["setup", "--client", "opencode"])
        .output()
        .unwrap();
    assert_success(&output);

    let output = env
        .cli()
        .env("SKILLVOLUTION_DB", "vault.sqlite3")
        .arg("doctor")
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("[ok] OpenCode: configured, up to date"),
        "{stdout}"
    );
}

#[test]
fn a_malformed_client_config_fails_that_client_and_checks_the_others() {
    let env = Env::new();
    assert_success(&env.setup().args(["--client", "opencode"]).output().unwrap());
    let gemini_dir = env.home.path().join(".gemini");
    fs::create_dir_all(&gemini_dir).unwrap();
    fs::write(gemini_dir.join("settings.json"), "not json").unwrap();

    let output = env.doctor().output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[fail] Gemini CLI: "), "{stdout}");
    assert!(
        stdout.contains("[ok] OpenCode: configured, up to date"),
        "{stdout}"
    );
    assert!(stdout.contains("[ok] Cursor: not installed"), "{stdout}");
}

#[test]
fn a_relocated_default_vault_warns_that_the_cli_still_uses_the_default() {
    let env = Env::new();
    let default_db = env.home.path().join(".local/share/skillvolution/skills.db");
    fs::create_dir_all(default_db.parent().unwrap()).unwrap();
    fs::write(
        default_db.with_file_name("skills.db.relocated.bak"),
        "old vault",
    )
    .unwrap();

    let output = env
        .cli()
        .env_remove("SKILLVOLUTION_DB")
        .env_remove("XDG_DATA_HOME")
        // On Windows, LOCALAPPDATA outranks this HOME/XDG fallback; remove it so the
        // default database resolves to `default_db` on every platform.
        .env_remove("LOCALAPPDATA")
        .arg("doctor")
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[warn] vault: relocated"), "{stdout}");
    assert!(stdout.contains("SKILLVOLUTION_DB"), "{stdout}");
    assert!(!default_db.exists());
}
