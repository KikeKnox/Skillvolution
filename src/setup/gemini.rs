//! Gemini CLI: `mcpServers`/`hooks` entries in `settings.json`, the native skill file,
//! and the shared marker block in `GEMINI.md`. A project keeps them under `.gemini/`
//! (`GEMINI.md` at the root); globally they live in `~/.gemini`.
//!
//! Sources (re-verified against the gemini-cli repo, since the rendered docs lag it):
//! - MCP config shape, tool naming (`mcp_{server}_{tool}`), and the "no underscores in
//!   server names" rule:
//!   https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md and
//!   `packages/cli/src/config/settingsSchema.ts` (`McpServerConfig`, `additionalProperties:
//!   false` — only documented keys are safe to write).
//! - `GEMINI_CLI_HOME` overrides the home directory `~/.gemini` is computed from; not in
//!   the published docs, but read directly from `packages/core/src/utils/paths.ts`
//!   (`homedir()`), which is what the CLI actually runs.
//! - Hooks: event names, payload fields, matcher semantics (`RegExp::test`, so an
//!   unanchored pattern matches a substring — anchored here like Devin's), and the
//!   `decision`/`reason` (top-level, not nested in `hookSpecificOutput`) blocking
//!   contract for AfterAgent: https://geminicli.com/docs/hooks/reference/ and
//!   `packages/core/src/hooks/types.ts` + `packages/core/src/core/client.ts`
//!   (`fireAfterAgentHookSafe`, which re-fires AfterAgent with `stop_hook_active: true`
//!   on the retry it sends back — the same loop-protection convention as Claude Code and
//!   Devin, so `hook::devin_stop` covers it unchanged).
//! - `write_file`/`replace` as the built-in file-editing tool names:
//!   https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/file-system.md
//! - GEMINI.md and skill locations: https://geminicli.com/docs/cli/gemini-md/ and
//!   https://geminicli.com/docs/cli/skills/

use super::{Change, Edit, Scope, common, fs_safe, hooks};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `~/.gemini`, computed from `$GEMINI_CLI_HOME` when set (it substitutes for the home
/// directory, not for the whole config dir — see the module doc comment), else `$HOME`.
pub(super) fn global_dir() -> Result<PathBuf> {
    common::env_dir("GEMINI_CLI_HOME")
        .or_else(common::home)
        .map(|home| home.join(".gemini"))
        .context("set HOME")
}

pub(super) fn changes(scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".gemini")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut changes = vec![common::skill_change(&dir)?];

    let settings_path = dir.join("settings.json");
    let mut settings = fs_safe::load_json(&settings_path)?;
    let entry = json!({"command": bin, "args": common::server_args(db, scope.key())});
    fs_safe::merge_server(&mut settings, "mcpServers", entry)?;
    default_trust(&mut settings);
    merge_hooks(&mut settings, bin, db, scope.key())?;
    changes.push((settings_path, common::json_text(&settings)?));

    // GEMINI.md is a Gemini CLI memory file at the same level as `.gemini/`, not inside
    // it (like AGENTS.md for Devin/OpenCode).
    changes.push(common::agents_change(&root.join("GEMINI.md"))?);

    Ok(changes)
}

pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
    let (root, dir) = match scope {
        Scope::Project { dir, .. } => (dir.to_owned(), dir.join(".gemini")),
        Scope::Global => {
            let dir = global_dir()?;
            (dir.clone(), dir)
        }
    };
    let mut edits = Vec::new();
    if let Some(edit) = common::skill_removal(&dir, notes)? {
        edits.push(edit);
    }

    let settings_path = dir.join("settings.json");
    let mut settings = fs_safe::load_json(&settings_path)?;
    let mut changed = fs_safe::remove_server(&mut settings, "mcpServers", "skillvolution")?;
    changed |= hooks::remove_owned(&mut settings)?;
    if changed {
        edits.push(Edit::Write(settings_path, common::json_text(&settings)?));
    }

    if let Some(edit) = common::agents_removal(&root.join("GEMINI.md"))? {
        edits.push(edit);
    }

    Ok(edits)
}

/// Sets `mcpServers.skillvolution.trust: true` unless the entry already has a `trust`
/// value: the only way Gemini's config exposes to skip the tool-confirmation prompt for
/// just our server's tools (search_skills/get_skill/report_skill_outcome/publish_skill),
/// without touching any other server or granting the broader `trust` any server can set.
/// Only sets it when absent, like `merge_opencode`'s permission grants, so a user who
/// explicitly turned it off (`trust: false`) keeps that choice across reruns.
fn default_trust(settings: &mut Value) {
    if let Some(server) = settings["mcpServers"]["skillvolution"].as_object_mut() {
        server.entry("trust").or_insert(json!(true));
    }
}

/// The `hooks` entries for a Gemini `settings.json`: session catalog, tool-use flagging
/// for our work tools and the vault's own review tools, the review-blocking AfterAgent
/// (Gemini's Stop equivalent), and session cleanup. `key` scopes session-start to the
/// project, like the other clients' setups do.
///
/// Every flag-tracked hook here passes `--client gemini` explicitly, unlike Devin's
/// `tool-use`/reuses of the default: `HookEvent::ToolUse`'s `--client` defaults to Devin
/// (see `main.rs`), so omitting it here would read `DEVIN_WORK_TOOLS` instead of
/// `GEMINI_WORK_TOOLS`.
fn merge_hooks(settings: &mut Value, bin: &Path, db: &Path, key: Option<&str>) -> Result<()> {
    let session_start_extra = format!(" --client gemini{}", hooks::project_flag(key));
    let after_tool_matcher = format!(
        "^({}|mcp_skillvolution_.*)$",
        crate::hook::GEMINI_WORK_TOOLS.join("|")
    );
    let entries = [
        (
            "SessionStart",
            None,
            hooks::hook_cmd(bin, db, "session-start", &session_start_extra)?,
        ),
        (
            "AfterTool",
            Some(after_tool_matcher),
            hooks::hook_cmd(bin, db, "tool-use", " --client gemini")?,
        ),
        (
            "AfterAgent",
            None,
            hooks::hook_cmd(bin, db, "stop", " --client gemini")?,
        ),
        (
            "SessionEnd",
            None,
            hooks::hook_cmd(bin, db, "session-end", "")?,
        ),
    ];
    hooks::merge(settings, &entries)
}
