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

/// A long-lived `skillvolution serve` subprocess, driven one JSON-RPC line at
/// a time. Copied from core_mcp.rs, where the same helper is private to that
/// test file.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl McpServer {
    /// Spawns with `--project` (if given) and a cwd of `db`'s (git-free) temp
    /// directory, with `CLAUDE_PROJECT_DIR` cleared.
    fn spawn(db: &Path, project: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
        command
            .args(["--db", db.to_str().unwrap(), "serve"])
            .env_remove("CLAUDE_PROJECT_DIR")
            .current_dir(db.parent().unwrap())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(project) = project {
            command.args(["--project", project]);
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

fn open() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    (dir, vault)
}

// --- MCP protocol --------------------------------------------------------

#[test]
fn ping_returns_an_empty_result_object() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let response = server.request(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
    assert_eq!(response["id"], 2);
    assert_eq!(response["result"], json!({}));
    assert!(!response.as_object().unwrap().contains_key("error"));
}

#[test]
fn a_notification_produces_no_response_and_the_next_request_is_answered() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");

    // A notification has no `id`, so the server must not reply to it.
    writeln!(
        server.stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .unwrap();
    server.stdin.flush().unwrap();

    // The next real request gets exactly the next line of output, proving
    // nothing was written for the notification above.
    let ping = server.request(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}));
    assert_eq!(ping["id"], 3);
    assert_eq!(ping["result"], json!({}));
}

#[test]
fn an_unknown_method_returns_method_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let response = server.request(json!({
        "jsonrpc": "2.0", "id": 4, "method": "no/such.method"
    }));
    assert_eq!(response["id"], 4);
    assert_eq!(response["error"]["code"], -32601);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("method not found"),
        "{response}"
    );
    assert!(!response.as_object().unwrap().contains_key("result"));
}

#[test]
fn tools_list_returns_exactly_the_four_tools_in_order() {
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
            "publish_skill"
        ]
    );
}

// --- vault ---------------------------------------------------------------

#[test]
fn diff_reports_no_changes_when_a_republish_matches_its_base() {
    let (_dir, mut vault) = open();
    let first = Draft::new("skill")
        .content("line one\nline two\n")
        .propose(&mut vault);

    // The first version has no base, so it diffs against the empty string:
    // every rendered line is an addition under a "v0" -> "v1" header.
    let first_diff = vault.diff("skill", first.version).unwrap();
    assert!(first_diff.contains("skill v0"), "{first_diff}");
    assert!(first_diff.contains("skill v1"), "{first_diff}");
    assert!(first_diff.contains("+line one"), "{first_diff}");

    // A republish whose rendered text (description, tags, content) matches its
    // base hits the "no changes" branch.
    // NOTE: `diff` compares the rendered description/tags/content only, so a
    // republish that changes just `evidence` still reports "no changes".
    let second = Draft::new("skill")
        .expected_version(1)
        .content("line one\nline two\n")
        .evidence("Different evidence text")
        .propose(&mut vault);
    assert_eq!(second.version, 2);
    assert_eq!(
        vault.diff("skill", second.version).unwrap(),
        "skill v2: no changes from its base\n"
    );
}

#[test]
fn text_search_pagination_past_the_end_reports_the_true_total() {
    let (_dir, mut vault) = open();
    for id in ["page-a", "page-b", "page-c"] {
        Draft::new(id)
            .description("Use when frobnicate fails")
            .publish(&mut vault);
    }
    let page = vault.search("frobnicate", None, 2, 4).unwrap();
    assert!(page.skills.is_empty());
    assert_eq!(page.total, 3);
    assert!(!page.has_more);
}

#[test]
fn deprecated_skills_are_hidden_from_text_search_until_undeprecated() {
    let (_dir, mut vault) = open();
    Draft::new("skill")
        .description("Use when frobnicate fails")
        .publish(&mut vault);
    assert_eq!(vault.search("frobnicate", None, 20, 0).unwrap().total, 1);
    vault.set_deprecated("skill", true).unwrap();
    assert_eq!(vault.search("frobnicate", None, 20, 0).unwrap().total, 0);
    vault.set_deprecated("skill", false).unwrap();
    assert_eq!(vault.search("frobnicate", None, 20, 0).unwrap().total, 1);
}

#[test]
fn republishing_replaces_the_indexed_text_so_old_version_words_stop_matching() {
    let (_dir, mut vault) = open();
    Draft::new("skill")
        .content("mentions quixoteword in v1")
        .publish(&mut vault);
    assert_eq!(vault.search("quixoteword", None, 20, 0).unwrap().total, 1);

    Draft::new("skill")
        .expected_version(1)
        .content("mentions different terms")
        .publish(&mut vault);
    // The index holds one row per skill (replaced by rowid on every publish),
    // so a word unique to v1 no longer matches even though v1 itself is still
    // retrievable.
    let page = vault.search("quixoteword", None, 20, 0).unwrap();
    assert!(page.skills.is_empty());
    assert_eq!(page.total, 0);
    assert!(
        vault
            .get("skill", Some(1), None)
            .unwrap()
            .content
            .contains("quixoteword")
    );
}

#[test]
fn get_skill_with_an_explicit_version_returns_that_versions_content() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    let first = Draft::new("skill")
        .content("first body")
        .propose(&mut vault);
    Draft::new("skill")
        .expected_version(1)
        .content("second body")
        .propose(&mut vault);
    drop(vault);

    let mut server = McpServer::spawn(&db, None);
    server.initialize("2025-06-18");
    let get = server.call(
        2,
        "get_skill",
        json!({"id": "skill", "version": first.version}),
    );
    assert_eq!(get["result"]["isError"], false);
    let body = text_of(&get);
    assert_eq!(body["version"], 1);
    assert_eq!(body["content"].as_str().unwrap(), first.content);

    let latest = server.call(3, "get_skill", json!({"id": "skill"}));
    assert_eq!(text_of(&latest)["version"], 2);
}

#[test]
fn a_project_scoped_skill_is_invisible_to_text_search_from_other_projects() {
    let (_dir, mut vault) = open();
    Draft::new("proj-skill")
        .scope("proja")
        .description("Use when frobnicate fails")
        .publish(&mut vault);
    assert_eq!(
        vault
            .search("frobnicate", Some("proja"), 20, 0)
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        vault
            .search("frobnicate", Some("projb"), 20, 0)
            .unwrap()
            .total,
        0
    );
    assert_eq!(vault.search("frobnicate", None, 20, 0).unwrap().total, 0);
}
