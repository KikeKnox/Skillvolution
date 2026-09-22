//! Devin CLI project setup: `.devin/mcp_config.json` server entry, `.devin/config.json`
//! permissions + lifecycle hooks, the native skill file, and the shared AGENTS.md
//! trigger block.
//!
//! Devin's hook payloads carry a `session_id` but no transcript path, so the
//! review reminder is driven by PostToolUse flag tracking instead of transcript
//! scanning, Stop blocks via a `{"decision":"block"}` JSON on stdout, and
//! PermissionRequest auto-approves the subagent/vault tools the flow needs
//! (`run_subagent`/`read_subagent` aren't documented permission rule names).

use super::{fs_safe, hooks, opencode, permissions};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn changes(project: &Path, bin: &Path, db: &Path, key: &str) -> Result<Vec<(PathBuf, String)>> {
    let mut changes = Vec::new();

    let skill_path = project.join(".devin/skills/evolution/SKILL.md");
    fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let mcp_path = project.join(".devin/mcp_config.json");
    let mut mcp = fs_safe::load_json(&mcp_path)?;
    let entry = json!({"command": bin, "args": ["--db", db, "serve", "--project", key]});
    fs_safe::merge_server(&mut mcp, "mcpServers", entry)?;
    changes.push((mcp_path, serde_json::to_string_pretty(&mcp)? + "\n"));

    let config_path = project.join(".devin/config.json");
    let mut config = fs_safe::load_json(&config_path)?;
    permissions::merge_devin(&mut config)?;
    merge_hooks(&mut config, bin, db, Some(key))?;
    changes.push((config_path, serde_json::to_string_pretty(&config)? + "\n"));

    // AGENTS.md at the project root is a Devin rules file and already carries the
    // shared Skillvolution marker block when OpenCode setup ran.
    let agents_path = project.join("AGENTS.md");
    let text = fs_safe::read_optional(&agents_path)?.unwrap_or_default();
    let updated =
        fs_safe::merge_marker_block(text, opencode::START, opencode::END, opencode::AGENTS_BLOCK)?;
    changes.push((agents_path, updated));

    Ok(changes)
}

/// The `hooks` entries for a Devin config: session catalog, per-tool
/// worked/reviewed tracking, the review-blocking stop, session cleanup, and
/// auto-approval for the tools the evolution flow needs. `key` scopes
/// session-start to the project, like the Claude Code setup does.
fn hook_entries(
    bin: &Path,
    db: &Path,
    key: Option<&str>,
) -> Result<Vec<(&'static str, Option<String>, String)>> {
    let bin = hooks::shell_quote(hooks::require_utf8(bin, "--bin")?);
    let db = hooks::shell_quote(hooks::require_utf8(db, "--db")?);
    let mut session_start = format!("{bin} --db {db} hook session-start --client devin");
    if let Some(key) = key {
        session_start = format!("{session_start} --project {}", hooks::shell_quote(key));
    }
    let post_tool_use_matcher = format!(
        "^({}|mcp__skillvolution__.*)$",
        crate::hook::DEVIN_WORK_TOOLS.join("|")
    );
    Ok(vec![
        ("SessionStart", None, session_start),
        (
            "PostToolUse",
            Some(post_tool_use_matcher),
            format!("{bin} --db {db} hook tool-use"),
        ),
        (
            "Stop",
            None,
            format!("{bin} --db {db} hook stop --client devin"),
        ),
        (
            "SessionEnd",
            None,
            format!("{bin} --db {db} hook session-end"),
        ),
        (
            "PermissionRequest",
            Some("^(run_subagent|read_subagent|mcp__skillvolution__.*)$".to_owned()),
            format!("{bin} --db {db} hook approve"),
        ),
    ])
}

pub(super) fn merge_hooks(
    config: &mut Value,
    bin: &Path,
    db: &Path,
    key: Option<&str>,
) -> Result<()> {
    hooks::merge(config, &hook_entries(bin, db, key)?)
}
