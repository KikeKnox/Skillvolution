//! `skillvolution relocate`: moves the vault database to a new local path and
//! points every configured client at it.

use crate::setup::{ClientKind, Edit, Scope, absolutize, register_claude_code_mcp, write_all};
use crate::vault::{self, Vault};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// Copies the vault at `from` to `to` (verifying the copy), repoints every global
/// client config that referenced `from`, and renames `from` aside as a backup so a
/// stale copy is never silently reopened.
pub fn run(from: &Path, to: &Path) -> Result<()> {
    ensure!(
        from.exists(),
        "nothing to relocate: {} does not exist",
        from.display()
    );
    let from = absolutize(from)?;
    let to = absolutize(to)?;
    ensure!(
        to != from,
        "relocate destination must differ from the source"
    );
    ensure!(!to.exists(), "{} already exists", to.display());
    if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    vault::warn_if_unsafe(&to);

    let from_vault = Vault::open(&from).with_context(|| format!("open {}", from.display()))?;
    from_vault.backup(&to)?;
    let to_vault = Vault::open(&to).with_context(|| format!("open {}", to.display()))?;
    verify_copy(&from_vault, &to_vault)?;
    drop(to_vault);

    repoint_clients(&from, &to)?;

    // Closing the last connection to a WAL database makes SQLite checkpoint it on its
    // own, so the sidecar files are gone (or empty) by the time we remove `from`.
    drop(from_vault);
    let backup = retire(&from)?;
    println!("Relocated the vault to {}.", to.display());
    println!(
        "The old vault was kept as a backup at {}.",
        backup.display()
    );
    if env_db_points_at(&from) {
        println!(
            "Note: SKILLVOLUTION_DB is set to the old path; update it to {}.",
            to.display()
        );
    }
    Ok(())
}

/// Confirms `to` holds exactly what `from` did — aside from the export timestamp, which
/// always differs — after `backup` wrote it via `VACUUM INTO`.
fn verify_copy(from: &Vault, to: &Vault) -> Result<()> {
    let without_timestamp = |json: &str| -> Result<serde_json::Value> {
        let mut value: serde_json::Value = serde_json::from_str(json)?;
        value
            .as_object_mut()
            .context("export must be a JSON object")?
            .remove("exported_at");
        Ok(value)
    };
    let from_json = without_timestamp(&from.export_json()?)?;
    let to_json = without_timestamp(&to.export_json()?)?;
    ensure!(
        from_json == to_json,
        "the relocated copy at the new path does not match the source vault; aborting \
         before touching any client configuration"
    );
    Ok(())
}

/// Rewrites every global client config that currently references `from` so it points
/// at `to` instead, keeping the binary the client is already configured with when it
/// still exists (otherwise the currently running one, global setup's own default). A config "references `from`" when its file on disk
/// contains that path string — the same simple check setup's own idempotent rerun
/// relies on for hook commands. Claude Code's MCP registration lives outside any file
/// we control (see `claude_cli`), so it's unconditionally re-registered at `to`
/// whenever its settings.json referenced `from`. Project-level installs are never
/// touched.
fn repoint_clients(from: &Path, to: &Path) -> Result<()> {
    let bin = std::env::current_exe().context("determine current executable")?;
    let needle = from.to_str().context("source path must be valid UTF-8")?;
    let mut repointed_any = false;
    for kind in ClientKind::ALL {
        let mut changes = kind.changes(Scope::Global, &bin, to)?;
        let client_bin = crate::setup::configured_bin(kind, &changes).unwrap_or(bin.clone());
        if client_bin != bin {
            changes = kind.changes(Scope::Global, &client_bin, to)?;
        }
        let referenced = changes
            .iter()
            .any(|(path, _)| std::fs::read_to_string(path).is_ok_and(|text| text.contains(needle)));
        if !referenced {
            continue;
        }
        write_all(
            changes
                .into_iter()
                .map(|(path, content)| Edit::Write(path, content))
                .collect(),
        )?;
        println!("Repointed {} to {}.", kind.display_name(), to.display());
        if kind == ClientKind::ClaudeCode {
            println!("{}", register_claude_code_mcp(&client_bin, to)?);
        }
        repointed_any = true;
    }
    if repointed_any {
        println!(
            "Project-level installs are untouched; rerun `skillvolution setup --project <dir>` \
             for any project that points at the old vault."
        );
    }
    Ok(())
}

/// Renames `from` to `from` + `.relocated.bak`, first dropping its now-unneeded
/// `-wal`/`-shm` sidecar files (if any remain) so a later accidental open of the backup
/// doesn't replay stale WAL frames against it.
fn retire(from: &Path) -> Result<PathBuf> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = with_suffix(from, suffix);
        if sidecar.exists() {
            std::fs::remove_file(&sidecar)
                .with_context(|| format!("remove {}", sidecar.display()))?;
        }
    }
    let backup = backup_path(from);
    std::fs::rename(from, &backup)
        .with_context(|| format!("rename {} to {}", from.display(), backup.display()))?;
    Ok(backup)
}

/// Where `relocate` keeps the old vault at `db` once it has moved it.
pub(crate) fn backup_path(db: &Path) -> PathBuf {
    with_suffix(db, ".relocated.bak")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Whether `SKILLVOLUTION_DB` is set in the environment and points at `from`.
fn env_db_points_at(from: &Path) -> bool {
    std::env::var_os("SKILLVOLUTION_DB")
        .filter(|v| !v.is_empty())
        .and_then(|v| absolutize(Path::new(&v)).ok())
        .is_some_and(|path| path == from)
}
