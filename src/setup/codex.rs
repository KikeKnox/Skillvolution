//! Codex CLI: an MCP server entry in `config.toml` (edited with `toml_edit` so the
//! user's formatting and comments survive), a standalone `hooks.json`, the native skill
//! file, and the shared AGENTS.md trigger block. A project keeps them under `.codex/`
//! (AGENTS.md at the project root); globally they live in `$CODEX_HOME` or `~/.codex/`.
//!
//! Docs re-verified 2026-09-26:
//! - config dir, AGENTS.md locations: <https://learn.chatgpt.com/docs/agent-configuration/agents-md>
//! - `config.toml` / `[mcp_servers.*]` keys, project trust requirement:
//!   <https://learn.chatgpt.com/docs/config-file/config-reference>
//! - `hooks.json` schema, event list, stdin fields, output formats, and MCP tool naming
//!   in `PostToolUse` (`mcp__<server>__<tool>`): <https://learn.chatgpt.com/docs/hooks>
//! - skills under `skills/<name>/SKILL.md`: <https://developers.openai.com/codex/skills>
//!
//! Codex's hook payloads carry a `session_id` but no `stop_hook_active` field, so, like
//! Devin, the review reminder is driven by PostToolUse flag tracking rather than
//! transcript scanning. `apply_patch` is confirmed as Codex's file-editing tool name in
//! `tool_name` (`CODEX_WORK_TOOLS` in `hook.rs`), and its MCP tool calls are reported as
//! `mcp__skillvolution__<tool>`, which already matches `is_review_tool`'s suffix check
//! with no change needed there.
//!
//! Codex documents no per-tool permission grant for MCP calls — only a global
//! `approval_policy`/`sandbox_mode` and a per-server `enabled_tools` allow-list (which
//! narrows the tools exposed at all, not which run unattended) — so there is no
//! `permissions::merge_codex`; whether the vault's tools run without a prompt is up to
//! the user's `approval_policy`. The subagent tool name is undocumented, so the
//! `PermissionRequest` hook below only auto-approves the vault's own MCP tools.

use super::{Change, Edit, Scope, common, fs_safe, hooks};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, value};

/// The per-user config directory: `$CODEX_HOME`, else `~/.codex`.
pub(super) fn global_dir() -> Result<PathBuf> {
    common::env_dir("CODEX_HOME")
        .or_else(|| common::home().map(|home| home.join(".codex")))
        .context("set HOME")
}

/// Codex only loads a project's `.codex/config.toml` once the project is trusted
/// (see the module doc's config-reference link), so a freshly created server entry
/// carries this reminder; an entry `changes` merely updates on a rerun keeps whatever
/// decor is already there instead of re-adding it on top of the user's own edits.
const PROJECT_TRUST_NOTE: &str = "# Skillvolution: Codex only loads this project's \
config once you trust the project (accept the trust prompt the first time you run \
`codex` here).\n";

pub(super) fn changes(scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".codex")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let is_project = matches!(scope, Scope::Project { .. });
    let mut changes = vec![common::skill_change(&dir)?];

    let config_path = dir.join("config.toml");
    let text = fs_safe::read_optional(&config_path)?.unwrap_or_default();
    let toml_text = merge_mcp_server(&text, bin, db, scope.key(), is_project)?;
    changes.push((config_path, toml_text));

    let hooks_path = dir.join("hooks.json");
    let mut hooks_config = fs_safe::load_json(&hooks_path)?;
    merge_hooks(&mut hooks_config, bin, db, scope.key())?;
    changes.push((hooks_path, common::json_text(&hooks_config)?));

    changes.push(common::agents_change(&root.join("AGENTS.md"))?);

    Ok(changes)
}

/// The removal counterpart of `changes`: deletes the managed skill file, strips our
/// `mcp_servers.skillvolution` table from `config.toml` (keeping every other key and the
/// user's formatting), our hooks from `hooks.json`, and our trigger block from AGENTS.md.
pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".codex")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut edits = Vec::new();
    if let Some(edit) = common::skill_removal(&dir, notes)? {
        edits.push(edit);
    }

    let config_path = dir.join("config.toml");
    if let Some(text) = fs_safe::read_optional(&config_path)?
        && let Some(updated) = remove_mcp_server(&text)?
    {
        edits.push(Edit::Write(config_path, updated));
    }

    let hooks_path = dir.join("hooks.json");
    let mut hooks_config = fs_safe::load_json(&hooks_path)?;
    if hooks::remove_owned(&mut hooks_config)? {
        edits.push(Edit::Write(hooks_path, common::json_text(&hooks_config)?));
    }

    if let Some(edit) = common::agents_removal(&root.join("AGENTS.md"))? {
        edits.push(edit);
    }

    Ok(edits)
}

/// The `hooks.json` entries: session catalog, per-tool worked/reviewed tracking, the
/// review-blocking stop, session cleanup, and auto-approval of the vault's own MCP tools.
/// Codex defaults `hook tool-use`/`hook stop` to Devin's client, so every event here
/// passes `--client codex` explicitly; `key` scopes session-start to the project, like
/// the other flag-tracked clients.
fn merge_hooks(config: &mut Value, bin: &Path, db: &Path, key: Option<&str>) -> Result<()> {
    let client_flag = " --client codex";
    let session_start_extra = format!("{client_flag}{}", hooks::project_flag(key));
    let post_tool_use_matcher = format!(
        "^({}|mcp__skillvolution__.*)$",
        crate::hook::CODEX_WORK_TOOLS.join("|")
    );
    let entries = [
        (
            "SessionStart",
            None,
            hooks::hook_cmd(bin, db, "session-start", &session_start_extra)?,
        ),
        (
            "PostToolUse",
            Some(post_tool_use_matcher),
            hooks::hook_cmd(bin, db, "tool-use", client_flag)?,
        ),
        ("Stop", None, hooks::hook_cmd(bin, db, "stop", client_flag)?),
        (
            "SessionEnd",
            None,
            hooks::hook_cmd(bin, db, "session-end", "")?,
        ),
        (
            "PermissionRequest",
            Some("^mcp__skillvolution__.*$".to_owned()),
            hooks::hook_cmd(bin, db, "approve", "")?,
        ),
    ];
    hooks::merge(config, &entries)
}

/// Ensures `[mcp_servers.skillvolution]` in `text` runs `<bin> --db <db> serve
/// [--project <key>]`, touching only `command`/`args` on an existing entry so every
/// other key the user set (`env`, `enabled`, `startup_timeout_sec`, ...) survives, and
/// leaving every other table's formatting and comments untouched. A brand new project
/// entry gets `PROJECT_TRUST_NOTE` right above its header.
fn merge_mcp_server(
    text: &str,
    bin: &Path,
    db: &Path,
    key: Option<&str>,
    project: bool,
) -> Result<String> {
    let mut doc = text
        .parse::<DocumentMut>()
        .context("config.toml must be valid TOML")?;
    if doc.get("mcp_servers").is_none() {
        doc["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = doc["mcp_servers"]
        .as_table_like_mut()
        .context("mcp_servers must be a table")?;
    let is_new = servers.get("skillvolution").is_none();
    if is_new {
        servers.insert("skillvolution", Item::Table(Table::new()));
    }
    let entry = servers
        .get_mut("skillvolution")
        .and_then(Item::as_table_mut)
        .context("mcp_servers.skillvolution must be a table")?;
    entry["command"] = value(hooks::require_utf8(bin, "--bin")?);
    let mut args = Array::new();
    for arg in server_argv(db, key)? {
        args.push(arg);
    }
    entry["args"] = value(args);
    if is_new && project {
        entry.decor_mut().set_prefix(PROJECT_TRUST_NOTE);
    }
    Ok(doc.to_string())
}

/// The removal counterpart of `merge_mcp_server`: drops the `skillvolution` table from
/// `mcp_servers`, and `mcp_servers` itself if that empties it. `None` when there was
/// nothing of ours to remove (no `mcp_servers`, or no `skillvolution` entry in it).
fn remove_mcp_server(text: &str) -> Result<Option<String>> {
    let mut doc = text
        .parse::<DocumentMut>()
        .context("config.toml must be valid TOML")?;
    let Some(item) = doc.get_mut("mcp_servers") else {
        return Ok(None);
    };
    let servers = item
        .as_table_like_mut()
        .context("mcp_servers must be a table")?;
    if servers.remove("skillvolution").is_none() {
        return Ok(None);
    }
    if servers.is_empty() {
        doc.remove("mcp_servers");
    }
    Ok(Some(doc.to_string()))
}

/// The MCP server's argv after the binary, as plain strings for a TOML array:
/// `--db <db> serve`, plus `--project <key>` for a project.
fn server_argv<'a>(db: &'a Path, key: Option<&'a str>) -> Result<Vec<&'a str>> {
    let db = hooks::require_utf8(db, "--db")?;
    let mut args = vec!["--db", db, "serve"];
    if let Some(key) = key {
        args.extend(["--project", key]);
    }
    Ok(args)
}
