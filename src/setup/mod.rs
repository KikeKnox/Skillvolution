mod claude;
mod claude_cli;
mod client;
mod codex;
mod common;
mod cursor;
mod devin;
mod fs_safe;
pub(crate) use fs_safe::MANAGED_MARKER;
mod gemini;
mod global;
mod hooks;
mod inspect;
mod opencode;
mod permissions;
mod plugin;
mod prompt;
mod remove;

pub(crate) use client::{ClientKind, Clients, Scope};
pub(crate) use inspect::{
    claude_mcp_registered, configured_bin, double_install_warning, is_configured,
    missing_configured_bin, skill_marker_version,
};
pub(crate) use remove::Edit;

use crate::vault::Vault;
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The managed skills every client gets, as `(name, SKILL.md content)`: each is installed
/// at `<skills dir>/<name>/SKILL.md` and carries the marker
/// `<!-- skillvolution-managed:<name>:vN -->`. `evolution` drives the vault loop;
/// `skill-evaluation` is the evaluator subagent's rubric; `skill-acquisition` is what an
/// agent follows when it notices mid-task that it lacks the know-how.
pub(crate) const SKILLS: &[(&str, &str)] = &[
    ("evolution", include_str!("../../assets/evolution/SKILL.md")),
    (
        "skill-evaluation",
        include_str!("../../assets/skill-evaluation/SKILL.md"),
    ),
    (
        "skill-acquisition",
        include_str!("../../assets/skill-acquisition/SKILL.md"),
    ),
];

/// A file setup writes, with its full new content. `write_clients` turns each into an
/// `Edit::Write` before handing it to `write_all`; `remove::run` builds `Edit`s directly,
/// since a removal can also delete a file outright.
pub(crate) type Change = (PathBuf, String);

#[derive(clap::Args)]
pub struct SetupArgs {
    /// Project directory to configure. Omit for global (per-user) setup, which every
    /// project then shares.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Which client(s) to configure: a comma-separated list of `claude-code`, `opencode`,
    /// `devin`, `codex`, `gemini`, `cursor`, `all` (all clients), or `both` (claude-code +
    /// opencode, for backwards compatibility). For `--project`: omit to configure all
    /// clients, or specify some. For global setup: omit to detect installed clients and
    /// ask interactively, or specify some. For `--remove`: omit to remove from all
    /// configured clients, or specify some.
    #[arg(
        long,
        value_delimiter = ',',
        value_parser = [
            "all", "both", "opencode", "claude-code", "devin", "codex", "gemini", "cursor"
        ]
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
    #[arg(long, conflicts_with_all = ["bin", "project_key"])]
    remove: bool,
    /// Print a unified diff of every file setup would write (or remove), without writing
    /// anything.
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
        let explicit = if args.client.is_empty() {
            None
        } else {
            Some(Clients::parse(&args.client)?)
        };
        let db = resolve_db(args.db)?;
        return remove::run(explicit, args.project.as_deref(), &db, args.dry_run);
    }
    let bin = resolve_bin(args.bin.clone())?;
    // Client configs embed this path as a JSON string, which must be valid UTF-8.
    hooks::require_utf8(&bin, "--bin")?;
    if args.dry_run {
        let db = resolve_db(args.db.clone())?;
        hooks::require_utf8(&db, "--db")?;
        return dry_run_install(&args, &bin, &db);
    }

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

/// `--dry-run` without `--remove`: computes exactly the edits a real run would write and
/// prints their diffs, plus the `claude` CLI command a global Claude Code install would
/// run, without writing anything or opening the vault (so it never creates the database
/// file). Client selection mirrors the real run, except that global setup with no
/// `--client` uses detection alone (skipping the interactive prompt, since a preview
/// shouldn't block on stdin).
fn dry_run_install(args: &SetupArgs, bin: &Path, db: &Path) -> Result<()> {
    match &args.project {
        Some(project) => {
            let project = fs::canonicalize(project).context("project must exist")?;
            ensure!(project.is_dir(), "project must be a directory");
            let key = resolve_project_key(args.project_key.as_deref(), &project)?;
            let clients = if args.client.is_empty() {
                Clients::all()
            } else {
                Clients::parse(&args.client)?
            };
            print_install_diff(
                clients,
                Scope::Project {
                    dir: &project,
                    key: &key,
                },
                bin,
                db,
            )
        }
        None => {
            let clients = if args.client.is_empty() {
                ClientKind::ALL
                    .into_iter()
                    .filter(|kind| kind.detect())
                    .collect()
            } else {
                Clients::parse(&args.client)?
            };
            print_install_diff(clients, Scope::Global, bin, db)
        }
    }
}

/// Prints the diffs `clients` would write for `scope`, plus the `claude mcp add-json`
/// command a global Claude Code install would run, unless the registration it would
/// make already exists as is (see `claude::planned_registration`). Prints "No changes."
/// if there's nothing to show.
fn print_install_diff(clients: Clients, scope: Scope, bin: &Path, db: &Path) -> Result<()> {
    let mut changes = Vec::new();
    for kind in clients.iter() {
        changes.extend(kind.changes(scope, bin, db)?);
    }
    let edits: Vec<Edit> = changes
        .into_iter()
        .map(|(path, content)| Edit::Write(path, content))
        .collect();
    let mut printed = remove::print_diffs(&edits);
    if matches!(scope, Scope::Global)
        && clients.contains(ClientKind::ClaudeCode)
        && let Some(line) = claude::planned_registration(bin, db)?
    {
        println!("{line}");
        printed = true;
    }
    if !printed {
        println!("No changes.");
    }
    Ok(())
}

/// Whether `kind` is configured globally or, with `project`, in that project too (the
/// global scope alone when `None`). Only paths matter to `is_configured`, so fixed
/// placeholders stand in for the binary and database. When `changes` can't even be
/// computed (an unparsable client config), falls back to the managed `evolution` skill
/// file, so a broken config doesn't read as "not configured".
pub(crate) fn client_configured(kind: ClientKind, project: Option<&Path>) -> bool {
    let (bin, db) = (Path::new("/skillvolution"), Path::new("/skillvolution.db"));
    let project_scope = project.map(|dir| Scope::Project {
        dir,
        key: "project",
    });
    [Some(Scope::Global), project_scope]
        .into_iter()
        .flatten()
        .any(|scope| match kind.changes(scope, bin, db) {
            Ok(changes) => is_configured(&changes),
            Err(_) => {
                let dir = match (kind, scope) {
                    (_, Scope::Global) => kind.global_dir().ok(),
                    (ClientKind::ClaudeCode, Scope::Project { dir, .. }) => {
                        Some(dir.join(".claude"))
                    }
                    (ClientKind::OpenCode, Scope::Project { dir, .. }) => {
                        Some(dir.join(".opencode"))
                    }
                    _ => None,
                };
                dir.is_some_and(|dir| {
                    let skill = dir.join("skills").join("evolution").join("SKILL.md");
                    fs_safe::is_our_skill(&skill, "evolution").unwrap_or(false)
                })
            }
        })
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
    let edits = changes
        .into_iter()
        .map(|(path, content)| Edit::Write(path, content))
        .collect();
    write_all(edits)
}

/// Saves the current content of `path` as the newest numbered `.skillvolution.bak`
/// backup (a no-op when the file does not exist).
pub(crate) fn backup_file(path: &Path) -> Result<()> {
    match fs_safe::read_optional_bytes(path)? {
        Some(old) => fs_safe::backup(path, &old),
        None => Ok(()),
    }
}

/// Validates every write target before touching any of them, so a bad target further
/// down the list leaves everything already-checked untouched. If an edit still fails,
/// the edits applied before it are put back as they were (see `roll_back`).
pub(crate) fn write_all(edits: Vec<Edit>) -> Result<()> {
    validate_edits(&edits)?;
    write_validated(edits)
}

/// The checks `write_all` makes before touching anything.
pub(crate) fn validate_edits(edits: &[Edit]) -> Result<()> {
    for edit in edits {
        let path = edit.path();
        fs_safe::check_target(path)?;
        fs_safe::check_backups(path)?;
        for parent in path.ancestors().skip(1) {
            if parent.exists() {
                ensure!(parent.is_dir(), "not a directory: {}", parent.display());
            }
        }
    }
    Ok(())
}

fn write_validated(edits: Vec<Edit>) -> Result<()> {
    // Each edited path with its previous content (`None`: it didn't exist).
    let mut written: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    for edit in edits {
        let path = edit.path().to_owned();
        let result = apply(&edit);
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

/// Applies one edit, returning the path's previous content (`None` if it didn't exist)
/// so a later failure can restore it. Every `removals` implementation only emits a
/// `Delete` for a path it already found on disk, so deleting an already-absent one here
/// is not expected; if it happens anyway, it's a no-op that reports `None`, same as a
/// path that never existed.
fn apply(edit: &Edit) -> Result<Option<Vec<u8>>> {
    let path = edit.path();
    let old = fs_safe::read_optional_bytes(path)?;
    match edit {
        Edit::Write(_, content) => fs_safe::write(path, content)?,
        Edit::Delete(_) => {
            if old.is_some() {
                fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
            }
        }
    }
    Ok(old)
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

/// Resolves `path` to an absolute path lexically: absolutizes it against the current
/// directory and collapses `.`/`..` components, without resolving symlinks. The vault is
/// therefore spelled the same in every config whether or not it existed when setup ran
/// (resolving symlinks only once the file exists made reruns rewrite every config, e.g.
/// under macOS's `/var` -> `/private/var`), a `--db` that doesn't exist yet may still
/// contain `..`, and the result is never Windows' verbatim `\\?\C:\...` form.
pub(crate) fn absolutize(path: &Path) -> Result<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolutize_is_the_same_before_and_after_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        fs::create_dir(&link).unwrap();
        let db = link.join("skills.db");
        let before = absolutize(&db).unwrap();
        fs::write(&db, "").unwrap();
        assert_eq!(absolutize(&db).unwrap(), before);
        assert!(before.starts_with(&link), "{before:?}");
        assert!(!before.to_str().unwrap().starts_with(r"\\?\"), "{before:?}");
    }
}
