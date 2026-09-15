#[path = "support/mod.rs"]
mod support;

use serde_json::{Value, json};
use skillvolution::vault::Vault;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
};
use support::Draft;

/// Creates `parent/name` as a git repository root (just enough for
/// `skillvolution::project::detect` to recognize it: a `.git` directory) and
/// returns its path. `name` should already be a valid project key so the
/// detected key matches it exactly.
fn git_repo(parent: &Path, name: &str) -> std::path::PathBuf {
    let repo = parent.join(name);
    fs::create_dir_all(repo.join(".git")).unwrap();
    repo
}

/// A long-lived `skillvolution serve` subprocess, driven one JSON-RPC line at a time
/// so a test can interleave requests with out-of-process CLI writes.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl McpServer {
    /// Spawns with `--project` (if given) and, so runtime detection never sees
    /// this test binary's own working directory or environment, a cwd of
    /// `db`'s (git-free) temp directory and `CLAUDE_PROJECT_DIR` cleared.
    fn spawn(db: &Path, project: Option<&str>) -> Self {
        Self::spawn_in(db, project, db.parent().unwrap(), &[])
    }

    /// Spawns without `--project`, in `cwd`, with `env` applied after clearing
    /// `CLAUDE_PROJECT_DIR` — exercises runtime project detection.
    fn spawn_detecting(db: &Path, cwd: &Path, env: &[(&str, &str)]) -> Self {
        Self::spawn_in(db, None, cwd, env)
    }

    fn spawn_in(db: &Path, project: Option<&str>, cwd: &Path, env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        command
            .args(["--db", db.to_str().unwrap(), "serve"])
            .env_remove("CLAUDE_PROJECT_DIR")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(project) = project {
            command.args(["--project", project]);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn skillvolution serve");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        assert!(!line.is_empty(), "server closed stdout unexpectedly");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad response {line:?}: {e}"))
    }

    fn initialize(&mut self, protocol_version: &str) -> Value {
        self.request(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": protocol_version, "clientInfo": {"name": "test-client"}}
        }))
    }

    fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.request(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        // The process reads a request loop off stdin with no explicit close message;
        // killing it is the simplest reliable teardown for a test double.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text_of(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    serde_json::from_str(text).expect("text content is JSON")
}

#[test]
fn initialize_negotiates_a_supported_protocol_version_or_falls_back_to_latest() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    for requested in ["2025-06-18", "2025-03-26", "2024-11-05"] {
        let mut server = McpServer::spawn(&db, None);
        let response = server.initialize(requested);
        assert_eq!(response["result"]["protocolVersion"], requested);
    }
    let mut server = McpServer::spawn(&db, None);
    let response = server.initialize("1999-01-01");
    assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn tools_list_exposes_the_four_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let response = server.request(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "search_skills",
            "get_skill",
            "report_skill_outcome",
            "propose_skill_change"
        ]
    );
}

#[test]
fn propose_then_cli_publish_is_visible_to_the_same_running_server() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");

    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let propose = server.call(
        2,
        "propose_skill_change",
        json!({
            "id": "rust-tests",
            "description": "Use when running the Rust test suite",
            "content": "Run cargo test --all-targets before committing.",
            "evidence": "Verified on commit abc123; tests caught two regressions.",
            "expected_version": 0
        }),
    );
    assert_eq!(propose["result"]["isError"], false);
    let body = text_of(&propose);
    assert_eq!(body["id"], "rust-tests");
    assert_eq!(body["version"], 1);

    let not_yet_visible = server.call(3, "search_skills", json!({"query": "cargo"}));
    assert_eq!(text_of(&not_yet_visible)["total"], 0);

    let publish = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("publish")
        .arg("rust-tests")
        .arg("--version")
        .arg("1")
        .output()
        .unwrap();
    assert!(
        publish.status.success(),
        "{}",
        String::from_utf8_lossy(&publish.stderr)
    );

    let search = server.call(4, "search_skills", json!({"query": "cargo"}));
    let search_body = text_of(&search);
    assert_eq!(search_body["total"], 1);
    assert_eq!(search_body["skills"][0]["id"], "rust-tests");
    assert!(
        !search_body["skills"][0]
            .as_object()
            .unwrap()
            .contains_key("content")
    );

    let get = server.call(5, "get_skill", json!({"id": "rust-tests"}));
    let get_body = text_of(&get);
    assert_eq!(
        get_body["content"],
        "Run cargo test --all-targets before committing."
    );
    assert_eq!(get_body["version"], 1);
}

#[test]
fn project_filters_search_and_get_and_scope_project_requires_a_project_key() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");

    let mut proja_server = McpServer::spawn(&db, Some("proja"));
    proja_server.initialize("2025-06-18");
    let propose = proja_server.call(
        2,
        "propose_skill_change",
        json!({
            "id": "proj-only",
            "description": "Use when working on this repository",
            "content": "Body",
            "evidence": "Evidence",
            "expected_version": 0,
            "scope": "project"
        }),
    );
    assert_eq!(propose["result"]["isError"], false);
    Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("publish")
        .arg("proj-only")
        .arg("--version")
        .arg("1")
        .output()
        .unwrap();

    let visible = proja_server.call(3, "search_skills", json!({}));
    assert_eq!(text_of(&visible)["total"], 1);
    assert_eq!(
        proja_server.call(4, "get_skill", json!({"id": "proj-only"}))["result"]["isError"],
        false
    );

    let mut projb_server = McpServer::spawn(&db, Some("projb"));
    projb_server.initialize("2025-06-18");
    assert_eq!(
        text_of(&projb_server.call(2, "search_skills", json!({})))["total"],
        0
    );
    assert_eq!(
        projb_server.call(3, "get_skill", json!({"id": "proj-only"}))["result"]["isError"],
        true
    );

    let mut global_server = McpServer::spawn(&db, None);
    global_server.initialize("2025-06-18");
    let scope_without_project = global_server.call(
        2,
        "propose_skill_change",
        json!({
            "id": "needs-project",
            "description": "Use when this needs a project key",
            "content": "Body",
            "evidence": "Evidence",
            "expected_version": 0,
            "scope": "project"
        }),
    );
    assert_eq!(scope_without_project["result"]["isError"], true);
}

#[test]
fn report_skill_outcome_records_it_for_a_visible_published_revision() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    let published = Draft::new("skill").publish(&mut vault);
    drop(vault);

    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let outcome = server.call(
        2,
        "report_skill_outcome",
        json!({"id": "skill", "version": published, "result": "helped", "note": "worked well"}),
    );
    assert_eq!(outcome["result"]["isError"], false);
    assert_eq!(text_of(&outcome), json!({"recorded": true}));
    drop(server);

    let vault = Vault::open(&db).unwrap();
    let log = vault.outcomes("skill").unwrap();
    assert_eq!(log[0].note, "worked well");
}

#[test]
fn unknown_arguments_are_rejected_as_tool_errors() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let response = server.call(2, "search_skills", json!({"query": "x", "bogus": true}));
    assert_eq!(response["result"]["isError"], true);
}

// --- malformed input on stdin -----------------------------------------------

#[test]
fn invalid_utf8_on_stdin_gets_a_parse_error_and_the_server_keeps_running() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);

    server.stdin.write_all(&[0xff, 0xfe, b'\n']).unwrap();
    server.stdin.flush().unwrap();
    let mut line = String::new();
    server.stdout.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["error"]["code"], -32700);
    assert_eq!(response["id"], Value::Null);

    // The server is still alive and answers a normal request afterwards.
    let init = server.initialize("2025-06-18");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn batch_requests_get_a_single_unsupported_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    let response = server.request(json!([
        {"jsonrpc": "2.0", "id": 1, "method": "ping"},
        {"jsonrpc": "2.0", "id": 2, "method": "ping"}
    ]));
    assert_eq!(response["error"]["code"], -32600);
    assert_eq!(response["id"], Value::Null);
}

#[test]
fn a_response_shaped_message_without_method_is_ignored_silently() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);

    // Looks like a client's response to some earlier request of ours (has id
    // and result, no method) - must not get a reply.
    writeln!(
        server.stdin,
        "{}",
        json!({"jsonrpc": "2.0", "id": 99, "result": {}})
    )
    .unwrap();
    server.stdin.flush().unwrap();

    // The next real request gets exactly the next line of output, proving
    // nothing was written for the response-shaped message above.
    let init = server.initialize("2025-06-18");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn a_request_with_an_id_but_no_method_gets_invalid_request() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    let response = server.request(json!({"jsonrpc": "2.0", "id": 7}));
    assert_eq!(response["error"]["code"], -32600);
    assert_eq!(response["id"], 7);
}

// --- runtime project detection (no --project flag) ------------------------

#[test]
fn serve_without_project_detects_the_git_root_and_scopes_by_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let repo_a = git_repo(dir.path(), "proja");
    let repo_b = git_repo(dir.path(), "projb");

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("proj-only").scope("proja").publish(&mut vault);
    drop(vault);

    let mut server_a = McpServer::spawn_detecting(&db, &repo_a, &[]);
    server_a.initialize("2025-06-18");
    let visible = server_a.call(2, "search_skills", json!({}));
    assert_eq!(text_of(&visible)["total"], 1);
    assert_eq!(text_of(&visible)["skills"][0]["id"], "proj-only");

    let mut server_b = McpServer::spawn_detecting(&db, &repo_b, &[]);
    server_b.initialize("2025-06-18");
    assert_eq!(
        text_of(&server_b.call(2, "search_skills", json!({})))["total"],
        0
    );
}

#[test]
fn cwd_repo_beats_claude_project_dir() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let repo_a = git_repo(dir.path(), "proja");
    let repo_b = git_repo(dir.path(), "projb");

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("proja-skill").scope("proja").publish(&mut vault);
    Draft::new("projb-skill").scope("projb").publish(&mut vault);
    drop(vault);

    // cwd is repo_b, a real repo; CLAUDE_PROJECT_DIR points elsewhere at
    // repo_a. cwd must win.
    let mut server = McpServer::spawn_detecting(
        &db,
        &repo_b,
        &[("CLAUDE_PROJECT_DIR", repo_a.to_str().unwrap())],
    );
    server.initialize("2025-06-18");
    let visible = server.call(2, "search_skills", json!({}));
    assert_eq!(text_of(&visible)["total"], 1);
    assert_eq!(text_of(&visible)["skills"][0]["id"], "projb-skill");
}

#[test]
fn claude_project_dir_used_when_cwd_is_not_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let repo_a = git_repo(dir.path(), "proja");
    let outside = dir.path().join("no-git");
    fs::create_dir_all(&outside).unwrap();

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("proj-only").scope("proja").publish(&mut vault);
    drop(vault);

    // cwd is not a repo, so CLAUDE_PROJECT_DIR is used as a fallback.
    let mut server = McpServer::spawn_detecting(
        &db,
        &outside,
        &[("CLAUDE_PROJECT_DIR", repo_a.to_str().unwrap())],
    );
    server.initialize("2025-06-18");
    let visible = server.call(2, "search_skills", json!({}));
    assert_eq!(text_of(&visible)["total"], 1);
    assert_eq!(text_of(&visible)["skills"][0]["id"], "proj-only");
}

#[test]
fn outside_a_git_repo_only_global_skills_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let outside = dir.path().join("no-git");
    fs::create_dir_all(&outside).unwrap();

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("global-skill").publish(&mut vault);
    Draft::new("proj-skill")
        .scope("someproj")
        .publish(&mut vault);
    drop(vault);

    let mut server = McpServer::spawn_detecting(&db, &outside, &[]);
    server.initialize("2025-06-18");
    let visible = server.call(2, "search_skills", json!({}));
    assert_eq!(text_of(&visible)["total"], 1);
    assert_eq!(text_of(&visible)["skills"][0]["id"], "global-skill");
}

#[test]
fn explicit_project_overrides_detection() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    // repo's own name would detect as "proja"; --project asks for "override" instead.
    let repo_a = git_repo(dir.path(), "proja");

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("proja-skill").scope("proja").publish(&mut vault);
    Draft::new("override-skill")
        .scope("override")
        .publish(&mut vault);
    drop(vault);

    let mut server = McpServer::spawn_in(&db, Some("override"), &repo_a, &[]);
    server.initialize("2025-06-18");
    let visible = server.call(2, "search_skills", json!({}));
    assert_eq!(text_of(&visible)["total"], 1);
    assert_eq!(text_of(&visible)["skills"][0]["id"], "override-skill");
}
