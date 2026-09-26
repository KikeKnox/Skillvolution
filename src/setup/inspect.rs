//! Read-only helpers `doctor` needs to inspect what `setup` already wrote,
//! without ever writing anything: every function here only reads files on
//! disk (via `fs_safe::load_json`, which treats a missing file as `{}`).

use super::{Change, ClientKind, claude_cli, fs_safe, hooks};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The version embedded in an evolution skill file's managed marker
/// (`<!-- skillvolution-managed:evolution:vN -->`), or `None` if `text` carries
/// no such marker.
pub(crate) fn skill_marker_version(text: &str) -> Option<u32> {
    let after = text.split_once("skillvolution-managed:evolution:v")?.1;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The file name that identifies `kind`'s hook/MCP config among the paths
/// `changes()` lists (as opposed to the skill file, AGENTS.md, or the
/// OpenCode plugin, none of which carry a bin path doctor can check
/// generically).
fn is_config_file(kind: ClientKind, path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match kind {
        ClientKind::ClaudeCode => name == "settings.json" || name == "settings.local.json",
        ClientKind::OpenCode => name == "opencode.json" || name == "opencode.jsonc",
        ClientKind::Devin => name == "config.json" || name == "mcp_config.json",
    }
}

/// Every Skillvolution binary path `kind`'s on-disk config currently points
/// at: hook commands (Claude Code, Devin) and MCP server commands (OpenCode,
/// Devin). Claude Code's MCP registration lives in the `claude` CLI's own
/// state, not a file, so it's checked separately (see `claude_mcp_registered`).
fn configured_bins(kind: ClientKind, changes: &[Change]) -> Vec<PathBuf> {
    let mut bins = Vec::new();
    for (path, _) in changes
        .iter()
        .filter(|(path, _)| is_config_file(kind, path))
    {
        let Ok(config) = fs_safe::load_json(path) else {
            continue;
        };
        bins.extend(hooks::owned_bin(&config));
        if let Some(bin) = config["mcp"]["skillvolution"]["command"][0].as_str() {
            bins.push(PathBuf::from(bin));
        }
        if let Some(bin) = config["mcpServers"]["skillvolution"]["command"].as_str() {
            bins.push(PathBuf::from(bin));
        }
    }
    bins
}

/// The first binary path among `configured_bins` that no longer exists or
/// isn't executable, if any.
pub(crate) fn missing_configured_bin(kind: ClientKind, changes: &[Change]) -> Option<PathBuf> {
    configured_bins(kind, changes)
        .into_iter()
        .find(|path| !claude_cli::is_executable(path))
}

/// Whether `path`'s config (if any) already carries an owned hook or MCP
/// entry: a hook command `owned_command` recognizes, or a `skillvolution`
/// entry under `mcpServers` or `mcp`.
fn config_has_owned(path: &Path) -> bool {
    let Ok(config) = fs_safe::load_json(path) else {
        return false;
    };
    hooks::has_owned(&config)
        || config["mcpServers"]["skillvolution"].is_object()
        || config["mcp"]["skillvolution"].is_object()
}

/// The project-level config paths `kind` writes under a project directory,
/// relative to the repo root, that could duplicate a global install.
fn project_config_paths(repo: &Path, kind: ClientKind) -> Vec<PathBuf> {
    match kind {
        ClientKind::ClaudeCode => vec![
            repo.join(".claude/settings.local.json"),
            repo.join(".mcp.json"),
        ],
        ClientKind::OpenCode => vec![repo.join("opencode.json")],
        ClientKind::Devin => vec![
            repo.join(".devin/config.json"),
            repo.join(".devin/mcp_config.json"),
        ],
    }
}

/// A warning when the current directory is inside a git repo that still
/// carries a project-level Skillvolution install (from `setup --project`) for
/// one of `globally_configured` clients: with the global setup too, that
/// client's hooks and MCP server would run twice in this repo.
pub(crate) fn double_install_warning(globally_configured: &[ClientKind]) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let repo = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let found: Vec<String> = globally_configured
        .iter()
        .flat_map(|kind| project_config_paths(repo, *kind))
        .filter(|path| config_has_owned(path))
        .map(|path| path.display().to_string())
        .collect();
    if found.is_empty() {
        return None;
    }
    Some(format!(
        "project-level install found ({}) alongside the global one; hooks will run twice",
        found.join(", ")
    ))
}

/// Whether `claude` is on `PATH` and its user-scope MCP registration for
/// skillvolution still exists. `None` when `claude` isn't on `PATH` (nothing
/// to check there).
pub(crate) fn claude_mcp_registered() -> Result<Option<bool>> {
    let Some(claude) = claude_cli::resolve() else {
        return Ok(None);
    };
    Ok(Some(claude_cli::mcp_registered(&claude)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_marker_version_parses_and_rejects_malformed_text() {
        assert_eq!(
            skill_marker_version("<!-- skillvolution-managed:evolution:v8 -->\nbody"),
            Some(8)
        );
        assert_eq!(skill_marker_version("no marker here"), None);
        assert_eq!(
            skill_marker_version("skillvolution-managed:evolution:vNOPE"),
            None
        );
    }
}
