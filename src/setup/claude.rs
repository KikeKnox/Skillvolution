//! Claude Code: the native skill file, SessionStart/Stop hooks and permissions in the
//! settings file (`.claude/settings.local.json` in a project, `settings.json` globally),
//! and the MCP server: in `.mcp.json` for a project, registered through the `claude`
//! CLI globally (see `claude_cli`). Project setup also removes the legacy CLAUDE.md
//! `@import` block.

use super::{Change, Edit, Scope, claude_cli, common, fs_safe, hooks, permissions};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const LEGACY_START: &str = "<!-- skillvolution:evolution:start -->";
const LEGACY_END: &str = "<!-- skillvolution:evolution:end -->";

/// `$CLAUDE_CONFIG_DIR` when set and nonempty, else `$HOME/.claude` (`$USERPROFILE` if
/// `$HOME` is unset).
pub(super) fn global_dir() -> Result<PathBuf> {
    common::env_dir("CLAUDE_CONFIG_DIR")
        .or_else(|| common::home().map(|home| home.join(".claude")))
        .context("set CLAUDE_CONFIG_DIR or HOME")
}

pub(super) fn changes(scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
    let (dir, settings_name) = match scope {
        Scope::Project { dir, .. } => (dir.join(".claude"), "settings.local.json"),
        Scope::Global => (global_dir()?, "settings.json"),
    };
    let mut changes = vec![common::skill_change(&dir)?];

    if let Scope::Project { dir: project, key } = scope {
        let path = project.join(".mcp.json");
        let mut config = fs_safe::load_json(&path)?;
        let entry = json!({
            "type": "stdio",
            "command": bin,
            "args": common::server_args(db, Some(key)),
        });
        fs_safe::merge_server(&mut config, "mcpServers", entry)?;
        changes.push((path, common::json_text(&config)?));

        let claude_md = project.join("CLAUDE.md");
        if let Some(text) = fs_safe::read_optional(&claude_md)?
            && let Some(updated) = fs_safe::remove_marker_block(text, LEGACY_START, LEGACY_END)?
        {
            changes.push((claude_md, updated));
        }
    }

    let settings_path = dir.join(settings_name);
    let mut settings = fs_safe::load_json(&settings_path)?;
    merge_hooks(&mut settings, bin, db, scope.key())?;
    permissions::merge_claude(&mut settings)?;
    changes.push((settings_path, common::json_text(&settings)?));

    Ok(changes)
}

fn merge_hooks(config: &mut Value, bin: &Path, db: &Path, key: Option<&str>) -> Result<()> {
    let session_start = hooks::hook_cmd(bin, db, "session-start", &hooks::project_flag(key))?;
    let stop = hooks::hook_cmd(bin, db, "stop", "")?;
    hooks::merge(
        config,
        &[("SessionStart", None, session_start), ("Stop", None, stop)],
    )
}

/// The removal counterpart of `changes`: deletes the managed skill file, strips our
/// hooks and the `mcp__skillvolution` permission grant from the settings JSON (keeping
/// `Agent`/`Task`, which the user may have granted independently), and for a project
/// removes the `.mcp.json` server entry. The global MCP registration itself has no file
/// to edit here; it's removed separately, via the `claude` CLI (see `unregister_mcp`),
/// only after every edit `write_all` returns has already succeeded.
pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    let (dir, settings_name) = match scope {
        Scope::Project { dir, .. } => (dir.join(".claude"), "settings.local.json"),
        Scope::Global => (global_dir()?, "settings.json"),
    };
    let mut edits = Vec::new();
    if let Some(edit) = common::skill_removal(&dir, notes)? {
        edits.push(edit);
    }

    if let Scope::Project { dir: project, .. } = scope {
        let path = project.join(".mcp.json");
        let mut config = fs_safe::load_json(&path)?;
        if fs_safe::remove_server(&mut config, "mcpServers", "skillvolution")? {
            edits.push(Edit::Write(path, common::json_text(&config)?));
        }
    }

    let settings_path = dir.join(settings_name);
    let mut settings = fs_safe::load_json(&settings_path)?;
    let mut changed = hooks::remove_owned(&mut settings)?;
    let (removed, kept) = permissions::remove_claude(&mut settings)?;
    changed |= removed;
    if !kept.is_empty() {
        notes.push(format!(
            "{} still allows {} (Skillvolution granted it; remove manually if you no longer want it)",
            settings_path.display(),
            kept.join(", "),
        ));
    }
    if changed {
        edits.push(Edit::Write(settings_path, common::json_text(&settings)?));
    }

    Ok(edits)
}

/// Warns (on stderr) when the current directory is inside a git repo whose Claude Code
/// project config still holds Skillvolution entries from a project-level setup (the
/// only kind 0.1.x had): with the global setup too, its hooks and MCP server would run
/// twice in that repo.
pub(super) fn warn_about_project_install() {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let Some(repo) = cwd.ancestors().find(|dir| dir.join(".git").exists()) else {
        return;
    };
    let settings = repo.join(".claude/settings.local.json");
    let mcp = repo.join(".mcp.json");
    let found = [
        fs_safe::load_json(&settings).is_ok_and(|config| hooks::has_owned(&config)),
        fs_safe::load_json(&mcp)
            .is_ok_and(|config| config["mcpServers"].get("skillvolution").is_some()),
    ];
    for (path, found) in [settings, mcp].iter().zip(found) {
        if found {
            eprintln!(
                "warning: {} already configures Skillvolution for this repo (project-level setup); \
                 with the global setup too, it would run twice here. Remove its skillvolution entries.",
                path.display()
            );
        }
    }
}

/// Registers the MCP server at user scope via the `claude` CLI, run only after every
/// file write has already succeeded. Returns a one-line summary: registered, or a note
/// with the command to run manually when `claude` isn't on `PATH`.
pub(super) fn register_mcp(bin: &Path, db: &Path) -> Result<String> {
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

/// Removes the user-scope MCP registration via the `claude` CLI, run only after every
/// file edit has already succeeded. Returns a one-line summary: removed (or already
/// gone), or a note with the command to run manually when `claude` isn't on `PATH`.
pub(super) fn unregister_mcp() -> Result<String> {
    let Some(claude) = claude_cli::resolve() else {
        return Ok(format!(
            "claude CLI not found on PATH; remove the MCP server manually: {}",
            claude_cli::remove_command()
        ));
    };
    claude_cli::unregister(&claude).context("remove the MCP server with the claude CLI")?;
    Ok("Removed skillvolution with `claude mcp remove --scope user`.".to_owned())
}
