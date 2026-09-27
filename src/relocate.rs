//! `skillvolution relocate`: moves the vault database to a new local path and
//! points every configured client at it.

use crate::setup::{
    Change, ClientKind, Edit, Scope, absolutize, configured_bin, register_claude_code_mcp,
    write_all,
};
use crate::vault::{self, Vault};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// A global client config to rewrite: the client, the binary it stays configured with,
/// and the new content of each of its files.
type Repoint = (ClientKind, PathBuf, Vec<Change>);

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
    // Resolved while `from` still exists, so it compares equal to the canonical `from`.
    let cli_uses_from = vault::default_database()
        .ok()
        .and_then(|db| absolutize(&db).ok())
        .is_some_and(|db| db == from);
    // Before anything is written, so an unreadable client config aborts the relocation
    // instead of leaving clients on a vault that has already been moved away.
    let repoints = repoints(&from, &to)?;

    println!(
        "NOTE: close running AI client sessions: their MCP servers and hooks keep the old \
         vault open, and anything they write to it from now on is not carried over."
    );
    if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    vault::warn_if_unsafe(&to);

    let from_vault = Vault::open(&from).with_context(|| format!("open {}", from.display()))?;
    from_vault.backup(&to)?;
    let to_vault = Vault::open(&to).with_context(|| format!("open {}", to.display()))?;
    verify_copy(&from_vault, &to_vault)?;
    drop(to_vault);
    drop(from_vault);

    apply(repoints, &to)?;

    // Best effort: another process may still hold the old vault open, and whatever
    // stays in its WAL is kept anyway, since `retire` renames the sidecars with it.
    if let Err(error) = checkpoint(&from) {
        eprintln!(
            "warning: could not checkpoint {}: {error:#}",
            from.display()
        );
    }
    let backup = retire(&from)?;
    println!("Relocated the vault to {}.", to.display());
    println!(
        "The old vault was kept as a backup at {}.",
        backup.display()
    );
    if cli_uses_from {
        println!(
            "Note: CLI commands without --db still resolve to the old path. Set \
             SKILLVOLUTION_DB={} in your shell profile (or pass --db) so they use the new \
             vault; AI clients are already repointed.",
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

/// Every global client config that currently references `from`, with the content that
/// points it at `to` instead, keeping the binary the client is already configured with
/// when it still exists (otherwise the currently running one, global setup's own
/// default). A config "references `from`" when one of its files on disk mentions that
/// path (see `mentions`) — the same simple check setup's own idempotent rerun relies on
/// for hook commands. Fails if any client's config can't be read. Project-level
/// installs are never touched.
fn repoints(from: &Path, to: &Path) -> Result<Vec<Repoint>> {
    let bin = std::env::current_exe().context("determine current executable")?;
    let forms = path_forms(from)?;
    let mut repoints = Vec::new();
    for kind in ClientKind::ALL {
        let context = || format!("read the {} configuration", kind.display_name());
        let mut changes = kind
            .changes(Scope::Global, &bin, to)
            .with_context(context)?;
        let client_bin = configured_bin(kind, &changes).unwrap_or(bin.clone());
        if client_bin != bin {
            changes = kind
                .changes(Scope::Global, &client_bin, to)
                .with_context(context)?;
        }
        let referenced = changes.iter().any(|(path, _)| {
            std::fs::read_to_string(path)
                .is_ok_and(|text| forms.iter().any(|form| mentions(&text, form)))
        });
        if referenced {
            repoints.push((kind, client_bin, changes));
        }
    }
    Ok(repoints)
}

/// The spellings of `from` a config may carry: as setup writes it today (`from` is
/// already `absolutize`d), and its raw canonical form, which on Windows is the verbatim
/// `\\?\C:\...` path earlier builds wrote once the vault existed.
fn path_forms(from: &Path) -> Result<Vec<String>> {
    let mut forms = vec![
        from.to_str()
            .context("source path must be valid UTF-8")?
            .to_owned(),
    ];
    if let Some(canonical) = std::fs::canonicalize(from)
        .ok()
        .and_then(|path| path.to_str().map(str::to_owned))
        .filter(|canonical| *canonical != forms[0])
    {
        forms.push(canonical);
    }
    Ok(forms)
}

/// Whether a config file's `text` mentions `path`: verbatim, or escaped the way a JSON
/// string (and a TOML basic string, which escapes `\` and `"` the same way) stores it —
/// the only way a Windows path, with its backslashes doubled, shows up in one.
fn mentions(text: &str, path: &str) -> bool {
    let quoted = serde_json::Value::from(path).to_string();
    text.contains(path) || text.contains(&quoted[1..quoted.len() - 1])
}

/// Writes every repoint. Claude Code's MCP registration lives outside any file we
/// control (see `claude_cli`), so it's unconditionally re-registered at `to` whenever
/// its settings.json referenced `from`.
fn apply(repoints: Vec<Repoint>, to: &Path) -> Result<()> {
    let repointed_any = !repoints.is_empty();
    for (kind, bin, changes) in repoints {
        write_all(
            changes
                .into_iter()
                .map(|(path, content)| Edit::Write(path, content))
                .collect(),
        )?;
        println!("Repointed {} to {}.", kind.display_name(), to.display());
        if kind == ClientKind::ClaudeCode {
            println!("{}", register_claude_code_mcp(&bin, to)?);
        }
    }
    if repointed_any {
        println!(
            "Project-level installs are untouched; rerun `skillvolution setup --project <dir>` \
             for any project that points at the old vault."
        );
    }
    Ok(())
}

/// Folds `db`'s WAL back into the main file on a fresh connection, so the backup
/// `retire` leaves is complete on its own whenever no other process holds it open.
fn checkpoint(db: &Path) -> Result<()> {
    let conn = rusqlite::Connection::open(db)?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_row| Ok(()))?;
    Ok(())
}

/// Renames `from` to `from` + `.relocated.bak`, along with any `-wal`/`-shm` sidecar
/// files that remain (another process may still hold the vault open), under the names
/// SQLite pairs with the backup, so opening it later still sees their content. Nothing
/// is ever deleted.
fn retire(from: &Path) -> Result<PathBuf> {
    let backup = backup_path(from);
    for suffix in ["-wal", "-shm"] {
        let sidecar = with_suffix(from, suffix);
        if sidecar.exists() {
            let renamed = with_suffix(&backup, suffix);
            std::fs::rename(&sidecar, &renamed).with_context(|| {
                format!("rename {} to {}", sidecar.display(), renamed.display())
            })?;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_matches_a_windows_path_raw_or_escaped_as_json_and_toml() {
        let path = r"C:\Users\me\AppData\Local\skillvolution\skills.db";
        let json = r#"{"args": ["--db", "C:\\Users\\me\\AppData\\Local\\skillvolution\\skills.db", "serve"]}"#;
        let toml = r#"args = ["--db", "C:\\Users\\me\\AppData\\Local\\skillvolution\\skills.db"]"#;
        let literal = r"args = ['--db', 'C:\Users\me\AppData\Local\skillvolution\skills.db']";
        for text in [json, toml, literal] {
            assert!(mentions(text, path), "{text}");
        }
        assert!(!mentions(json, r"C:\Users\me\other.db"));
    }

    #[test]
    fn retire_renames_sidecars_with_the_backup_instead_of_deleting_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("skills.db");
        for (suffix, content) in [("", "main"), ("-wal", "wal"), ("-shm", "shm")] {
            std::fs::write(with_suffix(&db, suffix), content).unwrap();
        }

        let backup = retire(&db).unwrap();

        assert_eq!(backup, dir.path().join("skills.db.relocated.bak"));
        for (suffix, content) in [("", "main"), ("-wal", "wal"), ("-shm", "shm")] {
            assert!(!with_suffix(&db, suffix).exists(), "{suffix}");
            assert_eq!(
                std::fs::read_to_string(with_suffix(&backup, suffix)).unwrap(),
                content
            );
        }
    }
}
