//! Global (per-user) `skillvolution setup`: no `--project`. Every test sets HOME,
//! XDG_CONFIG_HOME, CLAUDE_CONFIG_DIR and PATH explicitly and runs the built binary as a
//! subprocess, since these are process-global environment variables that an in-process
//! call to `setup::run` could not isolate between tests.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Env {
    home: tempfile::TempDir,
    xdg_config: PathBuf,
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
        let empty_path = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // Named exactly `skillvolution`: hook ownership (see `owned_command`) matches on
        // the binary's file name, so a rerun with this same path must recognize its own
        // previous hook entries and replace them instead of duplicating them.
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
    /// sandbox and no `claude` on PATH. Callers add `--client` and override PATH to add
    /// a fake `claude` when needed.
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        cmd.arg("--db")
            .arg(&self.db)
            .arg("setup")
            .arg("--bin")
            .arg(&self.bin)
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", self.empty_path.path());
        cmd
    }

    fn claude_dir(&self) -> PathBuf {
        self.home.path().join(".claude")
    }

    fn opencode_dir(&self) -> PathBuf {
        self.xdg_config.join("opencode")
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Writes an executable fake `claude` into `dir` that appends its argv to `log` (one
/// line per invocation) and, unless `fail_add_json`, always exits 0. With
/// `fail_add_json`, every `mcp add-json` call prints an unrelated error to stderr and
/// exits 1 (not "already exists"), so setup's fresh-add attempt fails outright without
/// ever calling `mcp remove`.
fn write_fake_claude(dir: &Path, log: &Path, fail_add_json: bool) -> PathBuf {
    let failure = if fail_add_json {
        "if [ \"$1 $2\" = \"mcp add-json\" ]; then echo 'fake add-json boom' >&2; exit 1; fi\n"
            .to_owned()
    } else {
        String::new()
    };
    write_fake_claude_script(
        dir,
        &format!("echo \"$@\" >> {{log}}\n{failure}exit 0\n"),
        log,
    )
}

/// Writes an executable fake `claude` that simulates updating an existing user-scope
/// `skillvolution` registration: its first `mcp add-json` call fails with "already
/// exists" (as the real CLI does), `mcp remove` succeeds, and its second `add-json` call
/// succeeds or fails per `second_add_succeeds`. Counts calls itself (in a sibling file,
/// via shell builtins only) rather than relying on `grep`/`wc`, since the test process
/// runs this script with `PATH` pointed only at its own directory.
fn write_fake_claude_already_registered(
    dir: &Path,
    log: &Path,
    second_add_succeeds: bool,
) -> PathBuf {
    let second = if second_add_succeeds {
        "exit 0\n".to_owned()
    } else {
        "echo 'fake second add-json boom' >&2; exit 1\n".to_owned()
    };
    let script = format!(
        "echo \"$@\" >> {{log}}\n\
         if [ \"$1 $2\" = \"mcp add-json\" ]; then\n\
         \x20\x20count=0\n\
         \x20\x20[ -f {{count_file}} ] && read count < {{count_file}}\n\
         \x20\x20count=$((count + 1))\n\
         \x20\x20echo \"$count\" > {{count_file}}\n\
         \x20\x20if [ \"$count\" = \"1\" ]; then\n\
         \x20\x20\x20\x20echo 'MCP server skillvolution already exists in user config' >&2\n\
         \x20\x20\x20\x20exit 1\n\
         \x20\x20fi\n\
         \x20\x20{second}\
         fi\n\
         exit 0\n"
    );
    write_fake_claude_script(
        dir,
        &script.replace(
            "{count_file}",
            &dir.join("add-json-count").display().to_string(),
        ),
        log,
    )
}

fn write_fake_claude_script(dir: &Path, body_template: &str, log: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let claude = dir.join("claude");
    let body = body_template.replace("{log}", &log.display().to_string());
    fs::write(&claude, format!("#!/bin/sh\n{body}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir.to_owned()
}

#[test]
fn global_both_clients_write_expected_files_and_register_mcp() {
    let env = Env::new();
    let log = env.home.path().join("claude.log");
    let bin_dir = write_fake_claude(&env.home.path().join("bin"), &log, false);

    let output = env
        .command()
        .arg("--client")
        .arg("both")
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert_success(&output);

    // Claude Code: skill + settings.json hooks, no --project anywhere.
    let skill = fs::read_to_string(env.claude_dir().join("skills/evolution/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: evolution\n"));
    let settings = read_json(env.claude_dir().join("settings.json"));
    let session_cmd = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let stop_cmd = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        session_cmd.ends_with(" hook session-start"),
        "{session_cmd}"
    );
    assert!(stop_cmd.ends_with(" hook stop"), "{stop_cmd}");
    assert!(!session_cmd.contains("--project"));

    // OpenCode: opencode.json entry with no --project, skill, AGENTS.md, plugin.
    let oc = read_json(env.opencode_dir().join("opencode.json"));
    let command = oc["mcp"]["skillvolution"]["command"].as_array().unwrap();
    assert_eq!(command.len(), 4, "{command:?}");
    assert_eq!(command[0], env.bin.to_str().unwrap());
    assert_eq!(command[1], "--db");
    assert_eq!(command[2], env.db.to_str().unwrap());
    assert_eq!(command[3], "serve");
    assert_eq!(oc["mcp"]["skillvolution"]["type"], "local");

    let oc_skill =
        fs::read_to_string(env.opencode_dir().join("skills/evolution/SKILL.md")).unwrap();
    assert!(oc_skill.starts_with("---\nname: evolution\n"));

    let agents = fs::read_to_string(env.opencode_dir().join("AGENTS.md")).unwrap();
    assert!(agents.contains("<!-- skillvolution:start -->"));
    assert!(agents.contains("<!-- skillvolution:end -->"));

    let plugin = fs::read_to_string(env.opencode_dir().join("plugins/skillvolution.js")).unwrap();
    assert!(plugin.contains("skillvolution-managed:opencode-plugin"));
    assert!(!plugin.contains("__SKILLVOLUTION_"));
    assert!(plugin.contains(&serde_json::to_string(env.bin.to_str().unwrap()).unwrap()));
    assert!(plugin.contains(&serde_json::to_string(env.db.to_str().unwrap()).unwrap()));

    // A fresh registration (nothing named skillvolution existed yet) is a single
    // `add-json` call; `remove` is only needed to replace an existing one.
    let log_text = fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = log_text.lines().collect();
    assert_eq!(lines.len(), 1, "{log_text}");
    assert!(lines[0].starts_with("mcp add-json --scope user skillvolution "));
    let json_arg = lines[0]
        .strip_prefix("mcp add-json --scope user skillvolution ")
        .unwrap();
    let mcp_json: Value = serde_json::from_str(json_arg).unwrap();
    assert_eq!(mcp_json["type"], "stdio");
    assert_eq!(mcp_json["command"], env.bin.to_str().unwrap());
    assert_eq!(
        mcp_json["args"],
        serde_json::json!(["--db", env.db, "serve"])
    );
}

#[test]
fn global_claude_code_without_claude_cli_still_writes_files_and_prints_note() {
    let env = Env::new();
    let output = env
        .command()
        .arg("--client")
        .arg("claude-code")
        .output()
        .unwrap();
    assert_success(&output);

    assert!(env.claude_dir().join("skills/evolution/SKILL.md").exists());
    assert!(env.claude_dir().join("settings.json").exists());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("claude CLI not found"), "{stdout}");
    assert!(
        stdout.contains("claude mcp add-json --scope user skillvolution"),
        "{stdout}"
    );
    assert!(stdout.contains(env.bin.to_str().unwrap()), "{stdout}");
}

#[test]
fn global_detect_only_claude_present_configures_claude_and_skips_opencode() {
    // No `--client`: setup must detect that only Claude Code is present (a fake `claude`
    // on PATH, no OpenCode config dir) and configure just that client.
    let env = Env::new();
    let log = env.home.path().join("claude.log");
    let bin_dir = write_fake_claude(&env.home.path().join("bin"), &log, false);

    let output = env.command().env("PATH", &bin_dir).output().unwrap();
    assert_success(&output);

    assert!(env.claude_dir().join("skills/evolution/SKILL.md").exists());
    assert!(env.claude_dir().join("settings.json").exists());
    assert!(!env.opencode_dir().join("opencode.json").exists());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "Skipped OpenCode: not detected (run `skillvolution setup --client opencode` after installing it)."
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("Skipped Claude Code"), "{stdout}");
}

#[test]
fn global_detect_only_opencode_config_dir_present_configures_opencode_and_skips_claude() {
    // No `--client`: no `claude`/`opencode` on PATH and no Claude config dir, but an
    // OpenCode config dir already exists, so only OpenCode gets configured.
    let env = Env::new();
    fs::create_dir_all(env.opencode_dir()).unwrap();

    let output = env.command().output().unwrap();
    assert_success(&output);

    assert!(env.opencode_dir().join("opencode.json").exists());
    assert!(!env.claude_dir().join("settings.json").exists());
    assert!(!env.claude_dir().join("skills/evolution/SKILL.md").exists());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "Skipped Claude Code: not detected (run `skillvolution setup --client claude-code` after installing it)."
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("Skipped OpenCode"), "{stdout}");
}

#[test]
fn global_detect_neither_present_exits_ok_without_writing_client_files() {
    // No `--client` and neither client detected: setup must still exit 0, print a note,
    // and write no client files (the database is created regardless).
    let env = Env::new();

    let output = env.command().output().unwrap();
    assert_success(&output);

    assert!(!env.claude_dir().exists());
    assert!(!env.opencode_dir().exists());
    assert!(env.db.is_file());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "No AI client detected; skipping setup (run `skillvolution setup --client <name>` after installing one)."
        ),
        "{stdout}"
    );
}

#[test]
fn global_explicit_client_both_configures_both_even_when_neither_detected() {
    // An explicit --client bypasses detection entirely, even when neither client would
    // otherwise be detected.
    let env = Env::new();

    let output = env.command().arg("--client").arg("both").output().unwrap();
    assert_success(&output);

    assert!(env.claude_dir().join("skills/evolution/SKILL.md").exists());
    assert!(env.claude_dir().join("settings.json").exists());
    assert!(env.opencode_dir().join("opencode.json").exists());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("Skipped"), "{stdout}");
}

#[test]
fn global_add_json_failure_fails_setup_but_keeps_the_files_it_already_wrote() {
    let env = Env::new();
    let log = env.home.path().join("claude.log");
    let bin_dir = write_fake_claude(&env.home.path().join("bin"), &log, true);

    let output = env
        .command()
        .arg("--client")
        .arg("claude-code")
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fake add-json boom"), "{stderr}");

    // Files were written before the CLI call, and setup does not roll them back.
    assert!(env.claude_dir().join("skills/evolution/SKILL.md").exists());
    assert!(env.claude_dir().join("settings.json").exists());
}

#[test]
fn global_mcp_update_of_existing_registration_removes_then_readds() {
    // The first add-json attempt reports the name already exists (as the real `claude`
    // CLI does for a second registration under the same name); setup must then remove
    // the old one and add the new definition, rather than treating the first failure as
    // fatal and leaving the old (possibly stale) registration in place.
    let env = Env::new();
    let log = env.home.path().join("claude.log");
    let bin_dir = write_fake_claude_already_registered(&env.home.path().join("bin"), &log, true);

    let output = env
        .command()
        .arg("--client")
        .arg("claude-code")
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert_success(&output);

    let lines: Vec<String> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines,
        vec![
            format!(
                "mcp add-json --scope user skillvolution {}",
                serde_json::json!({
                    "type": "stdio",
                    "command": env.bin,
                    "args": ["--db", &env.db, "serve"],
                })
            ),
            "mcp remove --scope user skillvolution".to_owned(),
            format!(
                "mcp add-json --scope user skillvolution {}",
                serde_json::json!({
                    "type": "stdio",
                    "command": env.bin,
                    "args": ["--db", &env.db, "serve"],
                })
            ),
        ],
        "{lines:?}"
    );
}

#[test]
fn global_mcp_update_failure_reports_removal_and_the_exact_recovery_command() {
    // The first add-json fails as "already exists", remove succeeds, but the follow-up
    // add-json fails too: setup must fail loudly with the exact command to re-register,
    // never pretend the update succeeded or silently leave no registration at all.
    let env = Env::new();
    let log = env.home.path().join("claude.log");
    let bin_dir = write_fake_claude_already_registered(&env.home.path().join("bin"), &log, false);

    let output = env
        .command()
        .arg("--client")
        .arg("claude-code")
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fake second add-json boom"), "{stderr}");
    assert!(stderr.contains("registration is now gone"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "claude mcp add-json --scope user skillvolution '{}'",
            serde_json::json!({
                "type": "stdio",
                "command": env.bin,
                "args": ["--db", &env.db, "serve"],
            })
        )),
        "{stderr}"
    );

    // Setup must still have run `mcp remove`, matching the removal the error reports.
    let log_text = fs::read_to_string(&log).unwrap();
    assert!(
        log_text.contains("mcp remove --scope user skillvolution"),
        "{log_text}"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_db_path_fails_cleanly_before_any_write() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let home = tempfile::tempdir().unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let bin = workspace.path().join("skillvolution");
    fs::write(&bin, "test binary").unwrap();
    // "vault-<invalid byte>.sqlite3": not representable as UTF-8, so it must be
    // rejected with a clear error rather than silently mangled into a hook command.
    let mut db_name = b"vault-".to_vec();
    db_name.push(0xff);
    db_name.extend_from_slice(b".sqlite3");
    let db = workspace.path().join(OsStr::from_bytes(&db_name));

    let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("setup")
        .arg("--client")
        .arg("claude-code")
        .arg("--bin")
        .arg(&bin)
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("PATH", empty_path.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not valid UTF-8"), "{stderr}");
    assert!(
        !home.path().join(".claude/settings.json").exists(),
        "must not write before the UTF-8 check fails"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_db_path_is_an_error_not_a_panic_in_every_mode() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let home = tempfile::tempdir().unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let bin = workspace.path().join("skillvolution");
    fs::write(&bin, "test binary").unwrap();
    let db = workspace
        .path()
        .join(OsStr::from_bytes(b"vault-\xff.sqlite3"));
    let project = workspace.path().join("project");
    fs::create_dir(&project).unwrap();

    let modes: [&[&OsStr]; 2] = [
        &[OsStr::new("--client"), OsStr::new("opencode")],
        &[OsStr::new("--project"), project.as_os_str()],
    ];
    for mode in modes {
        let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
            .arg("--db")
            .arg(&db)
            .arg("setup")
            .args(mode)
            .arg("--bin")
            .arg(&bin)
            .env("HOME", home.path())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", empty_path.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{mode:?}: {stderr}");
        assert!(stderr.contains("not valid UTF-8"), "{mode:?}: {stderr}");
    }
    assert!(!home.path().join(".config/opencode").exists());
    assert!(!project.join(".mcp.json").exists());
}

#[test]
fn global_settings_json_preserves_foreign_hooks_and_other_keys() {
    let env = Env::new();
    fs::create_dir_all(env.claude_dir()).unwrap();
    let original = serde_json::json!({
        "model": "existing",
        "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo foreign", "timeout": 5}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo pre"}]}],
        }
    });
    fs::write(env.claude_dir().join("settings.json"), original.to_string()).unwrap();

    let output = env
        .command()
        .arg("--client")
        .arg("claude-code")
        .output()
        .unwrap();
    assert_success(&output);

    let settings = read_json(env.claude_dir().join("settings.json"));
    assert_eq!(settings["model"], "existing");
    assert_eq!(
        settings["hooks"]["PreToolUse"],
        original["hooks"]["PreToolUse"]
    );
    let session_groups = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(session_groups.len(), 2);
    assert_eq!(session_groups[0]["hooks"][0]["command"], "echo foreign");
}

#[test]
fn global_rerun_is_idempotent_without_extra_backups() {
    let env = Env::new();
    let log = env.home.path().join("claude.log");
    let bin_dir = write_fake_claude(&env.home.path().join("bin"), &log, false);

    for _ in 0..2 {
        let output = env
            .command()
            .arg("--client")
            .arg("both")
            .env("PATH", &bin_dir)
            .output()
            .unwrap();
        assert_success(&output);
    }

    for path in [
        env.claude_dir().join("settings.json"),
        env.claude_dir().join("skills/evolution/SKILL.md"),
        env.opencode_dir().join("opencode.json"),
        env.opencode_dir().join("AGENTS.md"),
        env.opencode_dir().join("skills/evolution/SKILL.md"),
        env.opencode_dir().join("plugins/skillvolution.js"),
    ] {
        let backup = PathBuf::from(format!("{}.skillvolution.bak", path.display()));
        assert!(!backup.exists(), "unexpected backup for {}", path.display());
    }
}

#[test]
fn opencode_jsonc_without_comments_is_updated_in_place_globally() {
    let env = Env::new();
    fs::create_dir_all(env.opencode_dir()).unwrap();
    let original = serde_json::json!({"model": "existing"}).to_string();
    fs::write(env.opencode_dir().join("opencode.jsonc"), &original).unwrap();

    let output = env
        .command()
        .arg("--client")
        .arg("opencode")
        .output()
        .unwrap();
    assert_success(&output);

    assert!(!env.opencode_dir().join("opencode.json").exists());
    let config = read_json(env.opencode_dir().join("opencode.jsonc"));
    assert_eq!(config["model"], "existing");
    assert_eq!(config["mcp"]["skillvolution"]["type"], "local");
}

#[test]
fn opencode_jsonc_with_comments_is_refused_before_any_write_globally() {
    let env = Env::new();
    fs::create_dir_all(env.opencode_dir()).unwrap();
    let original = "{\n  // a comment\n  \"model\": \"existing\"\n}\n";
    fs::write(env.opencode_dir().join("opencode.jsonc"), original).unwrap();

    let output = env
        .command()
        .arg("--client")
        .arg("opencode")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("manually") || stderr.contains("comments"),
        "{stderr}"
    );

    assert_eq!(
        fs::read_to_string(env.opencode_dir().join("opencode.jsonc")).unwrap(),
        original
    );
    assert!(!env.opencode_dir().join("AGENTS.md").exists());
    assert!(
        !env.opencode_dir()
            .join("skills/evolution/SKILL.md")
            .exists()
    );
}

#[test]
fn opencode_json_and_jsonc_both_present_is_refused_globally() {
    let env = Env::new();
    fs::create_dir_all(env.opencode_dir()).unwrap();
    fs::write(env.opencode_dir().join("opencode.json"), "{}").unwrap();
    fs::write(env.opencode_dir().join("opencode.jsonc"), "{}").unwrap();

    let output = env
        .command()
        .arg("--client")
        .arg("opencode")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(env.opencode_dir().join("opencode.json")).unwrap(),
        "{}"
    );
    assert_eq!(
        fs::read_to_string(env.opencode_dir().join("opencode.jsonc")).unwrap(),
        "{}"
    );
}

#[test]
fn unmanaged_plugin_is_refused_globally() {
    let env = Env::new();
    let plugin_path = env.opencode_dir().join("plugins/skillvolution.js");
    fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
    let original = "// my own plugin, not skillvolution's\n";
    fs::write(&plugin_path, original).unwrap();

    let output = env
        .command()
        .arg("--client")
        .arg("opencode")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unowned"), "{stderr}");

    assert_eq!(fs::read_to_string(&plugin_path).unwrap(), original);
    assert!(!env.opencode_dir().join("opencode.json").exists());
}

#[test]
fn agents_md_block_is_created_in_the_global_config_dir() {
    let env = Env::new();
    fs::create_dir_all(env.opencode_dir()).unwrap();
    fs::write(
        env.opencode_dir().join("AGENTS.md"),
        "# Team notes\nKeep me.\n",
    )
    .unwrap();

    let output = env
        .command()
        .arg("--client")
        .arg("opencode")
        .output()
        .unwrap();
    assert_success(&output);

    let text = fs::read_to_string(env.opencode_dir().join("AGENTS.md")).unwrap();
    assert!(text.starts_with("# Team notes\nKeep me.\n"));
    assert!(text.contains("<!-- skillvolution:start -->"));
}
