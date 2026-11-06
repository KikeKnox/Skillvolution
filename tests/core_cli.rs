#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::Vault;
use std::{fs, path::Path, process::Output};
use support::Draft;

fn run(db: &std::path::Path, args: &[&str]) -> Output {
    support::cli()
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn show_exposes_a_published_revision() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("example")
        .description("Use when testing")
        .propose(&mut vault);
    drop(vault);

    let shown = run(&db, &["show", "example", "--version", "1"]);
    assert_success(&shown);
    let shown_out = String::from_utf8_lossy(&shown.stdout);
    assert!(shown_out.contains("example v1"));
    assert!(shown_out.contains("published"));
    assert!(shown_out.contains("Observed / Tried / Result"));
    assert!(shown_out.contains("+description"));
    // The evaluator verdict is persisted as the review note and surfaced here.
    assert!(shown_out.contains("keep global"), "{shown_out}");

    let shown_json = run(&db, &["show", "example", "--version", "1", "--json"]);
    assert_success(&shown_json);
    let shown_json: serde_json::Value = serde_json::from_slice(&shown_json.stdout).unwrap();
    assert_eq!(shown_json["id"], "example");
    assert_eq!(shown_json["status"], "published");
    assert!(
        shown_json["review_note"]
            .as_str()
            .is_some_and(|note| note.starts_with("keep global: ")),
        "{shown_json}"
    );
    assert!(shown_json["reviewed_at"].is_string(), "{shown_json}");
}

#[test]
fn deprecate_undeprecate_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("published-skill").publish(&mut vault);
    drop(vault);

    let deprecated = run(&db, &["deprecate", "published-skill"]);
    assert_success(&deprecated);
    let vault = Vault::open(&db).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 0);
    drop(vault);

    let undeprecated = run(&db, &["undeprecate", "published-skill"]);
    assert_success(&undeprecated);
    let vault = Vault::open(&db).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 1);
}

#[test]
fn outcomes_table_json_and_failing_filter() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    let helped = Draft::new("helped-skill").publish(&mut vault);
    let failed = Draft::new("failed-skill").publish(&mut vault);
    vault
        .record_outcome("helped-skill", helped, "helped", "worked", None)
        .unwrap();
    vault
        .record_outcome("failed-skill", failed, "failed", "broke", None)
        .unwrap();
    drop(vault);

    let summary = run(&db, &["outcomes", "--json"]);
    assert_success(&summary);
    let summary: serde_json::Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(summary.as_array().unwrap().len(), 2);

    let failing = run(&db, &["outcomes", "--failing", "--json"]);
    assert_success(&failing);
    let failing: serde_json::Value = serde_json::from_slice(&failing.stdout).unwrap();
    assert_eq!(failing.as_array().unwrap().len(), 1);
    assert_eq!(failing[0]["id"], "failed-skill");

    let log = run(&db, &["outcomes", "failed-skill", "--json"]);
    assert_success(&log);
    let log: serde_json::Value = serde_json::from_slice(&log.stdout).unwrap();
    assert_eq!(log[0]["note"], "broke");

    let table = run(&db, &["outcomes"]);
    assert_success(&table);
    assert!(String::from_utf8_lossy(&table.stdout).contains("helped-skill"));
}

#[test]
fn outcomes_shows_loads_and_lists_unreported_ones() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    let reported = Draft::new("reported-skill").publish(&mut vault);
    let forgotten = Draft::new("forgotten-skill").publish(&mut vault);
    Draft::new("unloaded-skill").publish(&mut vault);
    vault.record_load("reported-skill", reported, None).unwrap();
    vault
        .record_outcome("reported-skill", reported, "helped", "worked", None)
        .unwrap();
    vault
        .record_load("forgotten-skill", forgotten, Some("proja"))
        .unwrap();
    drop(vault);

    let table = run(&db, &["outcomes"]);
    assert_success(&table);
    let table = String::from_utf8_lossy(&table.stdout);
    assert!(
        table.contains(
            "forgotten-skill v1: helped 0, failed 0, not applicable 0, loaded 1, unreported 1"
        ),
        "{table}"
    );
    assert!(
        table.contains(
            "reported-skill v1: helped 1, failed 0, not applicable 0, loaded 1, unreported 0"
        ),
        "{table}"
    );

    let unreported = run(&db, &["outcomes", "--unreported", "--json"]);
    assert_success(&unreported);
    let unreported: serde_json::Value = serde_json::from_slice(&unreported.stdout).unwrap();
    assert_eq!(unreported.as_array().unwrap().len(), 1, "{unreported}");
    assert_eq!(unreported[0]["id"], "forgotten-skill");
    assert_eq!(unreported[0]["loaded"], 1);
    assert_eq!(unreported[0]["unreported"], 1);

    let table = run(&db, &["outcomes", "--unreported"]);
    assert_success(&table);
    let table = String::from_utf8_lossy(&table.stdout);
    assert!(table.contains("forgotten-skill"), "{table}");
    assert!(!table.contains("reported-skill v1"), "{table}");
}

#[test]
fn cli_error_cases_produce_stderr_and_no_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let mut vault = Vault::open(&db).unwrap();
    Draft::new("example").publish(&mut vault);
    drop(vault);

    for args in [
        vec!["show", "missing", "--version", "1"],
        vec!["deprecate", "missing-skill"],
        vec!["outcomes", "example", "--failing"],
        vec!["outcomes", "example", "--unreported"],
    ] {
        let result = run(&db, &args);
        assert!(!result.status.success(), "expected failure for {args:?}");
        assert!(result.stdout.is_empty(), "unexpected stdout for {args:?}");
        assert!(!result.stderr.is_empty(), "expected stderr for {args:?}");
    }
}

#[test]
fn help_lists_commands_without_opening_database() {
    let dir = tempfile::tempdir().unwrap();
    let output = support::cli()
        .env("HOME", dir.path())
        .env_remove("XDG_DATA_HOME")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["setup", "serve", "show", "hook"] {
        assert!(help.contains(command), "missing {command}");
    }
    assert!(!dir.path().join(".local").exists());
}

#[test]
fn default_database_respects_xdg_then_home() {
    for use_xdg in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = support::cli();
        command
            .arg("outcomes")
            .env("HOME", dir.path())
            .env_remove("XDG_DATA_HOME")
            // On Windows, LOCALAPPDATA takes priority over this XDG/HOME fallback;
            // remove it so this test exercises the fallback regardless of platform.
            .env_remove("LOCALAPPDATA");
        let base = if use_xdg {
            let base = dir.path().join("xdg data");
            command.env("XDG_DATA_HOME", &base);
            base
        } else {
            dir.path().join(".local/share")
        };
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(base.join("skillvolution/skills.db").exists());
    }
}

#[test]
#[cfg(windows)]
fn default_database_prefers_local_app_data_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let local_app_data = dir.path().join("Local");
    let output = support::cli()
        .arg("outcomes")
        .env("LOCALAPPDATA", &local_app_data)
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(local_app_data.join("skillvolution/skills.db").exists());
}

#[test]
#[cfg(windows)]
fn default_database_keeps_using_a_legacy_home_vault_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let local_app_data = dir.path().join("Local");
    let legacy = dir.path().join(".local/share/skillvolution/skills.db");
    skillvolution::vault::Vault::open(&legacy).unwrap();
    let output = support::cli()
        .arg("outcomes")
        .env("LOCALAPPDATA", &local_app_data)
        .env("USERPROFILE", dir.path())
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("SKILLVOLUTION_DB")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!local_app_data.join("skillvolution/skills.db").exists());
}

#[test]
fn setup_without_project_defaults_bin_and_xdg_database_globally() {
    // `setup` without `--project` now configures the user's global config instead of
    // defaulting the project to the current directory.
    let home = tempfile::tempdir().unwrap();
    let xdg_base = home.path().join("xdg data");
    let empty_path = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let bin = env!("CARGO_BIN_EXE_skillvolution");
    // Absolutized the same way `setup` resolves its own `--bin` default
    // (`resolve_bin`, via `std::path::absolute`): plain `fs::canonicalize` also
    // resolves symlinks and, on Windows, returns the verbatim `\\?\` form, neither
    // of which the app's own command string carries.
    let canonical_bin = std::path::absolute(bin).unwrap();

    let output = support::cli()
        .arg("setup")
        .arg("--client")
        .arg("claude-code")
        .current_dir(workspace.path())
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", &xdg_base)
        // LOCALAPPDATA outranks XDG_DATA_HOME on Windows; remove it so this
        // test's expected database path holds on every platform.
        .env_remove("LOCALAPPDATA")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("PATH", empty_path.path())
        .output()
        .unwrap();
    assert_success(&output);

    assert!(!workspace.path().join(".mcp.json").exists());

    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(home.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    let command = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    // --bin defaulted to this test's own binary, canonicalized.
    assert!(command.contains(canonical_bin.to_str().unwrap()));
    // --db defaulted to $XDG_DATA_HOME/skillvolution/skills.db. Two joins, not one
    // string with an embedded `/`: Windows keeps that `/` literally, while the app's
    // own path rebuilds the value component-by-component and gets `\`.
    let db_path = xdg_base.join("skillvolution").join("skills.db");
    assert!(command.contains(db_path.to_str().unwrap()));
    assert!(db_path.is_file(), "setup must initialize the database");
}

#[test]
fn setup_honours_global_db_flag() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    let db = dir.path().join("global.db");

    let output = support::cli()
        .arg("--db")
        .arg(&db)
        .arg("setup")
        .arg("--project")
        .arg(&project)
        .arg("--client")
        .arg("claude-code")
        .output()
        .unwrap();
    assert_success(&output);

    let mcp: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join(".mcp.json")).unwrap()).unwrap();
    let args = mcp["mcpServers"]["skillvolution"]["args"]
        .as_array()
        .unwrap();
    assert_eq!(args[1].as_str().unwrap(), db.to_str().unwrap());
    assert!(db.is_file());
}

#[test]
fn opening_the_database_is_idempotent_with_a_global_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("space directory/skills.db");
    for _ in 0..2 {
        let result = run(&db, &["outcomes"]);
        assert_success(&result);
        assert!(result.stderr.is_empty());
    }
    assert!(db.exists());
}

const SKILL_MD: &str = "---\nname: my-skill\ndescription: \"Use when testing publish\"\ntags: [cli, 'demo']\nunknown: ignored\n---\n\n## When to use\nAlways.\n## Procedure\n1. Do it.\n## Pitfalls\n- None.\n## Verification\nCheck.\n";

fn skill_file(dir: &std::path::Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("SKILL.md");
    fs::write(&path, text).unwrap();
    path
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn publish_creates_a_skill_and_requires_replace_for_a_new_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = skill_file(dir.path(), SKILL_MD);
    let path = file.to_str().unwrap();

    let first = run(&db, &["publish", path]);
    assert_success(&first);
    assert_eq!(stdout(&first), "Published my-skill v1 (global).\n");

    let again = run(&db, &["publish", path]);
    assert!(!again.status.success());
    assert!(
        stderr(&again)
            .contains("skill my-skill already exists at v1; pass --replace to publish v2"),
        "{}",
        stderr(&again)
    );

    let replaced = run(&db, &["publish", path, "--replace"]);
    assert_success(&replaced);
    assert_eq!(stdout(&replaced), "Published my-skill v2 (global).\n");
}

#[test]
fn publish_rejects_a_bad_file_with_every_problem_and_a_template() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    // Not named SKILL.md, so there is no directory-name fallback for the id.
    let file = dir.path().join("notes.md");
    fs::write(
        &file,
        "---\ndescription: Use when x\ntags: [Bad Tag]\n---\n## When to use\nx\n## Pitfalls\n- x\n",
    )
    .unwrap();
    let out = run(&db, &["publish", file.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("missing id"), "{err}");
    assert!(err.contains("tag"), "{err}");
    assert!(err.contains("## Procedure, ## Verification"), "{err}");
    assert!(err.contains("name: my-skill-id"), "{err}");
    assert!(err.contains("Publishing a skill manually"), "{err}");
}

fn skill_dir(root: &std::path::Path, name: &str, text: &str) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    skill_file(&dir, text);
}

#[test]
fn bulk_publish_reports_each_file_and_a_summary() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let root = dir.path().join("skills");
    skill_dir(&root, "a-new", &SKILL_MD.replace("my-skill", "a-new"));
    skill_dir(
        &root,
        "b-bad",
        "---\ndescription: Use when x\n---\nno sections\n",
    );
    skill_dir(
        &root,
        "c-existing",
        &SKILL_MD.replace("my-skill", "c-existing"),
    );
    assert_success(&run(
        &db,
        &["publish", root.join("c-existing").to_str().unwrap()],
    ));

    let out = run(&db, &["publish", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("[published] a-new v1 (global)\n"), "{text}");
    assert!(
        text.contains("[skipped] c-existing: already at v1; pass --replace\n"),
        "{text}"
    );
    assert!(text.contains("[rejected] "), "{text}");
    let rejected = format!("{}:\n  - ", Path::new("b-bad").join("SKILL.md").display());
    assert!(text.contains(&rejected), "{text}");
    assert!(
        text.contains("1 published, 1 skipped, 1 rejected"),
        "{text}"
    );
    assert_eq!(
        text.matches("A SKILL.md looks like this").count(),
        1,
        "{text}"
    );

    let id_flag = run(&db, &["publish", root.to_str().unwrap(), "--id", "x"]);
    assert!(!id_flag.status.success());
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(
        !run(&db, &["publish", empty.to_str().unwrap()])
            .status
            .success()
    );
}

#[test]
fn skill_md_without_a_name_takes_its_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let root = dir.path().join("skills");
    let nameless = SKILL_MD.replace("name: my-skill\n", "");
    skill_dir(&root, "from-dir", &nameless);
    let out = run(&db, &["publish", root.to_str().unwrap()]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("[published] from-dir v1 (global)"),
        "{}",
        stdout(&out)
    );
    let single = run(
        &db,
        &[
            "publish",
            root.join("from-dir").to_str().unwrap(),
            "--replace",
        ],
    );
    assert_success(&single);
    assert_eq!(stdout(&single), "Published from-dir v2 (global).\n");
}

#[test]
fn get_round_trips_through_publish_replace() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = skill_file(dir.path(), SKILL_MD);
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));

    let got = run(&db, &["get", "my-skill"]);
    assert_success(&got);
    let text = stdout(&got);
    assert!(text.starts_with("---\nname: my-skill\n"), "{text}");
    assert!(text.contains("tags: [cli, demo]"), "{text}");

    let copy = dir.path().join("copy.md");
    fs::write(&copy, &text).unwrap();
    let replaced = run(&db, &["publish", copy.to_str().unwrap(), "--replace"]);
    assert_success(&replaced);
    assert_eq!(stdout(&replaced), "Published my-skill v2 (global).\n");
    assert_eq!(stdout(&run(&db, &["get", "my-skill"])), text);
}

#[test]
fn search_and_list_show_published_skills() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = skill_file(dir.path(), SKILL_MD);
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let line = "my-skill v1 [global] (helped 0, failed 0): Use when testing publish\n";
    assert_eq!(stdout(&run(&db, &["search", "publish"])), line);
    assert_eq!(stdout(&run(&db, &["list"])), line);
    let json = run(&db, &["list", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["skills"][0]["id"], "my-skill");
}

#[test]
fn project_scoped_publish_is_invisible_from_another_project() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = skill_file(dir.path(), SKILL_MD);
    let published = run(
        &db,
        &[
            "publish",
            file.to_str().unwrap(),
            "--scope",
            "project",
            "--project",
            "alpha",
        ],
    );
    assert_success(&published);
    assert_eq!(stdout(&published), "Published my-skill v1 (project).\n");
    assert!(stdout(&run(&db, &["list", "--project", "alpha"])).contains("[alpha]"));
    assert_eq!(stdout(&run(&db, &["list", "--project", "beta"])), "");
}

const SECTIONS: &str = "## When to use\nAlways.\n## Procedure\n1. Do it.\n## Pitfalls\n- None.\n## Verification\nCheck.\n";

#[test]
fn publish_handles_crlf_files() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = skill_file(dir.path(), &SKILL_MD.replace('\n', "\r\n"));
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let got = stdout(&run(&db, &["get", "my-skill"]));
    assert!(got.contains("---\n## When to use\n"), "{got}");
    assert!(!got.contains('\r'), "{got}");
    assert_eq!(got.matches("---").count(), 2, "{got}");
}

#[test]
fn publish_reports_unclosed_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let text = format!("---\nname: my-skill\ndescription: Use when x\n{SECTIONS}");
    let file = skill_file(dir.path(), &text);
    let out = run(&db, &["publish", file.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("frontmatter starts with --- but is never closed by a --- line"),
        "{err}"
    );
    assert!(err.contains("name: my-skill-id"), "{err}");
}

#[test]
fn publish_rejects_multiline_yaml_and_ignores_nested_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let text =
        format!("---\nname: my-skill\ndescription: >\n  Use when x\ntags:\n  - a\n---\n{SECTIONS}");
    let file = skill_file(dir.path(), &text);
    let out = run(&db, &["publish", file.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("description: multi-line YAML values are not supported; write it on one line"),
        "{err}"
    );
    // Block lists (`tags:` then `- a`) are read like `[a]`, as Claude Code agents write them.
    assert!(!err.contains("write tags inline"), "{err}");
    assert!(!err.contains("missing id"), "{err}");

    let nested = format!(
        "---\nname: top-skill\ndescription: Use when x\nmeta:\n  name: nested\n---\n{SECTIONS}"
    );
    let file = skill_file(dir.path(), &nested);
    let out = run(&db, &["publish", file.to_str().unwrap()]);
    assert_eq!(stdout(&out), "Published top-skill v1 (global).\n");
}

#[test]
fn publish_ignores_yaml_inline_comments_in_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let text = format!(
        "---\nname: my-skill  # the id\ndescription: \"Use when C# # builds fail\" # why\n\
         tags: [dotnet, csharp]  # inline\nscope: project   # optional\n---\n{SECTIONS}"
    );
    let file = skill_file(dir.path(), &text);
    let out = run(
        &db,
        &["publish", file.to_str().unwrap(), "--project", "alpha"],
    );
    assert_eq!(
        stdout(&out),
        "Published my-skill v1 (project).\n",
        "{}",
        stderr(&out)
    );
    let got = stdout(&run(&db, &["get", "my-skill", "--project", "alpha"]));
    assert!(
        got.contains("description: \"Use when C# # builds fail\"\n"),
        "{got}"
    );
    assert!(got.contains("tags: [dotnet, csharp]\n"), "{got}");
}

#[test]
fn quoted_text_inside_a_description_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let desc = "Use when \"foo\" fails and \"bar\" hangs";
    let text = format!("---\nname: my-skill\ndescription: {desc}\n---\n{SECTIONS}");
    let file = skill_file(dir.path(), &text);
    assert_success(&run(&db, &["publish", file.to_str().unwrap()]));
    let got = stdout(&run(&db, &["get", "my-skill"]));
    assert!(got.contains(&format!("description: {desc}\n")), "{got}");
    let copy = dir.path().join("copy.md");
    fs::write(&copy, &got).unwrap();
    assert_success(&run(&db, &["publish", copy.to_str().unwrap(), "--replace"]));
    assert_eq!(stdout(&run(&db, &["get", "my-skill"])), got);
}

#[test]
fn project_scope_round_trips_through_get_and_publish() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let file = skill_file(dir.path(), SKILL_MD);
    assert_success(&run(
        &db,
        &[
            "publish",
            file.to_str().unwrap(),
            "--scope",
            "project",
            "--project",
            "alpha",
        ],
    ));
    let got = run(&db, &["get", "my-skill", "--project", "alpha"]);
    let text = stdout(&got);
    assert!(text.contains("\nscope: project\n"), "{text}");
    let copy = dir.path().join("copy.md");
    fs::write(&copy, &text).unwrap();
    let replaced = run(
        &db,
        &[
            "publish",
            copy.to_str().unwrap(),
            "--replace",
            "--project",
            "alpha",
        ],
    );
    assert_eq!(stdout(&replaced), "Published my-skill v2 (project).\n");
}

#[test]
fn bulk_publish_turns_a_propose_error_into_a_rejection_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let root = dir.path().join("skills");
    skill_dir(&root, "a-first", &SKILL_MD.replace("my-skill", "a-first"));
    skill_dir(
        &root,
        "b-secret",
        &SKILL_MD
            .replace("my-skill", "b-secret")
            .replace("Always.", "Key AKIAABCDEFGHIJKL1234 here."),
    );
    skill_dir(&root, "c-last", &SKILL_MD.replace("my-skill", "c-last"));

    let out = run(&db, &["publish", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("[published] a-first v1"), "{text}");
    assert!(text.contains("[published] c-last v1"), "{text}");
    let rejected = format!(
        "{}:\n  - ",
        Path::new("b-secret").join("SKILL.md").display()
    );
    assert!(text.contains(&rejected), "{text}");
    assert!(
        text.contains("2 published, 0 skipped, 1 rejected"),
        "{text}"
    );
    assert_eq!(
        text.matches("A SKILL.md looks like this").count(),
        1,
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn skill_md_id_fallback_does_not_resolve_symlinked_directories() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("skills.db");
    let root = dir.path().join("skills");
    fs::create_dir(&root).unwrap();
    let target = dir.path().join("other-name");
    fs::create_dir(&target).unwrap();
    skill_file(&target, &SKILL_MD.replace("name: my-skill\n", ""));
    std::os::unix::fs::symlink(&target, root.join("foo")).unwrap();

    let out = run(&db, &["publish", root.to_str().unwrap()]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("[published] foo v1 (global)"),
        "{}",
        stdout(&out)
    );
}
