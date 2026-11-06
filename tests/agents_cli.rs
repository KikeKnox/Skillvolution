mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use support::{BODY, run, stderr, stdout};

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

fn agent_text(extra: &str) -> String {
    format!(
        "---\nname: reviewer\ndescription: Use when a diff needs review\ntier: mid\ntools: [read, shell]\n{extra}---\n{BODY}"
    )
}

fn write(dir: &Path, rel: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn publish_agent_then_get_round_trips_through_replace() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = write(dir.path(), "reviewer/AGENT.md", &agent_text(""));

    let published = run(&db, &["publish", file.to_str().unwrap()]);
    assert_success(&published);
    assert_eq!(
        stdout(&published),
        "Published reviewer v1 (global, agent).\n"
    );

    let got = run(&db, &["get", "reviewer"]);
    assert_success(&got);
    let text = stdout(&got);
    assert!(text.contains("role: subagent\ntier: mid\ntools: [read, shell]\n"));
    assert!(text.contains("## Output contract"));

    let again = write(dir.path(), "again/AGENT.md", &text);
    let replaced = run(
        &db,
        &[
            "publish",
            again.to_str().unwrap(),
            "--replace",
            "--id",
            "reviewer",
        ],
    );
    assert_success(&replaced);
    assert_eq!(
        stdout(&replaced),
        "Published reviewer v2 (global, agent).\n"
    );
    assert_eq!(stdout(&run(&db, &["get", "reviewer"])), text);
}

#[test]
fn directory_publish_handles_skills_and_agents_and_list_filters_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let root = dir.path().join("pack");
    write(&root, "reviewer/AGENT.md", &agent_text(""));
    write(
        &root,
        "plain-skill/SKILL.md",
        "---\nname: plain-skill\ndescription: Use when testing skills\n---\n## When to use\nx\n## Procedure\n1. y\n## Pitfalls\n- z\n## Verification\nv\n",
    );

    let published = run(&db, &["publish", root.to_str().unwrap()]);
    assert_success(&published);
    let out = stdout(&published);
    assert!(out.contains("[published] plain-skill v1 (global)"), "{out}");
    assert!(
        out.contains("[published] reviewer v1 (global, agent)"),
        "{out}"
    );
    assert!(out.contains("2 published, 0 skipped, 0 rejected"), "{out}");

    let agents = stdout(&run(&db, &["list", "--kind", "agent"]));
    assert!(agents.contains("reviewer v1"), "{agents}");
    assert!(!agents.contains("plain-skill"), "{agents}");
    let skills = stdout(&run(&db, &["list"]));
    assert!(skills.contains("plain-skill v1"), "{skills}");
    assert!(!skills.contains("reviewer"), "{skills}");
    let found = stdout(&run(&db, &["search", "diff review"]));
    assert!(!found.contains("reviewer"), "{found}");
}

#[test]
fn agent_publish_rejects_primary_role_and_missing_output_contract() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");

    let primary = write(dir.path(), "a/AGENT.md", &agent_text("role: primary\n"));
    let out = run(&db, &["publish", primary.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("not supported yet"), "{err}");
    assert!(err.contains("An AGENT.md looks like this"), "{err}");

    let bad = agent_text("").replace("## Output contract\nA list of findings.\n", "");
    let missing = write(dir.path(), "b/AGENT.md", &bad);
    let out = run(&db, &["publish", missing.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("Output contract"), "{}", stderr(&out));
}

#[test]
fn directory_publish_reports_a_kind_conflict_as_rejected_not_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let first = write(
        dir.path(),
        "one/x/AGENT.md",
        &agent_text("").replace("reviewer", "x"),
    );
    assert_success(&run(&db, &["publish", first.to_str().unwrap()]));
    write(
        dir.path(),
        "two/x/SKILL.md",
        "---\nname: x\ndescription: Use when testing skills\n---\n## When to use\nx\n## Procedure\n1. y\n## Pitfalls\n- z\n## Verification\nv\n",
    );
    let out = run(&db, &["publish", dir.path().join("two").to_str().unwrap()]);
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("[rejected]") && text.contains("already exists as an agent"),
        "{text}"
    );
    assert!(!text.contains("[skipped]"), "{text}");
}

#[test]
fn agents_stay_out_of_the_search_index_and_doctor_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = write(dir.path(), "reviewer/AGENT.md", &agent_text(""));
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let conn = rusqlite::Connection::open(&db).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM skills_fts", [], |r| r.get(0))
        .unwrap();
    let skills: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skills WHERE kind = 'skill'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((rows, skills), (0, 0));
    assert!(stdout(&run(&db, &["list", "--kind", "agent"])).contains("reviewer v1"));
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let doctor = Command::new(env!("CARGO_BIN_EXE_skillvolution"))
        .arg("--db")
        .arg(&db)
        .arg("doctor")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let text = stdout(&doctor);
    assert!(
        text.contains("[ok] vault: search index: 0 skill(s) indexed, no orphans"),
        "{text}"
    );
}

#[test]
fn export_with_an_agent_is_version_2_and_skill_only_stays_version_1() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let skill = write(
        dir.path(),
        "s/SKILL.md",
        "---\nname: s\ndescription: Use when testing skills\n---\n## When to use\nx\n## Procedure\n1. y\n## Pitfalls\n- z\n## Verification\nv\n",
    );
    assert_success(&run(&db, &["publish", skill.to_str().unwrap()]));
    let v1: serde_json::Value = serde_json::from_str(&stdout(&run(&db, &["export"]))).unwrap();
    assert_eq!(v1["version"], 1);
    let file = write(dir.path(), "reviewer/AGENT.md", &agent_text(""));
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let out = stdout(&run(&db, &["export"]));
    let v2: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v2["version"], 2);
    let other = dir.path().join("other.db");
    let export = write(dir.path(), "export.json", &out);
    assert_success(&run(&other, &["import", export.to_str().unwrap()]));
    let conn = rusqlite::Connection::open(&other).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM skills_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

const SKILL_BODY: &str =
    "## When to use\nx\n## Procedure\n1. y\n## Pitfalls\n- z\n## Verification\nv\n";

#[test]
fn list_shows_tiers_and_says_so_when_there_is_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    assert_eq!(
        stdout(&run(&db, &["list", "--kind", "agent"])),
        "no agents\n"
    );
    let file = write(dir.path(), "reviewer/AGENT.md", &agent_text(""));
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let listed = stdout(&run(&db, &["list", "--kind", "agent"]));
    assert!(
        listed.contains("reviewer v1 [global] (tier mid"),
        "{listed}"
    );
}

#[test]
fn get_prints_builtins_and_show_labels_agents_readably() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let builtin = run(&db, &["get", "sv-mid"]);
    assert_success(&builtin);
    assert!(
        stdout(&builtin).contains("name: sv-mid"),
        "{}",
        stdout(&builtin)
    );
    let file = write(dir.path(), "reviewer/AGENT.md", &agent_text("skills: []\n"));
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let shown = stdout(&run(&db, &["show", "reviewer"]));
    assert!(shown.contains("reviewer v1 (agent, published"), "{shown}");
    assert!(
        shown.contains("tier: mid\n") && shown.contains("tools: read, shell\n"),
        "{shown}"
    );
    assert!(
        !shown.contains("{\"") && !shown.contains("\"tier\""),
        "{shown}"
    );
}

#[test]
fn forced_purge_names_the_agents_that_still_list_the_skill() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let skill = write(
        dir.path(),
        "helper/SKILL.md",
        &format!("---\nname: helper\ndescription: Use when testing skills\n---\n{SKILL_BODY}"),
    );
    assert_success(&run(&db, &["publish", skill.to_str().unwrap()]));
    let agent = write(
        dir.path(),
        "reviewer/AGENT.md",
        &agent_text("skills: [helper]\n").replace("[read, shell]", "[read, shell, mcp]"),
    );
    assert_success(&run(&db, &["publish", agent.to_str().unwrap()]));
    assert!(!run(&db, &["purge", "helper"]).status.success());
    let purged = run(&db, &["purge", "helper", "--force"]);
    assert_success(&purged);
    let out = stdout(&purged);
    assert!(
        out.contains("reviewer") && out.contains("will fail to load it"),
        "{out}"
    );
}

#[test]
fn purging_the_last_published_version_names_agents_even_with_superseded_rows_left() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = skillvolution::vault::Vault::open(&db).unwrap();
    support::Draft::new("helper").publish(&mut vault);
    support::Draft::new("helper")
        .expected_version(1)
        .publish(&mut vault);
    drop(vault);
    // A legacy superseded row (schema v5 maps old draft/rejected ones this way).
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE revisions SET status = 'superseded' WHERE id = 'helper' AND version = 1",
            [],
        )
        .unwrap();
    let agent = write(
        dir.path(),
        "reviewer/AGENT.md",
        &agent_text("skills: [helper]\n").replace("[read, shell]", "[read, shell, mcp]"),
    );
    assert_success(&run(&db, &["publish", agent.to_str().unwrap()]));
    assert!(
        !run(&db, &["purge", "helper", "--version", "2"])
            .status
            .success()
    );
    let purged = run(&db, &["purge", "helper", "--version", "2", "--force"]);
    assert_success(&purged);
    assert!(
        stdout(&purged).contains("agent(s) reviewer still list"),
        "{}",
        stdout(&purged)
    );
}

#[test]
fn forced_purge_also_names_deprecated_agents() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let skill = write(
        dir.path(),
        "helper/SKILL.md",
        &format!("---\nname: helper\ndescription: Use when testing skills\n---\n{SKILL_BODY}"),
    );
    assert_success(&run(&db, &["publish", skill.to_str().unwrap()]));
    let agent = write(
        dir.path(),
        "reviewer/AGENT.md",
        &agent_text("skills: [helper]\n").replace("[read, shell]", "[read, shell, mcp]"),
    );
    assert_success(&run(&db, &["publish", agent.to_str().unwrap()]));
    assert_success(&run(&db, &["deprecate", "reviewer"]));
    let refused = run(&db, &["purge", "helper"]);
    assert!(!refused.status.success() && stderr(&refused).contains("reviewer"));
    let purged = run(&db, &["purge", "helper", "--force"]);
    assert_success(&purged);
    assert!(
        stdout(&purged).contains("agent(s) reviewer still list"),
        "{}",
        stdout(&purged)
    );
}

#[test]
fn get_keeps_a_hash_in_a_skill_description_through_a_replace_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let skill = write(
        dir.path(),
        "hashy/SKILL.md",
        &format!("---\nname: hashy\ndescription: \"Use when a # sign matters\"\n---\n{SKILL_BODY}"),
    );
    assert_success(&run(&db, &["publish", skill.to_str().unwrap()]));
    let got = stdout(&run(&db, &["get", "hashy"]));
    assert!(
        got.contains("description: \"Use when a # sign matters\"\n"),
        "{got}"
    );
    let again = write(dir.path(), "again/SKILL.md", &got);
    assert_success(&run(
        &db,
        &[
            "publish",
            again.to_str().unwrap(),
            "--id",
            "hashy",
            "--replace",
        ],
    ));
    assert_eq!(stdout(&run(&db, &["get", "hashy"])), got);
}

#[test]
fn import_rejects_unknown_agent_meta_keys_but_reading_tolerates_them() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = write(dir.path(), "reviewer/AGENT.md", &agent_text(""));
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let export = stdout(&run(&db, &["export"]));
    let mut doc: serde_json::Value = serde_json::from_str(&export).unwrap();
    doc["skills"][0]["revisions"][0]["meta"]["future"] = serde_json::json!(true);
    let path = write(dir.path(), "future.json", &doc.to_string());
    let fresh = dir.path().join("fresh.db");
    let out = run(&fresh, &["import", path.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("unknown agent meta key `future`"),
        "{}",
        stderr(&out)
    );
    // A row written by a newer version still reads.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE revisions SET meta = json_set(meta, '$.future', 1) WHERE id = 'reviewer'",
            [],
        )
        .unwrap();
    assert_success(&run(&db, &["get", "reviewer"]));
    assert_success(&run(&db, &["show", "reviewer"]));
}
