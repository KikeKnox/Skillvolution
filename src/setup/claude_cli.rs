//! Registers the Skillvolution MCP server with Claude Code at user scope through the
//! `claude` CLI. We never edit `~/.claude.json` directly: Claude Code rewrites that file
//! on its own, so a direct edit would race it and get lost.

use anyhow::{Context, Result, bail};
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
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join("claude");
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

/// Removes any existing user-scope registration (a missing one is fine, so its failure
/// is ignored) then adds ours. `add-json` failing is fatal: it means the server did not
/// actually get registered.
pub fn register(claude: &Path, bin: &Path, db: &Path) -> Result<()> {
    let _ = Command::new(claude)
        .args(["mcp", "remove", "--scope", "user", "skillvolution"])
        .output();

    let json = mcp_json(bin, db);
    let output = Command::new(claude)
        .args(["mcp", "add-json", "--scope", "user", "skillvolution", &json])
        .output()
        .with_context(|| format!("run `{}`", add_json_command(bin, db)))?;
    if !output.status.success() {
        bail!(
            "claude mcp add-json failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
