//! `setup --remove`: undoes what setup wrote, keeping every entry it doesn't own, and
//! `setup --dry-run`, which previews either direction without writing anything.
//!
//! A client adds removal support by implementing one function, mirroring `changes`:
//!
//! ```ignore
//! pub(super) fn removals(scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>>;
//! ```
//!
//! It returns exactly the edits that undo what `changes` would write for that `scope`:
//! deleting a file `changes` creates outright, and rewriting a shared config to drop only
//! the entries `changes` put there (never a value the user could have added
//! independently, or entries some other tool owns). A file that doesn't exist, or that
//! exists but was never ours (missing our marker, or containing none of our entries),
//! produces no edit for it. `notes` collects anything worth telling the user that isn't a
//! file edit: an unowned file skipped instead of deleted, or a generic permission grant
//! (e.g. Claude Code's `Agent`/`Task`) kept because the user might have granted it
//! independently of Skillvolution. Dispatch it from `ClientKind::removals`.
//!
//! A side effect that isn't a file edit, like Claude Code's global MCP registration
//! (removed via the `claude` CLI, not a file), is handled outside `removals` and run only
//! after every file edit here has already succeeded (see `claude::unregister_mcp`).

use super::{ClientKind, Clients, Scope, claude, claude_cli, fs_safe};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A single file's edit: replace its content, or delete it outright.
#[derive(Clone, Debug)]
pub(crate) enum Edit {
    Write(PathBuf, String),
    Delete(PathBuf),
}

impl Edit {
    pub(super) fn path(&self) -> &Path {
        match self {
            Edit::Write(path, _) => path,
            Edit::Delete(path) => path,
        }
    }
}

/// Removes Skillvolution from the selected clients' configs: the project's files when
/// `project` is given, otherwise the user's global configs. `explicit` clients are always
/// attempted, even if there turns out to be nothing to remove for one of them; `None`
/// (no `--client`) instead removes from every client that has something to remove,
/// without prompting. Never touches the vault database or the skillvolution binary.
pub fn run(
    explicit: Option<Clients>,
    project: Option<&Path>,
    db: &Path,
    dry_run: bool,
) -> Result<()> {
    match project {
        Some(project) => {
            let project = fs::canonicalize(project).context("project must exist")?;
            ensure!(project.is_dir(), "project must be a directory");
            let key = super::resolve_project_key(None, &project)?;
            let scope = Scope::Project {
                dir: &project,
                key: &key,
            };
            run_scope(explicit, scope, db, dry_run)
        }
        None => run_scope(explicit, Scope::Global, db, dry_run),
    }
}

/// One client's computed removal, kept together so it can be printed and applied as a
/// unit.
struct Planned {
    kind: ClientKind,
    edits: Vec<Edit>,
    notes: Vec<String>,
}

fn run_scope(explicit: Option<Clients>, scope: Scope, db: &Path, dry_run: bool) -> Result<()> {
    let candidates: Vec<ClientKind> = match explicit {
        Some(clients) => clients.iter().collect(),
        None => ClientKind::ALL.to_vec(),
    };
    let mut planned = Vec::new();
    for kind in candidates {
        let mut notes = Vec::new();
        let edits = kind.removals(scope, &mut notes)?;
        // With no explicit --client, only clients with something to remove are
        // included, so nothing is reported (or written) for one that never had
        // Skillvolution configured.
        if explicit.is_some() || !edits.is_empty() {
            planned.push(Planned { kind, edits, notes });
        }
    }
    let claude_registered_globally =
        matches!(scope, Scope::Global) && planned.iter().any(|p| p.kind == ClientKind::ClaudeCode);

    if dry_run {
        return print_dry_run(&planned, claude_registered_globally);
    }
    if planned.is_empty() {
        println!("Nothing to remove.");
        return Ok(());
    }

    let all_edits: Vec<Edit> = planned.iter().flat_map(|p| p.edits.clone()).collect();
    super::write_all(all_edits)?;
    remove_emptied_skill_dirs(&planned);

    for p in &planned {
        let has_cli_action = p.kind == ClientKind::ClaudeCode && claude_registered_globally;
        if p.edits.is_empty() && p.notes.is_empty() && !has_cli_action {
            println!("Nothing to remove for {}.", p.kind.display_name());
            continue;
        }
        println!("{}", summary_line(p.kind, scope));
        if has_cli_action {
            println!("{}", claude::unregister_mcp()?);
        }
        for note in &p.notes {
            println!("note: {note}");
        }
    }
    println!(
        "Note: the vault database ({}) was left in place, and removing the skillvolution \
         binary itself is manual (e.g. `rm ~/.local/bin/skillvolution`). Backup files \
         (.skillvolution.bak*) next to any edited config were left too.",
        db.display()
    );
    Ok(())
}

/// Best-effort: once every edit has succeeded, drop the `evolution` skill directory that
/// a deleted `SKILL.md` leaves empty. Never fails setup: a directory that's non-empty
/// (another client's files still in `skills/`) or already gone is left alone.
fn remove_emptied_skill_dirs(planned: &[Planned]) {
    for p in planned {
        for edit in &p.edits {
            if let Edit::Delete(path) = edit
                && path.ends_with("skills/evolution/SKILL.md")
                && let Some(dir) = path.parent()
            {
                let _ = fs::remove_dir(dir);
            }
        }
    }
}

fn print_dry_run(planned: &[Planned], claude_registered_globally: bool) -> Result<()> {
    let mut printed = false;
    for p in planned {
        printed |= print_diffs(&p.edits);
        for note in &p.notes {
            println!("note: {note}");
            printed = true;
        }
    }
    if claude_registered_globally {
        println!("Would run: {}", claude_cli::remove_command());
        printed = true;
    }
    if !printed {
        println!("No changes.");
    }
    Ok(())
}

/// Prints a unified diff for each edit (`--- a/<path>` / `+++ b/<path>`; a deletion's diff
/// removes its current content in full, a new file's diff adds its new content in full).
/// Returns whether it printed anything.
pub(crate) fn print_diffs(edits: &[Edit]) -> bool {
    for edit in edits {
        print_diff(edit);
    }
    !edits.is_empty()
}

fn print_diff(edit: &Edit) {
    let path = edit.path();
    let old = fs_safe::read_optional(path)
        .unwrap_or_default()
        .unwrap_or_default();
    let new = match edit {
        Edit::Write(_, content) => content.clone(),
        Edit::Delete(_) => String::new(),
    };
    let diff = similar::TextDiff::from_lines(&old, &new)
        .unified_diff()
        .header(
            &format!("a/{}", path.display()),
            &format!("b/{}", path.display()),
        )
        .to_string();
    print!("{diff}");
}

/// The one-line summary printed once a client's files are removed.
fn summary_line(kind: ClientKind, scope: Scope) -> String {
    let where_ = match scope {
        Scope::Global => "global",
        Scope::Project { .. } => "project",
    };
    let mut what = match kind {
        ClientKind::ClaudeCode => "skill, settings hooks + permissions".to_owned(),
        ClientKind::OpenCode => "skill, config entry, AGENTS.md, plugin".to_owned(),
        ClientKind::Devin => {
            "skill, mcp_config.json, config.json hooks + permissions, AGENTS.md".to_owned()
        }
    };
    if kind == ClientKind::ClaudeCode {
        what.push_str(match scope {
            Scope::Global => ", MCP registration",
            Scope::Project { .. } => ", .mcp.json entry",
        });
    }
    format!(
        "Removed Skillvolution from {} ({where_}): {what}.",
        kind.display_name()
    )
}
