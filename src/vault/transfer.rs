//! Whole-vault operations: hard deletion (`purge`), JSON export/import, and
//! online backups.

use super::Vault;
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::Path;

/// What `purge` removed.
#[derive(Debug, Serialize)]
pub struct PurgeReport {
    pub revisions: usize,
    pub outcomes: usize,
    /// True when the skill itself (no revisions left) was removed.
    pub skill_removed: bool,
}

/// What `import_json` merged into the vault.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub revisions_imported: usize,
    pub revisions_skipped: usize,
    pub outcomes_imported: usize,
    pub outcomes_skipped: usize,
}

impl Vault {
    /// Permanently deletes one revision (`version = Some`) or a whole skill,
    /// including its outcomes and search-index rows.
    pub fn purge(&mut self, _id: &str, _version: Option<i64>) -> Result<PurgeReport> {
        bail!("purge is not implemented yet")
    }

    /// Serializes every skill, revision, and outcome as a versioned JSON document.
    pub fn export_json(&self) -> Result<String> {
        bail!("export is not implemented yet")
    }

    /// Merges a document produced by `export_json` into this vault.
    pub fn import_json(&mut self, _json: &str) -> Result<ImportReport> {
        bail!("import is not implemented yet")
    }

    /// Writes a consistent copy of the live database to `dest`.
    pub fn backup(&self, _dest: &Path) -> Result<()> {
        bail!("backup is not implemented yet")
    }
}
