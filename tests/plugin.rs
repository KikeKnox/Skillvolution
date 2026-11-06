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
    get: async ({ path: { id } }) => ({
      data: { parentID: { child: "top", c2: "m2", c3: "e2", c4: "l2", c5: "r5", c6: "p6" }[id] },
    }),
  },
};
// The plugin looks for project agents in the worktree (the repository root), not in
// the directory the session happens to run in.
const hooks = await SkillvolutionPlugin({
  client,
  directory: process.cwd() + "/sub",
  worktree: process.cwd(),
});
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
const idle = async (sessionID) => {
  hooks.event({ event: { type: "session.idle", properties: { sessionID } } });
  await new Promise((resolve) => setTimeout(resolve, 100));
  return system(sessionID);
};
await hooks["tool.execute.after"]({ tool: "task", sessionID: "u", args: { subagent_type: "unmanaged" } });
const unmanaged = await idle("u");
await hooks["tool.execute.after"]({ tool: "task", sessionID: "m", args: { subagent_type: "reviewer" } });
const managed = await idle("m");
await hooks["tool.execute.after"]({ tool: "task", sessionID: "g", args: { subagent_type: "global-reviewer" } });
const global = await idle("g");
// A subagent's own report must not mark the parent reviewed: the delegation is still owed.
await hooks["tool.execute.after"]({ tool: "task", sessionID: "m2", args: { subagent_type: "reviewer" } });
await hooks["tool.execute.after"]({ tool: "task", sessionID: "m2", args: { subagent_type: "global-reviewer" } });
await hooks["tool.execute.after"]({ tool: "task", sessionID: "m2", args: { subagent_type: "reviewer" } });
await hooks["tool.execute.after"]({ tool: "skillvolution_report_skill_outcome", sessionID: "c2" });
const childReported = await idle("m2");
// Reporting from the parent clears the reminder and the delegated list.
await hooks["tool.execute.after"]({ tool: "skillvolution_report_skill_outcome", sessionID: "m2" });
const parentReported = await system("m2");
// A subagent's edits count toward the parent; its skill loads do not.
await hooks["tool.execute.after"]({ tool: "edit", sessionID: "c3" });
const childEdit = await idle("e2");
await hooks["tool.execute.after"]({ tool: "skillvolution_get_skill", sessionID: "c4" });
const childLoad = await idle("l2");
// Work after a review within the same turn is still owed.
await hooks["tool.execute.after"]({ tool: "skillvolution_report_skill_outcome", sessionID: "o1" });
await hooks["tool.execute.after"]({ tool: "edit", sessionID: "o1" });
const workAfterReview = await idle("o1");
// Work before a review in the same turn is settled.
await hooks["tool.execute.after"]({ tool: "edit", sessionID: "o2" });
await hooks["tool.execute.after"]({ tool: "skillvolution_report_skill_outcome", sessionID: "o2" });
const workBeforeReview = await idle("o2");
// A child's session.error must not clear the parent's pending work.
await hooks["tool.execute.after"]({ tool: "edit", sessionID: "p6" });
hooks.event({ event: { type: "session.error", properties: { sessionID: "c6" } } });
await new Promise((resolve) => setTimeout(resolve, 100));
const childError = await idle("p6");
// A managed agent marker must follow the frontmatter and match the file stem.
await hooks["tool.execute.after"]({ tool: "task", sessionID: "bad1", args: { subagent_type: "misplaced" } });
const misplaced = await idle("bad1");
await hooks["tool.execute.after"]({ tool: "task", sessionID: "bad2", args: { subagent_type: "wrongid" } });
const wrongId = await idle("bad2");
// A hook that gets garbage must not throw.
let threw = false;
try {
  await hooks["tool.execute.after"]({});
  await hooks["tool.execute.after"]({ tool: "edit" });
  await hooks["chat.message"]({ sessionID: "z" }, undefined);
  hooks.event({ event: undefined });
} catch { threw = true; }
// Non-git project: worktree "/" falls back to directory.
const rooted = await SkillvolutionPlugin({ client, directory: process.cwd() + "/nogit", worktree: "/" });
await rooted["tool.execute.after"]({ tool: "task", sessionID: "n1", args: { subagent_type: "nogit-agent" } });
rooted.event({ event: { type: "session.idle", properties: { sessionID: "n1" } } });
await new Promise((resolve) => setTimeout(resolve, 100));
const rootedOut = { system: [] };
await rooted["experimental.chat.system.transform"]({ sessionID: "n1" }, rootedOut);
const nonGit = rootedOut.system;
console.log(JSON.stringify({ workAfterReview, workBeforeReview, childError, misplaced, wrongId, threw, nonGit, top, child, after, unmanaged, managed, global, childReported, parentReported, childEdit, childLoad }));
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
    fs::create_dir_all(project.join("sub")).unwrap();
    let agents = project.join(".opencode/agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(
        agents.join("reviewer.md"),
        "---\ndescription: x\n---\n<!-- skillvolution-managed:agent:reviewer:v1:abcdef12 -->\nbody\n",
    )
    .unwrap();
    fs::write(
        agents.join("unmanaged.md"),
        "---\ndescription: x\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        agents.join("misplaced.md"),
        "---\ndescription: x\n---\nbody\n<!-- skillvolution-managed:agent:misplaced:v1:abcdef12 -->\n",
    )
    .unwrap();
    fs::write(
        agents.join("wrongid.md"),
        "---\ndescription: x\n---\n<!-- skillvolution-managed:agent:other:v1:abcdef12 -->\nbody\n",
    )
    .unwrap();
    let nogit_agents = project.join("nogit/.opencode/agents");
    fs::create_dir_all(&nogit_agents).unwrap();
    fs::write(
        nogit_agents.join("nogit-agent.md"),
        "---\ndescription: x\n---\n<!-- skillvolution-managed:agent:nogit-agent:v2:abcdef12 -->\nbody\n",
    )
    .unwrap();
    let global_agents = project.join("xdg/opencode/agents");
    fs::create_dir_all(&global_agents).unwrap();
    fs::write(
        global_agents.join("global-reviewer.md"),
        "---\ndescription: x\n---\n<!-- skillvolution-managed:agent:global-reviewer:v1:abcdef12 -->\nbody\n",
    )
    .unwrap();
    let driver = project.join("driver.mjs");
    fs::write(&driver, PLUGIN_DRIVER).unwrap();

    let output = Command::new("node")
        .arg(&driver)
        .arg(&plugin)
        .current_dir(project)
        .env("XDG_CONFIG_HOME", project.join("xdg"))
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
        text.as_str().unwrap().starts_with(
            "Skillvolution review: you edited files, loaded vault skills, or delegated",
        )
    });
    assert!(reminded, "{result}");
    let has_reminder = |key: &str| {
        result[key]
            .as_array()
            .unwrap()
            .iter()
            .any(|text| text.as_str().unwrap().starts_with("Skillvolution review:"))
    };
    assert!(!has_reminder("unmanaged"), "{result}");
    assert!(
        has_reminder("managed") && has_reminder("global"),
        "{result}"
    );
    let reminder_text = |key: &str| {
        result[key]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|text| text.as_str())
            .find(|text| text.starts_with("Skillvolution review:"))
            .map(str::to_owned)
    };
    assert_eq!(
        reminder_text("childReported"),
        Some(format!(
            "{}\nDelegated vault agents to report: reviewer v1, global-reviewer v1.",
            skillvolution::hook::STOP_REASON
        ))
    );
    assert_eq!(
        reminder_text("managed").as_deref(),
        Some(
            format!(
                "{}\nDelegated vault agents to report: reviewer v1.",
                skillvolution::hook::STOP_REASON
            )
            .as_str()
        )
    );
    assert!(reminder_text("parentReported").is_none(), "{result}");
    assert_eq!(
        reminder_text("childEdit").as_deref(),
        Some(skillvolution::hook::STOP_REASON),
        "{result}"
    );
    assert!(reminder_text("childLoad").is_none(), "{result}");
    assert!(has_reminder("workAfterReview"), "{result}");
    assert!(!has_reminder("workBeforeReview"), "{result}");
    assert!(has_reminder("childError"), "{result}");
    assert!(!has_reminder("misplaced"), "{result}");
    assert!(!has_reminder("wrongId"), "{result}");
    assert_eq!(result["threw"], false);
    assert!(has_reminder("nonGit"), "{result}");
    assert_eq!(
        reminder_text("nonGit").as_deref(),
        Some(
            format!(
                "{}\nDelegated vault agents to report: nogit-agent v2.",
                skillvolution::hook::STOP_REASON
            )
            .as_str()
        )
    );
    let stdin: serde_json::Value = serde_json::from_slice(&fs::read(&stdin_log).unwrap()).unwrap();
    assert_eq!(stdin["prompt"], "fix the flaky cargo tests");
}
