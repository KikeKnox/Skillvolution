#![allow(dead_code)]

use skillvolution::vault::{Proposal, Revision, Vault};

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
}

impl Draft {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            description: format!("Use when working with {id}"),
            tags: Vec::new(),
            content: format!("Body for {id}"),
            evidence: "Observed / Tried / Result".to_owned(),
            expected_version: 0,
            scope: None,
        }
    }

    pub fn description(mut self, value: &str) -> Self {
        self.description = value.to_owned();
        self
    }

    pub fn content(mut self, value: &str) -> Self {
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

    pub fn proposal(&self) -> Proposal<'_> {
        Proposal {
            id: &self.id,
            description: &self.description,
            tags: &self.tags,
            content: &self.content,
            evidence: &self.evidence,
            expected_version: self.expected_version,
            scope: self.scope.as_deref(),
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
