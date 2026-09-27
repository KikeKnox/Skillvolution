//! Devin CLI: `mcp_config.json` server entry, `config.json` permissions + lifecycle
//! hooks, the native skill file, and the shared AGENTS.md trigger block. A project keeps
//! them under `.devin/` (AGENTS.md at the root); globally they live in `~/.config/devin/`
//! (`%APPDATA%\devin` on Windows).
//!
//! Devin's hook payloads carry a `session_id` but no transcript path, so the
//! review reminder is driven by PostToolUse flag tracking instead of transcript
//! scanning, Stop blocks via a `{"decision":"block"}` JSON on stdout, and
//! PermissionRequest auto-approves the subagent/vault tools the flow needs
//! (`run_subagent`/`read_subagent` aren't documented permission rule names).

use super::{Change, Edit, Scope, common, fs_safe, hooks, permissions};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The documented locations: `~/.config/devin` on Unix, `%APPDATA%\devin` on
/// Windows (`XDG_CONFIG_HOME` is not documented for Devin, so it is not honored
/// here).
pub(super) fn global_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    let dir = common::env_dir("APPDATA").map(|dir| dir.join("devin"));
    #[cfg(not(windows))]
    let dir = common::home().map(|home| home.join(".config/devin"));
    dir.context("set HOME or APPDATA")
}

pub(super) fn changes(scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".devin")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut changes = vec![common::skill_change(&dir)?];

    let mcp_path = dir.join("mcp_config.json");
    let mut mcp = fs_safe::load_json(&mcp_path)?;
    let entry = json!({"command": bin, "args": common::server_args(db, scope.key())});
    fs_safe::merge_server(&mut mcp, "mcpServers", entry, scope)?;
    changes.push((mcp_path, common::json_text(&mcp)?));

    let config_path = dir.join("config.json");
    let mut config = fs_safe::load_json(&config_path)?;
    permissions::merge_devin(&mut config)?;
    merge_hooks(&mut config, bin, db, scope.key())?;
    changes.push((config_path, common::json_text(&config)?));

    // AGENTS.md is a Devin rules file and already carries the shared Skillvolution
    // marker block when OpenCode setup ran.
    changes.push(common::agents_change(&root.join("AGENTS.md"))?);

    Ok(changes)
}

/// The removal counterpart of `changes`: deletes the managed skill file, strips our
/// server entry from `mcp_config.json` and our hooks + permission grant from
/// `config.json` (keeping `run_subagent`/`read_subagent`), and removes our trigger block
/// from AGENTS.md (shared with OpenCode; deleted here only if that empties it, same as
/// `changes` writes it either way regardless of which client's setup ran first).
pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".devin")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut edits = Vec::new();
    if let Some(edit) = common::skill_removal(&dir, notes)? {
        edits.push(edit);
    }

    let mcp_path = dir.join("mcp_config.json");
    let mut mcp = fs_safe::load_json(&mcp_path)?;
    if fs_safe::remove_server(&mut mcp, "mcpServers", "skillvolution")? {
        edits.push(Edit::Write(mcp_path, common::json_text(&mcp)?));
    }

    let config_path = dir.join("config.json");
    let mut config = fs_safe::load_json(&config_path)?;
    let mut changed = hooks::remove_owned(&mut config)?;
    let (removed, kept) = permissions::remove_devin(&mut config)?;
    changed |= removed;
    if !kept.is_empty() {
        notes.push(format!(
            "{} still allows {} (Skillvolution granted it; remove manually if you no longer want it)",
            config_path.display(),
            kept.join(", "),
        ));
    }
    if changed {
        edits.push(Edit::Write(config_path, common::json_text(&config)?));
    }

    if let Some(edit) = common::agents_removal(&root.join("AGENTS.md"))? {
        edits.push(edit);
    }

    Ok(edits)
}

/// The `hooks` entries for a Devin config: session catalog, per-tool
/// worked/reviewed tracking, the review-blocking stop, session cleanup, and
/// auto-approval for the tools the evolution flow needs. `key` scopes
/// session-start to the project, like the Claude Code setup does.
fn merge_hooks(config: &mut Value, bin: &Path, db: &Path, key: Option<&str>) -> Result<()> {
    let session_start_extra = format!(" --client devin{}", hooks::project_flag(key));
    let post_tool_use_matcher = format!(
        "^({}|mcp__skillvolution__.*)$",
        crate::hook::DEVIN_WORK_TOOLS.join("|")
    );
    let entries = [
        (
            "SessionStart",
            None,
            hooks::hook_cmd(bin, db, "session-start", &session_start_extra)?,
        ),
        (
            "PostToolUse",
            Some(post_tool_use_matcher),
            hooks::hook_cmd(bin, db, "tool-use", "")?,
        ),
        (
            "Stop",
            None,
            hooks::hook_cmd(bin, db, "stop", " --client devin")?,
        ),
        (
            "SessionEnd",
            None,
            hooks::hook_cmd(bin, db, "session-end", "")?,
        ),
        (
            "PermissionRequest",
            Some("^(run_subagent|read_subagent|mcp__skillvolution__.*)$".to_owned()),
            hooks::hook_cmd(bin, db, "approve", "")?,
        ),
    ];
    hooks::merge(config, &entries)
}
