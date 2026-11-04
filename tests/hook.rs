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
use support::{Draft, git_repo};

/// A tool_use line whose id is the tool name, so `tool_result` can answer it.
fn assistant_tool_use(name: &str) -> String {
    json!({"type":"assistant","message":{"role":"assistant","content":[
        {"type":"tool_use","id":name,"name":name,"input":{}}
    ]}})
    .to_string()
}

fn tool_result(tool_use_id: &str, is_error: bool) -> String {
    json!({"type":"user","message":{"role":"user","content":[
        {"type":"tool_result","tool_use_id":tool_use_id,"is_error":is_error,"content":""}
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
    writeln!(file, "{}", assistant_tool_use("Edit")).unwrap();
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
fn work_after_a_review_in_the_same_span_still_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(
        &transcript,
        &[
            assistant_tool_use("Edit"),
            assistant_tool_use("mcp__skillvolution__report_skill_outcome"),
            assistant_tool_use("Edit"),
        ],
    );
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_some());
}

#[test]
fn failed_publish_does_not_count_as_review() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let publish = "mcp__skillvolution__publish_skill";
    for (session, is_error, blocks) in [("s1", true, true), ("s2", false, false)] {
        let transcript = dir.path().join(format!("{session}.jsonl"));
        write_transcript(
            &transcript,
            &[
                assistant_tool_use("Edit"),
                assistant_tool_use(publish),
                tool_result(publish, is_error),
            ],
        );
        let input = stop_input(session, &transcript, false);
        assert_eq!(hook::stop(&vault, &input).unwrap().is_some(), blocks);
    }
}

#[test]
fn a_loaded_skill_without_a_report_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let get = "mcp__skillvolution__get_skill";
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(
        &transcript,
        &[
            user_text("fix the flaky test"),
            assistant_tool_use(get),
            tool_result(get, false),
        ],
    );
    let input = stop_input("s1", &transcript, false);
    assert_eq!(hook::stop(&vault, &input).unwrap(), Some(hook::STOP_REASON));
}

#[test]
fn a_loaded_skill_followed_by_a_report_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let get = "mcp__skillvolution__get_skill";
    let report = "mcp__skillvolution__report_skill_outcome";
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(
        &transcript,
        &[
            assistant_tool_use(get),
            tool_result(get, false),
            assistant_tool_use(report),
            tool_result(report, false),
        ],
    );
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_none());
}

#[test]
fn a_failed_skill_load_is_not_work() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let get = "mcp__skillvolution__get_skill";
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(
        &transcript,
        &[assistant_tool_use(get), tool_result(get, true)],
    );
    let input = stop_input("s1", &transcript, false);
    assert!(hook::stop(&vault, &input).unwrap().is_none());
}

#[test]
fn stop_reason_asks_for_outcomes_of_loaded_skills_and_names_the_evaluation_skill() {
    for expected in [
        "edited files or loaded vault skills",
        "report_skill_outcome for every skill you loaded with get_skill",
        "fresh subagent that follows the skill-evaluation skill",
        "publish_skill",
    ] {
        assert!(hook::STOP_REASON.contains(expected), "{expected}");
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

#[test]
fn session_start_mentions_per_request_suggestions_and_skill_acquisition() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let text = hook::session_start(&vault, None).unwrap();
    assert!(text.contains("suggested with each request"), "{text}");
    assert!(text.contains("`skill-acquisition` skill"), "{text}");
}

// --- prompt retrieval: keywords and the relevance gate -----------------------

#[test]
fn keywords_drop_english_and_spanish_stopwords() {
    assert_eq!(
        hook::keywords("How do I fix the flaky cargo tests in this repo, please?"),
        ["fix", "flaky", "cargo", "tests", "repo"]
    );
    assert_eq!(
        hook::keywords("¿Cómo puedo arreglar los tests de cargo para este proyecto?"),
        ["arreglar", "tests", "cargo", "proyecto"]
    );
    // "Use when" opens every skill description, so it must never be a keyword.
    assert!(hook::keywords("use it when you want").is_empty());
}

#[test]
fn keywords_drop_short_tokens_and_repeats_and_fold_case_and_accents() {
    assert_eq!(
        hook::keywords("to be ok: API api Api, Configuración configuracion"),
        ["api", "configuracion"]
    );
    // A token longer than any useful word (a hash, say) is dropped too.
    assert_eq!(
        hook::keywords(&format!("{} cargo", "f".repeat(64))),
        ["cargo"]
    );
}

#[test]
fn keywords_keep_two_letter_technical_terms() {
    assert_eq!(
        hook::keywords("fix the CI and Go db, to JS"),
        ["fix", "ci", "go", "db", "js"]
    );
}

#[test]
fn a_two_letter_tag_is_suggested_but_never_as_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("module-vendoring")
        .description("Use when dependencies must be vendored for offline builds")
        .tags(&["go"])
        .publish(&mut vault);
    Draft::new("cloud-deploys")
        .description("Use when a deploy to the cloud hangs")
        .tags(&["google-cloud"])
        .publish(&mut vault);
    let skills = hook::relevant_skills(&vault, None, "go modules vendoring").unwrap();
    assert_eq!(ids(&skills), ["module-vendoring"]);
    // "go" must not prefix-match google-cloud, with or without a second hit word.
    let skills = hook::relevant_skills(&vault, None, "go banana").unwrap();
    assert!(!ids(&skills).contains(&"cloud-deploys"), "{skills:?}");
}

#[test]
fn keywords_are_capped_in_first_seen_order() {
    let prompt: Vec<String> = (0..40).map(|i| format!("word{i}")).collect();
    let terms = hook::keywords(&prompt.join(" "));
    assert_eq!(terms.len(), 16);
    assert_eq!(terms, prompt[..16]);
}

/// A vault with three unrelated skills, distinct in id, description, and tags.
fn retrieval_vault(dir: &Path) -> Vault {
    let mut vault = Vault::open(&dir.join("skills.db")).unwrap();
    Draft::new("cargo-flaky-tests")
        .description("Use when cargo test fails intermittently with timeouts")
        .tags(&["rust", "cargo"])
        .publish(&mut vault);
    Draft::new("docker-compose-networking")
        .description("Use when containers cannot reach each other over the compose network")
        .tags(&["docker"])
        .publish(&mut vault);
    Draft::new("postgres-lock-migrations")
        .description("Use when a schema migration locks a large table")
        .tags(&["postgres", "sql"])
        .content("Also seen with kubernetes deployments.")
        .publish(&mut vault);
    vault
}

fn ids(skills: &[skillvolution::vault::SkillMetadata]) -> Vec<&str> {
    skills.iter().map(|skill| skill.id.as_str()).collect()
}

#[test]
fn a_relevant_prompt_suggests_the_matching_skill_first() {
    let dir = tempfile::tempdir().unwrap();
    let vault = retrieval_vault(dir.path());
    let skills =
        hook::relevant_skills(&vault, None, "my cargo tests time out, are they flaky?").unwrap();
    assert_eq!(ids(&skills), ["cargo-flaky-tests"]);
    // The same request in Spanish matches all the same: "fallan" reads as "fail".
    let skills = hook::relevant_skills(&vault, None, "los tests de cargo fallan a veces").unwrap();
    assert_eq!(ids(&skills)[0], "cargo-flaky-tests");
}

#[test]
fn an_unrelated_or_stopword_only_prompt_suggests_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let vault = retrieval_vault(dir.path());
    for prompt in [
        "write a poem about autumn leaves",
        "how do I do this, please?",
        "¿cómo puedo hacer esto para que funcione bien?",
        // One keyword is too little to go on, even when it names a skill.
        "docker",
    ] {
        assert!(
            hook::relevant_skills(&vault, None, prompt)
                .unwrap()
                .is_empty(),
            "{prompt}"
        );
        let input = json!({ "prompt": prompt }).to_string();
        assert!(
            hook::prompt(&vault, None, &input).unwrap().is_none(),
            "{prompt}"
        );
    }
}

#[test]
fn a_match_only_in_the_body_or_one_description_word_does_not_clear_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let vault = retrieval_vault(dir.path());
    // "kubernetes" is only in a body; "large" only in one description.
    for prompt in ["kubernetes pods restarting", "large autumn poem"] {
        assert!(
            hook::relevant_skills(&vault, None, prompt)
                .unwrap()
                .is_empty(),
            "{prompt}"
        );
    }
    // Two description words are enough without an id or tag match.
    let skills =
        hook::relevant_skills(&vault, None, "containers cannot reach the network").unwrap();
    assert_eq!(ids(&skills), ["docker-compose-networking"]);
}

#[test]
fn a_skill_with_net_failures_is_never_suggested() {
    let dir = tempfile::tempdir().unwrap();
    let vault = retrieval_vault(dir.path());
    let prompt = "cargo tests flaky";
    for project in ["proja", "projb"] {
        vault
            .record_outcome(
                "cargo-flaky-tests",
                1,
                "failed",
                "step 2 wrong",
                Some(project),
            )
            .unwrap();
    }
    assert!(
        hook::relevant_skills(&vault, None, prompt)
            .unwrap()
            .is_empty()
    );
    // One helped report brings it back under the threshold.
    vault
        .record_outcome("cargo-flaky-tests", 1, "helped", "worked", Some("projc"))
        .unwrap();
    assert_eq!(
        ids(&hook::relevant_skills(&vault, None, prompt).unwrap()),
        ["cargo-flaky-tests"]
    );
}

#[test]
fn a_skill_reported_not_applicable_is_not_suggested_but_still_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = retrieval_vault(dir.path());
    let prompt = "cargo tests flaky";
    for project in ["proja", "projb", "projc"] {
        vault
            .record_outcome(
                "cargo-flaky-tests",
                1,
                "not_applicable",
                "different problem",
                Some(project),
            )
            .unwrap();
    }
    vault
        .record_outcome("cargo-flaky-tests", 1, "helped", "worked", Some("projd"))
        .unwrap();
    // 3 not_applicable - 1 helped = 2: off the suggestions...
    assert!(
        hook::relevant_skills(&vault, None, prompt)
            .unwrap()
            .is_empty()
    );
    // ...but an explicit search still finds it, ranked as before.
    assert_eq!(
        ids(&vault.search(prompt, None, 20, 0).unwrap().skills)[0],
        "cargo-flaky-tests"
    );
    // A new version (say, a narrowed description) gets a fresh chance.
    Draft::new("cargo-flaky-tests")
        .description("Use when cargo test fails intermittently with timeouts")
        .tags(&["rust", "cargo"])
        .expected_version(1)
        .replaces_proven(true)
        .publish(&mut vault);
    assert_eq!(
        ids(&hook::relevant_skills(&vault, None, prompt).unwrap()),
        ["cargo-flaky-tests"]
    );
}

#[test]
fn at_most_three_skills_are_suggested() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    for id in ["rust-one", "rust-two", "rust-three", "rust-four"] {
        Draft::new(id)
            .description("Use when the borrow checker rejects a lifetime")
            .tags(&["rust"])
            .publish(&mut vault);
    }
    let skills = hook::relevant_skills(&vault, None, "rust lifetime errors").unwrap();
    assert_eq!(skills.len(), 3, "{:?}", ids(&skills));
}

#[test]
fn prompt_context_lists_matches_with_outcomes_and_the_instruction() {
    let dir = tempfile::tempdir().unwrap();
    let vault = retrieval_vault(dir.path());
    let input = json!({"session_id": "s1", "prompt": "cargo tests are flaky"}).to_string();
    let text = hook::prompt(&vault, None, &input).unwrap().unwrap();
    assert!(
        text.contains(
            "- cargo-flaky-tests v1: Use when cargo test fails intermittently with timeouts [helped 0, failed 0]\n"
        ),
        "{text}"
    );
    assert!(
        text.ends_with(
            "Before starting, call get_skill for the best match and tell the user in one line \
             which vault skill you are applying (id and version), or that none fits. After \
             applying it, call report_skill_outcome."
        ),
        "{text}"
    );
    // A payload without a prompt field suggests nothing.
    let input = json!({"session_id": "s1"}).to_string();
    assert!(hook::prompt(&vault, None, &input).unwrap().is_none());
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
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
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
fn devin_work_after_a_reviewed_span_blocks_again() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let input = devin_stop_input("s1", false);
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
    hook::devin_tool_use(
        &vault,
        &devin_tool("s1", "mcp__skillvolution__publish_skill"),
    )
    .unwrap();
    assert!(hook::devin_stop(&vault, &input).unwrap().is_none());
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
    assert!(hook::devin_stop(&vault, &input).unwrap().is_some());
}

#[test]
fn devin_work_after_a_review_prompted_by_a_block_blocks_again() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    let input = devin_stop_input("s1", false);
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
    assert!(hook::devin_stop(&vault, &input).unwrap().is_some());
    hook::devin_tool_use(
        &vault,
        &devin_tool("s1", "mcp__skillvolution__report_skill_outcome"),
    )
    .unwrap();
    assert!(
        hook::devin_stop(&vault, &devin_stop_input("s1", true))
            .unwrap()
            .is_none()
    );
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
    assert!(hook::devin_stop(&vault, &input).unwrap().is_some());
}

#[test]
fn a_loaded_skill_marks_work_for_every_flag_tracked_client() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    // Each client's own MCP tool naming, with its own work tools.
    for (session, tool, work_tools) in [
        (
            "devin",
            "mcp__skillvolution__get_skill",
            hook::DEVIN_WORK_TOOLS,
        ),
        (
            "codex",
            "mcp__skillvolution__get_skill",
            hook::CODEX_WORK_TOOLS,
        ),
        (
            "gemini",
            "mcp_skillvolution_get_skill",
            hook::GEMINI_WORK_TOOLS,
        ),
        (
            "cursor",
            "MCP:skillvolution_get_skill",
            hook::CURSOR_WORK_TOOLS,
        ),
    ] {
        hook::tool_use(&vault, &devin_tool(session, tool), work_tools).unwrap();
        let input = devin_stop_input(session, false);
        assert_eq!(
            hook::devin_stop(&vault, &input).unwrap(),
            Some(hook::STOP_REASON),
            "{session}"
        );
    }
}

#[test]
fn a_loaded_skill_followed_by_a_report_is_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "mcp__skillvolution__get_skill")).unwrap();
    hook::devin_tool_use(
        &vault,
        &devin_tool("s1", "mcp__skillvolution__report_skill_outcome"),
    )
    .unwrap();
    assert!(
        hook::devin_stop(&vault, &devin_stop_input("s1", false))
            .unwrap()
            .is_none()
    );
    // Searching is not loading: it owes no report.
    hook::devin_tool_use(
        &vault,
        &devin_tool("s1", "mcp__skillvolution__search_skills"),
    )
    .unwrap();
    assert!(
        hook::devin_stop(&vault, &devin_stop_input("s1", false))
            .unwrap()
            .is_none()
    );
}

#[test]
fn devin_unrelated_tools_and_sessions_leave_flags_alone() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "read")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "run_subagent")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("other", "write")).unwrap();
    let input = devin_stop_input("s1", false);
    assert!(hook::devin_stop(&vault, &input).unwrap().is_none());
}

#[test]
fn devin_session_end_clears_the_flags() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
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
fn cli_hook_stop_feeds_additional_context_when_blocking_and_prints_nothing_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    Vault::open(&db).unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_transcript(&transcript, &[assistant_tool_use("Edit")]);
    let input = stop_input("s1", &transcript, false);

    // additionalContext (stdout, exit 0) still blocks the stop, but the transcript
    // shows it as feedback rather than a hook-error notification.
    let blocked = run_hook(&db, &["stop"], &input);
    assert!(blocked.status.success());
    assert!(blocked.stderr.is_empty());
    let decision: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["hookEventName"], "Stop");
    assert!(
        decision["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Skillvolution"),
        "{decision}"
    );

    let clean = run_hook(&db, &["stop"], &input);
    assert!(clean.status.success());
    assert!(clean.stdout.is_empty());
    assert!(clean.stderr.is_empty());
}

/// Asserts a hook failed open: exit 0, nothing on stdout, the error on stderr.
fn assert_failed_open(output: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(output.stdout.is_empty());
    assert!(stderr.starts_with("skillvolution hook: "), "{stderr}");
}

#[test]
fn cli_hook_stop_with_malformed_stdin_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    assert_failed_open(&run_hook(&db, &["stop"], "not json"));
    assert_failed_open(&run_hook(&db, &["stop", "--client", "devin"], "{"));
}

#[test]
fn cli_hook_session_start_with_an_unopenable_db_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    // The database's parent directory is a regular file, so it cannot be created.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "").unwrap();
    let db = blocker.join("skills.db");
    assert_failed_open(&run_hook(&db, &["session-start"], ""));
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
fn cli_hook_session_start_prints_the_catalog_as_additional_context() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("rust-tests").publish(&mut vault);
    drop(vault);
    let output = run_hook_session_start(&db, None, dir.path(), &[]);
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let context = json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("rust-tests"));
    assert!(context.contains("tell the user in one line"));
}

// --- CLI: hook prompt ---------------------------------------------------------

/// `retrieval_vault` at `dir/skills.db`, closed so the CLI can open it.
fn retrieval_db(dir: &Path) -> std::path::PathBuf {
    drop(retrieval_vault(dir));
    dir.join("skills.db")
}

#[test]
fn cli_hook_prompt_prints_each_clients_context_injection_json() {
    let dir = tempfile::tempdir().unwrap();
    let db = retrieval_db(dir.path());
    let input = json!({"session_id": "s1", "prompt": "cargo tests are flaky"}).to_string();
    for (client, event) in [
        (None, "UserPromptSubmit"),
        (Some("claude-code"), "UserPromptSubmit"),
        (Some("codex"), "UserPromptSubmit"),
        (Some("devin"), "UserPromptSubmit"),
        (Some("gemini"), "BeforeAgent"),
    ] {
        let mut args = vec!["prompt", "--project", "proja"];
        args.extend(client.iter().flat_map(|client| ["--client", client]));
        let output = run_hook(&db, &args, &input);
        assert!(output.status.success(), "{client:?}");
        assert!(output.stderr.is_empty(), "{client:?}");
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["hookSpecificOutput"]["hookEventName"], event);
        let context = json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("- cargo-flaky-tests v1: "), "{context}");
        assert!(context.contains("call get_skill"), "{context}");
    }
}

#[test]
fn cli_hook_prompt_prints_nothing_without_a_relevant_match() {
    let dir = tempfile::tempdir().unwrap();
    let db = retrieval_db(dir.path());
    for prompt in ["write a poem about autumn leaves", "hola, ¿qué tal?"] {
        let input = json!({ "prompt": prompt }).to_string();
        let output = run_hook(&db, &["prompt", "--project", "proja"], &input);
        assert!(output.status.success());
        assert!(output.stdout.is_empty(), "{prompt}");
        assert!(output.stderr.is_empty(), "{prompt}");
    }
}

#[test]
fn cli_hook_prompt_is_scoped_by_project() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("widget-cache-reset")
        .tags(&["widget"])
        .scope("proja")
        .publish(&mut vault);
    drop(vault);
    let input = json!({"prompt": "reset the widget cache"}).to_string();
    let in_a = run_hook(&db, &["prompt", "--project", "proja"], &input);
    assert!(String::from_utf8_lossy(&in_a.stdout).contains("widget-cache-reset"));
    let in_b = run_hook(&db, &["prompt", "--project", "projb"], &input);
    assert!(in_b.status.success());
    assert!(in_b.stdout.is_empty());
}

#[test]
fn cli_hook_prompt_fails_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = retrieval_db(dir.path());
    assert_failed_open(&run_hook(
        &db,
        &["prompt", "--project", "proja"],
        "not json",
    ));
    // Cursor's beforeSubmitPrompt has no way to add context.
    let input = json!({"prompt": "cargo tests are flaky"}).to_string();
    assert_failed_open(&run_hook(
        &db,
        &["prompt", "--project", "proja", "--client", "cursor"],
        &input,
    ));
}

#[test]
fn cli_hook_prompt_reads_the_whole_payload_even_when_it_fails_open() {
    // Larger than any pipe buffer: a hook that bailed out before reading stdin would
    // leave this write blocked until it exited, then fail it with a broken pipe.
    let dir = tempfile::tempdir().unwrap();
    let db = retrieval_db(dir.path());
    let input = json!({"prompt": "cargo ".repeat(200_000)}).to_string();
    assert_failed_open(&run_hook(
        &db,
        &["prompt", "--project", "proja", "--client", "cursor"],
        &input,
    ));
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

#[test]
fn cli_devin_stop_prints_a_block_decision_once_per_unreviewed_span() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let vault = Vault::open(&db).unwrap();
    hook::devin_tool_use(&vault, &devin_tool("s1", "write")).unwrap();
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

    let tool = run_hook(&db, &["tool-use"], &devin_tool("s1", "write"));
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

#[test]
fn a_compound_tag_or_hyphenated_id_counts_as_one_match() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("spreadsheet-fills")
        .description("Use when a spreadsheet saves no cell fill")
        .tags(&["conditional-format", "xlsx"])
        .publish(&mut vault);
    Draft::new("kafka-consumer-lag")
        .description("Use when offsets stall after a rebalance")
        .publish(&mut vault);
    // Two keywords inside the one tag, or the one id, name one thing: a single match.
    for prompt in ["broken conditional format", "kafka consumer setup"] {
        assert!(
            hook::relevant_skills(&vault, None, prompt)
                .unwrap()
                .is_empty(),
            "{prompt}"
        );
    }
    // A match in a second place (here description words) makes two.
    let skills =
        hook::relevant_skills(&vault, None, "conditional format loses the cell fill").unwrap();
    assert_eq!(ids(&skills), ["spreadsheet-fills"]);
    let skills = hook::relevant_skills(&vault, None, "kafka offsets stall").unwrap();
    assert_eq!(ids(&skills), ["kafka-consumer-lag"]);
    // Spanish "forma" (a filler word) never reaches the tag word "format".
    let prompt = "subir skills de forma manual con un comando";
    assert!(
        hook::relevant_skills(&vault, None, prompt)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn generic_developer_words_never_count_toward_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let vault = retrieval_vault(dir.path());
    // "tests" is a whole word of cargo-flaky-tests' id, and "test" of its
    // description, but neither says which procedure applies; nor does "fix" or "run".
    for prompt in [
        "fix the test so the tests run",
        "write a test file for cargo",
    ] {
        assert!(
            hook::relevant_skills(&vault, None, prompt)
                .unwrap()
                .is_empty(),
            "{prompt}"
        );
    }
    // Two specific words still clear the gate, generic ones alongside or not.
    let skills = hook::relevant_skills(&vault, None, "fix the flaky cargo tests").unwrap();
    assert_eq!(ids(&skills), ["cargo-flaky-tests"]);
}

#[test]
fn spanish_prompts_reach_english_skills_through_the_dictionary() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("container-restart-loop")
        .description("Use when a container keeps restarting after a deploy")
        .tags(&["docker"])
        .publish(&mut vault);
    // No word in common with the skill: "contenedor", "reinicia", and "despliegue"
    // translate to container, restart, and deploy, inflected forms included.
    for prompt in [
        "el contenedor se reinicia tras el despliegue",
        "los contenedores se reinician al desplegar",
        "reiniciando contenedores",
    ] {
        assert_eq!(
            ids(&hook::relevant_skills(&vault, None, prompt).unwrap()),
            ["container-restart-loop"],
            "{prompt}"
        );
    }
    // The keywords themselves stay as typed; translation happens on lookup.
    assert_eq!(
        hook::keywords("el contenedor se reinicia"),
        ["contenedor", "reinicia"]
    );
}

#[test]
fn a_translated_spanish_word_no_longer_prefix_matches_english() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("generation-parent-sync")
        .description("Use when the parent generation drifts")
        .publish(&mut vault);
    // "genera" (generates) used to prefix "generation" and "pare" (stops) "parent";
    // translated, they are "generate" and "stop", whole words only.
    let prompt = "genera esto y que pare";
    assert!(
        hook::relevant_skills(&vault, None, prompt)
            .unwrap()
            .is_empty()
    );
    // An untranslated English prompt still matches by prefix.
    let skills = hook::relevant_skills(&vault, None, "parent generation drift").unwrap();
    assert_eq!(ids(&skills), ["generation-parent-sync"]);
}

#[test]
fn weak_matches_far_below_the_best_one_are_not_suggested() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("postgres-lock-migrations")
        .description("Use when a postgres schema migration locks a large table")
        .tags(&["postgres", "migrations"])
        .publish(&mut vault);
    Draft::new("table-render-speed")
        .description("Use when a large table renders slowly")
        .publish(&mut vault);
    // Both are candidates and both clear the gate on "large" and "table", but only
    // the migration skill matches the gist of the prompt.
    let prompt = "postgres migration locks a large table";
    let candidates = vault
        .search(&hook::keywords(prompt).join(" "), None, 5, 0)
        .unwrap();
    assert_eq!(
        ids(&candidates.skills),
        ["postgres-lock-migrations", "table-render-speed"]
    );
    assert_eq!(
        ids(&hook::relevant_skills(&vault, None, prompt).unwrap()),
        ["postgres-lock-migrations"]
    );
    // On its own topic the other one is the best match and is suggested alone.
    assert_eq!(
        ids(&hook::relevant_skills(&vault, None, "large table renders slowly").unwrap()),
        ["table-render-speed"]
    );
}

#[test]
fn two_forms_of_one_word_count_as_one_keyword() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&dir.path().join("skills.db")).unwrap();
    Draft::new("blue-green-deployment")
        .description("Use when a deploy must switch traffic without downtime")
        .tags(&["rollout"])
        .publish(&mut vault);
    // "deploy" and "deployment" match two places, but name one concept.
    assert!(
        hook::relevant_skills(&vault, None, "deploy deployment")
            .unwrap()
            .is_empty()
    );
    let skills = hook::relevant_skills(&vault, None, "deployment rollout").unwrap();
    assert_eq!(ids(&skills), ["blue-green-deployment"]);
}
