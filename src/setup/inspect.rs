//! Read-only helpers `doctor` needs to inspect what `setup` already wrote,
//! without ever writing anything: every function here only reads files on
//! disk (via `fs_safe::load_json`, which treats a missing file as `{}`).

use super::{Change, ClientKind, claude_cli, common, fs_safe, hooks, plugin};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The version embedded in the managed skill `name`'s marker
/// (`<!-- skillvolution-managed:<name>:vN -->`), or `None` if `text` carries
/// no such marker.
pub(crate) fn skill_marker_version(text: &str, name: &str) -> Option<u32> {
    let after = text
        .split_once(&format!("skillvolution-managed:{name}:v"))?
        .1;
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
        // mcp.json carries the bin path in mcpServers.skillvolution.command (the
        // generic check in `configured_bins` below already reads that); hooks.json
        // uses Cursor's own flat per-event shape (see `cursor::merge_hooks`), which
        // `hooks::owned_bin` doesn't parse, so listing it here finds no bin path but
        // is otherwise harmless.
        ClientKind::Cursor => name == "mcp.json" || name == "hooks.json",
        // Codex's MCP entry lives in `config.toml` (TOML, not JSON, so `load_json`
        // below can't read it); `hooks.json` alone carries a recognizable bin path.
        ClientKind::Codex => name == "hooks.json",
        ClientKind::Gemini => name == "settings.json",
    }
}

/// Every Skillvolution binary path `kind`'s on-disk config currently points
/// at: hook commands (Claude Code, Devin) and MCP server commands (OpenCode,
/// Devin). Claude Code's MCP registration lives in the `claude` CLI's own
/// state, not a file, so it's checked separately (see `claude_mcp_registration`).
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

/// The first configured binary that still exists and is executable, so a
/// client set up with `--bin` is compared against that binary, not this one.
pub(crate) fn configured_bin(kind: ClientKind, changes: &[Change]) -> Option<PathBuf> {
    configured_bins(kind, changes)
        .into_iter()
        .find(|path| claude_cli::is_executable(path))
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
/// entry under `mcpServers` or `mcp` (JSON) or `mcp_servers` (Codex's TOML).
fn config_has_owned(path: &Path) -> bool {
    if path.extension().is_some_and(|ext| ext == "toml") {
        let Ok(Some(text)) = fs_safe::read_optional(path) else {
            return false;
        };
        return text.parse::<toml_edit::DocumentMut>().is_ok_and(|doc| {
            doc.get("mcp_servers")
                .and_then(|servers| servers.get("skillvolution"))
                .is_some()
        });
    }
    let Ok(config) = fs_safe::load_json(path) else {
        return false;
    };
    hooks::has_owned(&config)
        || config["mcpServers"]["skillvolution"].is_object()
        || config["mcp"]["skillvolution"].is_object()
}

/// Whether a client is configured, judged by the files its `changes()` lists: one of
/// them already carries something setup owns, by setup's own ownership predicates (an
/// owned hook command or MCP server entry, our AGENTS.md/GEMINI.md marker block, a
/// managed skill file, or our OpenCode plugin). A file that merely mentions
/// "skillvolution" (a permission entry, a comment) doesn't count, so `doctor --fix`
/// never configures a client the user didn't.
pub(crate) fn is_configured(changes: &[Change]) -> bool {
    changes.iter().any(|(path, _)| carries_owned(path))
}

fn carries_owned(path: &Path) -> bool {
    let our_skill = super::SKILLS.iter().any(|(name, _)| {
        path.ends_with(common::skill_path(Path::new(""), name))
            && fs_safe::is_our_skill(path, name).unwrap_or(false)
    });
    let our_plugin = path.ends_with(Path::new("plugins").join("skillvolution.js"))
        && plugin::is_ours(path).unwrap_or(false);
    let our_block = fs_safe::read_optional(path)
        .is_ok_and(|text| text.is_some_and(|text| text.contains(common::START)));
    our_skill || our_plugin || our_block || config_has_owned(path)
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
        ClientKind::Cursor => vec![
            repo.join(".cursor/mcp.json"),
            repo.join(".cursor/hooks.json"),
        ],
        ClientKind::Codex => vec![
            repo.join(".codex/config.toml"),
            repo.join(".codex/hooks.json"),
        ],
        ClientKind::Gemini => vec![repo.join(".gemini/settings.json")],
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

/// The state of Claude Code's user-scope MCP registration for skillvolution.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClaudeMcp {
    Missing,
    /// Registered; whether its args lack `--client` (`false` when they can't be read).
    Registered {
        lacks_client: bool,
    },
}

/// Claude Code's user-scope MCP registration for skillvolution, read through the
/// `claude` CLI. `None` when `claude` isn't on `PATH` (nothing to check there).
pub(crate) fn claude_mcp_registration() -> Result<Option<ClaudeMcp>> {
    let Some(claude) = claude_cli::resolve() else {
        return Ok(None);
    };
    Ok(Some(match claude_cli::mcp_registration_args(&claude)? {
        None => ClaudeMcp::Missing,
        Some(args) => ClaudeMcp::Registered {
            lacks_client: args.is_some_and(|args| !has_client_flag(args.split_whitespace())),
        },
    }))
}

fn has_client_flag<'a>(mut args: impl Iterator<Item = &'a str>) -> bool {
    args.any(|arg| arg == "--client")
}

/// Whether one of the config files among `changes` registers the skillvolution MCP
/// server without `--client` (written by a setup from before v0.7.0).
pub(crate) fn mcp_lacks_client(changes: &[Change]) -> bool {
    changes.iter().any(|(path, _)| {
        mcp_server_args(path).is_some_and(|args| !has_client_flag(args.iter().map(String::as_str)))
    })
}

/// The argv of the `skillvolution` MCP server entry in the config file at `path`:
/// `mcpServers.skillvolution.args` or OpenCode's `mcp.skillvolution.command` (JSON), or
/// Codex's `mcp_servers.skillvolution.args` (TOML). `None` without such an entry.
fn mcp_server_args(path: &Path) -> Option<Vec<String>> {
    let strings = |values: &[serde_json::Value]| -> Vec<String> {
        values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => {
            let text = fs_safe::read_optional(path).ok()??;
            let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
            let args = doc
                .get("mcp_servers")?
                .get("skillvolution")?
                .get("args")?
                .as_array()?;
            Some(
                args.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
            )
        }
        Some("json" | "jsonc") => {
            let config = fs_safe::load_json(path).ok()?;
            config["mcpServers"]["skillvolution"]["args"]
                .as_array()
                .or_else(|| config["mcp"]["skillvolution"]["command"].as_array())
                .map(|values| strings(values))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_marker_version_parses_and_rejects_malformed_text() {
        assert_eq!(
            skill_marker_version(
                "<!-- skillvolution-managed:evolution:v8 -->\nbody",
                "evolution"
            ),
            Some(8)
        );
        assert_eq!(skill_marker_version("no marker here", "evolution"), None);
        assert_eq!(
            skill_marker_version("skillvolution-managed:evolution:vNOPE", "evolution"),
            None
        );
        assert_eq!(
            skill_marker_version(
                "<!-- skillvolution-managed:skill-evaluation:v2 -->",
                "evolution"
            ),
            None,
            "another skill's marker"
        );
    }

    #[test]
    fn every_managed_skill_carries_its_own_marker_and_frontmatter_name() {
        for (name, content) in crate::setup::SKILLS {
            assert!(
                content.starts_with(&format!("---\nname: {name}\n")),
                "{name}: frontmatter name"
            );
            assert!(
                skill_marker_version(content, name).is_some(),
                "{name}: managed marker"
            );
            for heading in [
                "## When to use",
                "## Procedure",
                "## Pitfalls",
                "## Verification",
            ] {
                assert!(content.contains(heading), "{name}: missing {heading}");
            }
        }
    }
}
