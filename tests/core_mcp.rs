use std::{
    io::Write,
    process::{Command, Stdio},
};

fn run(database: &std::path::Path, lines: &[&str]) -> Vec<String> {
    let binary = env!("CARGO_BIN_EXE_skillvolution");
    let mut child = Command::new(binary)
        .arg("--db")
        .arg(database)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn skillvolution serve");
    let mut stdin = child.stdin.take().expect("stdin pipe");
    for line in lines {
        writeln!(stdin, "{line}").expect("write request");
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait serve");
    assert!(
        output.status.success(),
        "serve failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("stdout utf8")
        .lines()
        .map(|line| line.to_string())
        .collect()
}

#[test]
fn serve_initializes_and_publishes_then_searches() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let binary = env!("CARGO_BIN_EXE_skillvolution");
    Command::new(binary)
        .arg("--db")
        .arg(&db)
        .arg("init")
        .output()
        .expect("init");
    let responses = run(
        &db,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"propose_skill_change","arguments":{"id":"rust-tests","description":"Procedural memory for TDD discipline.","content":"Run cargo test --all-targets before committing.","evidence":"Verified on commit abc123; tests caught two regressions.","expected_version":0}}}"#,
        ],
    );
    let init: serde_json::Value = serde_json::from_str(&responses[0]).expect("parse init");
    assert_eq!(init["result"]["serverInfo"]["name"], "skillvolution");
    let tools: serde_json::Value = serde_json::from_str(&responses[1]).expect("parse tools");
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"search_skills"));
    assert!(names.contains(&"get_skill"));
    assert!(names.contains(&"propose_skill_change"));
    let propose: serde_json::Value = serde_json::from_str(&responses[2]).expect("parse propose");
    assert_eq!(propose["result"]["isError"], false);
    assert_eq!(propose["result"]["content"][0]["type"], "text");
    let body: serde_json::Value =
        serde_json::from_str(propose["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["id"], "rust-tests");
    assert_eq!(body["version"], 1);
    assert_eq!(body["published"], false);

    let publish = Command::new(binary)
        .arg("--db")
        .arg(&db)
        .arg("publish")
        .arg("rust-tests")
        .arg("--version")
        .arg("1")
        .output()
        .expect("publish");
    assert!(
        publish.status.success(),
        "publish stderr={}",
        String::from_utf8_lossy(&publish.stderr)
    );

    let responses = run(
        &db,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_skills","arguments":{"query":"procedural"}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_skill","arguments":{"id":"rust-tests"}}}"#,
        ],
    );
    let search: serde_json::Value = serde_json::from_str(&responses[1]).expect("parse search");
    let body: serde_json::Value = serde_json::from_str(
        search["result"]["content"][0]["text"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["skills"][0]["id"], "rust-tests");
    assert_eq!(body["skills"][0]["version"], 1);
    assert!(!body["skills"][0].as_object().unwrap().contains_key("content"));
    assert!(!body["skills"][0].as_object().unwrap().contains_key("evidence"));
    let get: serde_json::Value = serde_json::from_str(&responses[2]).expect("parse get");
    let body: serde_json::Value =
        serde_json::from_str(get["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["content"], "Run cargo test --all-targets before committing.");
    assert_eq!(body["version"], 1);
    assert_eq!(body["published"], true);
}

#[test]
fn serve_refuses_uninitialized_calls() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let binary = env!("CARGO_BIN_EXE_skillvolution");
    Command::new(binary)
        .arg("--db")
        .arg(&db)
        .arg("init")
        .output()
        .expect("init");
    let responses = run(
        &db,
        &[r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#],
    );
    let error: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(error["error"]["code"], -32002);
}
