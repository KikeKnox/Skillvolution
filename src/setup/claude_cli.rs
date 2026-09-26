//! Registers the Skillvolution MCP server with Claude Code at user scope through the
//! `claude` CLI. We never edit `~/.claude.json` directly: Claude Code rewrites that file
//! on its own, so a direct edit would race it and get lost.

use super::{common, hooks};
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn mcp_json(bin: &Path, db: &Path) -> String {
    serde_json::json!({
        "type": "stdio",
        "command": bin,
        "args": common::server_args(db, None),
    })
    .to_string()
}

/// The exact `claude mcp add-json` invocation, shown to the user when `claude` isn't on
/// PATH so they can register the server themselves later.
pub fn add_json_command(bin: &Path, db: &Path) -> String {
    format!(
        "claude mcp add-json --scope user skillvolution {}",
        hooks::shell_quote(&mcp_json(bin, db))
    )
}

/// The exact `claude mcp remove` invocation, shown to the user when `claude` isn't on
/// PATH so they can remove the registration themselves.
pub fn remove_command() -> String {
    "claude mcp remove --scope user skillvolution".to_owned()
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

/// How long a `claude mcp` call may take before it's killed: the CLI can hang (e.g.
/// waiting on a login or network prompt), and setup must not hang with it.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Runs `claude` with `args`, capturing its output, and kills it once the timeout
/// passes. `SKILLVOLUTION_CLAUDE_TIMEOUT_SECS` overrides the timeout (used by tests).
/// Polling `try_wait` leaves the output in the pipes until exit, which is fine for the
/// few lines `claude mcp` prints.
fn run(claude: &Path, args: &[&str]) -> Result<Output> {
    let timeout_secs = std::env::var("SKILLVOLUTION_CLAUDE_TIMEOUT_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let command = format!("claude {}", args.join(" "));
    let mut child = Command::new(claude)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run `{command}`"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("`{command}` timed out after {timeout_secs}s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child
        .wait_with_output()
        .with_context(|| format!("run `{command}`"))
}

fn add_json(claude: &Path, json: &str) -> Result<AddOutcome> {
    let output = run(
        claude,
        &["mcp", "add-json", "--scope", "user", "skillvolution", json],
    )?;
    if output.status.success() {
        return Ok(AddOutcome::Added);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stdout.contains("already exists") || stderr.contains("already exists") {
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

    let remove = run(
        claude,
        &["mcp", "remove", "--scope", "user", "skillvolution"],
    )?;
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

/// Removes the user-scope `skillvolution` registration. Idempotent, like `register`: the
/// CLI's exact wording for "nothing registered under that name" isn't a documented,
/// stable string, so any failure whose output mentions "not found" or "no such" is
/// treated as already-removed rather than an error.
pub fn unregister(claude: &Path) -> Result<()> {
    let output = run(
        claude,
        &["mcp", "remove", "--scope", "user", "skillvolution"],
    )?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    let not_registered = |text: &str| text.contains("not found") || text.contains("no such");
    if not_registered(&stdout) || not_registered(&stderr) {
        return Ok(());
    }
    bail!(
        "claude mcp remove failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
