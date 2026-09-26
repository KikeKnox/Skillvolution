//! `skillvolution relocate`: moves the vault database to a new local path and
//! points every configured client at it.

use anyhow::{Result, bail};
use std::path::Path;

/// Copies the vault at `from` to `to`, verifies the copy, rewrites the client
/// configurations that reference `from`, and keeps `from` as a backup.
pub fn run(_from: &Path, _to: &Path) -> Result<()> {
    bail!("relocate is not implemented yet")
}
