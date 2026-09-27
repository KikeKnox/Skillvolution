//! OpenCode: the `opencode.json` MCP entry and permissions, the AGENTS.md trigger block,
//! the native skill file, and the plugin. A project keeps its config and AGENTS.md at the
//! root and the rest under `.opencode/`; globally everything lives in the config dir,
//! where an existing `opencode.jsonc` is used instead of `opencode.json`.

use super::{Change, Edit, Scope, common, fs_safe, permissions, plugin};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const LEGACY_INSTRUCTION: &str = ".opencode/skills/evolution/SKILL.md";

/// `$XDG_CONFIG_HOME/opencode` when `XDG_CONFIG_HOME` is an absolute, nonempty path;
/// otherwise `$HOME/.config/opencode` (`$USERPROFILE` if `$HOME` is unset).
pub(super) fn global_dir() -> Result<PathBuf> {
    if let Some(dir) = common::env_dir("XDG_CONFIG_HOME").filter(|path| path.is_absolute()) {
        return Ok(dir.join("opencode"));
    }
    common::home()
        .map(|home| home.join(".config/opencode"))
        .context("set XDG_CONFIG_HOME or HOME")
}

pub(super) fn changes(scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".opencode")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let config_path = config_path(&root, scope)?;
    let mut changes = vec![common::skill_change(&dir)?];

    let mut config = load_config(&config_path)?;
    let mut command = vec![json!(bin)];
    command.extend(common::server_args(db, scope.key()));
    let entry = json!({"type": "local", "command": command, "enabled": true});
    fs_safe::merge_server(&mut config, "mcp", entry, scope)?;
    if let Scope::Project { .. } = scope {
        remove_legacy_instruction(&mut config)?;
    }
    permissions::merge_opencode(&mut config)?;
    changes.push((config_path, common::json_text(&config)?));

    changes.push(common::agents_change(&root.join("AGENTS.md"))?);

    let plugin_path = dir.join("plugins/skillvolution.js");
    plugin::check_owner(&plugin_path)?;
    changes.push((plugin_path, plugin::content(bin, db, scope.key())?));

    Ok(changes)
}

/// The config file to write: `opencode.json` in a project, where `opencode.jsonc` is
/// refused; globally, whichever of the two exists (refusing if both do), or
/// `opencode.json` if neither does.
fn config_path(root: &Path, scope: Scope) -> Result<PathBuf> {
    let json = root.join("opencode.json");
    let jsonc = root.join("opencode.jsonc");
    if let Scope::Project { .. } = scope {
        ensure!(
            !jsonc.exists(),
            "opencode.jsonc is unsupported; merge it into strict opencode.json manually first"
        );
        return Ok(json);
    }
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
                 or remove the comments and rerun setup",
                path.display()
            )
        } else {
            format!("{} must be valid JSON", path.display())
        }
    })
}

/// The removal counterpart of `changes`: deletes the managed skill file and plugin file,
/// strips our `mcp` server entry and permission grants from the config (config selection
/// mirrors `changes`: `config_path` refuses an unsupported `opencode.jsonc` setup the
/// same way it does when installing), and removes our trigger block from AGENTS.md.
pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".opencode")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut edits = Vec::new();
    if let Some(edit) = common::skill_removal(&dir, notes)? {
        edits.push(edit);
    }

    let config_path = config_path(&root, scope)?;
    let mut config = load_config(&config_path)?;
    let mut changed = fs_safe::remove_server(&mut config, "mcp", "skillvolution")?;
    changed |= permissions::remove_opencode(&mut config)?;
    if changed {
        edits.push(Edit::Write(config_path, common::json_text(&config)?));
    }

    if let Some(edit) = common::agents_removal(&root.join("AGENTS.md"))? {
        edits.push(edit);
    }

    let plugin_path = dir.join("plugins/skillvolution.js");
    if let Some(edit) = plugin::removal(&plugin_path, notes)? {
        edits.push(edit);
    }

    Ok(edits)
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
