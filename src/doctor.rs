//! `skillvolution doctor`: read-only health checks of the vault and of the
//! client configurations setup wrote.

use anyhow::{Result, bail};
use std::path::Path;

/// Runs every check against the database at `db`, printing a report (JSON when
/// `json`). Returns whether everything is healthy.
pub fn run(_db: &Path, _json: bool) -> Result<bool> {
    bail!("doctor is not implemented yet")
}
