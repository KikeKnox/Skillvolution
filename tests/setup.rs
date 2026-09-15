#[path = "../src/setup.rs"]
mod setup;

use clap::Parser;
use serde_json::{json, Value};
use std::{fs, path::Path};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    setup: setup::SetupArgs,
}

fn install(project: &Path, client: &str) -> anyhow::Result<()> {
    let bin = project.join("binary with spaces");
    if !bin.exists() {
        fs::write(&bin, "test binary")?;
    }
    setup::run(
        Cli::parse_from([
            "setup".into(),
            "--project".into(),
            project.as_os_str().to_owned(),
            "--client".into(),
            client.into(),
            "--bin".into(),
            bin.into_os_string(),
            "--db".into(),
            project
                .join("data with spaces/vault.sqlite3")
                .into_os_string(),
        ])
        .setup,
    )
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
    assert_eq!(
        config["instructions"],
        json!(["team.md", ".opencode/skills/evolution/SKILL.md"])
    );
    assert_eq!(
        fs::read_to_string(p.join("opencode.json.skillvolution.bak")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_to_string(p.join("CLAUDE.md.skillvolution.bak")).unwrap(),
        "# My instructions\nKeep me.\n"
    );
    let before = fs::read_to_string(p.join("CLAUDE.md")).unwrap();
    assert!(before.starts_with("# My instructions\nKeep me.\n"));
    let oc_before = fs::read(p.join("opencode.json")).unwrap();
    install(p, "both").unwrap();
    assert_eq!(fs::read(p.join("opencode.json")).unwrap(), oc_before);
    assert_eq!(fs::read_to_string(p.join("CLAUDE.md")).unwrap(), before);
    assert!(!p.join("opencode.json.skillvolution.bak.1").exists());
    assert!(!p.join("CLAUDE.md.skillvolution.bak.1").exists());
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
    assert_eq!(fs::read_to_string(temp.path().join("opencode.json")).unwrap(), text);
    assert!(!temp.path().join(".mcp.json").exists());
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
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
    assert_eq!(oc["mcp"]["skillvolution"]["type"], "local");
    assert_eq!(oc["mcp"]["skillvolution"]["enabled"], true);
    assert_eq!(
        cc["mcpServers"]["skillvolution"],
        json!({"type":"stdio", "command":command[0], "args":[command[1],command[2],command[3]]})
    );
    for client in [".claude", ".opencode"] {
        let skill =
            fs::read_to_string(temp.path().join(client).join("skills/evolution/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: evolution\n"));
        for required in [
            "search_skills",
            "get_skill",
            "propose_skill_change",
            "expected_version",
            "never publish",
            "evidence",
            "No new lesson",
            "untrusted",
        ] {
            assert!(skill.contains(required), "missing {required}");
        }
    }
    assert_eq!(
        oc["instructions"],
        json!([".opencode/skills/evolution/SKILL.md"])
    );
    let instructions = fs::read_to_string(temp.path().join("CLAUDE.md")).unwrap();
    assert!(instructions.contains("@.claude/skills/evolution/SKILL.md"));
    assert!(!temp.path().join("AGENTS.md").exists());
}
