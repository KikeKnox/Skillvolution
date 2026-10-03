//! OpenCode plugin: end-to-end coverage of the `--project` propagation bug fix (a
//! project-mode setup must pass the same project key to the plugin's catalog lookup that
//! the MCP server itself was started with), a syntax sanity check on the installed
//! file, and a run of the plugin's prompt-suggestion and review hooks under `node`. Placeholder-substitution and node-syntax unit tests live in `src/setup/plugin.rs`,
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

/// The driver for `plugin_injects_prompt_suggestions_and_treats_get_skill_as_work`: loads
/// the plugin with a fake OpenCode client ("child" is a subagent of "top") and prints
/// each session's system prompt as JSON.
#[cfg(unix)]
const PLUGIN_DRIVER: &str = r#"
const { SkillvolutionPlugin } = await import(process.argv[2]);
const client = {
  app: { log: async () => {} },
  session: {
    get: async ({ path: { id } }) => ({ data: { parentID: id === "child" ? "top" : undefined } }),
  },
};
const hooks = await SkillvolutionPlugin({ client, directory: process.cwd() });
const system = async (sessionID) => {
  const output = { system: [] };
  await hooks["experimental.chat.system.transform"]({ sessionID }, output);
  return output.system;
};
const message = (sessionID, text) =>
  hooks["chat.message"]({ sessionID }, { message: {}, parts: [{ type: "text", text }] });

await message("top", "fix the flaky cargo tests");
const top = await system("top");
await message("child", "delegated task text");
const child = await system("child");
await hooks["tool.execute.after"]({ tool: "skillvolution_get_skill", sessionID: "top" });
hooks.event({ event: { type: "session.idle", properties: { sessionID: "top" } } });
await new Promise((resolve) => setTimeout(resolve, 100));
const after = await system("top");
console.log(JSON.stringify({ top, child, after }));
"#;

/// Drives the installed plugin in `node` with a fake `skillvolution` binary: a top-level
/// user message gets the skills `hook prompt` printed for it in that turn's system
/// prompt, a subagent's message gets none, and a `get_skill` call counts as work that
/// earns the review reminder once the turn goes idle. Skipped (not failed) when `node`
/// isn't on PATH; unix-only for the `#!/bin/sh` fake binary.
#[cfg(unix)]
#[test]
fn plugin_injects_prompt_suggestions_and_treats_get_skill_as_work() {
    use std::os::unix::fs::PermissionsExt;

    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping plugin run: node not found on PATH");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let stdin_log = project.join("prompt-stdin.json");
    let bin = project.join("skillvolution");
    let catalog = r#"{"hookSpecificOutput":{"additionalContext":"CATALOG"}}"#;
    let suggested = r#"{"hookSpecificOutput":{"additionalContext":"SUGGESTED"}}"#;
    fs::write(
        &bin,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n\
             *\"hook prompt\"*) cat > '{}'; printf '%s' '{suggested}' ;;\n\
             *) printf '%s' '{catalog}' ;;\nesac\n",
            stdin_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let argv = vec![
        "setup".to_owned(),
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--client".to_owned(),
        "opencode".to_owned(),
        "--bin".to_owned(),
        bin.to_string_lossy().into_owned(),
        "--db".to_owned(),
        project.join("vault.sqlite3").to_string_lossy().into_owned(),
    ];
    setup::run(Cli::parse_from(argv).setup).unwrap();
    // `.mjs` so node loads the plugin as an ES module wherever it sits.
    let plugin = project.join("plugin.mjs");
    fs::copy(project.join(".opencode/plugins/skillvolution.js"), &plugin).unwrap();
    let driver = project.join("driver.mjs");
    fs::write(&driver, PLUGIN_DRIVER).unwrap();

    let output = Command::new("node")
        .arg(&driver)
        .arg(&plugin)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["top"], serde_json::json!(["CATALOG", "SUGGESTED"]));
    assert_eq!(result["child"], serde_json::json!(["CATALOG"]));
    let reminded = result["after"].as_array().unwrap().iter().any(|text| {
        text.as_str()
            .unwrap()
            .starts_with("Skillvolution review: you edited files or loaded vault skills")
    });
    assert!(reminded, "{result}");
    let stdin: serde_json::Value = serde_json::from_slice(&fs::read(&stdin_log).unwrap()).unwrap();
    assert_eq!(stdin["prompt"], "fix the flaky cargo tests");
}
