//! Global (per-user) setup: writes to the user's config directories instead of a
//! project, and registers the Claude Code MCP server via the `claude` CLI instead of
//! editing `~/.claude.json` (see `claude_cli`).

use super::{ClientKind, Clients, Scope, claude, hooks, prompt, resolve_db, write_clients};
use crate::vault::{self, Vault};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Determines which clients to configure (detecting and asking when `clients` is
/// `None`, since `--client` was omitted), resolves the vault path — asking about it
/// too, right after, when neither `--db` nor `SKILLVOLUTION_DB` already picked one —
/// and writes every selected client's files.
pub fn run(clients: Option<Clients>, bin: &Path, db_flag: Option<PathBuf>) -> Result<()> {
    let selected = match clients {
        Some(clients) => clients,
        None => select_detected(),
    };

    // The vault is created even if nothing ends up configured below (matching setup's
    // long-standing behavior of always provisioning `--db` up front).
    let db = resolve_global_db(db_flag)?;
    hooks::require_utf8(bin, "--bin")?;
    hooks::require_utf8(&db, "--db")?;
    Vault::open(&db).with_context(|| format!("open {}", db.display()))?;
    vault::warn_if_unsafe(&db);

    if selected.is_empty() {
        return Ok(());
    }
    if selected.contains(ClientKind::ClaudeCode) {
        claude::warn_about_project_install();
    }
    write_clients(selected, Scope::Global, bin, &db)?;
    for kind in selected.iter() {
        println!("{}", summary(kind));
        if kind == ClientKind::ClaudeCode {
            println!("{}", claude::register_mcp(bin, &db)?);
        }
    }
    Ok(())
}

/// Detects which clients are installed and, on a terminal, asks which of those to
/// configure. Prints a one-line note for each skipped client, and for the two ways
/// ending up with none can happen (nothing detected, or everything declined). An empty
/// result means nothing gets configured; the caller still provisions the database.
fn select_detected() -> Clients {
    let detected: Clients = ClientKind::ALL
        .into_iter()
        .filter(|kind| kind.detect())
        .collect();
    if detected.is_empty() {
        println!(
            "No AI client detected; skipping setup (run `skillvolution setup --client <name>` after installing one)."
        );
        return Clients::default();
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
    }
    selected
}

/// The vault path for global setup: `resolve_db` handles `--db` and `SKILLVOLUTION_DB`
/// as always, but when neither picked one, the platform default is offered to the user
/// first (interactively; see `prompt::choose_db_on_terminal`) before falling through to
/// `resolve_db` for its usual validation.
fn resolve_global_db(db_flag: Option<PathBuf>) -> Result<PathBuf> {
    let env_db_set = std::env::var_os("SKILLVOLUTION_DB").is_some_and(|v| !v.is_empty());
    if db_flag.is_some() || env_db_set {
        return resolve_db(db_flag);
    }
    let default = crate::vault::default_database()?;
    let chosen = prompt::choose_db_on_terminal(&default);
    resolve_db(Some(chosen))
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
        ClientKind::Codex => {
            "Configured Codex CLI (global): skill, config.toml MCP entry, hooks.json, AGENTS.md."
        }
        ClientKind::Gemini => "Configured Gemini CLI (global).",
        ClientKind::Cursor => {
            "Configured Cursor (global): skill, mcp.json, hooks.json, cli-config.json permissions."
        }
    }
}
