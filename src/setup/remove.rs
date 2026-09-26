//! `setup --remove`: undoes what setup wrote, keeping every entry it doesn't own.

use anyhow::{Result, bail};
use std::path::Path;

use super::Clients;

/// Removes Skillvolution from the selected clients' configs: the project's
/// files when `project` is given, otherwise the user's global configs.
pub fn run(_clients: Clients, _project: Option<&Path>) -> Result<()> {
    bail!("setup --remove is not implemented yet")
}
