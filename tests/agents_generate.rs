mod support;

use skillvolution::agents::{self, Agent, Target};
use skillvolution::vault::AgentMeta;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use support::{BODY, stderr, stdout};
use tempfile::TempDir;

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    db: PathBuf,
    empty_path: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let empty_path = root.path().join("path");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&empty_path).unwrap();
        let bin = root.path().join("skillvolution-bin");
        fs::write(&bin, "").unwrap();
        let repo = support::git_repo(root.path(), "myproj");
        Self {
            db: root.path().join("vault.db"),
            _root: root,
            home,
            repo,
            bin,
            empty_path,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.repo, args)
    }

    fn run_in(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_skillvolution"))
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("xdg"))
            .env("APPDATA", self.home.join("appdata"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env("PATH", &self.empty_path)
            .current_dir(dir)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn err(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} should fail");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    /// Per-user (global) setup of `clients` inside the sandbox HOME.
    fn setup_global(&self, clients: &str) {
        self.ok(&[
            "setup",
            "--client",
            clients,
            "--bin",
            self.bin.to_str().unwrap(),
        ]);
    }

    fn user_claude(&self, id: &str) -> PathBuf {
        self.home.join(".claude/agents").join(format!("{id}.md"))
    }

    fn setup(&self, clients: &str) {
        let repo = self.repo.to_str().unwrap();
        let bin = self.bin.to_str().unwrap();
        self.ok(&[
            "setup",
            "--project",
            repo,
            "--client",
            clients,
            "--bin",
            bin,
        ]);
    }

    fn publish(&self, tools: &str, extra: &str, replace: bool) {
        self.publish_tier("mid", tools, extra, replace);
    }

    fn publish_tier(&self, tier: &str, tools: &str, extra: &str, replace: bool) {
        let file = self.repo.join("reviewer-src").join("AGENT.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            format!(
                "---\nname: reviewer\ndescription: Use when a diff needs review\ntier: {tier}\ntools: {tools}\n{extra}---\n{BODY}"
            ),
        )
        .unwrap();
        let mut args = vec!["publish", file.to_str().unwrap()];
        if replace {
            args.push("--replace");
        }
        self.ok(&args);
    }

    /// Publishes a global skill straight into the sandbox vault.
    fn skill(&self, id: &str) {
        let mut vault = skillvolution::vault::Vault::open(&self.db).unwrap();
        support::Draft::new(id).publish(&mut vault);
    }

    fn claude(&self, id: &str) -> PathBuf {
        self.repo.join(".claude/agents").join(format!("{id}.md"))
    }

    fn opencode(&self, id: &str) -> PathBuf {
        self.repo.join(".opencode/agents").join(format!("{id}.md"))
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn fixture_agent() -> Agent {
    Agent {
        id: "reviewer".into(),
        version: 2,
        description: "Use when a diff needs review".into(),
        content: BODY.into(),
        meta: AgentMeta {
            role: "subagent".into(),
            tier: "mid".into(),
            effort: None,
            tools: Some(vec!["read".into(), "shell".into(), "mcp".into()]),
            skills: vec!["rust-testing".into()],
        },
        deprecated: false,
        scope: None,
        skill_versions: vec![("rust-testing".into(), 3, "1a2b3c4d".into())],
    }
}

fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agents")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(&path, actual).unwrap();
    }
    assert_eq!(read(&path), actual, "golden {name}");
}

#[test]
fn render_matches_golden_files() {
    let agent = fixture_agent();
    let claude = agents::render(
        Target::ClaudeCode,
        &agent,
        Some(("sonnet".into(), Some("medium".into()))),
    );
    golden("reviewer.claude.md", &claude.0);
    assert!(claude.1.is_empty(), "{:?}", claude.1);
    let (opencode, warnings) = agents::render(Target::OpenCode, &agent, None);
    golden("reviewer.opencode.md", &opencode);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("agents models set opencode mid")),
        "{warnings:?}"
    );
}

#[test]
fn render_quotes_yaml_special_names_and_warns_about_opencode_limits() {
    let mut agent = fixture_agent();
    agent.description = "true".into();
    agent.meta.effort = Some("high".into());
    agent.meta.tools = Some(vec!["read".into()]);
    agent.meta.skills.clear();
    let (text, _) = agents::render(Target::ClaudeCode, &agent, None);
    assert!(text.contains("description: \"true\"\n"), "{text}");
    assert!(text.contains("effort: high\n"));
    let (_, warnings) = agents::render(Target::OpenCode, &agent, None);
    assert!(warnings.iter().any(|w| w.contains("MCP")));
    assert!(warnings.iter().any(|w| w.contains("effort")));
}

#[test]
fn add_writes_only_for_configured_clients() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    let out = sandbox.ok(&["agents", "add", "sv-mid"]);
    assert!(out.contains("[added] Claude Code sv-mid v1"), "{out}");
    assert!(out.contains("Cursor"), "{out}");
    let text = read(&sandbox.claude("sv-mid"));
    assert!(
        text.contains("model: sonnet\n") && text.contains("effort: medium\n"),
        "{text}"
    );
    assert!(text.contains("<!-- skillvolution-managed:agent:sv-mid:v1:"));
    assert!(!sandbox.opencode("sv-mid").exists());
    // Again: nothing to do.
    assert!(
        sandbox
            .ok(&["agents", "add", "sv-mid"])
            .contains("[unchanged]")
    );
}

#[test]
fn add_without_a_configured_client_fails() {
    let sandbox = Sandbox::new();
    let err = sandbox.err(&["agents", "add", "sv-mid"]);
    assert!(
        err.contains("no configured client supports agents"),
        "{err}"
    );
}

#[test]
fn vault_agents_render_for_both_clients_and_tools_become_denials() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    sandbox.publish("[read, shell]", "skills: []\n", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let claude = read(&sandbox.claude("reviewer"));
    assert!(
        claude.contains(
            "disallowedTools: Edit, Write, NotebookEdit, MultiEdit, WebFetch, WebSearch, mcp__*, Agent, Task\n"
        ),
        "{claude}"
    );
    let opencode = read(&sandbox.opencode("reviewer"));
    assert!(
        opencode.contains("mode: subagent\n")
            && opencode.contains("  edit: deny\n")
            && opencode.contains("  task: deny\n"),
        "{opencode}"
    );
    assert!(!opencode.contains("model:"), "{opencode}");
}

#[test]
fn a_foreign_file_refuses_add_and_nothing_is_written() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    fs::create_dir_all(sandbox.opencode("sv-mid").parent().unwrap()).unwrap();
    fs::write(sandbox.opencode("sv-mid"), "mine\n").unwrap();
    let err = sandbox.err(&["agents", "add", "sv-mid", "sv-low"]);
    assert!(err.contains("not generated by skillvolution"), "{err}");
    assert_eq!(read(&sandbox.opencode("sv-mid")), "mine\n");
    assert!(!sandbox.claude("sv-mid").exists() && !sandbox.claude("sv-low").exists());
}

#[test]
fn a_forged_marker_with_the_wrong_id_is_foreign() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    let path = sandbox.claude("sv-low");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "---\nname: x\n---\n<!-- skillvolution-managed:agent:sv-mid:v1:00000000 -->\n",
    )
    .unwrap();
    let out = sandbox.run(&["agents", "remove", "sv-low"]);
    assert!(!out.status.success(), "nothing was removed");
    assert!(stdout(&out).contains("[foreign]"), "{}", stdout(&out));
    assert!(path.exists());
}

#[test]
fn hand_edits_are_detected_by_sync_remove_and_doctor() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-mid"]);
    let path = sandbox.claude("sv-mid");
    fs::write(
        &path,
        read(&path).replace("Stay inside the task", "Roam freely"),
    )
    .unwrap();
    let out = sandbox.ok(&["agents", "sync", "--apply"]);
    assert!(out.contains("[edited] Claude Code sv-mid"), "{out}");
    assert!(read(&path).contains("Roam freely"));
    let removal = sandbox.run(&["agents", "remove", "sv-mid"]);
    assert!(!removal.status.success(), "nothing was removed");
    assert!(stdout(&removal).contains("[edited]"));
    assert!(path.exists());
    let doctor = sandbox.run(&["doctor"]);
    let doctor = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        doctor.contains("[warn] agents: Claude Code: sv-mid") && doctor.contains("edited by hand"),
        "{doctor}"
    );
    assert!(doctor.contains("agents: catalog"), "{doctor}");
    assert!(
        doctor.contains("delete the file and re-run `skillvolution agents add sv-mid`")
            && !doctor.contains("agents import"),
        "{doctor}"
    );
}

#[test]
fn crlf_checkouts_do_not_count_as_edits() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-mid"]);
    let path = sandbox.claude("sv-mid");
    fs::write(&path, read(&path).replace('\n', "\r\n")).unwrap();
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(
        out.contains("1 unchanged") && !out.contains("[edited]"),
        "{out}"
    );
}

#[test]
fn sync_dry_run_writes_nothing_and_flags_widened_tools_then_apply_updates() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read]", "", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let before = read(&sandbox.claude("reviewer"));
    assert!(before.contains("Bash"));
    sandbox.publish("[read, shell]", "", true);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(
        out.contains("[update] Claude Code reviewer v1 -> v2"),
        "{out}"
    );
    assert!(out.contains("! tools widened: shell"), "{out}");
    assert!(
        out.contains("! prompt changed on an agent with shell access")
            || !out.contains("prompt changed")
    );
    assert!(
        out.contains("dry run: re-run with --apply to write 1 file(s)"),
        "{out}"
    );
    assert_eq!(read(&sandbox.claude("reviewer")), before);
    sandbox.ok(&["agents", "sync", "--apply"]);
    let after = read(&sandbox.claude("reviewer"));
    assert!(after.contains(":v2:") && !after.contains("Bash"), "{after}");
    let doctor = String::from_utf8_lossy(&sandbox.run(&["doctor"]).stdout).into_owned();
    assert!(!doctor.contains("outdated"), "{doctor}");
}

#[test]
fn doctor_reports_outdated_and_missing_agent_files() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read]", "", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.publish("[read, shell]", "", true);
    let doctor = String::from_utf8_lossy(&sandbox.run(&["doctor"]).stdout).into_owned();
    assert!(
        doctor.contains("reviewer") && doctor.contains("outdated (v1 vs v2"),
        "{doctor}"
    );
    sandbox.ok(&["purge", "reviewer"]);
    let doctor = String::from_utf8_lossy(&sandbox.run(&["doctor"]).stdout).into_owned();
    assert!(doctor.contains("missing from the vault"), "{doctor}");
    let sync = sandbox.ok(&["agents", "sync"]);
    assert!(sync.contains("[missing] Claude Code reviewer"), "{sync}");
}

#[test]
fn remove_deletes_only_marked_files() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    sandbox.ok(&["agents", "add", "sv-low"]);
    let mine = sandbox.claude("mine");
    fs::write(&mine, "---\nname: mine\n---\nbody\n").unwrap();
    let removal = sandbox.run(&["agents", "remove", "sv-low", "mine", "nothing"]);
    assert!(
        !removal.status.success(),
        "removing something not installed must exit non-zero"
    );
    let out = String::from_utf8_lossy(&removal.stdout).into_owned();
    assert!(
        out.contains("[removed] Claude Code sv-low") && out.contains("[removed] OpenCode sv-low"),
        "{out}"
    );
    assert!(
        out.contains("[foreign] Claude Code mine") && out.contains("nothing: not installed"),
        "{out}"
    );
    assert!(
        mine.exists() && !sandbox.claude("sv-low").exists() && !sandbox.opencode("sv-low").exists()
    );
}

#[test]
fn setup_remove_notes_the_agent_files_it_leaves() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-low"]);
    let repo = sandbox.repo.to_str().unwrap();
    let out = sandbox.ok(&[
        "setup",
        "--remove",
        "--project",
        repo,
        "--client",
        "claude-code",
    ]);
    assert!(
        out.contains("managed agent files were left") && out.contains("sv-low"),
        "{out}"
    );
    assert!(sandbox.claude("sv-low").exists());
}

#[test]
fn models_set_changes_the_generated_model() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    let table = sandbox.ok(&["agents", "models"]);
    assert!(
        table.starts_with("client       tier  model   effort  origin\n"),
        "{table}"
    );
    assert!(
        table.contains("claude-code  mid   sonnet  medium  (default)"),
        "{table}"
    );
    assert!(
        table.contains("opencode     mid   -       -       (none)"),
        "{table}"
    );
    let out = sandbox.ok(&[
        "agents",
        "models",
        "set",
        "opencode",
        "mid",
        "anthropic/claude-sonnet-4",
        "--effort",
        "low",
    ]);
    assert!(out.contains("OpenCode files do not carry effort"), "{out}");
    sandbox.ok(&[
        "agents",
        "models",
        "set",
        "claude-code",
        "mid",
        "claude-sonnet-4-5",
    ]);
    sandbox.ok(&["agents", "add", "sv-mid"]);
    assert!(read(&sandbox.claude("sv-mid")).contains("model: claude-sonnet-4-5\n"));
    assert!(read(&sandbox.opencode("sv-mid")).contains("model: anthropic/claude-sonnet-4\n"));
}

#[test]
fn importing_a_generated_file_round_trips_and_native_files_map_sensibly() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read, shell]", "skills: []\n", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let imported = sandbox.ok(&[
        "agents",
        "import",
        sandbox.claude("reviewer").to_str().unwrap(),
    ]);
    assert!(
        imported.contains("name: reviewer\n") && imported.contains("tier: mid\n"),
        "{imported}"
    );
    assert!(
        imported.contains("tools: [read, shell]\n") && imported.contains("## Output contract"),
        "{imported}"
    );

    let native = sandbox.repo.join("native.md");
    fs::write(
        &native,
        "---\nname: My Native Agent\ndescription: >-\n---\n",
    )
    .unwrap();
    let rejected = sandbox.run(&["agents", "import", native.to_str().unwrap()]);
    assert!(!rejected.status.success());
    // Rejections go to stderr so a redirected stdout stays a clean AGENT.md.
    assert!(stdout(&rejected).is_empty(), "{}", stdout(&rejected));
    assert!(
        stderr(&rejected).contains("[rejected]") && stderr(&rejected).contains("multi-line"),
        "{}",
        stderr(&rejected)
    );
    fs::write(
        &native,
        format!("---\nname: My Native Agent\ndescription:   Does   things\nmodel: claude-opus-4-1\ntools: Read, Bash\ndisallowedTools: Write\nskills: [x]\n---\n{BODY}"),
    )
    .unwrap();
    let out = sandbox.run(&["agents", "import", native.to_str().unwrap()]);
    let (text, notes) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        text.contains("name: my-native-agent\n") && text.contains("description: Does things\n"),
        "{text}"
    );
    assert!(
        text.contains("tier: high\n") && text.contains("tools: [read, shell]\n"),
        "{text}"
    );
    assert!(
        notes.contains("normalized") && notes.contains("native `skills:` dropped"),
        "{notes}"
    );

    // Directory import writes files and never overwrites.
    let dir = sandbox.repo.join("imported");
    let generated = sandbox.claude("reviewer");
    let args = [
        "agents",
        "import",
        generated.to_str().unwrap(),
        "-o",
        dir.to_str().unwrap(),
    ];
    sandbox.ok(&args);
    assert!(dir.join("reviewer/AGENT.md").is_file());
    // A second run skips what exists and still succeeds.
    let again = sandbox.ok(&args);
    assert!(
        again.contains("[skipped]") && again.contains(": exists"),
        "{again}"
    );
    // The result publishes.
    sandbox.ok(&["publish", dir.to_str().unwrap(), "--replace"]);

    let opencode_like = sandbox.repo.join("oc.md");
    fs::write(
        &opencode_like,
        format!("---\ndescription: x\nmode: subagent\n---\n{BODY}"),
    )
    .unwrap();
    let rejected = sandbox.run(&["agents", "import", opencode_like.to_str().unwrap()]);
    assert!(!rejected.status.success());
    let shown = stderr(&rejected);
    assert!(
        shown.contains("[rejected]") && shown.contains("only Claude Code"),
        "{shown}"
    );
}

#[test]
fn skills_line_asks_for_reports_and_import_strips_both_lines() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.skill("rust-testing");
    sandbox.publish("[read, mcp]", "skills: [rust-testing]\n", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let generated = read(&sandbox.claude("reviewer"));
    assert!(
        generated.contains("get_skill, passing the version number shown (the #hash only detects content changes; do not pass it): rust-testing v1 (#"),
        "{generated}"
    );
    assert!(
        generated.contains(").\nBefore returning, call report_skill_outcome once for each of them"),
        "{generated}"
    );
    assert!(!generated.contains("the current one"), "{generated}");
    let imported = sandbox.ok(&[
        "agents",
        "import",
        sandbox.claude("reviewer").to_str().unwrap(),
    ]);
    assert!(
        imported.contains("skills: [rust-testing]")
            && !imported.contains("report_skill_outcome")
            && !imported.contains("Before starting"),
        "{imported}"
    );
}

#[test]
fn sv_prefix_and_client_agent_names_are_reserved_for_agents_but_not_skills() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    for id in ["sv-custom", "explore", "general-purpose", "evolution"] {
        let file = sandbox.repo.join(format!("{id}.md"));
        fs::write(
            &file,
            format!("---\nname: {id}\ndescription: x\ntier: mid\n---\n{BODY}"),
        )
        .unwrap();
        let err = sandbox.run(&["publish", file.to_str().unwrap()]);
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&err.stdout),
            String::from_utf8_lossy(&err.stderr)
        );
        assert!(
            !err.status.success() && all.contains("reserved"),
            "{id}: {all}"
        );
        assert!(
            sandbox.err(&["agents", "add", id]).contains("reserved"),
            "{id}"
        );
    }
    // Skills may use these ids.
    sandbox.skill("explore");
    sandbox.skill("sv-helper");
}

#[test]
fn agent_skills_must_exist_purge_refuses_them_and_add_warns_when_gone() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    let missing = {
        let file = sandbox.repo.join("reviewer-src/AGENT.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            format!("---\nname: reviewer\ndescription: x\ntier: mid\ntools: [read, mcp]\nskills: [rust-testing]\n---\n{BODY}"),
        )
        .unwrap();
        sandbox.err(&["publish", file.to_str().unwrap()])
    };
    assert!(
        missing.contains("rust-testing (missing or purged)"),
        "{missing}"
    );
    sandbox.skill("rust-testing");
    sandbox.publish("[read, mcp]", "skills: [rust-testing]\n", false);
    let refused = sandbox.err(&["purge", "rust-testing"]);
    assert!(
        refused.contains("reviewer") && refused.contains("--force"),
        "{refused}"
    );
    sandbox.ok(&["purge", "rust-testing", "--force"]);
    let added = sandbox.ok(&["agents", "add", "reviewer"]);
    assert!(
        added.contains(
            "[warning] agent reviewer: skill rust-testing is not available (missing or purged)"
        ),
        "{added}"
    );
}

#[test]
fn sync_flags_skill_and_tier_changes() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.skill("rust-testing");
    sandbox.publish("[read, mcp]", "skills: [rust-testing]\n", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.publish_tier("high", "[read, mcp]", "", true);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(out.contains("! tier changed: mid -> high"), "{out}");
    assert!(out.contains("! skills removed: rust-testing"), "{out}");
}

#[test]
fn sync_flags_a_new_skill_version_and_add_notes_pinned_skills() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.skill("helper");
    sandbox.publish("[read, shell, mcp]", "skills: [helper]\n", false);
    let added = sandbox.ok(&["agents", "add", "reviewer"]);
    assert!(
        added.contains("pinned") && added.contains("sync"),
        "{added}"
    );
    assert!(read(&sandbox.claude("reviewer")).contains("helper v1 (#"));
    // A new version of the skill rewrites the file without a new agent revision.
    let mut vault = skillvolution::vault::Vault::open(&sandbox.db).unwrap();
    support::Draft::new("helper")
        .expected_version(1)
        .publish(&mut vault);
    drop(vault);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(out.contains("! skill helper v1 -> v2"), "{out}");
    sandbox.ok(&["agents", "sync", "--apply"]);
    assert!(read(&sandbox.claude("reviewer")).contains("helper v2 (#"));
}

#[test]
fn import_notes_scoped_tools_and_dropped_keys() {
    let sandbox = Sandbox::new();
    let native = sandbox.repo.join("native.md");
    fs::write(
        &native,
        format!("---\nname: scoped\ndescription: x\nmodel: sonnet\ntools: Read, Bash(git log:*)\npermissionMode: plan\nhooks:\n  a: b\nmcpServers: x\n---\n{BODY}"),
    )
    .unwrap();
    let out = sandbox.run(&["agents", "import", native.to_str().unwrap()]);
    let notes = stderr(&out);
    assert!(
        notes.contains("Bash(git log:*)") && notes.contains("scoped"),
        "{notes}"
    );
    for key in ["permissionMode", "hooks", "mcpServers"] {
        assert!(
            notes.contains(&format!("`{key}` is not imported")),
            "{notes}"
        );
    }
}

#[test]
fn project_scoped_agent_is_refused_when_the_project_dir_is_the_user_dir() {
    let sandbox = Sandbox::new();
    // HOME itself is the "repository": project dir == per-user dir.
    fs::create_dir_all(sandbox.home.join(".git")).unwrap();
    let home = sandbox.home.to_str().unwrap();
    let in_home = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_skillvolution"))
            .arg("--db")
            .arg(&sandbox.db)
            .args(args)
            .env("HOME", &sandbox.home)
            .env("XDG_CONFIG_HOME", sandbox.home.join("xdg"))
            .env("APPDATA", sandbox.home.join("appdata"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env("PATH", &sandbox.empty_path)
            .current_dir(&sandbox.home)
            .output()
            .unwrap()
    };
    let setup = in_home(&[
        "setup",
        "--project",
        home,
        "--client",
        "claude-code",
        "--bin",
        sandbox.bin.to_str().unwrap(),
    ]);
    assert!(setup.status.success(), "{setup:?}");
    let file = sandbox.home.join("reviewer-src").join("AGENT.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        format!(
            "---\nname: reviewer\ndescription: Use when a diff needs review\ntier: mid\ntools: [read]\n---\n{BODY}"
        ),
    )
    .unwrap();
    let published = in_home(&["publish", file.to_str().unwrap(), "--scope", "project"]);
    assert!(published.status.success(), "{published:?}");

    let add = in_home(&["agents", "add", "reviewer"]);
    assert!(!add.status.success());
    assert!(
        String::from_utf8_lossy(&add.stderr).contains("reviewer is project-scoped"),
        "{add:?}"
    );
    assert!(!sandbox.home.join(".claude/agents/reviewer.md").exists());
}

#[test]
fn opencode_files_spell_out_every_permission() {
    let sandbox = Sandbox::new();
    sandbox.setup("opencode");
    sandbox.publish("[read, edit]", "", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let opencode = read(&sandbox.opencode("reviewer"));
    assert!(
        opencode.contains(
            "permission:\n  edit: allow\n  bash: deny\n  webfetch: deny\n  websearch: deny\n"
        ),
        "{opencode}"
    );
    // An agent with every tool still states its permissions, so a user-level file of
    // the same name cannot leak a `deny` into it.
    sandbox.ok(&["agents", "add", "sv-mid"]);
    let all = read(&sandbox.opencode("sv-mid"));
    assert!(
        all.contains(
            "permission:\n  edit: allow\n  bash: allow\n  webfetch: allow\n  websearch: allow\n"
        ),
        "{all}"
    );
}

#[test]
fn models_set_keeps_the_effort_unset_reverts_and_odd_names_warn() {
    let sandbox = Sandbox::new();
    let out = sandbox.ok(&[
        "agents",
        "models",
        "set",
        "claude-code",
        "mid",
        "claude-sonnet-4-5",
    ]);
    assert!(out.contains("effort medium kept"), "{out}");
    assert!(
        sandbox
            .ok(&["agents", "models"])
            .contains("claude-sonnet-4-5  medium  (set)")
    );
    sandbox.ok(&[
        "agents",
        "models",
        "set",
        "claude-code",
        "mid",
        "opus",
        "--effort",
        "xhigh",
    ]);
    let out = sandbox.ok(&["agents", "models", "set", "claude-code", "mid", "sonnet"]);
    assert!(out.contains("effort xhigh kept"), "{out}");
    let out = sandbox.ok(&[
        "agents",
        "models",
        "set",
        "claude-code",
        "high",
        "anthropic/opus",
    ]);
    assert!(
        out.contains("warning:") && out.contains("not `provider/model`"),
        "{out}"
    );
    let out = sandbox.ok(&["agents", "models", "set", "opencode", "mid", "sonnet"]);
    assert!(
        out.contains("warning:") && out.contains("`provider/model`"),
        "{out}"
    );
    let out = sandbox.ok(&["agents", "models", "unset", "claude-code", "mid"]);
    assert!(out.contains("reverts to the default"), "{out}");
    let table = sandbox.ok(&["agents", "models"]);
    let row = table
        .lines()
        .find(|l| l.starts_with("claude-code") && l.contains(" mid "))
        .unwrap();
    assert_eq!(
        row.split_whitespace().collect::<Vec<_>>(),
        ["claude-code", "mid", "sonnet", "medium", "(default)"]
    );
    let out = sandbox.ok(&["agents", "models", "unset", "claude-code", "mid"]);
    assert!(out.contains("nothing to remove"), "{out}");
}

#[test]
fn a_project_agent_cannot_be_added_from_another_repository() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read]", "scope: project\n", false);
    let other = support::git_repo(sandbox.repo.parent().unwrap(), "other");
    sandbox.ok(&[
        "setup",
        "--project",
        other.to_str().unwrap(),
        "--client",
        "claude-code",
        "--bin",
        sandbox.bin.to_str().unwrap(),
    ]);
    let out = sandbox.run_in(
        &other,
        &["agents", "add", "reviewer", "--project", "myproj"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("reviewer is scoped to project myproj; run this from inside myproj"),
        "{err}"
    );
    assert!(!other.join(".claude/agents/reviewer.md").exists());
    sandbox.ok(&["agents", "add", "reviewer", "--project", "myproj"]);
    assert!(sandbox.claude("reviewer").exists());
}

#[test]
fn global_add_explains_project_agents_and_a_missing_global_setup() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read]", "scope: project\n", false);
    let err = sandbox.err(&["agents", "add", "--global", "sv-mid"]);
    assert!(err.contains("skillvolution setup"), "{err}");
    sandbox.setup_global("claude-code");
    let err = sandbox.err(&["agents", "add", "--global", "reviewer"]);
    assert!(
        err.contains(
            "reviewer is project-scoped; run `skillvolution agents add reviewer` (without --global) inside its project"
        ),
        "{err}"
    );
}

#[test]
fn force_overwrites_foreign_and_edited_files_after_a_backup() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    fs::create_dir_all(sandbox.claude("x").parent().unwrap()).unwrap();
    fs::write(sandbox.claude("sv-low"), "mine\n").unwrap();
    let err = sandbox.err(&["agents", "add", "sv-low"]);
    assert!(err.contains("--force"), "{err}");
    let out = sandbox.ok(&["agents", "add", "sv-low", "--force"]);
    assert!(out.contains("[overwritten] Claude Code sv-low"), "{out}");
    let backup = sandbox
        .claude("sv-low")
        .with_extension("md.skillvolution.bak");
    assert_eq!(read(&backup), "mine\n");

    sandbox.ok(&["agents", "add", "sv-mid"]);
    let path = sandbox.claude("sv-mid");
    let edited = read(&path).replace("Stay inside the task", "Roam freely");
    fs::write(&path, &edited).unwrap();
    let out = sandbox.ok(&["agents", "add", "sv-mid"]);
    assert!(out.contains("[edited]") && out.contains("--force"), "{out}");
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(out.contains("--apply --force"), "{out}");
    assert_eq!(read(&path), edited);
    let err = sandbox.err(&["agents", "sync", "--force"]);
    assert!(err.contains("--apply"), "{err}");
    let out = sandbox.ok(&["agents", "sync", "--apply", "--force"]);
    assert!(
        out.contains("[update] Claude Code sv-mid v1: edited by hand"),
        "{out}"
    );
    assert!(!read(&path).contains("Roam freely"));
    assert_eq!(read(&path.with_extension("md.skillvolution.bak")), edited);
}

#[test]
fn sync_lists_unchanged_agents_and_explains_a_rendering_only_update() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-mid"]);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(out.contains("[ok] Claude Code sv-mid v1"), "{out}");
    sandbox.ok(&[
        "agents",
        "models",
        "set",
        "claude-code",
        "mid",
        "claude-sonnet-4-5",
    ]);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(
        out.contains("[update] Claude Code sv-mid v1: model sonnet -> claude-sonnet-4-5"),
        "{out}"
    );
    assert!(!out.contains("v1 -> v1"), "{out}");
}

#[test]
fn user_and_project_copies_of_one_agent_are_warned_about() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    sandbox.setup_global("claude-code,opencode");
    sandbox.ok(&["agents", "add", "--global", "sv-mid"]);
    let out = sandbox.ok(&["agents", "add", "sv-mid"]);
    assert!(
        out.contains("[warning] Claude Code sv-mid: also installed at"),
        "{out}"
    );
    let sync = sandbox.ok(&["agents", "sync"]);
    assert!(sync.contains("also installed at"), "{sync}");
    let doctor = stdout(&sandbox.run(&["doctor"]));
    assert!(
        doctor.contains("sv-mid installed at both project and user level"),
        "{doctor}"
    );
    assert!(!doctor.contains("[warning] agent"), "{doctor}");
    assert!(sandbox.user_claude("sv-mid").exists());
}

#[test]
fn doctor_groups_missing_models_per_client_and_tier_and_names_the_fix() {
    let sandbox = Sandbox::new();
    sandbox.setup("opencode");
    sandbox.publish("[read]", "", false);
    sandbox.ok(&["agents", "add", "reviewer", "sv-mid", "sv-low"]);
    let doctor = stdout(&sandbox.run(&["doctor"]));
    assert_eq!(
        doctor.matches("no model for tier mid (").count(),
        1,
        "{doctor}"
    );
    assert!(
        doctor.contains("reviewer, sv-mid")
            && doctor.contains("agents models set opencode mid <model>`")
            && doctor.contains("no model for tier low (sv-low)"),
        "{doctor}"
    );
    sandbox.publish("[read, shell]", "", true);
    let doctor = stdout(&sandbox.run(&["doctor"]));
    assert!(
        doctor.contains(
            "is outdated (v1 vs v2 in the vault); run `skillvolution agents sync --apply`"
        ),
        "{doctor}"
    );
    sandbox.ok(&["deprecate", "reviewer"]);
    let doctor = stdout(&sandbox.run(&["doctor"]));
    assert!(doctor.contains("is deprecated in the vault"), "{doctor}");
}

#[test]
fn import_skips_existing_outputs_and_says_how_to_fix_a_missing_output_contract() {
    let sandbox = Sandbox::new();
    let native = sandbox.repo.join("natives");
    fs::create_dir_all(&native).unwrap();
    for name in ["aaa", "bbb"] {
        fs::write(
            native.join(format!("{name}.md")),
            format!("---\nname: {name}\ndescription: Does things\n---\n## Role\nx\n"),
        )
        .unwrap();
    }
    let out_dir = sandbox.repo.join("out");
    fs::create_dir_all(out_dir.join("aaa")).unwrap();
    fs::write(out_dir.join("aaa/AGENT.md"), "keep\n").unwrap();
    let out = sandbox.run(&[
        "agents",
        "import",
        native.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (stdout, stderr) = (
        stdout(&out),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        stdout.contains("[skipped]") && stdout.contains("aaa"),
        "{stdout}"
    );
    assert!(stdout.contains("Wrote") && out_dir.join("bbb/AGENT.md").is_file());
    assert_eq!(read(&out_dir.join("aaa/AGENT.md")), "keep\n");
    assert!(
        stderr.contains(
            "add a `## Output contract` section describing what the agent returns, then publish"
        ),
        "{stderr}"
    );
}

#[test]
fn the_agent_template_in_the_publish_error_actually_publishes() {
    let sandbox = Sandbox::new();
    sandbox.skill("some-skill");
    let bad = sandbox.repo.join("bad/AGENT.md");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, "---\nname: bad\n---\nnothing\n").unwrap();
    let err = sandbox.err(&["publish", bad.to_str().unwrap()]);
    let template = err
        .split("An AGENT.md looks like this:\n\n")
        .nth(1)
        .expect("template in the error")
        .trim_end()
        .replace("my-agent-id", "from-template");
    let good = sandbox.repo.join("good/AGENT.md");
    fs::create_dir_all(good.parent().unwrap()).unwrap();
    fs::write(&good, format!("{template}\n")).unwrap();
    let out = sandbox.ok(&["publish", good.to_str().unwrap()]);
    assert!(out.contains("Published from-template v1"), "{out}");
}

fn bak(path: &Path) -> PathBuf {
    path.with_extension("md.skillvolution.bak")
}

#[test]
fn force_backs_up_a_foreign_copy_of_a_generated_file() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-high"]);
    // A renamed copy carries the marker text but its id does not match the file name.
    let copy = sandbox.claude("sv-low");
    fs::copy(sandbox.claude("sv-high"), &copy).unwrap();
    let mut mine = read(&copy);
    mine.push_str("my own notes\n");
    fs::write(&copy, &mine).unwrap();
    let err = sandbox.err(&["agents", "add", "sv-low"]);
    assert!(err.contains("not generated by skillvolution"), "{err}");
    let out = sandbox.ok(&["agents", "add", "sv-low", "--force"]);
    assert!(out.contains("[overwritten] Claude Code sv-low"), "{out}");
    assert_eq!(read(&bak(&copy)), mine);
}

#[test]
fn force_backs_up_only_after_the_targets_validate_and_only_once_per_id() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-low"]);
    let low = sandbox.claude("sv-low");
    let edited = read(&low) + "Roam freely\n";
    fs::write(&low, &edited).unwrap();
    // sv-mid's target is a symlink: validation refuses before any backup is taken.
    #[cfg(unix)]
    {
        let elsewhere = sandbox.repo.join("elsewhere.md");
        fs::write(&elsewhere, "x").unwrap();
        std::os::unix::fs::symlink(&elsewhere, sandbox.claude("sv-mid")).unwrap();
        let err = sandbox.err(&["agents", "add", "sv-low", "sv-mid", "--force"]);
        assert!(err.contains("symlink refused"), "{err}");
        assert!(
            !bak(&low).exists(),
            "backup taken although nothing was written"
        );
        assert_eq!(read(&low), edited);
        fs::remove_file(sandbox.claude("sv-mid")).unwrap();
    }
    let out = sandbox.ok(&["agents", "add", "sv-low", "sv-low", "--force"]);
    assert_eq!(out.matches("[overwritten]").count(), 1, "{out}");
    assert_eq!(read(&bak(&low)), edited);
    assert!(!low.with_extension("md.skillvolution.bak.1").exists());
}

#[test]
fn remove_exits_nonzero_when_only_a_foreign_or_edited_file_exists() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    fs::create_dir_all(sandbox.claude("mine").parent().unwrap()).unwrap();
    fs::write(sandbox.claude("mine"), "---\nname: mine\n---\nbody\n").unwrap();
    let out = sandbox.run(&["agents", "remove", "mine"]);
    assert!(!out.status.success());
    assert!(
        stdout(&out).contains("[foreign] Claude Code mine"),
        "{}",
        stdout(&out)
    );
    sandbox.ok(&["agents", "add", "sv-low"]);
    let low = sandbox.claude("sv-low");
    fs::write(&low, read(&low) + "Roam freely\n").unwrap();
    let out = sandbox.run(&["agents", "remove", "sv-low"]);
    assert!(!out.status.success());
    assert!(
        stdout(&out).contains("[edited] Claude Code sv-low"),
        "{}",
        stdout(&out)
    );
    assert!(low.exists());
}

#[test]
fn import_reports_a_rejected_file_and_keeps_going() {
    let sandbox = Sandbox::new();
    let native = sandbox.repo.join("natives");
    fs::create_dir_all(&native).unwrap();
    fs::write(
        native.join("aaa.md"),
        "---\nname: aaa\nmode: subagent\ndescription: An OpenCode file\n---\nx\n",
    )
    .unwrap();
    fs::write(
        native.join("bbb.md"),
        "---\nname: bbb\ndescription: Does things\n---\n## Role\nx\n",
    )
    .unwrap();
    let out_dir = sandbox.repo.join("out");
    let out = sandbox.run(&[
        "agents",
        "import",
        native.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stdout = stdout(&out);
    assert!(
        stdout.contains("[rejected]") && stdout.contains("aaa.md"),
        "{stdout}"
    );
    assert!(stdout.contains("Wrote") && out_dir.join("bbb/AGENT.md").is_file());
}

#[test]
fn doctor_advice_for_user_level_files_says_global() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("opencode");
    sandbox.publish("[read]", "", false);
    sandbox.ok(&["agents", "add", "--global", "reviewer"]);
    let doctor = stdout(&sandbox.run(&["doctor"]));
    assert!(
        doctor.contains("no model for tier mid (reviewer) in OpenCode user-level files")
            && doctor.contains("`skillvolution agents sync --apply --global`"),
        "{doctor}"
    );
    sandbox.publish("[read, shell]", "", true);
    let doctor = stdout(&sandbox.run(&["doctor"]));
    assert!(
        doctor.contains(
            "is outdated (v1 vs v2 in the vault); run `skillvolution agents sync --apply --global`"
        ),
        "{doctor}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_home_still_counts_as_the_user_dir() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.join(".git")).unwrap();
    let link = sandbox.home.with_file_name("home-link");
    std::os::unix::fs::symlink(&sandbox.home, &link).unwrap();
    let in_home = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_skillvolution"))
            .arg("--db")
            .arg(&sandbox.db)
            .args(args)
            .env("HOME", &link)
            .env("XDG_CONFIG_HOME", link.join("xdg"))
            .env("APPDATA", link.join("appdata"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env("PATH", &sandbox.empty_path)
            .current_dir(&sandbox.home)
            .output()
            .unwrap()
    };
    let home = sandbox.home.to_str().unwrap();
    let setup = in_home(&[
        "setup",
        "--project",
        home,
        "--client",
        "claude-code",
        "--bin",
        sandbox.bin.to_str().unwrap(),
    ]);
    assert!(setup.status.success(), "{setup:?}");
    let file = sandbox.home.join("reviewer-src").join("AGENT.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        format!(
            "---\nname: reviewer\ndescription: Use when a diff needs review\ntier: mid\ntools: [read]\n---\n{BODY}"
        ),
    )
    .unwrap();
    let published = in_home(&["publish", file.to_str().unwrap(), "--scope", "project"]);
    assert!(published.status.success(), "{published:?}");
    let add = in_home(&["agents", "add", "reviewer"]);
    assert!(!add.status.success(), "{add:?}");
    assert!(
        String::from_utf8_lossy(&add.stderr).contains("reviewer is project-scoped"),
        "{add:?}"
    );
    assert!(!sandbox.home.join(".claude/agents/reviewer.md").exists());
}

#[test]
fn a_read_only_agent_denies_everything_else_and_sv_mid_denies_nothing() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read]", "", false);
    sandbox.ok(&["agents", "add", "reviewer", "sv-mid"]);
    let claude = read(&sandbox.claude("reviewer"));
    assert!(
        claude.lines().any(|l| l
            == "disallowedTools: Edit, Write, NotebookEdit, MultiEdit, Bash, PowerShell, Monitor, WebFetch, WebSearch, mcp__*, Agent, Task"),
        "{claude}"
    );
    let mid = read(&sandbox.claude("sv-mid"));
    assert!(!mid.contains("disallowedTools"), "{mid}");
}

/// Skill `helper` v1 and an agent `reviewer` using it, added to the sandbox.
fn agent_with_helper(sandbox: &Sandbox) {
    sandbox.setup("claude-code");
    sandbox.skill("helper");
    sandbox.publish("[read, shell, mcp]", "skills: [helper]\n", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
}

#[test]
fn same_version_number_with_new_content_changes_the_pin_and_sync_says_so() {
    let sandbox = Sandbox::new();
    agent_with_helper(&sandbox);
    let before = read(&sandbox.claude("reviewer"));
    // v1 is the only published version: purging it is refused like a full purge.
    let err = sandbox.err(&["purge", "helper", "--version", "1"]);
    assert!(err.contains("used by agent(s) reviewer"), "{err}");
    assert!(err.contains("--force"), "{err}");
    sandbox.ok(&["purge", "helper", "--force"]);
    let mut vault = skillvolution::vault::Vault::open(&sandbox.db).unwrap();
    support::Draft::new("helper")
        .content("A different body")
        .publish(&mut vault);
    drop(vault);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(
        out.contains("[update] Claude Code reviewer v1: skill helper v1 content changed"),
        "{out}"
    );
    assert!(out.contains("  ! skill helper v1 content changed"), "{out}");
    assert!(!out.contains("rendering changed"), "{out}");
    sandbox.ok(&["agents", "sync", "--apply"]);
    let after = read(&sandbox.claude("reviewer"));
    assert_ne!(before, after);
    assert!(after.contains("helper v1 (#"), "{after}");
    assert!(
        sandbox
            .ok(&["agents", "sync"])
            .contains("[ok] Claude Code reviewer")
    );
}

#[test]
fn purging_a_published_skill_version_is_refused_for_its_agents_even_with_older_ones_kept() {
    let sandbox = Sandbox::new();
    agent_with_helper(&sandbox);
    let mut vault = skillvolution::vault::Vault::open(&sandbox.db).unwrap();
    support::Draft::new("helper")
        .expected_version(1)
        .publish(&mut vault);
    drop(vault);
    let err = sandbox.err(&["purge", "helper", "--version", "2"]);
    assert!(err.contains("used by agent(s) reviewer"), "{err}");
}

#[test]
fn import_strips_a_pin_with_or_without_version_and_hash() {
    let sandbox = Sandbox::new();
    agent_with_helper(&sandbox);
    let path = sandbox.claude("reviewer");
    let generated = read(&path);
    let pinned = generated
        .lines()
        .find(|l| l.starts_with("Before starting"))
        .unwrap()
        .to_owned();
    for item in ["helper", "helper v1", "helper v1 (#0badf00d)"] {
        let start = pinned.find("do not pass it): ").unwrap() + "do not pass it): ".len();
        let edited = generated.replace(&pinned, &format!("{}{item}.", &pinned[..start]));
        // The file name must equal the id, or the marker is rejected and the foreign
        // path would be tested instead.
        let file = sandbox.repo.join("scratch").join("reviewer.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, edited).unwrap();
        // An edited file still has our marker; import reads the vault entry, and the
        // edited line must be stripped whatever its form.
        let out = sandbox.ok(&["agents", "import", file.to_str().unwrap()]);
        assert!(
            !out.contains("Before starting") && out.contains("skills: [helper]"),
            "{item}: {out}"
        );
    }
}

#[test]
fn unavailable_skills_are_left_out_and_sync_flags_the_pin_that_disappears_and_returns() {
    let sandbox = Sandbox::new();
    agent_with_helper(&sandbox);
    sandbox.ok(&["purge", "helper", "--force"]);
    let out = sandbox.ok(&["agents", "sync"]);
    assert!(
        out.contains("[warning] agent reviewer: skill helper is not available"),
        "{out}"
    );
    assert!(
        out.contains("[update] Claude Code reviewer v1: skill helper dropped (not available)"),
        "{out}"
    );
    assert!(
        out.contains("  ! skill helper dropped (not available)"),
        "{out}"
    );
    sandbox.ok(&["agents", "sync", "--apply"]);
    let bare = read(&sandbox.claude("reviewer"));
    assert!(
        !bare.contains("Before starting") && !bare.contains("report_skill_outcome"),
        "{bare}"
    );
    assert!(!bare.contains("the current one"), "{bare}");
    // `add` still warns for a skill that is gone.
    let added = sandbox.ok(&["agents", "add", "reviewer"]);
    assert!(added.contains("skill helper is not available"), "{added}");
    sandbox.skill("helper");
    let back = sandbox.ok(&["agents", "sync"]);
    assert!(back.contains("  ! skill helper now v1"), "{back}");
    assert!(
        back.contains("[update] Claude Code reviewer v1: skill helper now v1"),
        "{back}"
    );
}

#[test]
fn deprecated_skills_stay_usable_but_add_sync_and_doctor_warn() {
    let sandbox = Sandbox::new();
    agent_with_helper(&sandbox);
    sandbox.ok(&["deprecate", "helper"]);
    let line = "[warning] agent reviewer: skill helper is deprecated";
    assert!(sandbox.ok(&["agents", "add", "reviewer"]).contains(line));
    let synced = sandbox.ok(&["agents", "sync"]);
    assert!(synced.contains(line), "{synced}");
    assert!(synced.contains("[ok] Claude Code reviewer"), "{synced}");
    let doctor = sandbox.run(&["doctor"]);
    let shown = format!("{}{}", stdout(&doctor), stderr(&doctor));
    assert!(
        shown.contains("agent reviewer: skill helper is deprecated"),
        "{shown}"
    );
    assert!(read(&sandbox.claude("reviewer")).contains("helper v1 (#"));
}

#[test]
fn importing_a_managed_project_agent_keeps_its_scope_and_tags() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish(
        "[read, mcp]",
        "scope: project\ntags: [alpha, beta]\n",
        false,
    );
    sandbox.ok(&["agents", "add", "reviewer"]);
    let imported = sandbox.ok(&[
        "agents",
        "import",
        sandbox.claude("reviewer").to_str().unwrap(),
    ]);
    assert!(
        imported.contains("scope: project\n") && imported.contains("tags: [alpha, beta]\n"),
        "{imported}"
    );
    let file = sandbox.repo.join("re-import/AGENT.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, imported).unwrap();
    let out = sandbox.ok(&["publish", file.to_str().unwrap(), "--replace"]);
    assert!(out.contains("Published reviewer v2"), "{out}");
}

#[test]
#[cfg(unix)]
fn symlinked_targets_are_never_followed_by_add_remove_or_sync() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.ok(&["agents", "add", "sv-mid"]);
    let path = sandbox.claude("sv-mid");
    let generated = read(&path);
    let target = sandbox.repo.join("elsewhere.md");
    fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let intact = |what: &str| {
        assert!(path.symlink_metadata().unwrap().is_symlink(), "{what}");
        assert_eq!(read(&target), generated, "{what}");
    };
    let err = sandbox.err(&["agents", "add", "sv-mid"]);
    assert!(err.contains("not generated by skillvolution"), "{err}");
    intact("add");
    let out = sandbox.run(&["agents", "remove", "sv-mid"]);
    let removed = stdout(&out);
    assert!(removed.contains("[foreign]"), "{removed}");
    intact("remove");
    // A model change makes sync want to rewrite the file; through the link it must not.
    sandbox.ok(&[
        "agents",
        "models",
        "set",
        "claude-code",
        "mid",
        "other-model",
    ]);
    let out = sandbox.run(&["agents", "sync", "--apply"]);
    let shown = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!shown.contains("other-model -> "), "{shown}");
    intact("sync");
}

#[test]
fn publishing_a_skill_over_an_agent_id_is_refused_and_leaves_the_agent_alone() {
    let sandbox = Sandbox::new();
    sandbox.publish("[read, mcp]", "", false);
    let before = sandbox.ok(&["get", "reviewer"]);
    let file = sandbox.repo.join("as-skill/SKILL.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        format!(
            "---\nname: reviewer\ndescription: Use when a diff needs review\n---\n{}",
            support::sectioned("Steps")
        ),
    )
    .unwrap();
    let out = sandbox.run(&["publish", file.to_str().unwrap(), "--replace"]);
    let shown = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{shown}");
    assert!(
        shown.contains("reviewer already exists as an agent"),
        "{shown}"
    );
    assert_eq!(sandbox.ok(&["get", "reviewer"]), before);
    assert!(sandbox.ok(&["show", "reviewer"]).contains("v1"));
}

#[test]
fn an_agent_keeps_its_own_scope_for_skill_availability() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.skill("helper");
    sandbox.publish("[read, shell, mcp]", "skills: [helper]\n", false);
    sandbox.ok(&["purge", "helper", "--force"]);
    let mut vault = skillvolution::vault::Vault::open(&sandbox.db).unwrap();
    support::Draft::new("helper")
        .scope("myproj")
        .publish(&mut vault);
    drop(vault);
    let out = sandbox.ok(&["agents", "add", "reviewer"]);
    assert!(
        out.contains("[warning] agent reviewer: skill helper is not available"),
        "{out}"
    );
    assert!(!read(&sandbox.claude("reviewer")).contains("helper v1"));
}

#[test]
fn importing_an_edited_managed_file_notes_edits_it_ignores_and_an_older_version() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("[read, shell]", "skills: []\n", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let generated = read(&sandbox.claude("reviewer"));
    let edited: String = generated
        .lines()
        .map(|l| {
            if l.starts_with("model:") {
                "model: claude-opus-4-1\neffort: high".to_owned()
            } else if l.starts_with("disallowedTools:") {
                "disallowedTools: Agent, Task".to_owned()
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let file = sandbox.repo.join("scratch").join("reviewer.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, edited).unwrap();
    let out = sandbox.run(&["agents", "import", file.to_str().unwrap()]);
    let (text, notes) = (stdout(&out), stderr(&out));
    assert!(text.contains("tier: mid\n"), "{text}");
    for field in ["model", "effort", "tools"] {
        assert!(
            notes.contains(&format!("`{field}` edit in the file not imported")),
            "{field}: {notes}"
        );
    }
    assert!(!notes.contains("`skills` edit"), "{notes}");

    // A file older than the vault entry is said so.
    sandbox.publish("[read]", "skills: []\n", true);
    let out = sandbox.run(&[
        "agents",
        "import",
        sandbox.claude("reviewer").to_str().unwrap(),
    ]);
    assert!(
        stderr(&out).contains("file is from v1; the vault is at v2"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn purging_the_current_agent_version_warns_about_skills_the_older_one_lists() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.skill("helper");
    sandbox.publish("[read, shell, mcp]", "skills: [helper]\n", false);
    sandbox.publish("[read, shell]", "skills: []\n", true);
    sandbox.ok(&["purge", "helper", "--force"]);
    let out = sandbox.ok(&["purge", "reviewer", "--version", "2"]);
    assert!(
        out.contains("[warning] agent reviewer: skill helper is not available"),
        "{out}"
    );
}
