mod claude;
mod claude_cli;
mod detect;
mod devin;
mod fs_safe;
mod global;
mod global_claude;
mod global_devin;
mod global_opencode;
mod hooks;
mod opencode;
mod permissions;
mod plugin;
mod prompt;

use crate::vault::Vault;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const SKILL: &str = include_str!("../../assets/evolution/SKILL.md");

/// The set of AI clients a setup run configures, whether chosen explicitly via
/// `--client` or detected/prompted for a global run.
#[derive(Clone, Copy, Default)]
struct Clients {
    claude_code: bool,
    opencode: bool,
    devin: bool,
}

impl Clients {
    fn all() -> Self {
        Self {
            claude_code: true,
            opencode: true,
            devin: true,
        }
    }

    fn any(self) -> bool {
        self.claude_code || self.opencode || self.devin
    }

    /// (selected, display name, `--client` token) in stable order.
    fn list(self) -> [(bool, &'static str, &'static str); 3] {
        [
            (self.claude_code, "Claude Code", "claude-code"),
            (self.opencode, "OpenCode", "opencode"),
            (self.devin, "Devin CLI", "devin"),
        ]
    }

    /// The `--client` tokens for the selected clients, in stable order.
    fn cli_names(self) -> Vec<&'static str> {
        self.list()
            .into_iter()
            .filter_map(|(on, _, token)| on.then_some(token))
            .collect()
    }

    /// Maps `--client` tokens to flags: `all` selects every client, `both` keeps
    /// its original meaning (Claude Code + OpenCode).
    fn parse(tokens: &[String]) -> Result<Self> {
        let mut clients = Clients::default();
        for token in tokens {
            match token.as_str() {
                "all" => clients = Clients::all(),
                "both" => {
                    clients.claude_code = true;
                    clients.opencode = true;
                }
                "claude-code" => clients.claude_code = true,
                "opencode" => clients.opencode = true,
                "devin" => clients.devin = true,
                other => bail!("unknown --client {other}"),
            }
        }
        ensure!(clients.any(), "--client selects no clients");
        Ok(clients)
    }
}

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
    // Client configs embed these paths as JSON strings, which must be valid UTF-8.
    hooks::require_utf8(&bin, "--bin")?;
    hooks::require_utf8(&db, "--db")?;

    match args.project {
        Some(project) => {
            let clients = if args.client.is_empty() {
                Clients::all()
            } else {
                Clients::parse(&args.client)?
            };
            run_project(project, clients, &bin, &db, args.project_key.as_deref())?;
        }
        None => {
            if args.client.is_empty() {
                global::run_detected(&bin, &db)?;
            } else {
                global::run(Clients::parse(&args.client)?, &bin, &db)?;
            }
        }
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
    clients: Clients,
    bin: &Path,
    db: &Path,
    project_key: Option<&str>,
) -> Result<()> {
    let project = fs::canonicalize(&project).context("project must exist")?;
    ensure!(project.is_dir(), "project must be a directory");
    let key = resolve_project_key(project_key, &project)?;

    let mut changes = Vec::new();
    if clients.opencode {
        changes.extend(opencode::changes(&project, bin, db, &key)?);
    }
    if clients.claude_code {
        changes.extend(claude::changes(&project, bin, db, &key)?);
    }
    if clients.devin {
        changes.extend(devin::changes(&project, bin, db, &key)?);
    }
    write_all(changes)?;
    println!(
        "Configured {} for {}.",
        clients.cli_names().join(", "),
        project.display()
    );
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
