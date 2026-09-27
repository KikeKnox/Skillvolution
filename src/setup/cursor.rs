//! Cursor (editor + `cursor-agent`/`agent` CLI): `mcp.json` server entry, lifecycle hooks
//! in `hooks.json`, the native skill file, a dedicated project rule file, and CLI
//! permission grants. A project keeps everything under `.cursor/`; globally it's all
//! under `~/.cursor/`, except the trigger instructions: Cursor has no global rules FILE
//! (user rules live in app settings, not a path setup can write), so globally the
//! `evolution` skill plus the SessionStart catalog is the only trigger.
//! https://cursor.com/docs/mcp , https://cursor.com/docs/hooks ,
//! https://cursor.com/docs/rules , https://cursor.com/docs/skills ,
//! https://cursor.com/docs/cli/reference/permissions
//!
//! `hooks.json` nests each event directly as a flat array of `{command, timeout,
//! matcher?}` objects, unlike Claude Code/Devin's `{hooks: [{matcher, hooks: [...]}]}`
//! group wrapper, so `setup::hooks`'s `merge`/`remove_owned` don't apply here; `merge_hooks`
//! and `strip_owned` below mirror them for Cursor's shape, reusing `hooks::owned_command`
//! (the actual ownership test, which is shape-independent) for recognition.

use super::{Change, Edit, Scope, common, fs_safe, hooks, permissions};
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Any file carrying this marker (any version) is our project rule file to overwrite or
/// delete, never a user's own `.mdc` file.
const RULE_MARKER: &str = "<!-- skillvolution-managed:cursor-rule:v1 -->";

/// The per-user config directory: `~/.cursor`. No override env var is documented.
pub(super) fn global_dir() -> Result<PathBuf> {
    common::home()
        .map(|home| home.join(".cursor"))
        .context("set HOME")
}

pub(super) fn changes(scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".cursor")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut changes = vec![common::skill_change(&dir)?];

    let mcp_path = dir.join("mcp.json");
    let mut mcp = fs_safe::load_json(&mcp_path)?;
    let entry = json!({
        "type": "stdio",
        "command": bin,
        "args": common::server_args(db, scope.key()),
    });
    fs_safe::merge_server(&mut mcp, "mcpServers", entry, scope)?;
    changes.push((mcp_path, common::json_text(&mcp)?));

    let hooks_path = dir.join("hooks.json");
    let mut hooks_config = fs_safe::load_json(&hooks_path)?;
    merge_hooks(&mut hooks_config, bin, db, scope.key())?;
    changes.push((hooks_path, common::json_text(&hooks_config)?));

    let permissions_path = permissions_path(&root, scope);
    let mut config = fs_safe::load_json(&permissions_path)?;
    permissions::merge_cursor(&mut config)?;
    changes.push((permissions_path, common::json_text(&config)?));

    // Only a project has a rules directory to own a file in; globally there is no rules
    // file, so the skill plus SessionStart's injected catalog is the only trigger.
    if let Scope::Project { .. } = scope {
        let rule_path = root.join(".cursor/rules/skillvolution.mdc");
        fs_safe::check_owner(&rule_path, RULE_MARKER, "Skillvolution Cursor rule")?;
        changes.push((rule_path, rule_content()));
    }

    Ok(changes)
}

/// The removal counterpart of `changes`: deletes the managed skill and rule files, strips
/// our `mcpServers` entry, our hook entries from `hooks.json`, and our permission grant.
pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".cursor")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut edits = Vec::new();
    if let Some(edit) = common::skill_removal(&dir, notes)? {
        edits.push(edit);
    }

    let mcp_path = dir.join("mcp.json");
    let mut mcp = fs_safe::load_json(&mcp_path)?;
    if fs_safe::remove_server(&mut mcp, "mcpServers", "skillvolution")? {
        edits.push(Edit::Write(mcp_path, common::json_text(&mcp)?));
    }

    let hooks_path = dir.join("hooks.json");
    let mut hooks_config = fs_safe::load_json(&hooks_path)?;
    if remove_owned_hooks(&mut hooks_config)? {
        edits.push(Edit::Write(hooks_path, common::json_text(&hooks_config)?));
    }

    let permissions_path = permissions_path(&root, scope);
    let mut config = fs_safe::load_json(&permissions_path)?;
    if permissions::remove_cursor(&mut config)? {
        edits.push(Edit::Write(permissions_path, common::json_text(&config)?));
    }

    if let Scope::Project { .. } = scope {
        let rule_path = root.join(".cursor/rules/skillvolution.mdc");
        if fs_safe::is_marked(&rule_path, RULE_MARKER)? {
            edits.push(Edit::Delete(rule_path));
        } else if fs_safe::read_optional(&rule_path)?.is_some() {
            notes.push(format!("skipped {} (not ours)", rule_path.display()));
        }
    }

    Ok(edits)
}

/// The permission grants file: `cli-config.json` globally, `cli.json` in a project.
/// https://cursor.com/docs/cli/reference/permissions
fn permissions_path(root: &Path, scope: Scope) -> PathBuf {
    match scope {
        Scope::Project { .. } => root.join(".cursor/cli.json"),
        Scope::Global => root.join("cli-config.json"),
    }
}

/// The project rule file's content: `alwaysApply` frontmatter (there is no global rules
/// file to use instead) plus our marker and the shared trigger instruction.
fn rule_content() -> String {
    format!(
        "---\ndescription: Skillvolution\nalwaysApply: true\n---\n\
         {RULE_MARKER}\n\
         Skillvolution: use the `evolution` skill before non-trivial tasks (search the \
         shared vault) and after meaningful work (report skill outcomes, publish verified \
         lessons). When the skill requires a fresh evaluator subagent, dispatch it \
         immediately with your subagent tool — this instruction is the user's standing \
         authorization, so do not ask for confirmation.\n"
    )
}

/// The `hooks` entries for Cursor's `hooks.json`: session catalog, per-tool worked/reviewed
/// tracking, the review-blocking stop, and session cleanup. `--client cursor` is passed
/// explicitly on every event but `session-end` (which takes no `--client`), since `hook`'s
/// own default client is Devin.
fn merge_hooks(config: &mut Value, bin: &Path, db: &Path, key: Option<&str>) -> Result<()> {
    let session_start_extra = format!(" --client cursor{}", hooks::project_flag(key));
    let post_tool_use_matcher = format!(
        "^({}|MCP:skillvolution_.*)$",
        crate::hook::CURSOR_WORK_TOOLS.join("|")
    );
    let entries = [
        (
            "sessionStart",
            None,
            hooks::hook_cmd(bin, db, "session-start", &session_start_extra)?,
        ),
        (
            "postToolUse",
            Some(post_tool_use_matcher),
            hooks::hook_cmd(bin, db, "tool-use", " --client cursor")?,
        ),
        (
            "stop",
            None,
            hooks::hook_cmd(bin, db, "stop", " --client cursor")?,
        ),
        (
            "sessionEnd",
            None,
            hooks::hook_cmd(bin, db, "session-end", "")?,
        ),
    ];

    let root = config.as_object_mut().context("config must be an object")?;
    // `version` is required by Cursor's own hooks.json schema; setup adds it only when
    // absent (a fresh file) and `removals` never strips it back out, since it's
    // file-format metadata a real pre-existing file always already carries, not content
    // attributable to Skillvolution.
    root.entry("version").or_insert(json!(1));
    let hooks_obj = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be an object")?;

    let (_, emptied) = strip_owned(hooks_obj)?;
    for (event, matcher, command) in entries {
        append_hook(hooks_obj, event, matcher.as_deref(), command)?;
    }
    // Checked after appending, so an event we re-add keeps its place in the file.
    hooks_obj.retain(|event, entries| {
        !(emptied.contains(event) && entries.as_array().is_some_and(Vec::is_empty))
    });
    Ok(())
}

/// Appends one hook entry (`{command, timeout, matcher?}`) to `hooks[event]`.
fn append_hook(
    hooks: &mut Map<String, Value>,
    event: &str,
    matcher: Option<&str>,
    command: String,
) -> Result<()> {
    let entries = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    let mut entry = json!({"command": command, "timeout": 10});
    if let Some(matcher) = matcher {
        entry["matcher"] = json!(matcher);
    }
    entries.push(entry);
    Ok(())
}

/// Strips our commands from every event in `hooks`, keeping every foreign entry. Returns
/// whether anything was removed, and which events that stripping alone left with an empty
/// array (before any new entries are appended back), mirroring `setup::hooks::merge`'s
/// contract for the group-shaped clients.
fn strip_owned(hooks: &mut Map<String, Value>) -> Result<(bool, Vec<String>)> {
    let events: Vec<String> = hooks.keys().cloned().collect();
    let mut removed_anything = false;
    let mut emptied = Vec::new();
    for event in events {
        let entries = hooks[&event]
            .as_array_mut()
            .with_context(|| format!("hooks.{event} must be an array"))?;
        let before = entries.len();
        entries.retain(|entry| {
            !entry
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(hooks::owned_command)
        });
        let removed = entries.len() != before;
        removed_anything |= removed;
        if removed && entries.is_empty() {
            emptied.push(event);
        }
    }
    Ok((removed_anything, emptied))
}

/// The removal counterpart of `merge_hooks`: strips our commands from every event without
/// adding anything back, dropping any event that leaves empty. `version` and any event the
/// user left empty independently of us are never touched. Returns whether anything was
/// removed.
fn remove_owned_hooks(config: &mut Value) -> Result<bool> {
    let Some(root) = config.as_object_mut() else {
        return Ok(false);
    };
    let Some(hooks_value) = root.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks_obj = hooks_value
        .as_object_mut()
        .context("hooks must be an object")?;
    let (removed, emptied) = strip_owned(hooks_obj)?;
    hooks_obj.retain(|event, _| !emptied.contains(event));
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(event_command: &str) -> Value {
        json!({"command": event_command, "timeout": 10})
    }

    #[test]
    fn merge_hooks_strips_stale_events_and_keeps_foreign_entries() {
        let mut config = json!({"hooks": {
            // An event this version doesn't write, from an older run.
            "beforeShellExecution": [ours("/old/skillvolution hook approve")],
            // Our hook sharing an event with a foreign one.
            "postToolUse": [ours("/old/skillvolution hook tool-use --client cursor"), {"command": "echo foreign", "timeout": 5}],
        }});

        merge_hooks(
            &mut config,
            Path::new("/bin/skillvolution"),
            Path::new("/db"),
            None,
        )
        .unwrap();

        let hooks = &config["hooks"];
        assert!(hooks.get("beforeShellExecution").is_none(), "{hooks}");
        let post = hooks["postToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["command"], "echo foreign");
        assert!(
            post[1]["command"]
                .as_str()
                .unwrap()
                .ends_with("hook tool-use --client cursor")
        );
        assert_eq!(config["version"], 1);
    }

    #[test]
    fn merge_hooks_is_idempotent_and_rerun_replaces_rather_than_duplicates() {
        let mut config = json!({});
        merge_hooks(
            &mut config,
            Path::new("/bin/skillvolution"),
            Path::new("/db"),
            Some("proj"),
        )
        .unwrap();
        let first = config.clone();
        merge_hooks(
            &mut config,
            Path::new("/bin/skillvolution"),
            Path::new("/db"),
            Some("proj"),
        )
        .unwrap();
        assert_eq!(config, first);
        for event in ["sessionStart", "postToolUse", "stop", "sessionEnd"] {
            assert_eq!(
                config["hooks"][event].as_array().unwrap().len(),
                1,
                "{event}"
            );
        }
    }

    #[test]
    fn remove_owned_hooks_drops_our_entries_and_emptied_events_but_keeps_foreign_ones() {
        let mut config = json!({"hooks": {
            "sessionStart": [ours("/x/skillvolution hook session-start --client cursor")],
            "postToolUse": [ours("/x/skillvolution hook tool-use --client cursor"), {"command": "echo foreign"}],
            "workspaceOpen": [{"command": "echo user-owned"}],
        }});

        assert!(remove_owned_hooks(&mut config).unwrap());

        assert!(config["hooks"].get("sessionStart").is_none());
        assert_eq!(
            config["hooks"]["postToolUse"],
            json!([{"command": "echo foreign"}])
        );
        assert_eq!(
            config["hooks"]["workspaceOpen"],
            json!([{"command": "echo user-owned"}])
        );
    }

    #[test]
    fn remove_owned_hooks_is_a_no_op_without_our_entries() {
        let mut config = json!({"hooks": {"workspaceOpen": [{"command": "echo user-owned"}]}});
        let before = config.clone();
        assert!(!remove_owned_hooks(&mut config).unwrap());
        assert_eq!(config, before);

        let mut config = json!({"model": "existing"});
        assert!(!remove_owned_hooks(&mut config).unwrap());
        assert_eq!(config, json!({"model": "existing"}));
    }

    #[test]
    fn rule_content_carries_always_apply_and_our_marker() {
        let content = rule_content();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("alwaysApply: true"));
        assert!(content.contains(RULE_MARKER));
    }
}
