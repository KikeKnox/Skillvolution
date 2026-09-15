//! Global (per-user) setup: writes to the user's config directories instead of a
//! project, and registers the Claude Code MCP server via the `claude` CLI instead of
//! editing `~/.claude.json` (see `claude_cli`).

use super::{global_claude, global_opencode, write_all};
use anyhow::Result;
use std::path::Path;

pub fn run(client: &str, bin: &Path, db: &Path) -> Result<()> {
    let mut changes = Vec::new();
    if client != "claude-code" {
        changes.extend(global_opencode::changes(bin, db)?);
    }
    if client != "opencode" {
        changes.extend(global_claude::changes(bin, db)?);
    }
    write_all(changes)?;

    if client != "claude-code" {
        println!("Configured OpenCode (global): skill, config entry, AGENTS.md, plugin.");
    }
    if client != "opencode" {
        println!("Configured Claude Code (global): skill, settings.json hooks.");
        println!("{}", global_claude::register_mcp(bin, db)?);
    }
    Ok(())
}
