//! What `hook session-start` does about agents: the catalog's agents block, the refresh
//! of stale managed files, the daily promotion to a per-user install and the retirement
//! of promoted agents' project copies. Every command runs with HOME and every XDG
//! directory inside the sandbox, never the developer's own.

mod support;

use serde_json::{Value, json};
use skillvolution::clients::{ClientKind, Version};
use skillvolution::vault::{Event, EventRecord, Vault};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
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

    fn run_in(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = support::isolated(&self.home);
        command
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("APPDATA", self.home.join("AppData/Roaming"))
            .env("LOCALAPPDATA", self.home.join("AppData/Local"))
            .env("USERPROFILE", &self.home)
            .env("PATH", &self.empty_path)
            .current_dir(dir);
        command.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run_in(&self.repo, args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        stdout(&out)
    }

    /// Project setup of `clients` for the sandbox repo.
    fn setup(&self, clients: &str) {
        let (repo, bin) = (self.repo.to_str().unwrap(), self.bin.to_str().unwrap());
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

    /// Per-user setup of `clients` inside the sandbox HOME.
    fn setup_global(&self, clients: &str) {
        self.ok(&[
            "setup",
            "--client",
            clients,
            "--bin",
            self.bin.to_str().unwrap(),
        ]);
    }

    fn publish(&self, id: &str, tier: &str, description: &str, replace: bool) {
        let file = self.repo.join(format!("{id}-src")).join("AGENT.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            format!(
                "---\nname: {id}\ndescription: {description}\ntier: {tier}\ntools: [read]\n---\n{BODY}"
            ),
        )
        .unwrap();
        let mut args = vec!["publish", file.to_str().unwrap()];
        if replace {
            args.push("--replace");
        }
        self.ok(&args);
    }

    /// The context `hook session-start` printed in the repo for `client`.
    fn session_start(&self, client: &str) -> String {
        let out = self.run_in(&self.repo, &["hook", "session-start", "--client", client]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(stderr(&out).is_empty(), "hook errors: {}", stderr(&out));
        let output: Value = serde_json::from_str(&stdout(&out)).unwrap();
        output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .or_else(|| output["additional_context"].as_str())
            .unwrap()
            .to_owned()
    }

    fn claude(&self, id: &str) -> PathBuf {
        self.repo.join(".claude/agents").join(format!("{id}.md"))
    }

    fn user_claude(&self, id: &str) -> PathBuf {
        self.home.join(".claude/agents").join(format!("{id}.md"))
    }

    fn vault(&self) -> Vault {
        Vault::open(&self.db).unwrap()
    }

    fn events(&self, kind: &str) -> Vec<EventRecord> {
        self.vault().list_events(Some(kind), None, 1000).unwrap()
    }

    fn key(&self) -> String {
        skillvolution::project::detect(&self.repo).unwrap()
    }

    /// `projects` other projects that installed `id` and then delegated to it
    /// `delegations` times each.
    fn usage(&self, id: &str, projects: usize, delegations: usize) {
        let vault = self.vault();
        for p in 0..projects {
            let project = format!("other{p}");
            vault
                .record_event(
                    Event::new("agent_install")
                        .project(Some(&project))
                        .subject(id, Some(1))
                        .data(json!({"scope": "project"})),
                )
                .unwrap();
            for _ in 0..delegations {
                vault
                    .record_event(
                        Event::new("delegation")
                            .project(Some(&project))
                            .subject(id, Some(1))
                            .data(json!({"source": "vault", "level": "mid", "model": null})),
                    )
                    .unwrap();
            }
        }
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn marker_version(path: &Path) -> String {
    let text = read(path);
    let marker = text
        .lines()
        .find(|line| line.starts_with("<!-- skillvolution-managed:agent:"))
        .unwrap();
    marker.split(':').nth(3).unwrap().to_owned()
}

#[test]
fn the_catalog_lists_installed_and_available_agents_without_bodies() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.publish("planner", "high", "Use when a task needs a plan", false);
    sandbox.ok(&["agents", "add", "reviewer"]);

    let text = sandbox.session_start("claude-code");
    assert!(
        text.contains(
            "Skillvolution agents: before delegating to Claude Code's own agents, check these; \
             delegate to an installed one by its id, or install one with install_agent"
        ),
        "{text}"
    );
    assert!(
        text.contains("- reviewer v1 (mid) [installed: local]: Use when a diff needs review\n"),
        "{text}"
    );
    assert!(
        text.contains("- planner v1 (high) [not installed]: Use when a task needs a plan\n"),
        "{text}"
    );
    assert!(!text.contains("Reviews diffs."), "never a body: {text}");
    assert!(!text.contains("Agent files updated"), "{text}");

    // The OpenCode plugin asks for OpenCode's agents.
    let opencode = sandbox.session_start("opencode");
    assert!(
        opencode.contains("before delegating to OpenCode's own agents")
            && opencode.contains("- reviewer v1 (mid) [installed: local]"),
        "{opencode}"
    );
    // A client without subagents gets the skills catalog only.
    let codex = sandbox.session_start("codex");
    assert!(codex.contains("Skillvolution vault"), "{codex}");
    assert!(!codex.contains("Skillvolution agents"), "{codex}");
}

#[test]
fn the_agents_block_is_omitted_when_there_are_no_agents() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    let text = sandbox.session_start("claude-code");
    assert!(text.contains("Skillvolution vault"), "{text}");
    assert!(!text.contains("agents"), "{text}");
}

#[test]
fn session_start_updates_stale_managed_files_but_never_edited_ones() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.publish("planner", "high", "Use when a task needs a plan", false);
    sandbox.ok(&["agents", "add", "reviewer", "planner"]);
    let edited = read(&sandbox.claude("planner")).replace("Reviews diffs.", "My own notes.");
    fs::write(sandbox.claude("planner"), &edited).unwrap();
    sandbox.publish("reviewer", "mid", "Use when a diff needs a review", true);
    sandbox.publish("planner", "high", "Use when a task needs planning", true);

    let text = sandbox.session_start("claude-code");
    assert_eq!(marker_version(&sandbox.claude("reviewer")), "v2");
    assert!(read(&sandbox.claude("reviewer")).contains("a diff needs a review"));
    assert_eq!(
        read(&sandbox.claude("planner")),
        edited,
        "hand edits are kept"
    );
    assert!(
        text.contains("Agent files updated to the vault's current version: reviewer v2 (local).\n"),
        "{text}"
    );
    assert!(
        text.contains("- planner v2 (high) [installed: local v1]"),
        "{text}"
    );
    let updates = sandbox.events("agent_update");
    assert_eq!(updates.len(), 1, "only the managed file");
    assert_eq!(updates[0].subject.as_deref(), Some("reviewer"));
    assert_eq!(updates[0].version, Some(2));
    assert_eq!(updates[0].project.as_deref(), Some(sandbox.key().as_str()));
    assert_eq!(updates[0].data["scope"], "project");

    // Up to date now: nothing more to say.
    let again = sandbox.session_start("claude-code");
    assert!(!again.contains("Agent files updated"), "{again}");
    assert_eq!(sandbox.events("agent_update").len(), 1);
}

#[test]
fn a_subagent_session_gets_no_agents_block_and_runs_no_upkeep() {
    let sandbox = Sandbox::new();
    sandbox.setup("opencode");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.publish("reviewer", "mid", "Use when a diff needs a review", true);
    let path = sandbox.repo.join(".opencode/agents/reviewer.md");
    let stale = read(&path);
    let out = sandbox.run_in(
        &sandbox.repo,
        &[
            "hook",
            "session-start",
            "--client",
            "opencode",
            "--subagent",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let output: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let text = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("Skillvolution vault"), "{text}");
    assert!(!text.contains("Skillvolution agents"), "{text}");
    assert!(!text.contains("install_agent"), "{text}");
    assert_eq!(read(&path), stale, "no upkeep");
    assert!(sandbox.events("agent_update").is_empty());
    // The top-level session does both.
    let text = sandbox.session_start("opencode");
    assert!(text.contains("Skillvolution agents"), "{text}");
    assert_eq!(marker_version(&path), "v2");
}

#[test]
fn the_updated_line_names_only_the_session_clients_files() {
    let sandbox = Sandbox::new();
    sandbox.setup("claude-code,opencode");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.publish("reviewer", "mid", "Use when a diff needs a review", true);
    let text = sandbox.session_start("claude-code");
    assert!(
        text.contains("Agent files updated to the vault's current version: reviewer v2 (local).\n"),
        "{text}"
    );
    assert_eq!(
        sandbox.events("agent_update").len(),
        2,
        "both clients' files"
    );
}

#[test]
fn an_update_tells_a_client_without_hot_reload_to_restart() {
    let sandbox = Sandbox::new();
    sandbox.setup("opencode");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.publish("reviewer", "mid", "Use when a diff needs a review", true);
    sandbox
        .vault()
        .record_client_version(ClientKind::OpenCode, Version(1, 14, 0))
        .unwrap();
    let text = sandbox.session_start("opencode");
    assert!(
        text.contains("reviewer v2 (local); restart OpenCode to load them.\n"),
        "{text}"
    );
}

#[test]
fn a_heavily_used_agent_is_promoted_and_its_project_copies_retired_later() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let exclude = sandbox.repo.join(".git/info/exclude");
    assert!(read(&exclude).contains("/.claude/agents/reviewer.md"));
    sandbox.usage("reviewer", 6, 3);

    // The promotion installs per user and leaves this project's copy alone for now.
    let text = sandbox.session_start("claude-code");
    assert!(sandbox.user_claude("reviewer").exists(), "{text}");
    assert!(sandbox.claude("reviewer").exists());
    let promoted = sandbox.events("agent_promote");
    assert_eq!(promoted.len(), 1);
    assert_eq!(promoted[0].subject.as_deref(), Some("reviewer"));
    assert_eq!(promoted[0].data["projects"], 6);
    assert_eq!(promoted[0].data["clients"], json!(["claude-code"]));
    let installs = sandbox.events("agent_install");
    assert!(
        installs
            .iter()
            .any(|e| e.data["scope"] == "global" && e.project.is_none()),
        "{:?}",
        installs.iter().map(|e| &e.data).collect::<Vec<_>>()
    );
    let status = sandbox.ok(&["agents", "promotion"]);
    assert!(status.contains("Promoted: reviewer."), "{status}");
    assert!(status.contains("Candidates: none."), "{status}");

    // The next session start in the project retires its managed copy.
    sandbox.session_start("claude-code");
    assert!(!sandbox.claude("reviewer").exists());
    assert!(
        !read(&exclude).contains("reviewer.md"),
        "{}",
        read(&exclude)
    );
    assert!(sandbox.user_claude("reviewer").exists());
    let removed = sandbox.events("agent_remove");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].project.as_deref(), Some(sandbox.key().as_str()));
    assert_eq!(removed[0].data["scope"], "project");
    assert_eq!(sandbox.events("agent_promote").len(), 1, "once a day");
}

#[test]
fn a_stale_project_copy_of_a_promoted_agent_is_retired_not_rewritten() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    sandbox.usage("reviewer", 6, 3);
    sandbox.session_start("claude-code");
    assert!(sandbox.user_claude("reviewer").exists());
    sandbox.publish("reviewer", "mid", "Use when a diff needs a review", true);
    let text = sandbox.session_start("claude-code");
    assert!(!sandbox.claude("reviewer").exists());
    assert_eq!(marker_version(&sandbox.user_claude("reviewer")), "v2");
    let updates = sandbox.events("agent_update");
    assert!(
        updates.iter().all(|e| e.data["scope"] == "global"),
        "the project copy was retired, not rewritten: {:?}",
        updates.iter().map(|e| &e.data).collect::<Vec<_>>()
    );
    assert!(
        text.contains(
            "Agent files updated to the vault's current version: reviewer v2 (global).\n"
        ),
        "{text}"
    );
}

#[test]
fn an_edited_project_copy_of_a_promoted_agent_is_kept() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.ok(&["agents", "add", "reviewer"]);
    let edited = read(&sandbox.claude("reviewer")).replace("Reviews diffs.", "Mine.");
    fs::write(sandbox.claude("reviewer"), &edited).unwrap();
    sandbox.usage("reviewer", 6, 3);
    sandbox.session_start("claude-code");
    sandbox.session_start("claude-code");
    assert!(sandbox.user_claude("reviewer").exists());
    assert_eq!(read(&sandbox.claude("reviewer")), edited);
    assert!(sandbox.events("agent_remove").is_empty());
}

#[test]
fn project_scoped_and_lightly_used_agents_are_never_promoted() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.usage("reviewer", 5, 10);
    let file = sandbox.repo.join("local-src/AGENT.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        format!("---\nname: local-agent\ndescription: Use when local\ntier: low\ntools: [read]\n---\n{BODY}"),
    )
    .unwrap();
    sandbox.ok(&["publish", file.to_str().unwrap(), "--scope", "project"]);
    sandbox.usage("local-agent", 6, 3);
    sandbox.session_start("claude-code");
    assert!(
        !sandbox.user_claude("reviewer").exists(),
        "5 projects are not enough"
    );
    assert!(
        !sandbox.user_claude("local-agent").exists(),
        "project scope"
    );
    assert!(sandbox.events("agent_promote").is_empty());
}

#[test]
fn doctor_reports_candidates_and_promotes_only_with_fix() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.usage("reviewer", 6, 3);
    let doctor = sandbox.run_in(&sandbox.repo, &["doctor"]);
    let report = stdout(&doctor);
    assert!(
        report.contains(
            "[warn] agents: promotion: candidates for a per-user install: reviewer (6/6 active projects, 3.0 delegations each)"
        ),
        "{report}"
    );
    assert!(!sandbox.user_claude("reviewer").exists(), "read-only");
    assert!(sandbox.events("agent_promote").is_empty());

    let fixed = sandbox.run_in(&sandbox.repo, &["doctor", "--fix"]);
    let report = stdout(&fixed);
    assert!(
        report.contains("[fixed] agents: promotion: installed per user: reviewer"),
        "{report}"
    );
    assert!(
        report.contains("[ok] agents: promotion: promoted to a per-user install: reviewer"),
        "{report}"
    );
    assert!(sandbox.user_claude("reviewer").exists());
    assert_eq!(sandbox.events("agent_promote").len(), 1);
}

#[test]
fn promotion_thresholds_are_configurable() {
    let sandbox = Sandbox::new();
    sandbox.setup_global("claude-code");
    sandbox.publish("reviewer", "mid", "Use when a diff needs review", false);
    sandbox.usage("reviewer", 3, 2);
    let defaults = sandbox.ok(&["agents", "promotion"]);
    assert!(
        defaults.contains(
            "Promotion thresholds: window 30d, min share 75%, min projects 6, min delegations 3"
        ),
        "{defaults}"
    );
    assert!(
        defaults.contains("Active projects in the window: 3."),
        "{defaults}"
    );
    assert!(defaults.contains("Candidates: none."), "{defaults}");
    let changed = sandbox.ok(&[
        "agents",
        "promotion",
        "--window",
        "14d",
        "--min-projects",
        "3",
        "--min-delegations",
        "2",
    ]);
    assert!(
        changed.contains("Saved the promotion thresholds."),
        "{changed}"
    );
    assert!(
        changed.contains("window 14d, min share 75%, min projects 3, min delegations 2"),
        "{changed}"
    );
    assert!(
        changed.contains("Candidates: reviewer (3/3 active projects, 2.0 delegations each)."),
        "{changed}"
    );
    let bad = sandbox.run_in(&sandbox.repo, &["agents", "promotion", "--min-share", "0"]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("--min-share"), "{}", stderr(&bad));
}
