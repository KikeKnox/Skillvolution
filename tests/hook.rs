#[path = "support/mod.rs"]
mod support;

use serde_json::{Value, json};
use skillvolution::{hook, vault::Vault};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
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
        ("s2", "mcp__skillvolution__publish_skill"),
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
fn stop_hook_active_still_advances_the_offset_so_the_next_turn_sees_only_new_work() {
    // Reproduces: turn 1 Edit -> Stop(false) blocks and saves offset A; Claude
    // then calls report_skill_outcome (lines A..B) -> Stop(true) fires (this
    // hook's own block caused it) and must not block, but must still save
    // offset B so a stale review from before B never masks new work after B.
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[assistant_tool_use("Edit")]);

    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_some()); // turn 1: blocks

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    writeln!(
        file,
        "{}",
        assistant_tool_use("mcp__skillvolution__report_skill_outcome")
    )
    .unwrap();
    let active_input = stop_input("s1", &transcript, true);
    assert!(hook::stop(&vault, &active_input).unwrap().is_none()); // stop_hook_active: never blocks

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    writeln!(file, "{}", assistant_tool_use("Edit")).unwrap();
    let input = stop_input("s1", &transcript, false);
    // turn 2: a fresh, unreviewed Edit must block again. If the offset was not
    // saved above, this call would re-see the old Edit+report pair together
    // with the new Edit and conclude everything was already reviewed.
    assert!(hook::stop(&vault, &input).unwrap().is_some());
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

// --- Devin hooks (flag-based; the payloads carry no transcript) -------------

fn devin_tool(session: &str, tool: &str) -> String {
    json!({"session_id": session, "tool_name": tool}).to_string()
}

fn devin_stop_input(session: &str, stop_hook_active: bool) -> String {
    json!({"session_id": session, "stop_hook_active": stop_hook_active}).to_string()
}

#[test]
fn devin_work_blocks_once_then_not_again() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "exec")).unwrap();
    let input = devin_stop_input("s1", false);
    assert!(hook::devin_stop(&vault, &input).unwrap().is_some());
    assert!(hook::devin_stop(&vault, &input).unwrap().is_none());
}

#[test]
fn devin_review_tool_marks_the_span_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    for (session, review_tool) in [
        ("s1", "mcp__skillvolution__report_skill_outcome"),
        ("s2", "mcp__skillvolution__publish_skill"),
    ] {
        hook::devin_tool_use(&vault, &devin_tool(session, "write")).unwrap();
        hook::devin_tool_use(&vault, &devin_tool(session, review_tool)).unwrap();
        let input = devin_stop_input(session, false);
        assert!(hook::devin_stop(&vault, &input).unwrap().is_none());
    }
}

#[test]
fn devin_stop_hook_active_never_blocks_and_keeps_the_flags() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "edit")).unwrap();
    assert!(
        hook::devin_stop(&vault, &devin_stop_input("s1", true))
            .unwrap()
            .is_none()
    );
    // The flags survive, so the next real stop still sees the unreviewed work.
    assert!(
        hook::devin_stop(&vault, &devin_stop_input("s1", false))
            .unwrap()
            .is_some()
    );
}

#[test]
fn devin_new_work_after_a_block_blocks_again() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let input = devin_stop_input("s1", false);
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
    assert!(hook::devin_stop(&vault, &input).unwrap().is_some());
    hook::devin_tool_use(&vault, &devin_tool("s1", "apply_patch")).unwrap();
    assert!(hook::devin_stop(&vault, &input).unwrap().is_some());
}

#[test]
fn devin_unrelated_tools_and_sessions_leave_flags_alone() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "read")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "run_subagent")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("other", "exec")).unwrap();
    let input = devin_stop_input("s1", false);
    assert!(hook::devin_stop(&vault, &input).unwrap().is_none());
}

#[test]
fn devin_session_end_clears_the_flags() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "exec")).unwrap();
    hook::devin_session_end(&vault, &json!({"session_id": "s1"}).to_string()).unwrap();
    let input = devin_stop_input("s1", false);
    assert!(hook::devin_stop(&vault, &input).unwrap().is_none());
}

#[test]
fn devin_approve_only_approves_subagent_and_vault_tools() {
    for tool in [
        "run_subagent",
        "read_subagent",
        "mcp__skillvolution__search_skills",
        "mcp__skillvolution__publish_skill",
    ] {
        let input = json!({"session_id": "s1", "tool_name": tool}).to_string();
        assert!(hook::devin_approve(&input).unwrap().is_some(), "{tool}");
    }
    for tool in [
        "exec",
        "write",
        "mcp__github__create_issue",
        "mcp__skillvolution",
    ] {
        let input = json!({"session_id": "s1", "tool_name": tool}).to_string();
        assert!(hook::devin_approve(&input).unwrap().is_none(), "{tool}");
    }
}

#[test]
fn devin_session_start_wraps_the_catalog_in_hook_specific_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("rust-tests")
        .description("Use when running tests")
        .publish(&mut vault);
    let output: serde_json::Value =
        serde_json::from_str(&hook::devin_session_start(&vault, None).unwrap()).unwrap();
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("rust-tests"));
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

/// Runs `hook session-start`, isolated from this test binary's own cwd and
/// environment: `cwd` and `env` (applied after clearing `CLAUDE_PROJECT_DIR`)
/// are the only inputs to runtime project detection. `project` sets `--project`
/// when given.
fn run_hook_session_start(
    db: &Path,
    project: Option<&str>,
    cwd: &Path,
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
    command
        .arg("--db")
        .arg(db)
        .arg("hook")
        .arg("session-start")
        .env_remove("CLAUDE_PROJECT_DIR")
        .current_dir(cwd);
    if let Some(project) = project {
        command.arg("--project").arg(project);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

#[test]
fn cli_hook_session_start_prints_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("rust-tests").publish(&mut vault);
    drop(vault);
    let output = run_hook_session_start(&db, None, dir.path(), &[]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("rust-tests"));
}

// --- runtime project detection (no --project flag) ------------------------

#[test]
fn session_start_without_project_detects_the_git_root_and_scopes_by_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let repo_a = git_repo(dir.path(), "proja");
    let repo_b = git_repo(dir.path(), "projb");

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("proj-only").scope("proja").publish(&mut vault);
    drop(vault);

    let in_a = run_hook_session_start(&db, None, &repo_a, &[]);
    assert!(in_a.status.success());
    assert!(String::from_utf8_lossy(&in_a.stdout).contains("proj-only"));

    let in_b = run_hook_session_start(&db, None, &repo_b, &[]);
    assert!(in_b.status.success());
    assert!(!String::from_utf8_lossy(&in_b.stdout).contains("proj-only"));
}

#[test]
fn session_start_cwd_repo_beats_claude_project_dir() {
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
    let output = run_hook_session_start(
        &db,
        None,
        &repo_b,
        &[("CLAUDE_PROJECT_DIR", repo_a.to_str().unwrap())],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("projb-skill"));
    assert!(!stdout.contains("proja-skill"));
}

#[test]
fn session_start_claude_project_dir_used_when_cwd_is_not_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let repo_a = git_repo(dir.path(), "proja");
    let outside = dir.path().join("no-git");
    fs::create_dir_all(&outside).unwrap();

    let mut vault = Vault::open(&db).unwrap();
    Draft::new("proj-only").scope("proja").publish(&mut vault);
    drop(vault);

    // cwd is not a repo, so CLAUDE_PROJECT_DIR is used as a fallback.
    let output = run_hook_session_start(
        &db,
        None,
        &outside,
        &[("CLAUDE_PROJECT_DIR", repo_a.to_str().unwrap())],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("proj-only"));
}

#[test]
fn session_start_outside_a_git_repo_only_shows_global_skills() {
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

    let output = run_hook_session_start(&db, None, &outside, &[]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("global-skill"));
    assert!(!stdout.contains("proj-skill"));
}

#[test]
fn session_start_explicit_project_overrides_detection() {
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

    let output = run_hook_session_start(&db, Some("override"), &repo_a, &[]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("override-skill"));
    assert!(!stdout.contains("proja-skill"));
}

// --- CLI: Devin hook events ---------------------------------------------------

/// Runs `hook <args>` with `input` on stdin; stdout/stderr are captured.
fn run_hook(db: &Path, args: &[&str], input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(db)
        .arg("hook")
        .args(args)
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
fn cli_devin_stop_prints_a_block_decision_once_per_unreviewed_span() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let vault = Vault::open(&db).unwrap();
    vault.set_devin_hook_state("s1", true, false).unwrap();
    drop(vault);

    let blocked = run_hook(
        &db,
        &["stop", "--client", "devin"],
        &devin_stop_input("s1", false),
    );
    assert!(blocked.status.success());
    let decision: serde_json::Value =
        serde_json::from_slice(&blocked.stdout).expect("stdout must be the decision JSON");
    assert_eq!(decision["decision"], "block");
    assert!(
        decision["reason"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
        "{decision}"
    );

    // The span was consumed: an unchanged second stop prints nothing.
    let clean = run_hook(
        &db,
        &["stop", "--client", "devin"],
        &devin_stop_input("s1", false),
    );
    assert!(clean.status.success());
    assert!(clean.stdout.is_empty());
}

#[test]
fn cli_devin_tool_use_then_stop_flows_through_the_flag_table() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");

    let tool = run_hook(&db, &["tool-use"], &devin_tool("s1", "exec"));
    assert!(tool.status.success());
    assert!(tool.stdout.is_empty());

    let blocked = run_hook(
        &db,
        &["stop", "--client", "devin"],
        &devin_stop_input("s1", false),
    );
    assert!(blocked.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&blocked.stdout).unwrap()["decision"],
        "block"
    );
}

#[test]
fn cli_hook_approve_prints_an_approve_decision_only_for_our_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();

    let approved = run_hook(
        &db,
        &["approve"],
        &json!({"session_id": "s", "tool_name": "run_subagent"}).to_string(),
    );
    assert!(approved.status.success());
    let decision: serde_json::Value = serde_json::from_slice(&approved.stdout).unwrap();
    assert_eq!(decision["decision"], "approve");

    let other = run_hook(
        &db,
        &["approve"],
        &json!({"session_id": "s", "tool_name": "exec"}).to_string(),
    );
    assert!(other.status.success());
    assert!(other.stdout.is_empty());
}
