//! Global (per-user) setup: writes to the user's config directories instead of a
//! project, and registers the Claude Code MCP server via the `claude` CLI instead of
//! editing `~/.claude.json` (see `claude_cli`).

use super::{ClientKind, Clients, Scope, claude, prompt, write_clients};
use anyhow::Result;
use std::path::Path;

/// Detects which clients are installed, asks which to configure when a terminal
/// is attached (see `prompt`), and configures the selection. Prints a one-line
/// note for each skipped client; if none is detected, prints a note and returns
/// without writing any client files.
pub fn run_detected(bin: &Path, db: &Path) -> Result<()> {
    let detected: Clients = ClientKind::ALL
        .into_iter()
        .filter(|kind| kind.detect())
        .collect();
    if detected.is_empty() {
        println!(
            "No AI client detected; skipping setup (run `skillvolution setup --client <name>` after installing one)."
        );
        return Ok(());
    }
    for kind in ClientKind::ALL {
        if !detected.contains(kind) {
            println!(
                "Skipped {}: not detected (run `skillvolution setup --client {}` after installing it).",
                kind.display_name(),
                kind.token()
            );
        }
    }
    let selected = prompt::choose_on_terminal(detected);
    for kind in detected.iter() {
        if !selected.contains(kind) {
            println!("Skipped {}: not selected.", kind.display_name());
        }
    }
    if selected.is_empty() {
        println!("No clients selected; nothing configured.");
        return Ok(());
    }
    run(selected, bin, db)
}

pub fn run(clients: Clients, bin: &Path, db: &Path) -> Result<()> {
    if clients.contains(ClientKind::ClaudeCode) {
        claude::warn_about_project_install();
    }
    write_clients(clients, Scope::Global, bin, db)?;
    for kind in clients.iter() {
        println!("{}", summary(kind));
        if kind == ClientKind::ClaudeCode {
            println!("{}", claude::register_mcp(bin, db)?);
        }
    }
    Ok(())
}

/// The line printed once a client's global files are written.
fn summary(kind: ClientKind) -> &'static str {
    match kind {
        ClientKind::ClaudeCode => {
            "Configured Claude Code (global): skill, settings.json hooks + permissions."
        }
        ClientKind::OpenCode => {
            "Configured OpenCode (global): skill, config entry, AGENTS.md, plugin."
        }
        ClientKind::Devin => {
            "Configured Devin CLI (global): skill, mcp_config.json, config.json permissions + hooks, AGENTS.md."
        }
    }
}
