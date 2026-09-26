//! Cursor: not configured yet (scaffold; setup writes nothing for it).

use super::{Change, Edit, Scope, common};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The per-user config directory: `~/.cursor`.
pub(super) fn global_dir() -> Result<PathBuf> {
    common::home()
        .map(|home| home.join(".cursor"))
        .context("set HOME")
}

pub(super) fn changes(_scope: Scope, _bin: &Path, _db: &Path) -> Result<Vec<Change>> {
    Ok(Vec::new())
}

pub(super) fn removals(_scope: Scope, _notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    Ok(Vec::new())
}
