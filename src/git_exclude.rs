//! Keeps the agent files Skillvolution writes inside a project out of git: one line per
//! file, anchored at the repository root, in a marked block of the repository's local
//! `info/exclude` (never `.gitignore`, which the user versions). A generated agent must
//! not end up in a commit by accident.

use crate::project;
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

const START: &str = "# skillvolution:start (generated agent files, kept out of git)";
const END: &str = "# skillvolution:end";

/// Adds a line for each of `files` (inside `project_dir`) to the exclude file of the
/// repository holding `project_dir`. Never fails the caller: returns a short note when
/// it could not (not a git repository, git unavailable for a worktree, a write error).
pub fn add(project_dir: &Path, files: &[PathBuf]) -> Option<String> {
    update(project_dir, files, true)
}

/// Removes the lines `add` wrote for `files`, and the block once it is empty. Every
/// worktree of a repository shares one exclude file, so a line stays while its path still
/// exists in another worktree.
pub fn remove(project_dir: &Path, files: &[PathBuf]) -> Option<String> {
    update(project_dir, files, false)
}

fn update(project_dir: &Path, files: &[PathBuf], add: bool) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let Some(root) = project::repo_dir(project_dir) else {
        return add.then(|| {
            format!(
                "note: {} is not a git repository; agent files were not added to .git/info/exclude",
                project_dir.display()
            )
        });
    };
    let Some(exclude) = exclude_path(&root) else {
        return Some(format!(
            "note: could not locate the git directory of {} (is git on PATH?); .git/info/exclude not updated",
            root.display()
        ));
    };
    let lines: Vec<String> = files.iter().filter_map(|f| pattern(&root, f)).collect();
    let text = match fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Some(format!("note: could not read {}: {e}", exclude.display())),
    };
    let updated = if add {
        with_lines(&text, &lines, &[])
    } else {
        let trees = worktrees(&root);
        let gone: Vec<String> = lines
            .into_iter()
            .filter(|line| {
                let relative = line.trim_start_matches('/');
                !trees.iter().any(|tree| tree.join(relative).exists())
            })
            .collect();
        with_lines(&text, &[], &gone)
    };
    if updated == text {
        return None;
    }
    let written = exclude
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&exclude, updated));
    written
        .err()
        .map(|e| format!("note: could not update {}: {e}", exclude.display()))
}

/// The `info/exclude` of the repository whose work tree root is `root`: under `.git`
/// when it is a directory; otherwise (a worktree or submodule, where `.git` is a file)
/// under the common git directory git reports.
fn exclude_path(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return Some(git.join("info").join("exclude"));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-common-dir"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let dir = String::from_utf8(output.stdout).ok()?;
    let dir = dir.trim();
    (output.status.success() && !dir.is_empty())
        .then(|| root.join(dir).join("info").join("exclude"))
}

/// The work tree roots of the repository whose work tree `root` is, as `git worktree list
/// --porcelain` reports them; just `root` when git cannot say.
fn worktrees(root: &Path) -> Vec<PathBuf> {
    let listed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.strip_prefix("worktree "))
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if listed.is_empty() {
        vec![root.to_path_buf()]
    } else {
        listed
    }
}

/// `/`-anchored path of `file` relative to `root`, `None` when it is not inside it.
fn pattern(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| format!("/{}", parts.join("/")))
}

/// `text` with our block holding its previous lines minus `remove` plus `add` (in that
/// order, without duplicates); the block is dropped when that leaves it empty and
/// appended at the end when there was none. A start marker without an end marker opens a
/// block that runs to the end of the file.
fn with_lines(text: &str, add: &[String], remove: &[String]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim_end() == START);
    let end = start.and_then(|s| {
        lines[s..]
            .iter()
            .position(|l| l.trim_end() == END)
            .map(|e| s + e)
    });
    let entries_of = |block: &[&str]| -> Vec<String> {
        block
            .iter()
            .map(|l| l.trim_end().to_owned())
            .filter(|l| !l.is_empty())
            .collect()
    };
    let (before, mut entries, after): (Vec<&str>, Vec<String>, Vec<&str>) = match (start, end) {
        (Some(s), Some(e)) => (
            lines[..s].to_vec(),
            entries_of(&lines[s + 1..e]),
            lines[e + 1..].to_vec(),
        ),
        (Some(s), None) => (lines[..s].to_vec(), entries_of(&lines[s + 1..]), Vec::new()),
        _ => (lines.clone(), Vec::new(), Vec::new()),
    };
    entries.retain(|entry| !remove.contains(entry));
    for line in add {
        if !entries.contains(line) {
            entries.push(line.clone());
        }
    }
    let mut out: Vec<String> = before.iter().map(|l| (*l).to_owned()).collect();
    if !entries.is_empty() {
        out.push(START.to_owned());
        out.append(&mut entries);
        out.push(END.to_owned());
    }
    out.extend(after.iter().map(|l| (*l).to_owned()));
    if out.is_empty() {
        return String::new();
    }
    let joined = out.join("\n") + "\n";
    // Leave the file byte-identical when nothing changed (e.g. no trailing newline).
    if joined.trim_end() == text.trim_end() {
        text.to_owned()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| (*l).to_owned()).collect()
    }

    #[test]
    fn adds_a_marked_block_after_existing_rules_and_is_idempotent() {
        let original = "# git ls-files --others --exclude-from=.git/info/exclude\n*.log\n";
        let once = with_lines(original, &s(&["/.claude/agents/reviewer.md"]), &[]);
        assert_eq!(
            once,
            format!("{original}{START}\n/.claude/agents/reviewer.md\n{END}\n")
        );
        assert_eq!(
            with_lines(&once, &s(&["/.claude/agents/reviewer.md"]), &[]),
            once
        );
    }

    #[test]
    fn removing_the_last_line_drops_the_block_and_keeps_user_rules() {
        let text = format!("*.log\n{START}\n/a.md\n/b.md\n{END}\nbuild/\n");
        let one_left = with_lines(&text, &[], &s(&["/a.md"]));
        assert_eq!(one_left, format!("*.log\n{START}\n/b.md\n{END}\nbuild/\n"));
        assert_eq!(
            with_lines(&one_left, &[], &s(&["/b.md"])),
            "*.log\nbuild/\n"
        );
        assert_eq!(with_lines("", &[], &s(&["/b.md"])), "");
    }

    #[test]
    fn a_start_marker_without_an_end_runs_to_the_end_of_the_file() {
        let text = format!("*.log\n{START}\n/a.md\n");
        let added = with_lines(&text, &s(&["/b.md"]), &[]);
        assert_eq!(added, format!("*.log\n{START}\n/a.md\n/b.md\n{END}\n"));
        assert_eq!(added.matches(START).count(), 1);
        assert_eq!(with_lines(&text, &[], &s(&["/a.md"])), "*.log\n");
    }

    #[test]
    fn patterns_are_anchored_relative_paths_inside_the_root() {
        let root = Path::new("/repo");
        assert_eq!(
            pattern(root, Path::new("/repo/.opencode/agents/x.md")).as_deref(),
            Some("/.opencode/agents/x.md")
        );
        assert_eq!(pattern(root, Path::new("/elsewhere/x.md")), None);
    }

    #[test]
    fn writes_and_cleans_the_exclude_file_of_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("proj");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let file = repo.join(".claude/agents/reviewer.md");
        assert_eq!(add(&repo, std::slice::from_ref(&file)), None);
        let exclude = repo.join(".git/info/exclude");
        assert_eq!(
            fs::read_to_string(&exclude).unwrap(),
            format!("{START}\n/.claude/agents/reviewer.md\n{END}\n")
        );
        assert_eq!(remove(&repo, &[file]), None);
        assert_eq!(fs::read_to_string(&exclude).unwrap(), "");
    }

    #[test]
    fn a_worktree_uses_the_common_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&main)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .is_ok_and(|o| o.status.success())
        };
        fs::create_dir_all(&main).unwrap();
        let ready = git(&["init", "-q"])
            && git(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ])
            && git(&["worktree", "add", "-q", "../wt"]);
        if !ready {
            eprintln!("git unavailable; skipping");
            return;
        }
        let wt = dir.path().join("wt");
        assert!(wt.join(".git").is_file());
        let exclude = main.join(".git/info/exclude");
        // The same agent installed in both worktrees: one shared line.
        let files = [
            main.join(".claude/agents/x.md"),
            wt.join(".claude/agents/x.md"),
        ];
        for file in &files {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "x").unwrap();
        }
        assert_eq!(add(&wt, std::slice::from_ref(&files[1])), None);
        assert_eq!(add(&main, std::slice::from_ref(&files[0])), None);
        let text = fs::read_to_string(&exclude).unwrap();
        assert_eq!(
            text.matches("\n/.claude/agents/x.md\n").count(),
            1,
            "{text}"
        );

        // Removed from the worktree: the main checkout still has it, so it stays hidden.
        fs::remove_file(&files[1]).unwrap();
        assert_eq!(remove(&wt, std::slice::from_ref(&files[1])), None);
        let text = fs::read_to_string(&exclude).unwrap();
        assert!(text.contains("\n/.claude/agents/x.md\n"), "{text}");

        // Removed from the last one: the line goes.
        fs::remove_file(&files[0]).unwrap();
        assert_eq!(remove(&main, std::slice::from_ref(&files[0])), None);
        let text = fs::read_to_string(&exclude).unwrap();
        assert!(!text.contains("x.md"), "{text}");
    }

    #[test]
    fn outside_a_repository_it_only_notes_why() {
        let dir = tempfile::tempdir().unwrap();
        let note = add(dir.path(), &[dir.path().join("x.md")]).unwrap();
        assert!(note.contains("not a git repository"), "{note}");
        assert_eq!(remove(dir.path(), &[dir.path().join("x.md")]), None);
    }
}
