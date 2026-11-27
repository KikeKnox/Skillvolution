//! Local usage telemetry: which events the existing code paths record, what they leave
//! out (prompts, notes, credentials, absolute paths), and that recording never breaks
//! the operation it observes.

mod support;

use serde_json::{Value, json};
use skillvolution::{
    clients::{ClientKind, Version},
    hook,
    vault::Vault,
};
use std::{fs, io::Write, path::Path, process::Stdio};
use support::{Draft, StoredEvent, events};

/// Runs one MCP session on `db` (project `proj`): every request in `calls`
/// (`(tool, arguments)`) after `initialize`, then closes stdin. Returns the `isError`
/// flag of each call.
fn mcp(db: &Path, calls: &[(&str, Value)]) -> Vec<bool> {
    let mut child = support::cli()
        .args(["--db", db.to_str().unwrap(), "serve", "--project", "proj"])
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
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|reply| reply["id"].as_i64().is_some_and(|id| id > 0))
        .map(|reply| reply["result"]["isError"].as_bool().unwrap_or(true))
        .collect()
}

fn hook_cmd(db: &Path, args: &[&str], input: &str) -> std::process::Output {
    let mut child = support::cli()
        .args(["--db", db.to_str().unwrap(), "hook"])
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

fn db_with_skill(dir: &Path) -> std::path::PathBuf {
    let db = dir.join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("sqlite-busy-timeout")
        .description("Use when SQLite reports database is locked under concurrent writers")
        .publish(&mut vault);
    db
}

fn only(mut found: Vec<StoredEvent>) -> StoredEvent {
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

#[test]
fn mcp_calls_record_search_load_outcome_and_publish_events() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    let draft = Draft::new("new-skill");
    let results = mcp(
        &db,
        &[
            (
                "search_skills",
                json!({"query": "sqlite locked /home/ana/secret-project"}),
            ),
            ("get_skill", json!({"id": "sqlite-busy-timeout"})),
            (
                "report_skill_outcome",
                json!({"id": "sqlite-busy-timeout", "version": 1, "result": "helped",
                       "note": "the note must never reach events"}),
            ),
            (
                "publish_skill",
                json!({"id": "new-skill", "description": draft.proposal().description,
                       "content": draft.proposal().content, "evidence": "e",
                       "expected_version": 0, "verdict": "keep global",
                       "verdict_reason": "r"}),
            ),
            (
                "publish_skill",
                json!({"id": "bad", "description": "d", "content": "no sections",
                       "evidence": "e", "expected_version": 0,
                       "verdict": "keep global", "verdict_reason": "r"}),
            ),
        ],
    );
    assert_eq!(results, [false, false, false, false, true]);

    let search = only(events(&db, Some("search")));
    assert_eq!(search.data["query"], "sqlite locked");
    assert_eq!(search.data["total"], 1);
    assert_eq!(search.data["ids"], json!(["sqlite-busy-timeout"]));
    assert_eq!(search.project.as_deref(), Some("proj"));
    // Started without --client, the MCP server does not know its client.
    assert_eq!((search.client, search.client_version), (None, None));

    let load = only(events(&db, Some("load")));
    assert_eq!(load.subject.as_deref(), Some("sqlite-busy-timeout"));
    assert_eq!(load.version, Some(1));

    let outcome = only(events(&db, Some("outcome")));
    assert_eq!(outcome.data, json!({"result": "helped"}));
    assert!(!format!("{:?}", events(&db, None)).contains("must never reach"));

    let publish = only(events(&db, Some("publish")));
    assert_eq!(publish.subject.as_deref(), Some("new-skill"));
    assert_eq!(publish.data["scope"], "global");
    let rejected = only(events(&db, Some("publish_rejected")));
    assert_eq!(rejected.subject.as_deref(), Some("bad"));
    assert!(rejected.data["reason"].as_str().unwrap().contains("##"));
    assert!(events(&db, Some("mcp_error")).is_empty());
}

#[test]
fn a_search_query_with_a_credential_is_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    mcp(
        &db,
        &[(
            "search_skills",
            json!({"query": "deploy ghp_abcdefghijklmnopqrstuvwxyz0123456789"}),
        )],
    );
    let search = only(events(&db, Some("search")));
    assert_eq!(search.data["query"], Value::Null);
    assert!(!format!("{search:?}").contains("ghp_"));
}

#[test]
fn a_failing_tool_records_an_mcp_error_with_a_class_and_no_message() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    let results = mcp(
        &db,
        &[
            ("get_skill", json!({"id": "no-such-skill"})),
            ("search_skills", json!({"bogus": 1})),
        ],
    );
    assert_eq!(results, [true, true]);
    let errors = events(&db, Some("mcp_error"));
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert_eq!(
        errors[0].data,
        json!({"tool": "get_skill", "class": "rejected"})
    );
    assert_eq!(
        errors[1].data,
        json!({"tool": "search_skills", "class": "invalid_arguments"})
    );
}

#[test]
fn cli_search_publish_and_rejection_record_events() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    assert!(
        support::run(&db, &["search", "sqlite", "--project", "proj"])
            .status
            .success()
    );
    let search = only(events(&db, Some("search")));
    assert_eq!(search.data["query"], "sqlite");
    assert_eq!(search.project.as_deref(), Some("proj"));

    let good = dir.path().join("good/SKILL.md");
    fs::create_dir_all(good.parent().unwrap()).unwrap();
    fs::write(
        &good,
        "---\nname: good-skill\ndescription: Use when testing events\n---\n## When to use\nx\n## Procedure\n1. y\n## Pitfalls\n- z\n## Verification\nv\n",
    )
    .unwrap();
    let bad = dir.path().join("bad/SKILL.md");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, "---\nname: bad-skill\ndescription: nope\n---\nbody\n").unwrap();
    assert!(
        support::run(&db, &["publish", good.to_str().unwrap()])
            .status
            .success()
    );
    assert!(
        !support::run(&db, &["publish", bad.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(
        only(events(&db, Some("publish"))).subject.as_deref(),
        Some("good-skill")
    );
    let rejected = only(events(&db, Some("publish_rejected")));
    let text = rejected.data.to_string();
    assert!(!text.contains(dir.path().to_str().unwrap()), "{text}");
}

#[test]
fn hooks_record_session_start_and_suggest_with_the_stored_client_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    let vault = Vault::open(&db).unwrap();
    vault
        .record_client_version(ClientKind::Codex, Version(0, 9, 1))
        .unwrap();
    drop(vault);

    let start = hook_cmd(
        &db,
        &["session-start", "--client", "codex", "--project", "proj"],
        "",
    );
    assert!(start.status.success());
    let start = only(events(&db, Some("session_start")));
    assert_eq!(start.client.as_deref(), Some("codex"));
    assert_eq!(start.client_version.as_deref(), Some("0.9.1"));
    assert_eq!(start.project.as_deref(), Some("proj"));
    assert_eq!(start.data, json!({"catalog": 1}));

    let prompt = json!({"session_id": "s-7",
        "prompt": "sqlite database locked concurrent writers zebra-unique-word"})
    .to_string();
    let out = hook_cmd(
        &db,
        &["prompt", "--client", "codex", "--project", "proj"],
        &prompt,
    );
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sqlite-busy-timeout"));
    let suggest = only(events(&db, Some("suggest")));
    assert_eq!(suggest.session.as_deref(), Some("s-7"));
    let suggested = &suggest.data["suggested"][0];
    assert_eq!(suggested["id"], "sqlite-busy-timeout");
    assert!(suggested["score"].as_f64().unwrap() > 0.0, "{suggested}");
    assert!(!format!("{suggest:?}").contains("zebra"));

    // A searchable prompt with no match records an empty list (and no prompt text); a
    // prompt too short to be searched records nothing.
    let none =
        json!({"session_id": "s-8", "prompt": "quantum chromodynamics lattice gauge"}).to_string();
    hook_cmd(
        &db,
        &["prompt", "--client", "codex", "--project", "proj"],
        &none,
    );
    hook_cmd(
        &db,
        &["prompt", "--client", "codex"],
        &json!({"prompt": "hola"}).to_string(),
    );
    let all = events(&db, Some("suggest"));
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1].data, json!({"suggested": []}));
    assert!(!format!("{all:?}").contains("quantum"));
}

#[test]
fn a_failing_hook_records_a_hook_error_and_still_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    let out = hook_cmd(&db, &["tool-use", "--client", "cursor"], "not json {");
    assert!(out.status.success());
    let error = only(events(&db, Some("hook_error")));
    assert_eq!(error.client.as_deref(), Some("cursor"));
    assert_eq!(error.data, json!({"hook": "tool-use", "class": "json"}));
}

fn tool_use(id: &str, name: &str, input: Value) -> String {
    json!({"type":"assistant","message":{"role":"assistant","content":[
        {"type":"tool_use","id":id,"name":name,"input":input}]}})
    .to_string()
}

fn stop_input(session: &str, transcript: &Path, active: bool) -> String {
    json!({"session_id": session, "transcript_path": transcript,
           "stop_hook_active": active})
    .to_string()
}

#[test]
fn stop_records_delegations_and_the_review_reminder_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("reviewer").publish_agent(&mut vault);
    let hooks = &ClientKind::ClaudeCode.spec().hooks;
    let transcript = dir.path().join("t.jsonl");
    let write = |lines: &[String]| fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let stop = |active: bool| {
        hook::transcript_stop_as(
            &vault,
            &stop_input("s1", &transcript, active),
            Some("proj"),
            &[],
            hooks,
            Some(ClientKind::ClaudeCode),
        )
        .unwrap()
    };

    let mut lines = vec![
        tool_use(
            "d1",
            "Agent",
            json!({"subagent_type": "reviewer", "prompt": "secret text"}),
        ),
        tool_use(
            "d2",
            "Agent",
            json!({"subagent_type": "sv-low", "model": "haiku"}),
        ),
        tool_use("d3", "Agent", json!({"subagent_type": "general-purpose"})),
        tool_use("w1", "Edit", json!({"file_path": "/home/ana/x.rs"})),
    ];
    write(&lines);
    assert!(stop(false).is_some());
    let delegations = events(&vault_path(dir.path()), Some("delegation"));
    let seen: Vec<_> = delegations
        .iter()
        .map(|e| {
            (
                e.subject.as_deref().unwrap(),
                e.data["source"].as_str().unwrap(),
                e.data["level"].as_str(),
                e.data["model"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("reviewer", "vault", Some("mid"), None),
            ("sv-low", "builtin", Some("low"), Some("haiku")),
            ("general-purpose", "other", None, None),
        ]
    );
    assert_eq!(delegations[0].client.as_deref(), Some("claude-code"));
    assert_eq!(delegations[0].session.as_deref(), Some("s1"));
    let text = format!("{:?}", events(&vault_path(dir.path()), None));
    assert!(
        !text.contains("secret text") && !text.contains("/home/ana"),
        "{text}"
    );
    assert_eq!(reminders(dir.path()), ["fired"]);

    // The agent continues without reviewing: the reminder was ignored.
    lines.push(tool_use("r0", "Read", json!({})));
    write(&lines);
    assert!(stop(true).is_none());
    assert_eq!(reminders(dir.path()), ["fired", "ignored"]);

    // Continues and reviews: nothing more to record.
    lines.push(tool_use(
        "r1",
        "mcp__skillvolution__report_skill_outcome",
        json!({}),
    ));
    write(&lines);
    assert!(stop(true).is_none());
    assert_eq!(reminders(dir.path()), ["fired", "ignored"]);
}

fn vault_path(dir: &Path) -> std::path::PathBuf {
    dir.join("skills.db")
}

fn reminders(dir: &Path) -> Vec<String> {
    events(&vault_path(dir), Some("review_reminder"))
        .iter()
        .map(|e| e.data["state"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn flag_tracked_clients_record_the_reminder_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&vault_path(dir.path())).unwrap();
    let client = Some(ClientKind::Devin);
    let input = |active: bool| json!({"session_id": "d1", "stop_hook_active": active}).to_string();
    vault.mark_devin_work("d1").unwrap();
    assert!(
        hook::devin_stop_as(&vault, &input(false), None, client)
            .unwrap()
            .is_some()
    );
    assert!(
        hook::devin_stop_as(&vault, &input(true), None, client)
            .unwrap()
            .is_none()
    );
    assert_eq!(reminders(dir.path()), ["fired", "ignored"]);
    vault.mark_devin_review("d1").unwrap();
    assert!(
        hook::devin_stop_as(&vault, &input(true), None, client)
            .unwrap()
            .is_none()
    );
    assert_eq!(reminders(dir.path()), ["fired", "ignored"]);
}

/// Opening the vault prunes again once the day it last pruned is past. That it prunes at
/// most once a day is `pruning_runs_at_most_once_per_day` in `src/vault/events.rs`, with a
/// pinned day: here the real day would make the result depend on midnight UTC passing
/// between the two opens.
#[test]
fn retention_runs_on_open_once_the_recorded_day_is_past() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "INSERT INTO events (at, kind) VALUES ('2000-01-01T00:00:00Z', 'search');
             UPDATE vault_meta SET value = '2000-01-01' WHERE key = 'events_pruned_on';",
        )
        .unwrap();
    drop(Vault::open(&db).unwrap());
    assert!(events(&db, None).is_empty());
}

#[test]
fn export_does_not_include_events() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    mcp(&db, &[("search_skills", json!({"query": "zebra-marker"}))]);
    assert_eq!(events(&db, Some("search")).len(), 1);
    let out = support::run(&db, &["export"]);
    assert!(out.status.success());
    let text = support::stdout(&out);
    assert!(
        !text.contains("zebra-marker") && !text.contains("events"),
        "{text}"
    );
    let json: Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("events").is_none());
}

#[test]
fn a_failure_to_record_never_breaks_the_operation() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE events;")
        .unwrap();
    assert_eq!(
        mcp(&db, &[("search_skills", json!({"query": "sqlite"}))]),
        [false]
    );
    let out = support::run(&db, &["search", "sqlite"]);
    assert!(out.status.success(), "{}", support::stderr(&out));
    assert!(support::stdout(&out).contains("sqlite-busy-timeout"));
    assert!(support::stderr(&out).contains("could not record search event"));
    assert!(hook_cmd(&db, &["session-start"], "").status.success());
}

#[test]
fn opening_the_vault_drops_events_past_the_retention() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "DELETE FROM vault_meta;
         INSERT INTO events (at, kind) VALUES ('2000-01-01T00:00:00Z', 'search');
         INSERT INTO events (kind) VALUES ('search');",
    )
    .unwrap();
    drop(conn);
    drop(Vault::open(&db).unwrap());
    let kept = events(&db, None);
    assert_eq!(kept.len(), 1, "{kept:?}");
}

#[test]
fn opencode_plugin_hooks_record_delegations_reminders_and_suggestions_as_opencode() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_skill(dir.path());
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("reviewer").publish_agent(&mut vault);
    drop(vault);
    let delegate = |project: &str, input: Value| {
        let out = hook_cmd(
            &db,
            &["delegation", "--client", "opencode", "--project", project],
            &input.to_string(),
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        assert!(out.stdout.is_empty());
    };

    delegate("p0", json!({"agent": "reviewer", "session": "o1"}));
    delegate(
        "p0",
        json!({"agent": "sv-low", "model": "ollama/llama3", "session": "o1"}),
    );
    delegate("p0", json!({"agent": "general"}));
    let seen: Vec<_> = events(&db, Some("delegation"))
        .iter()
        .map(|e| {
            (
                e.client.clone().unwrap(),
                e.subject.clone().unwrap(),
                e.version,
                e.session.clone(),
                e.data.clone(),
            )
        })
        .collect();
    let opencode = || "opencode".to_owned();
    assert_eq!(
        seen,
        [
            (
                opencode(),
                "reviewer".to_owned(),
                Some(1),
                Some("o1".to_owned()),
                json!({"source": "vault", "level": "mid", "model": null})
            ),
            (
                opencode(),
                "sv-low".to_owned(),
                None,
                Some("o1".to_owned()),
                json!({"source": "builtin", "level": "low", "model": "ollama/llama3"})
            ),
            (
                opencode(),
                "general".to_owned(),
                None,
                None,
                json!({"source": "other", "level": null, "model": null})
            ),
        ]
    );

    // The plugin's delegations count toward promotion like Claude Code's.
    for project in 1..6 {
        for _ in 0..3 {
            delegate(&format!("p{project}"), json!({"agent": "reviewer"}));
        }
    }
    delegate("p0", json!({"agent": "reviewer"}));
    delegate("p0", json!({"agent": "reviewer"}));
    let status = Vault::open(&db).unwrap().promotion_status().unwrap();
    let ids: Vec<_> = status.candidates.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["reviewer"], "{status:?}");

    let out = hook_cmd(
        &db,
        &["reminder", "--client", "opencode", "--project", "p0"],
        &json!({"session": "o1"}).to_string(),
    );
    assert!(out.status.success() && out.stdout.is_empty());
    let reminder = only(events(&db, Some("review_reminder")));
    assert_eq!(reminder.client.as_deref(), Some("opencode"));
    assert_eq!(reminder.session.as_deref(), Some("o1"));
    assert_eq!(reminder.data, json!({"state": "fired"}));

    // `hook prompt --client opencode` prints what the plugin has always read and
    // records the suggestion as OpenCode's.
    let prompt = json!({"session_id": "o1", "prompt": "sqlite database locked concurrent writers"})
        .to_string();
    let as_opencode = hook_cmd(
        &db,
        &["prompt", "--client", "opencode", "--project", "p0"],
        &prompt,
    );
    let as_claude = hook_cmd(&db, &["prompt", "--project", "p0"], &prompt);
    assert!(as_opencode.status.success(), "{as_opencode:?}");
    assert_eq!(as_opencode.stdout, as_claude.stdout);
    let printed: Value = serde_json::from_slice(&as_opencode.stdout).unwrap();
    assert_eq!(
        printed["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let suggests = events(&db, Some("suggest"));
    let clients: Vec<_> = suggests.iter().map(|e| e.client.as_deref()).collect();
    assert_eq!(clients, [Some("opencode"), Some("claude-code")]);
    assert_eq!(suggests[0].session.as_deref(), Some("o1"));

    // A payload without an agent is a hook error, never a failed hook.
    let bad = hook_cmd(
        &db,
        &["delegation", "--client", "opencode"],
        &json!({"session": "o1"}).to_string(),
    );
    assert!(bad.status.success());
    let error = only(events(&db, Some("hook_error")));
    assert_eq!(error.client.as_deref(), Some("opencode"));
    assert_eq!(error.data["hook"], "delegation");
}
