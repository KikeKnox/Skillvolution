//! Claude Code: .mcp.json entry, session-start/stop hooks, native skill file,
//! and removal of the legacy CLAUDE.md `@import` block.

use super::{fs_safe, hooks};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const LEGACY_START: &str = "<!-- skillvolution:evolution:start -->";
const LEGACY_END: &str = "<!-- skillvolution:evolution:end -->";

pub fn changes(project: &Path, bin: &Path, db: &Path, key: &str) -> Result<Vec<(PathBuf, String)>> {
    let mut changes = Vec::new();

    let skill_path = project.join(".claude/skills/evolution/SKILL.md");
    fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let path = project.join(".mcp.json");
    let mut config = fs_safe::load_json(&path)?;
    let entry =
        json!({"type": "stdio", "command": bin, "args": ["--db", db, "serve", "--project", key]});
    fs_safe::merge_server(&mut config, "mcpServers", entry)?;
    changes.push((path, serde_json::to_string_pretty(&config)? + "\n"));

    let claude_md = project.join("CLAUDE.md");
    if let Some(text) = fs_safe::read_optional(&claude_md)?
        && let Some(updated) = fs_safe::remove_marker_block(text, LEGACY_START, LEGACY_END)?
    {
        changes.push((claude_md, updated));
    }

    let settings_path = project.join(".claude/settings.local.json");
    let mut settings = fs_safe::load_json(&settings_path)?;
    merge_hooks(&mut settings, bin, db, key)?;
    changes.push((
        settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    ));

    Ok(changes)
}

fn merge_hooks(config: &mut Value, bin: &Path, db: &Path, key: &str) -> Result<()> {
    let bin = hooks::shell_quote(&bin.display().to_string());
    let db = hooks::shell_quote(&db.display().to_string());
    let key = hooks::shell_quote(key);
    let session_start = format!("{bin} --db {db} hook session-start --project {key}");
    let stop = format!("{bin} --db {db} hook stop");
    hooks::merge(config, session_start, stop)
}
