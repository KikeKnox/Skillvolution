//! SessionStart/Stop hook merging shared by project (`.claude/settings.local.json`) and
//! global (`settings.json`) Claude Code setup. Both own their hook entries by matching on
//! the command text, so a rerun (even with a changed `--bin`/`--db`) replaces the old
//! entry instead of duplicating it, and any foreign hook is left untouched.

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A Skillvolution-owned hook command, independent of the current bin/db/project args,
/// so a changed one replaces the old entry instead of duplicating it.
fn owned_command(command: &str) -> bool {
    command.contains(" hook stop") || command.contains(" hook session-start")
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

/// Replaces our SessionStart/Stop entries in `config["hooks"]` with `session_start`/`stop`,
/// preserving every other event and every foreign hook.
pub fn merge(config: &mut Value, session_start: String, stop: String) -> Result<()> {
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
