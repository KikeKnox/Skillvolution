//! `SKILLVOLUTION_DB` precedence and `skillvolution relocate`, run against the built
//! binary (never against `skillvolution::setup` directly: `SetupArgs` is only
//! constructible through clap, and these process-global environment variables couldn't
//! be isolated between tests from in-process calls anyway).

use serde_json::Value;
use skillvolution::vault::{Proposal, Vault};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_skillvolution"))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success(), "expected failure but succeeded");
}

/// Publishes one valid skill into `db`, directly through the library, so the tests
/// below don't need a whole propose/evaluate round trip.
fn publish(db: &Path, id: &str) {
    let mut vault = Vault::open(db).unwrap();
    vault
        .propose(&Proposal {
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

// --- SKILLVOLUTION_DB precedence -------------------------------------------------

#[test]
fn skillvolution_db_env_var_is_the_default_database_when_no_flag_is_given() {
    let workspace = tempfile::tempdir().unwrap();
    let db = workspace.path().join("from-env/vault.sqlite3");

    let output = bin()
        .arg("export")
        .env("SKILLVOLUTION_DB", &db)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_success(&output);
    assert!(db.is_file(), "{}", db.display());
}

#[test]
fn a_db_flag_still_wins_over_skillvolution_db() {
    let workspace = tempfile::tempdir().unwrap();
    let from_env = workspace.path().join("from-env.sqlite3");
    let from_flag = workspace.path().join("from-flag.sqlite3");

    let output = bin()
        .arg("--db")
        .arg(&from_flag)
        .arg("export")
        .env("SKILLVOLUTION_DB", &from_env)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(from_flag.is_file());
    assert!(!from_env.exists());
}

#[test]
fn a_relative_skillvolution_db_resolves_against_the_current_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let output = bin()
        .arg("export")
        .env("SKILLVOLUTION_DB", "relative/vault.sqlite3")
        .current_dir(workspace.path())
        .output()
        .unwrap();
    assert_success(&output);
    assert!(workspace.path().join("relative/vault.sqlite3").is_file());
}

// --- relocate ---------------------------------------------------------------------

/// Writes an executable fake `claude` into `dir` that appends its argv to `log` (one
/// line per invocation, `\n`-joined) and always exits 0.
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

struct Env {
    home: tempfile::TempDir,
    /// `home`'s path, canonicalized: `relocate` canonicalizes an existing source path
    /// (see `absolutize`), so test-side paths built for comparison must start from the
    /// same canonical base, in case the OS temp directory itself is a symlink.
    home_real: PathBuf,
    xdg_config: PathBuf,
    bin_dir: PathBuf,
    log: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let home_real = fs::canonicalize(home.path()).unwrap();
        let xdg_config = home.path().join("xdg-config");
        let bin_dir = home.path().join("bin");
        let log = home.path().join("claude.log");
        write_fake_claude(&bin_dir, &log);
        Self {
            home,
            home_real,
            xdg_config,
            bin_dir,
            log,
        }
    }

    /// Base command with HOME/XDG_CONFIG_HOME/PATH pinned to this sandbox (PATH has
    /// only the fake `claude`) and no `--bin`, so setup and relocate both fall back to
    /// the running test binary's own path, which is a real, existing executable.
    fn command(&self) -> Command {
        let mut cmd = bin();
        cmd.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", &self.bin_dir)
            .env("SKILLVOLUTION_NO_INPUT", "1")
            // Outside any git repo, so the project-install warning never fires.
            .current_dir(self.home.path());
        cmd
    }

    fn claude_settings(&self) -> Value {
        read_json(self.home.path().join(".claude/settings.json"))
    }

    fn opencode_config(&self) -> Value {
        read_json(self.xdg_config.join("opencode/opencode.json"))
    }

    fn devin_mcp_config(&self) -> Value {
        read_json(self.home.path().join(".config/devin/mcp_config.json"))
    }

    fn claude_log_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The `--db` a settings.json Claude Code hook command was built with, read back out
/// of the (shell-quoted) SessionStart command string.
fn hook_db_arg(settings: &Value) -> String {
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    command.to_owned()
}

#[test]
fn relocate_moves_the_vault_repoints_every_global_client_and_backs_up_the_old_file() {
    let env = Env::new();
    let db_a = env.home_real.join("vault-a.sqlite3");
    let db_b = env.home_real.join("moved/vault-b.sqlite3");

    let output = env
        .command()
        .arg("--db")
        .arg(&db_a)
        .arg("setup")
        .arg("--client")
        .arg("all")
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(
        env.claude_log_lines().len(),
        1,
        "{:?}",
        env.claude_log_lines()
    );

    publish(&db_a, "a-test-skill");

    let output = env
        .command()
        .arg("--db")
        .arg(&db_a)
        .arg("relocate")
        .arg(&db_b)
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(stdout.contains("Repointed Claude Code"), "{stdout}");
    assert!(stdout.contains("Repointed OpenCode"), "{stdout}");
    assert!(stdout.contains("Repointed Devin"), "{stdout}");
    assert!(stdout.contains("skillvolution setup --project"), "{stdout}");

    // Every global client config now points at B, not A.
    let db_b_str = db_b.to_str().unwrap();
    assert!(hook_db_arg(&env.claude_settings()).contains(db_b_str));
    assert_eq!(
        env.opencode_config()["mcp"]["skillvolution"]["command"][2],
        db_b_str
    );
    assert_eq!(
        env.devin_mcp_config()["mcpServers"]["skillvolution"]["args"][1],
        db_b_str
    );

    // Claude Code's MCP registration (outside any file) was re-run.
    assert_eq!(
        env.claude_log_lines().len(),
        2,
        "{:?}",
        env.claude_log_lines()
    );

    // B has the data, A is gone but kept as a renamed backup.
    let moved = Vault::open(&db_b).unwrap();
    assert!(moved.export_json().unwrap().contains("a-test-skill"));
    assert!(!db_a.exists());
    let backup = env.home_real.join("vault-a.sqlite3.relocated.bak");
    assert!(backup.is_file());
    let restored = Vault::open(&backup).unwrap();
    assert!(restored.export_json().unwrap().contains("a-test-skill"));
}

#[test]
fn relocate_onto_an_existing_path_fails_without_changing_anything() {
    let workspace = tempfile::tempdir().unwrap();
    let from = workspace.path().join("vault.sqlite3");
    let to = workspace.path().join("taken.sqlite3");
    publish(&from, "existing-skill");
    fs::write(&to, "not a vault").unwrap();

    let output = bin()
        .arg("--db")
        .arg(&from)
        .arg("relocate")
        .arg(&to)
        .env("SKILLVOLUTION_NO_INPUT", "1")
        .output()
        .unwrap();
    assert_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("already exists"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(fs::read_to_string(&to).unwrap(), "not a vault");
    let vault = Vault::open(&from).unwrap();
    assert!(vault.export_json().unwrap().contains("existing-skill"));
}

#[test]
fn relocate_with_a_missing_source_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let from = workspace.path().join("missing.sqlite3");
    let to = workspace.path().join("new.sqlite3");

    let output = bin()
        .arg("--db")
        .arg(&from)
        .arg("relocate")
        .arg(&to)
        .env("SKILLVOLUTION_NO_INPUT", "1")
        .output()
        .unwrap();
    assert_failure(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to relocate"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!to.exists());
}

#[test]
fn relocate_with_a_malformed_client_config_fails_before_copying() {
    let env = Env::new();
    let from = env.home_real.join("vault.sqlite3");
    let to = env.home_real.join("moved/vault.sqlite3");
    publish(&from, "kept-skill");
    let gemini_dir = env.home.path().join(".gemini");
    fs::create_dir_all(&gemini_dir).unwrap();
    fs::write(gemini_dir.join("settings.json"), "not json").unwrap();

    let output = env
        .command()
        .arg("--db")
        .arg(&from)
        .arg("relocate")
        .arg(&to)
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Gemini CLI"), "{stderr}");
    assert!(!to.exists());
    assert!(!to.parent().unwrap().exists());
    assert!(from.is_file());
}

#[test]
fn relocating_the_default_vault_tells_how_to_point_the_cli_at_the_new_one() {
    let env = Env::new();
    let from = env.home_real.join(".local/share/skillvolution/skills.db");
    let to = env.home_real.join("moved/vault.sqlite3");
    publish(&from, "kept-skill");

    let output = env
        .command()
        .env_remove("SKILLVOLUTION_DB")
        .env_remove("XDG_DATA_HOME")
        .arg("relocate")
        .arg(&to)
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("close running AI client sessions"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("SKILLVOLUTION_DB={}", to.display())),
        "{stdout}"
    );
    assert!(!from.exists());
}
