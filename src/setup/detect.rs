//! Detects which AI clients are installed, for global `setup` runs that don't pass an
//! explicit `--client`.

use super::{claude_cli, global_claude, global_opencode};

pub struct Detected {
    pub claude_code: bool,
    pub opencode: bool,
}

/// A client is present if its CLI is on `PATH` or its global config directory already
/// exists.
pub fn detect() -> Detected {
    Detected {
        claude_code: claude_cli::resolve().is_some() || global_claude::config_dir_exists(),
        opencode: claude_cli::find_on_path("opencode").is_some()
            || global_opencode::config_dir_exists(),
    }
}
