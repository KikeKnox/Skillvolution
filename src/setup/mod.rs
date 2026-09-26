mod claude;
mod claude_cli;
mod client;
mod common;
mod devin;
mod fs_safe;
mod global;
mod hooks;
mod opencode;
mod permissions;
mod plugin;
mod prompt;
mod remove;

pub(crate) use client::{ClientKind, Clients, Scope};

use crate::vault::Vault;
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const SKILL: &str = include_str!("../../assets/evolution/SKILL.md");

/// A file setup writes, with its full new content.
pub(crate) type Change = (PathBuf, String);

#[derive(clap::Args)]
pub struct SetupArgs {
    /// Project directory to configure. Omit for global (per-user) setup, which every
    /// project then shares.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Which client(s) to configure: a comma-separated list of `claude-code`,
    /// `opencode`, `devin`, `all`, or `both` (claude-code + opencode, kept for
    /// backwards compatibility). Defaults to `all` for `--project`; for global
    /// setup, omitting it detects installed clients and asks which to configure.
    #[arg(
        long,
        value_delimiter = ',',
        value_parser = ["all", "both", "opencode", "claude-code", "devin"]
    )]
    client: Vec<String>,
    /// Path to the skillvolution binary; defaults to the currently running executable.
    #[arg(long)]
    bin: Option<PathBuf>,
    /// Database path; defaults to the global --db, then the standard data directory.
    #[arg(long)]
    db: Option<PathBuf>,
    /// Project scope key for the MCP server; defaults to the project directory name.
    /// Requires --project.
    #[arg(long)]
    project_key: Option<String>,
    /// Remove Skillvolution from the selected clients instead of configuring them,
    /// keeping every entry setup does not own.
    #[arg(long, conflicts_with_all = ["bin", "project_key", "dry_run"])]
    remove: bool,
    /// Print a unified diff of every file setup would write, without writing anything.
    #[arg(long)]
    dry_run: bool,
}

impl SetupArgs {
    /// Fills in `--db` from the global `--db` flag when `setup` wasn't given its own.
    pub fn with_default_db(mut self, db: Option<PathBuf>) -> Self {
        self.db = self.db.or(db);
        self
    }
}

/// Configures one project when `--project` is given, or the current user's global
/// (per-user, shared by every project) setup when it's omitted.
pub fn run(args: SetupArgs) -> Result<()> {
    ensure!(
        args.project.is_some() || args.project_key.is_none(),
        "--project-key requires --project"
    );
    if args.remove {
        let clients = if args.client.is_empty() {
            Clients::all()
        } else {
            Clients::parse(&args.client)?
        };
        return remove::run(clients, args.project.as_deref());
    }
    ensure!(!args.dry_run, "--dry-run is not implemented yet");
    let bin = resolve_bin(args.bin)?;
    // Client configs embed this path as a JSON string, which must be valid UTF-8.
    hooks::require_utf8(&bin, "--bin")?;

    match args.project {
        Some(project) => {
            let db = resolve_db(args.db)?;
            hooks::require_utf8(&db, "--db")?;
            // Before any client config is written, so an unusable --db leaves no config
            // pointing at it.
            Vault::open(&db).with_context(|| format!("open {}", db.display()))?;
            crate::vault::warn_if_unsafe(&db);
            let clients = if args.client.is_empty() {
                Clients::all()
            } else {
                Clients::parse(&args.client)?
            };
            run_project(project, clients, &bin, &db, args.project_key.as_deref())?;
        }
        None => {
            let clients = if args.client.is_empty() {
                None
            } else {
                Some(Clients::parse(&args.client)?)
            };
            global::run(clients, &bin, args.db)?;
        }
    }
    Ok(())
}

/// Makes `bin` absolute without resolving symlinks: package managers (Homebrew, Nix,
/// mise) expose a stable symlink into a versioned store path, and recording the
/// resolved target would break every config on the next upgrade.
fn resolve_bin(bin: Option<PathBuf>) -> Result<PathBuf> {
    let bin = match bin {
        Some(bin) => bin,
        None => std::env::current_exe().context("determine current executable")?,
    };
    let bin = std::path::absolute(&bin).context("resolve binary path")?;
    ensure!(bin.exists(), "binary must exist: {}", bin.display());
    ensure!(bin.is_file(), "binary must be a regular file");
    Ok(bin)
}

/// Registers the MCP server at user scope through the `claude` CLI, exactly as global
/// setup does. Exposed to `relocate`, which is outside this module's tree and so can't
/// reach `claude::register_mcp` (`pub(super)`) directly.
pub(crate) fn register_claude_code_mcp(bin: &Path, db: &Path) -> Result<String> {
    claude::register_mcp(bin, db)
}

fn resolve_db(db: Option<PathBuf>) -> Result<PathBuf> {
    let db = match db {
        Some(db) => db,
        None => crate::vault::default_database()?,
    };
    let db = absolutize(&db)?;
    ensure!(
        !db.exists() || db.is_file(),
        "database must be a regular file or a new path"
    );
    Ok(db)
}

fn run_project(
    project: PathBuf,
    clients: Clients,
    bin: &Path,
    db: &Path,
    project_key: Option<&str>,
) -> Result<()> {
    let project = fs::canonicalize(&project).context("project must exist")?;
    ensure!(project.is_dir(), "project must be a directory");
    let key = resolve_project_key(project_key, &project)?;

    let scope = Scope::Project {
        dir: &project,
        key: &key,
    };
    write_clients(clients, scope, bin, db)?;
    let tokens: Vec<&str> = clients.iter().map(ClientKind::token).collect();
    println!(
        "Configured {} for {}.",
        tokens.join(", "),
        project.display()
    );
    Ok(())
}

/// Writes every selected client's files for `scope`, all computed (and so validated)
/// before the first write.
fn write_clients(clients: Clients, scope: Scope, bin: &Path, db: &Path) -> Result<()> {
    let mut changes = Vec::new();
    for kind in clients.iter() {
        changes.extend(kind.changes(scope, bin, db)?);
    }
    write_all(changes)
}

/// Validates every write target before touching any of them, so a bad target further
/// down the list leaves everything already-checked untouched. If a write still fails,
/// the files written before it are put back as they were (see `roll_back`).
pub(crate) fn write_all(changes: Vec<Change>) -> Result<()> {
    for (path, _) in &changes {
        fs_safe::check_target(path)?;
        fs_safe::check_backups(path)?;
        for parent in path.ancestors().skip(1) {
            if parent.exists() {
                ensure!(parent.is_dir(), "not a directory: {}", parent.display());
            }
        }
    }
    // Each written path with its previous content (`None`: it didn't exist).
    let mut written: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    for (path, content) in changes {
        let result = fs_safe::read_optional_bytes(&path)
            .and_then(|old| fs_safe::write(&path, &content).map(|()| old));
        match result {
            Ok(old) => written.push((path, old)),
            Err(e) => {
                let context = match roll_back(written) {
                    Ok(()) => format!(
                        "setup failed at {}; earlier changes were rolled back",
                        path.display()
                    ),
                    Err(rollback) => format!(
                        "setup failed at {}; rolling back earlier changes also failed ({rollback:#}); \
                         inspect .skillvolution.bak backups",
                        path.display()
                    ),
                };
                return Err(e.context(context));
            }
        }
    }
    Ok(())
}

/// Undoes `written` newest first: a created file is deleted, a changed one gets its
/// previous content back. Keeps going past a failure and reports the first one.
fn roll_back(written: Vec<(PathBuf, Option<Vec<u8>>)>) -> Result<()> {
    let mut first_error = None;
    for (path, old) in written.into_iter().rev() {
        let result = match old {
            None => fs::remove_file(&path).with_context(|| format!("remove {}", path.display())),
            Some(old) => fs_safe::restore(&path, &old),
        };
        if let Err(e) = result {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Resolves `path` to an absolute path: canonicalizes it (also resolving symlinks) when
/// it exists, otherwise absolutizes it against the current directory and lexically
/// collapses `.`/`..` components. Unlike a plain existence check, this lets a `--db` path
/// that doesn't exist yet still contain `..`.
pub(crate) fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()));
    }
    let mut normalized = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

fn resolve_project_key(explicit: Option<&str>, project: &Path) -> Result<String> {
    let key = match explicit {
        Some(key) => key.to_owned(),
        None => crate::project::key_from_dir(project).with_context(|| {
            format!(
                "cannot derive a project key from {}; pass --project-key",
                project.display()
            )
        })?,
    };
    crate::vault::validate_id(&key).context("invalid --project-key")?;
    Ok(key)
}
