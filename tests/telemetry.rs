//! `telemetry summary` / `telemetry events` and the MCP `usage_report` tool.

mod support;

use serde_json::{Value, json};
use std::{io::Write, path::Path, process::Stdio};
use support::{Draft, run, stderr, stdout};

/// A vault in `dir` with events and loads seeded directly (timestamps included).
fn seeded(dir: &Path) -> std::path::PathBuf {
    let db = dir.join("v.db");
    let mut vault = skillvolution::vault::Vault::open(&db).unwrap();
    Draft::new("loaded-skill").publish(&mut vault);
    Draft::new("reported-skill").publish(&mut vault);
    drop(vault);
    let conn = rusqlite::Connection::open(&db).unwrap();
    let ev = |at: &str, kind: &str, client: Option<&str>, subject: Option<&str>, data: Value| {
        conn.execute(
            "INSERT INTO events (at, kind, client, subject, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![at, kind, client, subject, data.to_string()],
        )
        .unwrap();
    };
    let recent = "2099-01-01T00:00:00Z";
    // Far-future rows are inside any window; the 2001 rows are outside every window.
    ev(
        recent,
        "search",
        None,
        None,
        json!({"query": "kubernetes", "total": 0, "ids": []}),
    );
    ev(
        recent,
        "search",
        None,
        None,
        json!({"query": "kubernetes", "total": 0, "ids": []}),
    );
    ev(
        recent,
        "search",
        None,
        None,
        json!({"query": null, "total": 0, "ids": []}),
    );
    ev(
        recent,
        "search",
        None,
        None,
        json!({"query": "cargo", "total": 2, "ids": ["a", "b"]}),
    );
    ev(
        "2001-01-01T00:00:00Z",
        "search",
        None,
        None,
        json!({"query": "old", "total": 0}),
    );
    ev(
        recent,
        "session_start",
        Some("claude-code"),
        None,
        json!({"catalog": 3}),
    );
    ev(
        recent,
        "delegation",
        Some("claude-code"),
        Some("sv-mid"),
        json!({"source": "builtin", "model": "sonnet"}),
    );
    ev(
        recent,
        "delegation",
        Some("claude-code"),
        Some("sv-mid"),
        json!({"source": "builtin", "model": null}),
    );
    ev(
        recent,
        "mcp_error",
        None,
        None,
        json!({"tool": "get_skill", "class": "rejected"}),
    );
    ev(
        recent,
        "hook_error",
        Some("codex"),
        None,
        json!({"hook": "prompt", "class": "io"}),
    );
    conn.execute(
        "INSERT INTO loads (id, version, project, created_at) VALUES
         ('loaded-skill', 1, 'p', '2099-01-01T00:00:00Z'),
         ('reported-skill', 1, 'p', '2099-01-01T00:00:00Z'),
         ('loaded-skill', 1, 'q', '2001-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO outcomes (id, version, result, note, project, created_at)
         VALUES ('reported-skill', 1, 'helped', 'ok', 'p', '2099-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    db
}

fn summary(db: &Path, since: &str) -> Value {
    let output = run(db, &["telemetry", "summary", "--since", since, "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn the_summary_aggregates_searches_loads_usage_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded(dir.path());
    let report = summary(&db, "2090-01-01");

    assert_eq!(report["events"], 9);
    assert_eq!(report["searches"], 4);
    assert_eq!(report["searches_empty"], 3);
    assert_eq!(
        report["searches_without_results"],
        json!([{"name": "kubernetes", "count": 2}]),
        "dropped (null) queries are not listed; searches outside the window are excluded"
    );
    assert_eq!(
        report["loaded_without_report"],
        json!([{"id": "loaded-skill", "version": 1, "unreported": 1, "loaded": 1}]),
        "same rule as outcomes --unreported, limited to the window"
    );
    assert_eq!(
        report["usage"]["sessions_by_client"],
        json!([{"name": "claude-code", "count": 1}])
    );
    assert_eq!(
        report["usage"]["delegations_by_agent"],
        json!([{"name": "sv-mid", "count": 2}])
    );
    assert_eq!(
        report["usage"]["delegations_by_model"],
        json!([{"name": "sonnet", "count": 1}, {"name": "unknown", "count": 1}])
    );
    assert_eq!(report["errors"].as_array().unwrap().len(), 2);
    assert!(report["errors"].as_array().unwrap().contains(
        &json!({"kind": "mcp_error", "source": "get_skill", "class": "rejected", "count": 1})
    ));
}

#[test]
fn since_accepts_dates_and_day_counts_and_rejects_anything_else() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded(dir.path());
    // 36500 days back is before the 2001 rows, so they now count too.
    assert_eq!(summary(&db, "36500d")["searches"], 5);
    assert_eq!(summary(&db, "2000-01-01")["searches"], 5);
    // Default window is 30 days: the seeded far-future rows are in, the 2001 one is not.
    let output = run(&db, &["telemetry", "summary", "--json"]);
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["searches"], 4);
    for bad in ["yesterday", "30", "d", "2026-13-45", "-5d"] {
        let output = run(&db, &["telemetry", "summary", &format!("--since={bad}")]);
        assert!(!output.status.success(), "{bad}");
        assert!(
            stderr(&output).contains("invalid --since"),
            "{bad}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn the_readable_summary_is_concise_and_omits_empty_sections() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded(dir.path());
    let output = run(&db, &["telemetry", "summary", "--since", "2090-01-01"]);
    let text = stdout(&output);
    assert!(
        text.contains("Top queries without results:\n  kubernetes: 2"),
        "{text}"
    );
    assert!(text.contains("loaded-skill v1: 1 of 1 loads"), "{text}");
    assert!(text.contains("mcp_error get_skill rejected: 1"), "{text}");

    let empty = tempfile::tempdir().unwrap();
    let output = run(&empty.path().join("e.db"), &["telemetry", "summary"]);
    let text = stdout(&output);
    assert!(text.contains("Searches: 0, without results: 0"), "{text}");
    assert!(!text.contains("Errors"), "{text}");
}

#[test]
fn events_lists_newest_first_filtered_by_kind_since_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded(dir.path());
    let list = |args: &[&str]| -> Vec<Value> {
        let mut full = vec!["telemetry", "events", "--json"];
        full.extend_from_slice(args);
        let output = run(&db, &full);
        assert!(output.status.success(), "{}", stderr(&output));
        serde_json::from_str(&stdout(&output)).unwrap()
    };
    let searches = list(&["--kind", "search"]);
    assert_eq!(searches.len(), 5);
    assert_eq!(
        searches.last().unwrap()["data"]["query"],
        "old",
        "oldest last"
    );
    assert_eq!(list(&["--kind", "search", "--since", "30d"]).len(), 4);
    assert_eq!(list(&["--limit", "3"]).len(), 3);
    let delegation = &list(&["--kind", "delegation"])[0];
    assert_eq!(delegation["subject"], "sv-mid");
    assert_eq!(delegation["client"], "claude-code");

    let text = stdout(&run(
        &db,
        &[
            "telemetry",
            "events",
            "--kind",
            "delegation",
            "--limit",
            "1",
        ],
    ));
    assert!(text.contains("delegation [claude-code] sv-mid"), "{text}");
}

#[test]
fn the_mcp_usage_report_matches_the_cli_summary_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded(dir.path());
    let before = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let mut child = support::cli()
        .args(["--db", db.to_str().unwrap(), "serve", "--project", "proj"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for (id, arguments) in [
        (1, json!({"since": "2090-01-01"})),
        (2, json!({"since": "soon"})),
        (3, json!({"x": 1})),
    ] {
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
                   "params":{"name":"usage_report","arguments":arguments}})
        )
        .unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let replies: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let report: Value =
        serde_json::from_str(replies[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report, summary(&db, "2090-01-01"));
    assert_eq!(replies[1]["result"]["isError"], true);
    assert_eq!(replies[2]["result"]["isError"], true);
    // Only the two failures are recorded (as mcp_error); the report itself logs nothing.
    let after = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(after, before + 2);
}
