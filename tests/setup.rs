use skillvolution::setup;

use clap::Parser;
use serde_json::{Value, json};
use std::{fs, path::Path};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    setup: setup::SetupArgs,
}

fn install(project: &Path, client: &str) -> anyhow::Result<()> {
    install_full(project, client, "binary with spaces", None)
}

fn install_full(
    project: &Path,
    client: &str,
    bin_name: &str,
    project_key: Option<&str>,
) -> anyhow::Result<()> {
    let bin = project.join(bin_name);
    if !bin.exists() {
        if let Some(parent) = bin.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&bin, "test binary")?;
    }
    let mut argv = vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        client.to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        project
            .join("data with spaces/vault.sqlite3")
            .to_string_lossy()
            .into_owned(),
    ];
    if let Some(key) = project_key {
        argv.push("--project-key".to_owned());
        argv.push(key.to_owned());
    }
    setup::run(Cli::parse_from(argv).setup)
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn preserves_settings_and_instructions_with_backups_and_idempotence() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    let original = "{\"model\":\"existing\",\"mcp\":{\"other\":{\"enabled\":false}},\"instructions\":[\"team.md\"]}";
    fs::write(p.join("opencode.json"), original).unwrap();
    fs::write(p.join("CLAUDE.md"), "# My instructions\nKeep me.\n").unwrap();
    install(p, "both").unwrap();
    let config = read_json(p.join("opencode.json"));
    assert_eq!(config["model"], "existing");
    assert_eq!(config["mcp"]["other"]["enabled"], false);
    // No legacy instructions entry was present, so nothing is added.
    assert_eq!(config["instructions"], json!(["team.md"]));
    assert_eq!(
        fs::read_to_string(p.join("opencode.json.skillvolution.bak")).unwrap(),
        original
    );
    // CLAUDE.md carries no legacy markers, so setup leaves it untouched: no rewrite, no backup.
    assert_eq!(
        fs::read_to_string(p.join("CLAUDE.md")).unwrap(),
        "# My instructions\nKeep me.\n"
    );
    assert!(!p.join("CLAUDE.md.skillvolution.bak").exists());

    let oc_before = fs::read(p.join("opencode.json")).unwrap();
    let agents_before = fs::read(p.join("AGENTS.md")).unwrap();
    install(p, "both").unwrap();
    assert_eq!(fs::read(p.join("opencode.json")).unwrap(), oc_before);
    assert_eq!(fs::read(p.join("AGENTS.md")).unwrap(), agents_before);
    assert!(!p.join("opencode.json.skillvolution.bak.1").exists());
    assert!(!p.join("AGENTS.md.skillvolution.bak.1").exists());
}

#[test]
fn rejects_conflicts_before_writing_any_client() {
    let cases = [
        (".mcp.json", "not json"),
        (".mcp.json", "[]"),
        (".mcp.json", "{\"mcpServers\":null}"),
        (".mcp.json", "{\"mcpServers\":{\"skillvolution\":false}}"),
        ("opencode.json", "{\"mcp\":[]}"),
        ("opencode.json", "{\"instructions\":[42]}"),
        ("opencode.jsonc", "{}"),
        (
            ".opencode/skills/evolution/SKILL.md",
            "# My own evolution skill",
        ),
        (
            "CLAUDE.md",
            "User text\n<!-- skillvolution:evolution:start -->\nmissing end",
        ),
        (
            "CLAUDE.md",
            "<!-- skillvolution:evolution:end -->\n<!-- skillvolution:evolution:start -->",
        ),
        (
            "AGENTS.md",
            "Team notes\n<!-- skillvolution:start -->\nmissing end",
        ),
        (".claude/settings.local.json", "{\"hooks\": \"nope\"}"),
        (
            ".claude/settings.local.json",
            "{\"hooks\": {\"SessionStart\": \"nope\"}}",
        ),
    ];
    for (file, text) in cases {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, text).unwrap();
        let result = install(temp.path(), "both");
        assert!(result.is_err(), "must reject {file}: {text}");
        assert_eq!(fs::read_to_string(&target).unwrap(), text);
        if file != "opencode.json" {
            assert!(
                !temp.path().join("opencode.json").exists(),
                "partial write for {file}"
            );
        }
        if file != ".mcp.json" {
            assert!(
                !temp.path().join(".mcp.json").exists(),
                "partial write for {file}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn refuses_symlink_targets_and_ancestors_before_writes() {
    for target in [
        "opencode.json",
        ".mcp.json",
        ".claude",
        "opencode.json.skillvolution.bak",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = temp.path().join(target);
        let directory = target == ".claude";
        let original = outside.path().join("original");
        if directory {
            fs::create_dir(&original).unwrap();
        } else {
            fs::write(&original, "{}").unwrap();
        }
        if target.ends_with(".bak") {
            fs::write(temp.path().join("opencode.json"), "{}").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&original, &link).unwrap();
        #[cfg(windows)]
        if directory {
            std::os::windows::fs::symlink_dir(&original, &link).unwrap();
        } else {
            std::os::windows::fs::symlink_file(&original, &link).unwrap();
        }
        let result = install(temp.path(), "both");
        assert!(result.is_err(), "must reject {target}");
        assert!(format!("{:#}", result.unwrap_err()).contains("symlink"));
        if !directory {
            assert_eq!(fs::read_to_string(&original).unwrap(), "{}");
        }
        assert!(!temp.path().join("CLAUDE.md").exists());
    }
}

#[cfg(windows)]
#[test]
fn refuses_windows_junction_ancestor_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(temp.path().join(".claude"))
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let result = install(temp.path(), "both");
    assert!(result.is_err(), "must reject directory junction");
    assert!(!temp.path().join("opencode.json").exists());
    assert!(!outside.path().join("skills").exists());
}

#[test]
fn rejects_directory_binary_and_database() {
    for bad in ["binary with spaces", "data with spaces/vault.sqlite3"] {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(bad)).unwrap();
        assert!(
            install(temp.path(), "both").is_err(),
            "must reject directory {bad}"
        );
        assert!(!temp.path().join("opencode.json").exists());
    }
}

#[test]
fn rejects_duplicate_json_keys_without_losing_settings() {
    let temp = tempfile::tempdir().unwrap();
    let text = "{\"model\":\"first\",\"model\":\"second\"}";
    fs::write(temp.path().join("opencode.json"), text).unwrap();
    assert!(install(temp.path(), "both").is_err());
    assert_eq!(
        fs::read_to_string(temp.path().join("opencode.json")).unwrap(),
        text
    );
    assert!(!temp.path().join(".mcp.json").exists());
}

#[test]
fn installs_both_documented_clients_with_absolute_argv() {
    let temp = tempfile::tempdir().unwrap();
    install(temp.path(), "both").unwrap();
    let oc = read_json(temp.path().join("opencode.json"));
    let cc = read_json(temp.path().join(".mcp.json"));
    let command = oc["mcp"]["skillvolution"]["command"].as_array().unwrap();
    assert!(Path::new(command[0].as_str().unwrap()).is_absolute());
    assert!(Path::new(command[2].as_str().unwrap()).is_absolute());
    assert_eq!(command[1], "--db");
    assert_eq!(command[3], "serve");
    assert_eq!(command[4], "--project");
    assert!(command[5].is_string());
    assert_eq!(oc["mcp"]["skillvolution"]["type"], "local");
    assert_eq!(oc["mcp"]["skillvolution"]["enabled"], true);
    assert_eq!(
        cc["mcpServers"]["skillvolution"],
        json!({
            "type": "stdio",
            "command": command[0],
            "args": [command[1], command[2], command[3], command[4], command[5]],
        })
    );
    for client in [".claude", ".opencode"] {
        let skill =
            fs::read_to_string(temp.path().join(client).join("skills/evolution/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: evolution\n"));
        for required in [
            "skillvolution-managed:evolution:",
            "search_skills",
            "get_skill",
            "report_skill_outcome",
            "propose_skill_change",
        ] {
            assert!(skill.contains(required), "missing {required}");
        }
    }
    assert!(oc.get("instructions").is_none());
    assert!(!temp.path().join("CLAUDE.md").exists());
    let agents = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("<!-- skillvolution:start -->"));
    assert!(agents.contains("<!-- skillvolution:end -->"));
    let settings = fs::read_to_string(temp.path().join(".claude/settings.local.json")).unwrap();
    assert!(settings.contains("hook stop"));
    assert!(settings.contains("hook session-start"));
}

#[test]
fn mcp_entries_include_project_key_and_derive_default_from_directory_name() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project with spaces");
    fs::create_dir(&project).unwrap();
    install(&project, "both").unwrap();
    let oc = read_json(project.join("opencode.json"));
    let cc = read_json(project.join(".mcp.json"));
    let oc_command = oc["mcp"]["skillvolution"]["command"].as_array().unwrap();
    assert_eq!(oc_command[4], "--project");
    assert_eq!(oc_command[5], "project-with-spaces");
    let cc_args = cc["mcpServers"]["skillvolution"]["args"]
        .as_array()
        .unwrap();
    assert_eq!(cc_args[3], "--project");
    assert_eq!(cc_args[4], "project-with-spaces");
}

#[test]
fn project_key_override_and_invalid_key_rejected() {
    let temp = tempfile::tempdir().unwrap();
    install_full(
        temp.path(),
        "both",
        "binary with spaces",
        Some("custom-key"),
    )
    .unwrap();
    let cc = read_json(temp.path().join(".mcp.json"));
    let args = cc["mcpServers"]["skillvolution"]["args"]
        .as_array()
        .unwrap();
    assert_eq!(args[4], "custom-key");

    let temp2 = tempfile::tempdir().unwrap();
    let result = install_full(
        temp2.path(),
        "both",
        "binary with spaces",
        Some("Not Valid!"),
    );
    assert!(result.is_err());
    assert!(!temp2.path().join(".mcp.json").exists());
    assert!(!temp2.path().join("opencode.json").exists());
}

#[test]
fn settings_local_json_gets_hooks_with_correct_commands() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    install(p, "claude-code").unwrap();
    let settings = read_json(p.join(".claude/settings.local.json"));
    let session = &settings["hooks"]["SessionStart"][0]["hooks"][0];
    let stop = &settings["hooks"]["Stop"][0]["hooks"][0];
    assert_eq!(session["type"], "command");
    assert_eq!(session["timeout"], 10);
    let session_cmd = session["command"].as_str().unwrap();
    assert!(session_cmd.contains(" hook session-start --project "));
    assert!(session_cmd.contains(" --db "));
    let stop_cmd = stop["command"].as_str().unwrap();
    assert!(stop_cmd.ends_with(" hook stop"));
}

#[test]
fn settings_local_json_preserves_foreign_hooks_and_other_keys() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    fs::create_dir_all(p.join(".claude")).unwrap();
    let original = json!({
        "model": "existing",
        "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo foreign", "timeout": 5}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo pre"}]}],
        }
    });
    fs::write(p.join(".claude/settings.local.json"), original.to_string()).unwrap();
    install(p, "claude-code").unwrap();
    let settings = read_json(p.join(".claude/settings.local.json"));
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
fn settings_local_json_idempotent_rerun_without_extra_backup() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    install(p, "claude-code").unwrap();
    let before = fs::read(p.join(".claude/settings.local.json")).unwrap();
    install(p, "claude-code").unwrap();
    assert_eq!(
        fs::read(p.join(".claude/settings.local.json")).unwrap(),
        before
    );
    assert!(
        !p.join(".claude/settings.local.json.skillvolution.bak")
            .exists()
    );
}

#[test]
fn changed_bin_path_replaces_owned_hooks_without_duplicating() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    install_full(p, "claude-code", "bin-one", None).unwrap();
    install_full(p, "claude-code", "bin-two", None).unwrap();
    let settings = read_json(p.join(".claude/settings.local.json"));
    let session_groups = settings["hooks"]["SessionStart"].as_array().unwrap();
    let stop_groups = settings["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(session_groups.len(), 1, "{session_groups:?}");
    assert_eq!(stop_groups.len(), 1, "{stop_groups:?}");
    let stop_cmd = stop_groups[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(stop_cmd.contains("bin-two"));
    assert!(!stop_cmd.contains("bin-one"));
}

#[test]
fn agents_md_block_created_rewritten_and_malformed_refused() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    fs::write(p.join("AGENTS.md"), "# Team notes\nKeep me.\n").unwrap();
    install(p, "opencode").unwrap();
    let text = fs::read_to_string(p.join("AGENTS.md")).unwrap();
    assert!(text.starts_with("# Team notes\nKeep me.\n"));
    assert!(text.contains("<!-- skillvolution:start -->"));
    let before = text.clone();
    install(p, "opencode").unwrap();
    assert_eq!(fs::read_to_string(p.join("AGENTS.md")).unwrap(), before);

    let temp2 = tempfile::tempdir().unwrap();
    fs::write(
        temp2.path().join("AGENTS.md"),
        "<!-- skillvolution:start -->\nmissing end",
    )
    .unwrap();
    assert!(install(temp2.path(), "opencode").is_err());
    assert!(!temp2.path().join("opencode.json").exists());
}

#[test]
fn removes_legacy_claude_md_block_and_does_not_create_when_absent() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    install(p, "claude-code").unwrap();
    assert!(!p.join("CLAUDE.md").exists());

    fs::write(
        p.join("CLAUDE.md"),
        "# Notes\n<!-- skillvolution:evolution:start -->\n@.claude/skills/evolution/SKILL.md\n<!-- skillvolution:evolution:end -->\nTail\n",
    )
    .unwrap();
    install(p, "claude-code").unwrap();
    let text = fs::read_to_string(p.join("CLAUDE.md")).unwrap();
    assert!(!text.contains("skillvolution:evolution"));
    assert!(text.contains("# Notes"));
    assert!(text.contains("Tail"));
}

#[test]
fn removes_legacy_opencode_instructions_entry_without_creating_key() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    fs::write(
        p.join("opencode.json"),
        json!({"instructions": [".opencode/skills/evolution/SKILL.md", "team.md"]}).to_string(),
    )
    .unwrap();
    install(p, "opencode").unwrap();
    let config = read_json(p.join("opencode.json"));
    assert_eq!(config["instructions"], json!(["team.md"]));

    let temp2 = tempfile::tempdir().unwrap();
    install(temp2.path(), "opencode").unwrap();
    let config2 = read_json(temp2.path().join("opencode.json"));
    assert!(config2.get("instructions").is_none());
}
