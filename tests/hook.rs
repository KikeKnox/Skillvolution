#[path = "support/mod.rs"]
mod support;

use serde_json::json;
use skillvolution::{hook, vault::Vault};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};
use support::Draft;

fn assistant_tool_use(name: &str) -> String {
    json!({"type":"assistant","message":{"role":"assistant","content":[
        {"type":"tool_use","id":"t1","name":name,"input":{}}
    ]}})
    .to_string()
}

fn user_text(text: &str) -> String {
    json!({"type":"user","message":{"role":"user","content":text}}).to_string()
}

fn write_transcript(path: &std::path::Path, lines: &[String]) {
    let mut content = lines.join("\n");
    content.push('\n');
    fs::write(path, content).unwrap();
}

fn stop_input(session: &str, transcript: &std::path::Path, stop_hook_active: bool) -> String {
    json!({
        "session_id": session,
        "transcript_path": transcript,
        "stop_hook_active": stop_hook_active,
    })
    .to_string()
}

// --- unit tests via the lib, one Vault + transcript fixture per case ------

#[test]
fn no_work_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[user_text("hi"), assistant_tool_use("Read")]);
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_none());
}

#[test]
fn edit_blocks_once_then_not_again_on_an_unchanged_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(
        &transcript,
        &[user_text("please edit"), assistant_tool_use("Edit")],
    );
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_some());
    assert!(hook::stop(&vault, &input).unwrap().is_none());
}

#[test]
fn new_lines_appended_after_the_offset_block_again() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[assistant_tool_use("Edit")]);
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_some());
    assert!(hook::stop(&vault, &input).unwrap().is_none());

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    writeln!(file, "{}", assistant_tool_use("Bash")).unwrap();
    assert!(hook::stop(&vault, &input).unwrap().is_some());
}

#[test]
fn work_plus_review_tool_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    for (session, review_tool) in [
        ("s1", "mcp__skillvolution__report_skill_outcome"),
        ("s2", "mcp__skillvolution__propose_skill_change"),
    ] {
        let transcript = dir.path().join(format!("{session}.jsonl"));
        write_transcript(
            &transcript,
            &[assistant_tool_use("Edit"), assistant_tool_use(review_tool)],
        );
        let input = stop_input(session, &transcript, false);
        assert!(hook::stop(&vault, &input).unwrap().is_none());
    }
}

#[test]
fn stop_hook_active_never_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[assistant_tool_use("Edit")]);
    let input = stop_input("s1", &transcript, true);
    assert!(hook::stop(&vault, &input).unwrap().is_none());
}

#[test]
fn missing_transcript_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let input = stop_input("s1", &dir.path().join("does-not-exist.jsonl"), false);
    assert!(hook::stop(&vault, &input).unwrap().is_none());
}

#[test]
fn trailing_partial_line_is_not_consumed() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    // A complete no-work line, then a truncated line still being written (no
    // trailing newline) that happens to contain an Edit tool_use.
    let partial = assistant_tool_use("Edit");
    let partial = &partial[..partial.len() - 5];
    fs::write(&transcript, format!("{}\n{partial}", user_text("hi"))).unwrap();
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_none());

    // Once the writer finishes the line, it is picked up on the next call.
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    writeln!(file, "{}", &assistant_tool_use("Edit")[partial.len()..]).unwrap();
    assert!(hook::stop(&vault, &input).unwrap().is_some());
}

#[test]
fn a_truncated_or_rotated_transcript_restarts_at_zero() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[assistant_tool_use("Edit")]);
    vault.set_transcript_offset("s1", 10_000).unwrap();
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_some());
}

#[test]
fn session_start_lists_published_skills_or_says_there_are_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let text = hook::session_start(&vault, None).unwrap();
    assert!(text.contains("No published skills"));

    Draft::new("rust-tests")
        .description("Use when running tests")
        .publish(&mut vault);
    let text = hook::session_start(&vault, None).unwrap();
    assert!(text.contains("rust-tests"));
    assert!(text.contains("Use when running tests"));
}

// --- CLI ------------------------------------------------------------------

fn run_hook_stop(db: &std::path::Path, input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(db)
        .arg("hook")
        .arg("stop")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn cli_hook_stop_exits_2_with_a_reason_when_blocking_and_0_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[assistant_tool_use("Edit")]);
    let input = stop_input("s1", &transcript, false);

    let blocked = run_hook_stop(&db, &input);
    assert_eq!(blocked.status.code(), Some(2));
    assert!(!blocked.stderr.is_empty());

    let clean = run_hook_stop(&db, &input);
    assert!(clean.status.success());
    assert!(clean.stderr.is_empty());
}

#[test]
fn cli_hook_session_start_prints_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("rust-tests").publish(&mut vault);
    drop(vault);
    let output = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("hook")
        .arg("session-start")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("rust-tests"));
}
