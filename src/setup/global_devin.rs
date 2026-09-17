//! Global (per-user) Devin CLI config: `~/.config/devin/` (`%APPDATA%\devin` on
//! Windows) holding the managed skill, `mcp_config.json`, `config.json`
//! permissions + hooks, and the AGENTS.md trigger block.

use super::{devin, fs_safe, opencode, permissions};
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

/// The documented locations: `~/.config/devin` on Unix, `%APPDATA%\devin` on
/// Windows (`XDG_CONFIG_HOME` is not documented for Devin, so it is not honored
/// here).
fn config_dir() -> Result<PathBuf> {
    let nonempty_env = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    #[cfg(windows)]
    let dir = nonempty_env("APPDATA").map(|dir| dir.join("devin"));
    #[cfg(not(windows))]
    let dir = nonempty_env("HOME")
        .or_else(|| nonempty_env("USERPROFILE"))
        .map(|home| home.join(".config/devin"));
    dir.ok_or_else(|| anyhow::anyhow!("set HOME or APPDATA"))
}

/// Whether Devin's global config directory already exists, used by `detect` to
/// tell if Devin CLI is installed.
pub(super) fn config_dir_exists() -> bool {
    config_dir().is_ok_and(|dir| dir.exists())
}

pub fn changes(bin: &Path, db: &Path) -> Result<Vec<(PathBuf, String)>> {
    let dir = config_dir()?;
    let mut changes = Vec::new();

    let skill_path = dir.join("skills/evolution/SKILL.md");
    fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let mcp_path = dir.join("mcp_config.json");
    let mut mcp = fs_safe::load_json(&mcp_path)?;
    let entry = json!({"command": bin, "args": ["--db", db, "serve"]});
    fs_safe::merge_server(&mut mcp, "mcpServers", entry)?;
    changes.push((mcp_path, serde_json::to_string_pretty(&mcp)? + "\n"));

    let config_path = dir.join("config.json");
    let mut config = fs_safe::load_json(&config_path)?;
    permissions::merge_devin(&mut config)?;
    devin::merge_hooks(&mut config, bin, db, None)?;
    changes.push((config_path, serde_json::to_string_pretty(&config)? + "\n"));

    let agents_path = dir.join("AGENTS.md");
    let text = fs_safe::read_optional(&agents_path)?.unwrap_or_default();
    let updated =
        fs_safe::merge_marker_block(text, opencode::START, opencode::END, opencode::AGENTS_BLOCK)?;
    changes.push((agents_path, updated));

    Ok(changes)
}
