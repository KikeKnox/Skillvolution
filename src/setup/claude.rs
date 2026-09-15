//! Claude Code: .mcp.json entry, session-start/stop hooks, native skill file,
//! and removal of the legacy CLAUDE.md `@import` block.

use super::fs_safe;
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const LEGACY_START: &str = "<!-- skillvolution:evolution:start -->";
const LEGACY_END: &str = "<!-- skillvolution:evolution:end -->";

pub fn changes(project: &Path, bin: &Path, db: &Path, key: &str) -> Result<Vec<(PathBuf, String)>> {
    let mut changes = Vec::new();

    let skill_path = project.join(".claude/skills/evolution/SKILL.md");
    let owned = fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let path = project.join(".mcp.json");
    let mut config = fs_safe::load_json(&path)?;
    let entry =
        json!({"type": "stdio", "command": bin, "args": ["--db", db, "serve", "--project", key]});
    fs_safe::merge_server(&mut config, "mcpServers", entry, owned)?;
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

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A Skillvolution-owned hook command, independent of the current bin/db path,
/// so a changed binary path replaces the old entry instead of duplicating it.
fn owned_command(command: &str) -> bool {
    command.ends_with(" hook stop")
        || (command.contains(" hook session-start") && command.contains(" --db "))
}

fn strip_owned(hooks: &mut Map<String, Value>, event: &str) -> Result<()> {
    let Some(value) = hooks.get_mut(event) else {
        return Ok(());
    };
    let groups = value
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    for group in groups.iter_mut() {
        let group = group
            .as_object_mut()
            .with_context(|| format!("hooks.{event} entries must be objects"))?;
        if let Some(entries) = group.get_mut("hooks") {
            let entries = entries
                .as_array_mut()
                .with_context(|| format!("hooks.{event}[].hooks must be an array"))?;
            entries.retain(|entry| {
                !entry
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(owned_command)
            });
        }
    }
    groups.retain(|group| !matches!(group.get("hooks"), Some(Value::Array(e)) if e.is_empty()));
    Ok(())
}

fn append_group(hooks: &mut Map<String, Value>, event: &str, command: String) -> Result<()> {
    let groups = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    groups.push(json!({"hooks": [{"type": "command", "command": command, "timeout": 10}]}));
    Ok(())
}

fn merge_hooks(config: &mut Value, bin: &Path, db: &Path, key: &str) -> Result<()> {
    let bin = shell_quote(&bin.display().to_string());
    let db = shell_quote(&db.display().to_string());
    let key = shell_quote(key);
    let session_start = format!("{bin} --db {db} hook session-start --project {key}");
    let stop = format!("{bin} --db {db} hook stop");

    let hooks = config
        .as_object_mut()
        .context("config must be an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be an object")?;
    strip_owned(hooks, "SessionStart")?;
    strip_owned(hooks, "Stop")?;
    append_group(hooks, "SessionStart", session_start)?;
    append_group(hooks, "Stop", stop)?;
    Ok(())
}
