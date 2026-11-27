//! The MCP agent tools (`search_agents`, `install_agent`, `list_models`, `publish_agent`) and the
//! server's `--client`: what each returns, that none ever returns an agent's body, the
//! files and `.git/info/exclude` lines `install_agent` writes, and its `agent_*` events.

mod support;

use serde_json::{Value, json};
use skillvolution::{
    clients::{ClientKind, Version},
    vault::Vault,
};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
use support::{events, stderr};
use tempfile::TempDir;

/// A line of the agent body that must never reach the orchestrator.
const SECRET_BODY: &str = "ZEBRA-QUOKKA-7731 private reviewer instructions";

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    db: PathBuf,
    empty_path: PathBuf,
}

impl Sandbox {
    /// A repository `myproj` set up for `clients`, with the vault agent `reviewer`.
    fn new(clients: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let empty_path = root.path().join("path");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&empty_path).unwrap();
        let bin = root.path().join("skillvolution-bin");
        fs::write(&bin, "").unwrap();
        let repo = support::git_repo(root.path(), "myproj");
        let sandbox = Self {
            db: root.path().join("vault.db"),
            _root: root,
            home,
            repo,
            bin,
            empty_path,
        };
        sandbox.ok(&[
            "setup",
            "--project",
            sandbox.repo.to_str().unwrap(),
            "--client",
            clients,
            "--bin",
            sandbox.bin.to_str().unwrap(),
        ]);
        sandbox.publish("Reviews diffs.");
        sandbox
    }

    fn command(&self) -> Command {
        let mut command = support::isolated(&self.home);
        command
            .arg("--db")
            .arg(&self.db)
            .env("PATH", &self.empty_path)
            .current_dir(&self.repo);
        command
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Publishes (or republishes) `reviewer` with `role` in its body.
    fn publish(&self, role: &str) {
        let file = self.home.join("src/reviewer/AGENT.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            format!(
                "---\nname: reviewer\ndescription: Use when a diff needs review\ntier: mid\ntools: [read, shell]\n---\n\
                 ## Role\n{role}\n{SECRET_BODY}\n## Procedure\n1. Read the diff.\n## Output contract\nFindings.\n## Limits\nNever edits files.\n"
            ),
        )
        .unwrap();
        let mut args = vec!["publish", file.to_str().unwrap()];
        let exists = self
            .command()
            .args(["get", "reviewer"])
            .output()
            .unwrap()
            .status
            .success();
        if exists {
            args.push("--replace");
        }
        self.ok(&args);
    }

    fn vault(&self) -> Vault {
        Vault::open(&self.db).unwrap()
    }

    /// One MCP session (`serve`, with `--client` when given): each call's result text
    /// parsed as JSON, or `Err(text)` for a tool error.
    fn mcp(&self, client: Option<&str>, calls: &[(&str, Value)]) -> Vec<Result<Value, String>> {
        self.mcp_in(&self.repo, client, calls)
    }

    /// `mcp` with the server started in `cwd`.
    fn mcp_in(
        &self,
        cwd: &std::path::Path,
        client: Option<&str>,
        calls: &[(&str, Value)],
    ) -> Vec<Result<Value, String>> {
        let mut command = self.command();
        command.current_dir(cwd).arg("serve");
        if let Some(client) = client {
            command.args(["--client", client]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}})
        )
        .unwrap();
        for (index, (tool, arguments)) in calls.iter().enumerate() {
            writeln!(
                stdin,
                "{}",
                json!({"jsonrpc":"2.0","id":index + 1,"method":"tools/call",
                       "params":{"name":tool,"arguments":arguments}})
            )
            .unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            !stdout.contains("ZEBRA-QUOKKA"),
            "an agent body leaked: {stdout}"
        );
        let replies: Vec<Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|reply: &Value| reply["id"].as_i64().is_some_and(|id| id > 0))
            .collect();
        assert_eq!(replies.len(), calls.len(), "{stdout}");
        replies
            .into_iter()
            .map(|reply| {
                let result = &reply["result"];
                let text = result["content"][0]["text"]
                    .as_str()
                    .unwrap_or_else(|| panic!("no result: {reply}"))
                    .to_owned();
                if result["isError"] == true {
                    Err(text)
                } else {
                    Ok(serde_json::from_str(&text).unwrap())
                }
            })
            .collect()
    }

    fn one(&self, client: Option<&str>, tool: &str, arguments: Value) -> Result<Value, String> {
        self.mcp(client, &[(tool, arguments)]).remove(0)
    }

    fn claude(&self, id: &str) -> PathBuf {
        self.repo.join(".claude/agents").join(format!("{id}.md"))
    }

    fn opencode(&self, id: &str) -> PathBuf {
        self.repo.join(".opencode/agents").join(format!("{id}.md"))
    }
}

fn agent<'a>(result: &'a Value, id: &str) -> &'a Value {
    result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {result}"))
}

#[test]
fn tools_list_has_the_agent_tools() {
    let sandbox = Sandbox::new("claude-code");
    let mut child = sandbox
        .command()
        .args(["serve", "--client", "claude-code"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<&str> = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for tool in [
        "search_agents",
        "install_agent",
        "list_models",
        "publish_agent",
    ] {
        assert!(names.contains(&tool), "{names:?}");
    }
}

#[test]
fn search_agents_reports_tier_install_state_and_staleness_without_bodies() {
    let sandbox = Sandbox::new("claude-code");
    let found = sandbox
        .one(
            Some("claude-code"),
            "search_agents",
            json!({"query": "review"}),
        )
        .unwrap();
    let reviewer = agent(&found, "reviewer");
    assert_eq!(reviewer["version"], 1);
    assert_eq!(reviewer["tier"], "mid");
    assert_eq!(reviewer["description"], "Use when a diff needs review");
    assert_eq!(reviewer["installed"], "no");
    assert_eq!(reviewer["outdated"], false);
    assert!(
        found["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != "sv-low"),
        "{found}"
    );

    // The empty query lists the built-in tier agents too.
    let all = sandbox
        .one(Some("claude-code"), "search_agents", json!({}))
        .unwrap();
    assert_eq!(agent(&all, "sv-low")["tier"], "low");

    sandbox.ok(&["agents", "add", "reviewer"]);
    let bin = sandbox.bin.to_str().unwrap();
    sandbox.ok(&["setup", "--client", "claude-code", "--bin", bin]);
    sandbox.ok(&["agents", "add", "--global", "sv-low"]);
    sandbox.publish("Reviews diffs carefully.");
    let all = sandbox
        .one(Some("claude-code"), "search_agents", json!({}))
        .unwrap();
    let reviewer = agent(&all, "reviewer");
    assert_eq!(reviewer["version"], 2);
    assert_eq!(reviewer["installed"], "local");
    assert_eq!(reviewer["outdated"], true);
    assert_eq!(agent(&all, "sv-low")["installed"], "global");
    assert_eq!(agent(&all, "sv-low")["outdated"], false);
}

#[test]
fn install_agent_writes_for_the_server_client_only_and_records_events() {
    let sandbox = Sandbox::new("claude-code,opencode");
    let result = sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer"}),
        )
        .unwrap();
    let line = result["result"].as_str().unwrap();
    assert!(
        line.contains(".claude/agents/reviewer.md")
            && line.contains("v1")
            && line.contains("\"reviewer\"")
            && line.contains("no restart needed"),
        "{line}"
    );
    assert!(sandbox.claude("reviewer").exists());
    assert!(!sandbox.opencode("reviewer").exists());
    let exclude = fs::read_to_string(sandbox.repo.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.contains("\n/.claude/agents/reviewer.md\n"),
        "{exclude}"
    );

    let installs = events(&sandbox.db, Some("agent_install"));
    assert_eq!(installs.len(), 1, "{installs:?}");
    assert_eq!(installs[0].client.as_deref(), Some("claude-code"));
    assert_eq!(installs[0].subject.as_deref(), Some("reviewer"));
    assert_eq!(installs[0].version, Some(1));
    assert_eq!(installs[0].project.as_deref(), Some("myproj"));
    assert_eq!(installs[0].data["scope"], "project");

    // Again: unchanged, no new event. A new version is an update.
    let again = sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer"}),
        )
        .unwrap();
    assert!(
        again["result"].as_str().unwrap().contains("unchanged"),
        "{again}"
    );
    assert_eq!(events(&sandbox.db, Some("agent_install")).len(), 1);
    sandbox.publish("Reviews diffs carefully.");
    sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer"}),
        )
        .unwrap();
    let updates = events(&sandbox.db, Some("agent_update"));
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(updates[0].version, Some(2));

    // The CLI removal takes the exclude line back out.
    sandbox.ok(&["agents", "remove", "reviewer"]);
    let exclude = fs::read_to_string(sandbox.repo.join(".git/info/exclude")).unwrap();
    assert!(!exclude.contains("reviewer"), "{exclude}");
}

#[test]
fn install_agent_without_client_writes_for_every_configured_client() {
    let sandbox = Sandbox::new("claude-code,opencode");
    let result = sandbox
        .one(None, "install_agent", json!({"id": "reviewer"}))
        .unwrap();
    assert!(sandbox.claude("reviewer").exists() && sandbox.opencode("reviewer").exists());
    let line = result["result"].as_str().unwrap();
    assert!(
        line.contains("Claude Code") && line.contains("OpenCode"),
        "{line}"
    );
}

#[test]
fn install_agent_flags_a_restart_for_clients_without_hot_reload() {
    let sandbox = Sandbox::new("opencode");
    sandbox
        .vault()
        .record_client_version(ClientKind::OpenCode, Version(1, 4, 0))
        .unwrap();
    let result = sandbox
        .one(Some("opencode"), "install_agent", json!({"id": "reviewer"}))
        .unwrap();
    let line = result["result"].as_str().unwrap();
    assert!(line.contains("restart OpenCode"), "{line}");
    assert_eq!(result["restart_needed"], true);
}

#[test]
fn install_agent_accepts_only_models_of_the_agent_tier() {
    let sandbox = Sandbox::new("claude-code");
    // mid's only model is the compiled default, sonnet.
    let refused = sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer", "model": "opus"}),
        )
        .unwrap_err();
    assert!(
        refused.contains("`opus`") && refused.contains("sonnet"),
        "{refused}"
    );
    assert!(!sandbox.claude("reviewer").exists());

    sandbox.ok(&["agents", "models", "add", "claude-code", "mid", "opus"]);
    sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer", "model": "opus"}),
        )
        .unwrap();
    let text = fs::read_to_string(sandbox.claude("reviewer")).unwrap();
    assert!(text.contains("model: opus\n"), "{text}");
}

#[test]
fn install_agent_refuses_a_client_without_subagents() {
    let sandbox = Sandbox::new("claude-code");
    let refused = sandbox
        .one(Some("codex"), "install_agent", json!({"id": "reviewer"}))
        .unwrap_err();
    assert!(refused.contains("Codex"), "{refused}");
    assert!(!sandbox.claude("reviewer").exists());
}

#[test]
fn list_models_needs_a_client_without_one_on_the_server() {
    let sandbox = Sandbox::new("claude-code");
    let refused = sandbox
        .one(None, "list_models", json!({"tier": "mid"}))
        .unwrap_err();
    assert!(refused.contains("client"), "{refused}");
    let listed = sandbox
        .one(
            None,
            "list_models",
            json!({"tier": "mid", "client": "claude-code"}),
        )
        .unwrap();
    assert_eq!(listed["client"], "claude-code");
}

#[test]
fn list_models_says_how_to_pass_each_model_per_call() {
    let sandbox = Sandbox::new("claude-code,opencode");
    sandbox.ok(&["agents", "models", "add", "claude-code", "high", "opus"]);
    sandbox.ok(&[
        "agents",
        "models",
        "add",
        "claude-code",
        "high",
        "claude-opus-4-1",
    ]);
    let claude = sandbox
        .one(Some("claude-code"), "list_models", json!({"tier": "high"}))
        .unwrap();
    let models = claude["models"].as_array().unwrap();
    assert_eq!(models.len(), 2, "{claude}");
    assert_eq!(models[0]["model"], "opus");
    assert_eq!(models[0]["provider"], "anthropic");
    assert_eq!(models[0]["priority"], 0);
    assert!(
        models[0]["per_call"].as_str().unwrap().contains("\"opus\""),
        "{claude}"
    );
    assert!(
        models[1]["per_call"]
            .as_str()
            .unwrap()
            .contains("install_agent"),
        "{claude}"
    );

    sandbox.ok(&[
        "agents",
        "models",
        "add",
        "opencode",
        "mid",
        "anthropic/claude-sonnet-4-5",
        "--effort",
        "high",
    ]);
    sandbox.ok(&["agents", "models", "set", "opencode", "low", "bare-model"]);
    let vault = sandbox.vault();
    vault
        .record_client_version(ClientKind::OpenCode, Version(2, 2, 5))
        .unwrap();
    let v2 = sandbox
        .one(Some("opencode"), "list_models", json!({}))
        .unwrap();
    let mid = v2["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tier"] == "mid")
        .unwrap();
    assert!(
        mid["per_call"]
            .as_str()
            .unwrap()
            .contains("\"anthropic/claude-sonnet-4-5#high\""),
        "{v2}"
    );
    let low = v2["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tier"] == "low")
        .unwrap();
    assert_eq!(low["provider"], "unknown");
    assert!(
        low["per_call"].as_str().unwrap().contains("install_agent"),
        "{v2}"
    );

    vault
        .record_client_version(ClientKind::OpenCode, Version(1, 4, 0))
        .unwrap();
    let v1 = sandbox
        .one(Some("opencode"), "list_models", json!({"tier": "mid"}))
        .unwrap();
    let per_call = v1["models"][0]["per_call"].as_str().unwrap();
    assert!(
        per_call.contains("not supported") && per_call.contains("install_agent"),
        "{v1}"
    );
}

#[test]
fn mcp_events_carry_the_server_client_and_its_stored_version() {
    let sandbox = Sandbox::new("claude-code");
    sandbox
        .vault()
        .record_client_version(ClientKind::ClaudeCode, Version(2, 1, 0))
        .unwrap();
    sandbox
        .one(
            Some("claude-code"),
            "search_skills",
            json!({"query": "anything"}),
        )
        .unwrap();
    let search = &events(&sandbox.db, Some("search"))[0];
    assert_eq!(search.client.as_deref(), Some("claude-code"));
    assert_eq!(search.client_version.as_deref(), Some("2.1.0"));
}

#[test]
fn get_skill_still_refuses_agents() {
    let sandbox = Sandbox::new("claude-code");
    let refused = sandbox
        .one(Some("claude-code"), "get_skill", json!({"id": "reviewer"}))
        .unwrap_err();
    assert!(refused.contains("is an agent"), "{refused}");
}

#[test]
fn an_installed_model_is_pinned_and_sync_keeps_it_until_it_is_disabled() {
    let sandbox = Sandbox::new("claude-code");
    sandbox.ok(&["agents", "models", "add", "claude-code", "mid", "opus"]);
    sandbox.ok(&["agents", "models", "add", "claude-code", "mid", "sonnet"]);
    sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer", "model": "sonnet"}),
        )
        .unwrap();
    let text = fs::read_to_string(sandbox.claude("reviewer")).unwrap();
    assert!(
        text.contains("model: sonnet\n") && text.contains(":model=sonnet -->"),
        "{text}"
    );

    // sync keeps the pin although opus ranks first.
    let synced = sandbox.ok(&["agents", "sync", "--apply"]);
    assert!(synced.contains("[ok] Claude Code reviewer v1"), "{synced}");
    assert_eq!(
        fs::read_to_string(sandbox.claude("reviewer")).unwrap(),
        text
    );
    let found = sandbox
        .one(Some("claude-code"), "search_agents", json!({}))
        .unwrap();
    assert_eq!(agent(&found, "reviewer")["outdated"], false, "{found}");

    // Disabled: sync falls back to the tier's best and says why.
    sandbox.ok(&[
        "agents",
        "models",
        "disable",
        "claude-code",
        "mid",
        "sonnet",
    ]);
    let found = sandbox
        .one(Some("claude-code"), "search_agents", json!({}))
        .unwrap();
    assert_eq!(agent(&found, "reviewer")["outdated"], true, "{found}");
    let synced = sandbox.ok(&["agents", "sync", "--apply"]);
    assert!(
        synced.contains("warning: pinned model sonnet is no longer enabled for Claude Code mid"),
        "{synced}"
    );
    let text = fs::read_to_string(sandbox.claude("reviewer")).unwrap();
    assert!(
        text.contains("model: opus\n") && !text.contains(":model="),
        "{text}"
    );

    // Installing without a model clears a pin.
    sandbox.ok(&["agents", "models", "enable", "claude-code", "mid", "sonnet"]);
    sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer", "model": "sonnet"}),
        )
        .unwrap();
    sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer"}),
        )
        .unwrap();
    let text = fs::read_to_string(sandbox.claude("reviewer")).unwrap();
    assert!(
        text.contains("model: opus\n") && !text.contains(":model="),
        "{text}"
    );
}

#[test]
fn a_pinned_model_with_a_colon_round_trips() {
    let sandbox = Sandbox::new("opencode");
    sandbox.ok(&[
        "agents",
        "models",
        "add",
        "opencode",
        "mid",
        "anthropic/claude-sonnet-4-5",
    ]);
    sandbox.ok(&[
        "agents",
        "models",
        "add",
        "opencode",
        "mid",
        "ollama/llama3:8b",
    ]);
    sandbox
        .one(
            Some("opencode"),
            "install_agent",
            json!({"id": "reviewer", "model": "ollama/llama3:8b"}),
        )
        .unwrap();
    let text = fs::read_to_string(sandbox.opencode("reviewer")).unwrap();
    assert!(text.contains(":model=ollama/llama3:8b -->"), "{text}");
    let synced = sandbox.ok(&["agents", "sync", "--apply"]);
    assert!(synced.contains("[ok] OpenCode reviewer v1"), "{synced}");
    assert_eq!(
        fs::read_to_string(sandbox.opencode("reviewer")).unwrap(),
        text
    );
}

#[test]
fn search_agents_flags_a_same_version_file_that_sync_would_rewrite() {
    let sandbox = Sandbox::new("claude-code");
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.ok(&["agents", "models", "set", "claude-code", "mid", "opus"]);
    let found = sandbox
        .one(Some("claude-code"), "search_agents", json!({}))
        .unwrap();
    let reviewer = agent(&found, "reviewer");
    assert_eq!(
        (reviewer["version"].as_i64(), reviewer["outdated"].as_bool()),
        (Some(1), Some(true))
    );
}

#[test]
fn an_install_from_home_lands_per_user_and_is_recorded_as_global() {
    let sandbox = Sandbox::new("claude-code");
    let bin = sandbox.bin.to_str().unwrap();
    sandbox.ok(&["setup", "--client", "claude-code", "--bin", bin]);
    let result = sandbox
        .mcp_in(
            &sandbox.home,
            Some("claude-code"),
            &[("install_agent", json!({"id": "reviewer"}))],
        )
        .remove(0)
        .unwrap();
    assert!(
        sandbox.home.join(".claude/agents/reviewer.md").exists(),
        "{result}"
    );
    let installs = events(&sandbox.db, Some("agent_install"));
    assert_eq!(installs.len(), 1, "{installs:?}");
    assert_eq!(installs[0].data["scope"], "global");
    assert_eq!(installs[0].project, None);
    assert!(!sandbox.home.join(".git").exists());
}

#[test]
fn install_agent_returns_every_note_of_the_install() {
    let sandbox = Sandbox::new("claude-code");
    let result = sandbox
        .one(
            Some("claude-code"),
            "install_agent",
            json!({"id": "reviewer"}),
        )
        .unwrap();
    let notes: Vec<&str> = result["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("note: Cursor also reads .claude/agents/")),
        "{notes:?}"
    );
}

/// `publish_agent` arguments for a valid new global agent `id`.
fn agent_candidate(id: &str) -> Value {
    json!({
        "id": id,
        "description": "Use when a changelog entry must be drafted from a diff",
        "tier": "low",
        "tools": ["read", "shell"],
        "content": "## Role\nDrafts changelog entries.\n## Procedure\n1. Read the diff.\n\
                    ## Output contract\n- Result: done or blocked.\n- Entry: the drafted lines.\n\
                    ## Limits\nNever edits files.\n",
        "evidence": "Observed: two delegations. Tried: a hand-written prompt. Result: both returned the contract.",
        "expected_version": 0,
        "verdict": "keep global",
        "verdict_reason": "Bounded role, low tier fits, minimal tools, checkable contract"
    })
}

#[test]
fn publish_agent_publishes_only_after_a_keep_verdict_and_never_echoes_the_body() {
    let sandbox = Sandbox::new("claude-code");
    let mut discard = agent_candidate("changelog-writer");
    discard["verdict"] = json!("discard");
    let mut unevaluated = agent_candidate("changelog-writer");
    unevaluated.as_object_mut().unwrap().remove("verdict");
    let results = sandbox.mcp(
        Some("claude-code"),
        &[
            ("publish_agent", discard),
            ("publish_agent", unevaluated),
            ("publish_agent", agent_candidate("changelog-writer")),
            ("search_agents", json!({"query": "changelog"})),
            ("get_skill", json!({"id": "changelog-writer"})),
        ],
    );
    let discarded = results[0].as_ref().unwrap_err();
    assert!(discarded.contains("discarded"), "{discarded}");
    let unevaluated = results[1].as_ref().unwrap_err();
    assert!(unevaluated.contains("verdict"), "{unevaluated}");
    let published = results[2].as_ref().unwrap();
    assert_eq!(published["id"], "changelog-writer");
    assert_eq!(published["version"], 1);
    assert_eq!(published["scope"], Value::Null);
    assert!(
        published["next"]
            .as_str()
            .unwrap()
            .contains("install_agent"),
        "{published}"
    );
    assert!(
        !published.to_string().contains("Drafts changelog entries"),
        "{published}"
    );
    let found = results[3].as_ref().unwrap();
    assert_eq!(agent(found, "changelog-writer")["tier"], "low");
    assert!(results[4].as_ref().unwrap_err().contains("is an agent"));

    let vault = sandbox.vault();
    assert_eq!(
        vault.kind("changelog-writer").unwrap().as_deref(),
        Some("agent")
    );
    let revision = vault.inspect("changelog-writer", 1).unwrap();
    let meta = revision.meta.unwrap();
    assert_eq!(
        (meta.role.as_str(), meta.tier.as_str()),
        ("subagent", "low")
    );
    assert_eq!(meta.tools, Some(vec!["read".into(), "shell".into()]));
    assert_eq!(
        revision.review_note.as_deref(),
        Some("keep global: Bounded role, low tier fits, minimal tools, checkable contract")
    );

    // The sandbox's own CLI publish of `reviewer` is the first event.
    let publish: Vec<_> = events(&sandbox.db, Some("publish"))
        .into_iter()
        .filter(|e| e.subject.as_deref() == Some("changelog-writer"))
        .collect();
    assert_eq!(publish.len(), 1, "{publish:?}");
    assert_eq!(publish[0].client.as_deref(), Some("claude-code"));
    assert_eq!(publish[0].data["agent"], true);
    assert_eq!(publish[0].data["scope"], "global");
    let rejected = events(&sandbox.db, Some("publish_rejected"));
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert_eq!(rejected[0].subject.as_deref(), Some("changelog-writer"));
    // Only the refused get_skill is a tool error; a refused publish is not.
    let errors: Vec<_> = events(&sandbox.db, Some("mcp_error"))
        .into_iter()
        .map(|e| e.data["tool"].clone())
        .collect();
    assert_eq!(errors, [json!("get_skill")]);
}

#[test]
fn publish_agent_applies_the_agent_checks_and_logs_each_refusal() {
    let sandbox = Sandbox::new("claude-code");
    support::Draft::new("lint-skill").publish(&mut sandbox.vault());
    let with = |id: &str, key: &str, value: Value| {
        let mut args = agent_candidate(id);
        args[key] = value;
        args
    };
    let results = sandbox.mcp(
        Some("claude-code"),
        &[
            ("publish_agent", agent_candidate("lint-skill")),
            (
                "publish_agent",
                with("no-contract", "content", json!("## Role\nDrafts.\n")),
            ),
            ("publish_agent", agent_candidate("agent-evaluation")),
            ("publish_agent", agent_candidate("sv-writer")),
            ("publish_agent", {
                let mut args = with("skilled", "skills", json!(["missing-skill"]));
                args["tools"] = json!(["read", "mcp"]);
                args
            }),
            ("publish_agent", with("scoped", "scope", json!("project"))),
            (
                "publish_skill",
                json!({"id": "reviewer", "description": "Use when reviewing",
                       "content": support::sectioned("Body"), "evidence": "e",
                       "expected_version": 1, "verdict": "keep global",
                       "verdict_reason": "r"}),
            ),
        ],
    );
    let errors: Vec<&String> = results.iter().map(|r| r.as_ref().unwrap_err()).collect();
    assert!(
        errors[0].contains("already exists as a skill"),
        "{}",
        errors[0]
    );
    assert!(errors[1].contains("## Output contract"), "{}", errors[1]);
    assert!(errors[2].contains("reserved"), "{}", errors[2]);
    assert!(errors[3].contains("reserved"), "{}", errors[3]);
    assert!(errors[4].contains("missing-skill"), "{}", errors[4]);
    assert!(errors[5].contains("does not match scope"), "{}", errors[5]);
    assert!(errors[6].contains("publish_agent"), "{}", errors[6]);

    let rejected: Vec<String> = events(&sandbox.db, Some("publish_rejected"))
        .into_iter()
        .map(|e| e.subject.unwrap())
        .collect();
    assert_eq!(
        rejected,
        [
            "lint-skill",
            "no-contract",
            "agent-evaluation",
            "sv-writer",
            "skilled",
            "scoped",
            "reviewer"
        ]
    );
    assert!(events(&sandbox.db, Some("mcp_error")).is_empty());
    assert_eq!(sandbox.vault().latest_version("reviewer").unwrap(), 1);
}

#[test]
fn publish_agent_replaces_at_the_current_version_and_scopes_to_the_project() {
    let sandbox = Sandbox::new("claude-code");
    let replace = |expected: i64| {
        let mut args = agent_candidate("reviewer");
        args["description"] = json!("Use when a diff needs review");
        args["expected_version"] = json!(expected);
        args
    };
    let mut project = agent_candidate("repo-writer");
    project["scope"] = json!("project");
    project["verdict"] = json!("keep project");
    let results = sandbox.mcp(
        Some("claude-code"),
        &[
            ("publish_agent", replace(0)),
            ("publish_agent", replace(1)),
            ("publish_agent", project),
        ],
    );
    let stale = results[0].as_ref().unwrap_err();
    assert!(stale.contains("stale base"), "{stale}");
    assert_eq!(results[1].as_ref().unwrap()["version"], 2);
    assert_eq!(results[2].as_ref().unwrap()["scope"], "myproj");
    let other = sandbox.mcp_in(
        &sandbox.home,
        Some("claude-code"),
        &[("search_agents", json!({"query": "changelog"}))],
    );
    let other = other[0].as_ref().unwrap();
    assert!(
        other["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != "repo-writer"),
        "{other}"
    );
}

#[test]
fn publish_agent_requires_tools_so_published_agents_never_delegate() {
    let sandbox = Sandbox::new("claude-code,opencode");
    let mut missing = agent_candidate("no-tools");
    missing.as_object_mut().unwrap().remove("tools");
    let mut empty = agent_candidate("empty-tools");
    empty["tools"] = json!([]);
    let results = sandbox.mcp(
        None,
        &[
            ("publish_agent", missing),
            ("publish_agent", empty),
            ("publish_agent", agent_candidate("changelog-writer")),
            ("install_agent", json!({"id": "changelog-writer"})),
        ],
    );
    for refused in &results[..2] {
        let refused = refused.as_ref().unwrap_err();
        assert!(refused.contains("tools"), "{refused}");
    }
    assert_eq!(results[2].as_ref().unwrap()["version"], 1);
    results[3].as_ref().unwrap();
    let vault = sandbox.vault();
    assert!(vault.inspect("no-tools", 1).is_err());
    assert!(vault.inspect("empty-tools", 1).is_err());
    let rejected: Vec<String> = events(&sandbox.db, Some("publish_rejected"))
        .into_iter()
        .map(|e| e.subject.unwrap())
        .collect();
    assert_eq!(rejected, ["no-tools", "empty-tools"]);

    let claude = fs::read_to_string(sandbox.claude("changelog-writer")).unwrap();
    let denied = claude
        .lines()
        .find(|l| l.starts_with("disallowedTools:"))
        .unwrap_or_else(|| panic!("{claude}"));
    assert!(denied.contains("Agent, Task"), "{claude}");
    let opencode = fs::read_to_string(sandbox.opencode("changelog-writer")).unwrap();
    assert!(opencode.contains("\n  task: deny\n"), "{opencode}");
}
