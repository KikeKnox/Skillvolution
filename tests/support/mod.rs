#![allow(dead_code)]

use skillvolution::vault::{Proposal, Revision, Vault};

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

    /// Proposes it and returns the published version.
    pub fn publish(&self, vault: &mut Vault) -> i64 {
        self.propose(vault).version
    }
}
