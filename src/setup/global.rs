//! Global (per-user) setup: writes to the user's config directories instead of a
//! project, and registers the Claude Code MCP server via the `claude` CLI instead of
//! editing `~/.claude.json` (see `claude_cli`).

use super::{detect, global_claude, global_opencode, write_all};
use anyhow::Result;
use std::path::Path;

pub fn run(client: &str, bin: &Path, db: &Path) -> Result<()> {
    configure(client != "claude-code", client != "opencode", bin, db)
}

/// Detects which clients are installed and configures only those, printing a one-line
/// note for each client skipped. If neither is detected, prints a note and returns
/// without writing any client files.
pub fn run_detected(bin: &Path, db: &Path) -> Result<()> {
    let detected = detect::detect();
    if !detected.claude_code && !detected.opencode {
        println!(
            "No AI client detected; skipping setup (run `skillvolution setup --client <name>` after installing one)."
        );
        return Ok(());
    }
    if !detected.opencode {
        println!(
            "Skipped OpenCode: not detected (run `skillvolution setup --client opencode` after installing it)."
        );
    }
    if !detected.claude_code {
        println!(
            "Skipped Claude Code: not detected (run `skillvolution setup --client claude-code` after installing it)."
        );
    }
    configure(detected.opencode, detected.claude_code, bin, db)
}

fn configure(opencode: bool, claude_code: bool, bin: &Path, db: &Path) -> Result<()> {
    let mut changes = Vec::new();
    if opencode {
        changes.extend(global_opencode::changes(bin, db)?);
    }
    if claude_code {
        changes.extend(global_claude::changes(bin, db)?);
    }
    write_all(changes)?;

    if opencode {
        println!("Configured OpenCode (global): skill, config entry, AGENTS.md, plugin.");
    }
    if claude_code {
        println!("Configured Claude Code (global): skill, settings.json hooks.");
        println!("{}", global_claude::register_mcp(bin, db)?);
    }
    Ok(())
}
