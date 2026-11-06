#![allow(dead_code)]

use skillvolution::vault::{Proposal, Revision, Vault};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A valid agent body with every section.
pub const BODY: &str = "## Role\nReviews diffs.\n## Procedure\n1. Read the diff.\n## Output contract\nA list of findings.\n## Limits\nNever edits files.\n";

/// A process-wide empty directory used as HOME by CLI helpers that have no
/// sandbox of their own, so they never read or write the developer's real
/// configuration. Lives under the cargo target dir, not the system temp dir.
pub fn shared_home() -> PathBuf {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("isolated-home");
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// A `skillvolution` command isolated from the host environment, with `home`
/// as the user's home. Callers layer their own overrides (HOME, XDG_*, PATH,
/// cwd, ...) on top; whatever they leave alone stays isolated.
///
/// Sets HOME, XDG_CONFIG_HOME and XDG_DATA_HOME (plus USERPROFILE, APPDATA and
/// LOCALAPPDATA on Windows) under `home`, and clears every variable the binary
/// reads that could point at real state: CLAUDE_CONFIG_DIR (Claude Code config
/// dir), CLAUDE_PROJECT_DIR (project key fallback), CODEX_HOME and
/// GEMINI_CLI_HOME (Codex/Gemini config roots), SKILLVOLUTION_DB (vault path),
/// SKILLVOLUTION_NO_INPUT (prompt behaviour) and
/// SKILLVOLUTION_CLAUDE_TIMEOUT_SECS (`claude mcp` timeout). The working
/// directory is `home`, so nothing depends on the repo checkout.
pub fn isolated(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillvolution"));
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"));
    if cfg!(windows) {
        cmd.env("USERPROFILE", home)
            .env("APPDATA", home.join("AppData/Roaming"))
            .env("LOCALAPPDATA", home.join("AppData/Local"));
    }
    for var in [
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_PROJECT_DIR",
        "CODEX_HOME",
        "GEMINI_CLI_HOME",
        "SKILLVOLUTION_DB",
        "SKILLVOLUTION_NO_INPUT",
        "SKILLVOLUTION_CLAUDE_TIMEOUT_SECS",
    ] {
        cmd.env_remove(var);
    }
    cmd.current_dir(home);
    cmd
}

/// `isolated` with the shared sandbox home.
pub fn cli() -> Command {
    isolated(&shared_home())
}

/// Runs the `skillvolution` binary against `db`, isolated from the host.
pub fn run(db: &Path, args: &[&str]) -> Output {
    cli().arg("--db").arg(db).args(args).output().unwrap()
}

/// A fresh directory for databases that must not touch the disk: `/dev/shm`
/// when it exists and is writable, else the normal temp dir.
pub fn fast_tempdir() -> tempfile::TempDir {
    let shm = Path::new("/dev/shm");
    if shm.is_dir()
        && let Ok(dir) = tempfile::Builder::new()
            .prefix("skillvolution-")
            .tempdir_in(shm)
    {
        return dir;
    }
    tempfile::tempdir().unwrap()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Creates `parent/name` as a git repository root (just enough for
/// `skillvolution::project::detect` to recognize it: a `.git` directory) and
/// returns its path. `name` should already be a valid project key so the
/// detected key matches it exactly.
pub fn git_repo(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    repo
}

/// Wraps a test body in the four sections `propose` requires, so a test can
/// keep passing the one distinctive line it searches for or diffs.
pub fn sectioned(body: &str) -> String {
    format!(
        "## When to use\n{body}\n## Procedure\n1. Apply the body above.\n## Pitfalls\n- None known.\n## Verification\nRe-run the command.\n"
    )
}

/// Builds a `Proposal` with valid filler defaults, overridden field by field.
/// Owns its strings so the borrowed `Proposal` it hands out can outlive the
/// builder call chain.
pub struct Draft {
    id: String,
    description: String,
    tags: Vec<String>,
    content: String,
    evidence: String,
    expected_version: i64,
    scope: Option<String>,
    /// Left unset by default so `proposal` can derive the verdict that agrees
    /// with `scope`; set it explicitly to exercise a mismatch.
    verdict: Option<String>,
    verdict_reason: String,
    replaces_proven: bool,
}

impl Draft {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            description: format!("Use when working with {id}"),
            tags: Vec::new(),
            content: sectioned(&format!("Body for {id}")),
            evidence: "Observed / Tried / Result".to_owned(),
            expected_version: 0,
            scope: None,
            verdict: None,
            verdict_reason: "Verified and reusable".to_owned(),
            replaces_proven: false,
        }
    }

    pub fn description(mut self, value: &str) -> Self {
        self.description = value.to_owned();
        self
    }

    pub fn content(mut self, value: &str) -> Self {
        self.content = sectioned(value);
        self
    }

    /// Sets `content` to the literal value, unwrapped — for tests that need
    /// to exercise invalid or exact content rather than the auto-sectioned
    /// default.
    pub fn raw_content(mut self, value: &str) -> Self {
        self.content = value.to_owned();
        self
    }

    pub fn evidence(mut self, value: &str) -> Self {
        self.evidence = value.to_owned();
        self
    }

    pub fn expected_version(mut self, value: i64) -> Self {
        self.expected_version = value;
        self
    }

    pub fn tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|t| t.to_string()).collect();
        self
    }

    pub fn scope(mut self, value: &str) -> Self {
        self.scope = Some(value.to_owned());
        self
    }

    /// Pins the verdict instead of deriving it from `scope` — for tests that
    /// need a rejected or contradictory verdict.
    pub fn verdict(mut self, value: &str) -> Self {
        self.verdict = Some(value.to_owned());
        self
    }

    pub fn verdict_reason(mut self, value: &str) -> Self {
        self.verdict_reason = value.to_owned();
        self
    }

    pub fn replaces_proven(mut self, value: bool) -> Self {
        self.replaces_proven = value;
        self
    }

    pub fn proposal(&self) -> Proposal<'_> {
        // Unless a test pins one, the verdict follows the scope, so every
        // existing `.scope(...)` draft still satisfies the verdict/scope cross
        // check.
        let verdict = self.verdict.as_deref().unwrap_or(if self.scope.is_some() {
            "keep project"
        } else {
            "keep global"
        });
        Proposal {
            id: &self.id,
            description: &self.description,
            tags: &self.tags,
            content: &self.content,
            evidence: &self.evidence,
            expected_version: self.expected_version,
            scope: self.scope.as_deref(),
            verdict,
            verdict_reason: &self.verdict_reason,
            replaces_proven: self.replaces_proven,
        }
    }

    /// Proposes the draft, panicking on failure (test setup, not the case under test).
    /// Proposals are published immediately, so the returned revision is live.
    pub fn propose(&self, vault: &mut Vault) -> Revision {
        vault.propose(&self.proposal()).unwrap()
    }

    /// Publishes the draft as an agent (mid tier, no tools restriction) with a
    /// valid agent body; returns the published version.
    pub fn publish_agent(self, vault: &mut Vault) -> i64 {
        let body = "## Role\nReviews diffs.\n## Procedure\n1. Read the diff.\n## Output contract\nFindings.\n## Limits\nNever edits files.\n";
        let draft = self.raw_content(body);
        let meta = skillvolution::vault::AgentMeta {
            role: "subagent".into(),
            tier: "mid".into(),
            effort: None,
            tools: None,
            skills: vec![],
        };
        vault
            .propose_agent(&draft.proposal(), &meta)
            .unwrap()
            .version
    }

    /// Proposes it and returns the published version.
    pub fn publish(&self, vault: &mut Vault) -> i64 {
        self.propose(vault).version
    }
}
