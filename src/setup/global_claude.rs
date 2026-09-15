//! Global (per-user) Claude Code config: the managed skill file, SessionStart/Stop
//! hooks merged into `settings.json` (not `settings.local.json`), and MCP registration
//! via the `claude` CLI (see `claude_cli`).

use super::{claude_cli, fs_safe, hooks};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// `$CLAUDE_CONFIG_DIR` when set and nonempty, else `$HOME/.claude` (`$USERPROFILE` if
/// `$HOME` is unset).
fn config_dir() -> Result<PathBuf> {
    let nonempty_env = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = nonempty_env("CLAUDE_CONFIG_DIR") {
        return Ok(dir);
    }
    let home = nonempty_env("HOME")
        .or_else(|| nonempty_env("USERPROFILE"))
        .ok_or_else(|| anyhow::anyhow!("set CLAUDE_CONFIG_DIR or HOME"))?;
    Ok(home.join(".claude"))
}

pub fn changes(bin: &Path, db: &Path) -> Result<Vec<(PathBuf, String)>> {
    let dir = config_dir()?;
    let mut changes = Vec::new();

    let skill_path = dir.join("skills/evolution/SKILL.md");
    fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let settings_path = dir.join("settings.json");
    let mut settings = fs_safe::load_json(&settings_path)?;
    merge_hooks(&mut settings, bin, db)?;
    changes.push((
        settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    ));

    Ok(changes)
}

fn merge_hooks(config: &mut serde_json::Value, bin: &Path, db: &Path) -> Result<()> {
    let bin = hooks::shell_quote(&bin.display().to_string());
    let db = hooks::shell_quote(&db.display().to_string());
    let session_start = format!("{bin} --db {db} hook session-start");
    let stop = format!("{bin} --db {db} hook stop");
    hooks::merge(config, session_start, stop)
}

/// Registers the MCP server at user scope via the `claude` CLI, run only after every
/// file write above has already succeeded. Returns a one-line summary: registered, or a
/// note with the command to run manually when `claude` isn't on `PATH`.
pub fn register_mcp(bin: &Path, db: &Path) -> Result<String> {
    let Some(claude) = claude_cli::resolve() else {
        return Ok(format!(
            "claude CLI not found on PATH; register the MCP server manually: {}",
            claude_cli::add_json_command(bin, db)
        ));
    };
    claude_cli::register(&claude, bin, db)
        .context("register the MCP server with the claude CLI")?;
    Ok("Registered skillvolution with `claude mcp add-json --scope user`.".to_owned())
}
