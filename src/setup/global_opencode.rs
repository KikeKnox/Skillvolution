//! Global (per-user) OpenCode config: the existing `opencode.json`/`opencode.jsonc`
//! (whichever is present; refuses if both are), the managed skill file, the AGENTS.md
//! trigger block, and the plugin.

use super::{fs_safe, opencode, permissions, plugin};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `$XDG_CONFIG_HOME/opencode` when `XDG_CONFIG_HOME` is an absolute, nonempty path;
/// otherwise `$HOME/.config/opencode` (`$USERPROFILE` if `$HOME` is unset).
fn config_dir() -> Result<PathBuf> {
    let nonempty_env = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = nonempty_env("XDG_CONFIG_HOME").filter(|path| path.is_absolute()) {
        return Ok(dir.join("opencode"));
    }
    let home = nonempty_env("HOME")
        .or_else(|| nonempty_env("USERPROFILE"))
        .ok_or_else(|| anyhow::anyhow!("set XDG_CONFIG_HOME or HOME"))?;
    Ok(home.join(".config/opencode"))
}

/// The config file to write: whichever of `opencode.json`/`opencode.jsonc` exists
/// (refusing if both do), or `opencode.json` if neither does.
fn config_path(dir: &Path) -> Result<PathBuf> {
    let json = dir.join("opencode.json");
    let jsonc = dir.join("opencode.jsonc");
    match (json.exists(), jsonc.exists()) {
        (true, true) => bail!(
            "both {} and {} exist; remove one before running setup",
            json.display(),
            jsonc.display()
        ),
        (false, true) => Ok(jsonc),
        _ => Ok(json),
    }
}

fn load_config(path: &Path) -> Result<Value> {
    let is_jsonc = path.extension().and_then(|ext| ext.to_str()) == Some("jsonc");
    fs_safe::load_json(path).with_context(|| {
        if is_jsonc {
            format!(
                "{} could not be parsed as strict JSON; add the mcp.skillvolution entry manually, \
                 or remove the comments/trailing commas and rerun setup",
                path.display()
            )
        } else {
            format!("{} must be valid JSON", path.display())
        }
    })
}

/// Whether OpenCode's global config directory already exists, used by `detect` to tell
/// if OpenCode is installed.
pub(super) fn config_dir_exists() -> bool {
    config_dir().is_ok_and(|dir| dir.exists())
}

pub fn changes(bin: &Path, db: &Path) -> Result<Vec<(PathBuf, String)>> {
    let dir = config_dir()?;
    let mut changes = Vec::new();

    let config_path = config_path(&dir)?;
    let mut config = load_config(&config_path)?;
    let entry = json!({
        "type": "local",
        "command": [bin, "--db", db, "serve"],
        "enabled": true,
    });
    fs_safe::merge_server(&mut config, "mcp", entry)?;
    permissions::merge_opencode(&mut config)?;
    changes.push((config_path, serde_json::to_string_pretty(&config)? + "\n"));

    let skill_path = dir.join("skills/evolution/SKILL.md");
    fs_safe::check_skill(&skill_path)?;
    changes.push((skill_path, super::SKILL.to_owned()));

    let agents_path = dir.join("AGENTS.md");
    let text = fs_safe::read_optional(&agents_path)?.unwrap_or_default();
    let updated =
        fs_safe::merge_marker_block(text, opencode::START, opencode::END, opencode::AGENTS_BLOCK)?;
    changes.push((agents_path, updated));

    let plugin_path = dir.join("plugins/skillvolution.js");
    plugin::check_owner(&plugin_path)?;
    changes.push((plugin_path, plugin::content(bin, db)?));

    Ok(changes)
}
