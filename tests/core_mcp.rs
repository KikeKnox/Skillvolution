#[path = "support/mod.rs"]
mod support;

use serde_json::{Value, json};
use skillvolution::vault::Vault;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
};
use support::Draft;

/// A long-lived `skillvolution serve` subprocess, driven one JSON-RPC line at a time
/// so a test can interleave requests with out-of-process CLI writes.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl McpServer {
    fn spawn(db: &Path, project: Option<&str>) -> Self {
        let mut args = vec!["--db", db.to_str().unwrap(), "serve"];
        if let Some(project) = project {
            args.push("--project");
            args.push(project);
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn skillvolution serve");
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
fn uninitialized_tools_list_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    let response = server.request(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    assert_eq!(response["error"]["code"], -32002);
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
    Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("init")
        .output()
        .unwrap();

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
    assert_eq!(body, propose["result"]["structuredContent"]);
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
    assert_eq!(search_body, search["result"]["structuredContent"]);
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
fn report_skill_outcome_records_the_client_name_from_initialize() {
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
        json!({"id": "skill", "version": published.version, "result": "helped", "note": "worked well"}),
    );
    assert_eq!(outcome["result"]["isError"], false);
    drop(server);

    let vault = Vault::open(&db).unwrap();
    let log = vault.outcomes("skill").unwrap();
    assert_eq!(log[0].client.as_deref(), Some("test-client"));
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
