//! Global (per-user) setup: writes to the user's config directories instead of a
//! project, and registers the Claude Code MCP server via the `claude` CLI instead of
//! editing `~/.claude.json` (see `claude_cli`).

use super::{Clients, detect, global_claude, global_devin, global_opencode, prompt, write_all};
use anyhow::Result;
use std::path::Path;

pub fn run(clients: Clients, bin: &Path, db: &Path) -> Result<()> {
    configure(clients, bin, db)
}

/// Detects which clients are installed, asks which to configure when a terminal
/// is attached (see `prompt`), and configures the selection. Prints a one-line
/// note for each skipped client; if none is detected, prints a note and returns
/// without writing any client files.
pub fn run_detected(bin: &Path, db: &Path) -> Result<()> {
    let detected = detect::detect();
    if !detected.any() {
        println!(
            "No AI client detected; skipping setup (run `skillvolution setup --client <name>` after installing one)."
        );
        return Ok(());
    }
    for (present, name, token) in detected.list() {
        if !present {
            println!(
                "Skipped {name}: not detected (run `skillvolution setup --client {token}` after installing it)."
            );
        }
    }
    let selected = prompt::choose(detected);
    for ((was_present, name, _), (still, _, _)) in detected.list().into_iter().zip(selected.list())
    {
        if was_present && !still {
            println!("Skipped {name}: not selected.");
        }
    }
    if !selected.any() {
        println!("No clients selected; nothing configured.");
        return Ok(());
    }
    configure(selected, bin, db)
}

fn configure(clients: Clients, bin: &Path, db: &Path) -> Result<()> {
    let mut changes = Vec::new();
    if clients.opencode {
        changes.extend(global_opencode::changes(bin, db)?);
    }
    if clients.claude_code {
        changes.extend(global_claude::changes(bin, db)?);
    }
    if clients.devin {
        changes.extend(global_devin::changes(bin, db)?);
    }
    write_all(changes)?;

    if clients.opencode {
        println!("Configured OpenCode (global): skill, config entry, AGENTS.md, plugin.");
    }
    if clients.claude_code {
        println!("Configured Claude Code (global): skill, settings.json hooks + permissions.");
        println!("{}", global_claude::register_mcp(bin, db)?);
    }
    if clients.devin {
        println!(
            "Configured Devin CLI (global): skill, mcp_config.json, config.json permissions + hooks, AGENTS.md."
        );
    }
    Ok(())
}
