mod claude;
mod claude_cli;
mod detect;
mod fs_safe;
mod global;
mod global_claude;
mod global_opencode;
mod hooks;
mod opencode;
mod plugin;

use crate::vault::Vault;
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const SKILL: &str = include_str!("../../assets/evolution/SKILL.md");

#[derive(clap::Args)]
pub struct SetupArgs {
    /// Project directory to configure. Omit for global (per-user) setup, which every
    /// project then shares.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Which client(s) to configure. Defaults to "both" for `--project`; for global
    /// setup, omitting it detects installed clients instead and configures only those.
    #[arg(long, value_parser = ["both", "opencode", "claude-code"])]
    client: Option<String>,
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
    let bin = resolve_bin(args.bin)?;
    let db = resolve_db(args.db)?;

    match args.project {
        Some(project) => {
            let client = args.client.as_deref().unwrap_or("both");
            run_project(project, client, &bin, &db, args.project_key.as_deref())?;
        }
        None => match args.client.as_deref() {
            Some(client) => global::run(client, &bin, &db)?,
            None => global::run_detected(&bin, &db)?,
        },
    }

    Vault::open(&db).with_context(|| format!("open {}", db.display()))?;
    Ok(())
}

fn resolve_bin(bin: Option<PathBuf>) -> Result<PathBuf> {
    let bin = match bin {
        Some(bin) => bin,
        None => std::env::current_exe().context("determine current executable")?,
    };
    let bin = fs::canonicalize(&bin).context("binary must exist")?;
    ensure!(bin.is_file(), "binary must be a regular file");
    Ok(bin)
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
    client: &str,
    bin: &Path,
    db: &Path,
    project_key: Option<&str>,
) -> Result<()> {
    let project = fs::canonicalize(&project).context("project must exist")?;
    ensure!(project.is_dir(), "project must be a directory");
    let key = resolve_project_key(project_key, &project)?;

    let mut changes = Vec::new();
    if client != "claude-code" {
        changes.extend(opencode::changes(&project, bin, db, &key)?);
    }
    if client != "opencode" {
        changes.extend(claude::changes(&project, bin, db, &key)?);
    }
    write_all(changes)?;
    println!("Configured {} for {}.", client, project.display());
    Ok(())
}

/// Validates every write target before touching any of them, so a bad target further
/// down the list leaves everything already-checked untouched.
fn write_all(changes: Vec<(PathBuf, String)>) -> Result<()> {
    for (path, _) in &changes {
        fs_safe::check_target(path)?;
        if path.exists() {
            fs_safe::backup_path(path)?;
        }
        for parent in path.ancestors().skip(1) {
            if parent.exists() {
                ensure!(parent.is_dir(), "not a directory: {}", parent.display());
            }
        }
    }
    for (path, content) in changes {
        fs_safe::write(&path, &content).with_context(|| {
            format!(
                "setup failed at {}; earlier changes may exist; inspect .skillvolution.bak backups",
                path.display()
            )
        })?;
    }
    Ok(())
}

/// Resolves `path` to an absolute path: canonicalizes it (also resolving symlinks) when
/// it exists, otherwise absolutizes it against the current directory and lexically
/// collapses `.`/`..` components. Unlike a plain existence check, this lets a `--db` path
/// that doesn't exist yet still contain `..`.
fn absolutize(path: &Path) -> Result<PathBuf> {
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
