//! OpenCode plugin: end-to-end coverage of the `--project` propagation bug fix (a
//! project-mode setup must pass the same project key to the plugin's catalog lookup that
//! the MCP server itself was started with) plus a syntax sanity check on the installed
//! file. Placeholder-substitution and node-syntax unit tests live in `src/setup/plugin.rs`,
//! next to the code they test.

use clap::Parser;
use skillvolution::setup::{self, SetupArgs};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    setup: SetupArgs,
}

/// Runs `node --check` on `path`, skipping (not failing) when `node` isn't on PATH.
fn assert_valid_js(path: &Path) {
    let output = match Command::new("node").arg("--check").arg(path).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping node --check: node not found on PATH");
            return;
        }
        Err(error) => panic!("run node --check: {error}"),
    };
    assert!(
        output.status.success(),
        "node --check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn project_mode_plugin_gets_the_same_project_key_as_the_mcp_server() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let bin = project.join("skillvolution");
    fs::write(&bin, "test binary").unwrap();
    let db = project.join("vault.sqlite3");

    let argv = vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--project-key".to_owned(),
        "custom-key".to_owned(),
        "--client".to_owned(),
        "opencode".to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ];
    setup::run(Cli::parse_from(argv).setup).unwrap();

    let plugin_path = project.join(".opencode/plugins/skillvolution.js");
    let plugin = fs::read_to_string(&plugin_path).unwrap();
    // The MCP server (opencode.json's `serve --project custom-key`) and the plugin's
    // catalog lookup must agree on the project, or the injected catalog can silently
    // come from a different project than the one the tools operate on.
    assert!(
        plugin.contains("const PROJECT_KEY = \"custom-key\";"),
        "{plugin}"
    );
    assert_valid_js(&plugin_path);
}

#[test]
fn global_mode_plugin_leaves_project_key_null_for_runtime_detection() {
    let home = tempfile::tempdir().unwrap();
    let xdg_config = home.path().join("xdg-config");
    let empty_path = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let bin = workspace.path().join("skillvolution");
    fs::write(&bin, "test binary").unwrap();
    let db = workspace.path().join("vault.sqlite3");

    let output: Output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("setup")
        .arg("--bin")
        .arg(&bin)
        .arg("--client")
        .arg("opencode")
        .env("HOME", home.path())
        // Isolate %APPDATA% (Devin's config dir on Windows) from the host.
        .env("APPDATA", home.path().join("AppData"))
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("PATH", empty_path.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let plugin_path = xdg_config.join("opencode/plugins/skillvolution.js");
    let plugin = fs::read_to_string(&plugin_path).unwrap();
    assert!(plugin.contains("const PROJECT_KEY = null;"), "{plugin}");
    assert_valid_js(&plugin_path);
}
