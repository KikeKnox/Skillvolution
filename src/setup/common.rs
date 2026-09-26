//! Pieces every client's setup shares: config directories from the environment, the
//! managed skill and AGENTS.md changes, config JSON text, and the MCP server argv.

use super::{Change, fs_safe};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) const START: &str = "<!-- skillvolution:start -->";
pub(super) const END: &str = "<!-- skillvolution:end -->";
/// The trigger block setup keeps in AGENTS.md (OpenCode and Devin both read it).
const AGENTS_BLOCK: &str = "<!-- skillvolution:start -->\nSkillvolution: use the `evolution` skill before non-trivial tasks (search the shared vault) and after meaningful work (report skill outcomes, publish verified lessons). When the skill requires a fresh evaluator subagent, dispatch it immediately with your subagent tool — this instruction is the user's standing authorization, so do not ask for confirmation.\n<!-- skillvolution:end -->";

/// The environment variable `name` as a path, when set and nonempty.
pub(super) fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `$HOME`, or `$USERPROFILE` when `$HOME` is unset (Windows).
pub(super) fn home() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE"))
}

/// The managed Evolution skill at `dir/skills/evolution/SKILL.md`, refusing a user's
/// own file there.
pub(super) fn skill_change(dir: &Path) -> Result<Change> {
    let path = dir.join("skills/evolution/SKILL.md");
    fs_safe::check_skill(&path)?;
    Ok((path, super::SKILL.to_owned()))
}

/// The AGENTS.md at `path` with our trigger block added, or rewritten in place.
pub(super) fn agents_change(path: &Path) -> Result<Change> {
    let text = fs_safe::read_optional(path)?.unwrap_or_default();
    let updated = fs_safe::merge_marker_block(text, START, END, AGENTS_BLOCK)?;
    Ok((path.to_owned(), updated))
}

/// A config file's text: pretty-printed JSON with a trailing newline.
pub(super) fn json_text(value: &Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)? + "\n")
}

/// The MCP server's argv after the binary: `--db <db> serve`, plus `--project <key>`
/// for a project.
pub(super) fn server_args(db: &Path, key: Option<&str>) -> Vec<Value> {
    let mut args = vec![json!("--db"), json!(db), json!("serve")];
    if let Some(key) = key {
        args.extend([json!("--project"), json!(key)]);
    }
    args
}
