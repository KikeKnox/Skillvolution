//! Registers the Skillvolution MCP server with Claude Code at user scope through the
//! `claude` CLI. We never edit `~/.claude.json` directly: Claude Code rewrites that file
//! on its own, so a direct edit would race it and get lost.

use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn mcp_json(bin: &Path, db: &Path) -> String {
    serde_json::json!({
        "type": "stdio",
        "command": bin,
        "args": ["--db", db, "serve"],
    })
    .to_string()
}

/// The exact `claude mcp add-json` invocation, shown to the user when `claude` isn't on
/// PATH so they can register the server themselves later.
pub fn add_json_command(bin: &Path, db: &Path) -> String {
    format!(
        "claude mcp add-json --scope user skillvolution '{}'",
        mcp_json(bin, db)
    )
}

/// Finds `claude` on `PATH`, the same way a shell would: the first executable regular
/// file named `claude` in a `PATH` entry.
pub fn resolve() -> Option<PathBuf> {
    find_on_path("claude")
}

/// Finds `name` on `PATH`, the same way a shell would: the first executable regular file
/// named `name` in a `PATH` entry. Shared with `detect`, which uses it to look for other
/// clients' CLIs (e.g. `opencode`).
pub(super) fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        is_executable(&candidate).then_some(candidate)
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The result of one `claude mcp add-json` attempt.
enum AddOutcome {
    Added,
    /// Failed because a server named `skillvolution` is already registered there.
    AlreadyExists,
    Failed(String),
}

fn add_json(claude: &Path, json: &str) -> Result<AddOutcome> {
    let output = Command::new(claude)
        .args(["mcp", "add-json", "--scope", "user", "skillvolution", json])
        .output()
        .context("run `claude mcp add-json`")?;
    if output.status.success() {
        return Ok(AddOutcome::Added);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stderr.contains("already exists") {
        Ok(AddOutcome::AlreadyExists)
    } else {
        Ok(AddOutcome::Failed(stderr))
    }
}

/// Registers `skillvolution` at user scope without ever leaving a working registration
/// lost to a failed update. `add-json` is tried directly first, so a name that isn't
/// registered yet is a single call; the existing entry is removed only once we know from
/// that first failure that it's actually there. If the follow-up add then fails too, the
/// registration is gone: `claude mcp get` reports a stdio server's args space-joined
/// with no escaping, so a previous entry (particularly one with spaces in its own
/// `--db`) can't be read back and reconstructed reliably, and we report the removal
/// clearly instead, with the exact command to redo it.
pub fn register(claude: &Path, bin: &Path, db: &Path) -> Result<()> {
    let json = mcp_json(bin, db);
    match add_json(claude, &json)? {
        AddOutcome::Added => return Ok(()),
        AddOutcome::AlreadyExists => {}
        AddOutcome::Failed(stderr) => bail!("claude mcp add-json failed: {stderr}"),
    }

    let remove = Command::new(claude)
        .args(["mcp", "remove", "--scope", "user", "skillvolution"])
        .output()
        .context("run `claude mcp remove --scope user skillvolution`")?;
    ensure!(
        remove.status.success(),
        "claude mcp remove failed: {}",
        String::from_utf8_lossy(&remove.stderr)
    );

    match add_json(claude, &json)? {
        AddOutcome::Added | AddOutcome::AlreadyExists => Ok(()),
        AddOutcome::Failed(stderr) => bail!(
            "claude mcp add-json failed after removing the previous skillvolution \
             registration: {stderr}\nThe user-scope registration is now gone; re-add it with:\n{}",
            add_json_command(bin, db)
        ),
    }
}
