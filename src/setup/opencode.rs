//! OpenCode: opencode.json MCP entry, AGENTS.md trigger block, native skill file.

use super::fs_safe;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const START: &str = "<!-- skillvolution:start -->";
const END: &str = "<!-- skillvolution:end -->";
const AGENTS_BLOCK: &str = "<!-- skillvolution:start -->\nSkillvolution: use the `evolution` skill before non-trivial tasks (search the shared vault) and after meaningful work (report skill outcomes, propose verified lessons).\n<!-- skillvolution:end -->";
const LEGACY_INSTRUCTION: &str = ".opencode/skills/evolution/SKILL.md";

pub fn changes(project: &Path, bin: &Path, db: &Path, key: &str) -> Result<Vec<(PathBuf, String)>> {
    ensure!(
        !project.join("opencode.jsonc").exists(),
        "opencode.jsonc is unsupported; merge it into strict opencode.json manually first"
    );

    let mut changes = Vec::new();

    let skill_path = project.join(".opencode/skills/evolution/SKILL.md");
    let owned = fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let path = project.join("opencode.json");
    let mut config = fs_safe::load_json(&path)?;
    let entry = json!({
        "type": "local",
        "command": [bin, "--db", db, "serve", "--project", key],
        "enabled": true,
    });
    fs_safe::merge_server(&mut config, "mcp", entry, owned)?;
    remove_legacy_instruction(&mut config)?;
    changes.push((path, serde_json::to_string_pretty(&config)? + "\n"));

    let agents_path = project.join("AGENTS.md");
    let text = fs_safe::read_optional(&agents_path)?.unwrap_or_default();
    let updated = fs_safe::merge_marker_block(text, START, END, AGENTS_BLOCK)?;
    changes.push((agents_path, updated));

    Ok(changes)
}

/// The old always-on `instructions` entry loaded the whole skill on every turn; drop it if
/// present, but never (re)create the `instructions` key.
fn remove_legacy_instruction(config: &mut Value) -> Result<()> {
    let Some(instructions) = config
        .as_object_mut()
        .context("config must be an object")?
        .get_mut("instructions")
    else {
        return Ok(());
    };
    let instructions = instructions
        .as_array_mut()
        .context("instructions must be an array of strings")?;
    ensure!(
        instructions.iter().all(Value::is_string),
        "instructions must be an array of strings"
    );
    instructions.retain(|v| v != &json!(LEGACY_INSTRUCTION));
    Ok(())
}
