use std::path::{Path, PathBuf};

/// Project key for `dir`: the sanitized name of the enclosing git repository root,
/// or `None` outside a git repository.
pub fn detect(dir: &Path) -> Option<String> {
    let ancestor = dir
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())?;
    key_from_dir(&git_root(ancestor))
}

/// Resolves `dir` (which contains a `.git` entry) to the actual project root.
///
/// A plain `.git` directory means `dir` is the root. A `.git` *file* (used by
/// worktrees and submodules) holds a `gitdir: <path>` pointer instead: for a
/// worktree that pointed-to directory has a `commondir` file naming the main
/// repository's git dir, whose parent is the real root; for a submodule (or
/// anything we can't parse) `dir` itself is the root.
fn git_root(dir: &Path) -> PathBuf {
    let git_path = dir.join(".git");
    if git_path.is_dir() {
        return dir.to_path_buf();
    }
    resolve_git_file(dir, &git_path).unwrap_or_else(|| dir.to_path_buf())
}

fn resolve_git_file(dir: &Path, git_path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(git_path).ok()?;
    let target = contents
        .lines()
        .next()?
        .trim()
        .strip_prefix("gitdir:")?
        .trim();
    let gitdir = resolve_relative(dir, target);

    let Ok(commondir) = std::fs::read_to_string(gitdir.join("commondir")) else {
        // No commondir: a submodule's gitdir (or anything unrecognized), so the
        // directory holding the `.git` file is its own project root.
        return Some(dir.to_path_buf());
    };
    let common_git_dir = resolve_relative(&gitdir, commondir.trim());
    (common_git_dir.file_name()?.to_str()? == ".git")
        .then(|| common_git_dir.parent().map(Path::to_path_buf))
        .flatten()
}

/// Joins `base` and `target` the way git does: absolute `target` replaces `base`
/// outright, otherwise `target`'s components (including `..`) are applied on top
/// of `base` without touching the filesystem.
fn resolve_relative(base: &Path, target: &str) -> PathBuf {
    let target = Path::new(target);
    let mut result = if target.is_absolute() {
        PathBuf::new()
    } else {
        base.to_path_buf()
    };
    for component in target.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Sanitized directory name: lowercase ASCII letters and digits joined by single hyphens, at most 64 bytes.
pub fn key_from_dir(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_str()?;
    let mut key = String::new();
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            key.push(lower);
        } else if !key.ends_with('-') {
            key.push('-');
        }
    }
    let mut key = key.trim_matches('-').to_owned();
    key.truncate(64);
    let key = key.trim_matches('-');
    (!key.is_empty()).then(|| key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{detect, key_from_dir};
    use std::path::Path;

    #[test]
    fn derives_keys_from_directory_names() {
        assert_eq!(
            key_from_dir(Path::new("/tmp/project with spaces")).as_deref(),
            Some("project-with-spaces")
        );
        assert_eq!(
            key_from_dir(Path::new("/tmp/__café__")).as_deref(),
            Some("caf")
        );
        assert_eq!(key_from_dir(Path::new("/tmp/---")), None);
        assert_eq!(
            key_from_dir(Path::new(&"a".repeat(80))).map(|k| k.len()),
            Some(64)
        );
    }

    #[test]
    fn detects_the_git_root_name_from_nested_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("My Repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        assert_eq!(detect(&root.join("src/deep")).as_deref(), Some("my-repo"));
        assert_eq!(detect(temp.path()), None);
    }

    #[test]
    fn worktree_resolves_to_the_main_repo_name() {
        let temp = tempfile::tempdir().unwrap();
        let main_repo = temp.path().join("main-repo");
        let worktree_gitdir = main_repo.join(".git/worktrees/wt");
        std::fs::create_dir_all(&worktree_gitdir).unwrap();
        std::fs::write(worktree_gitdir.join("commondir"), "../..\n").unwrap();

        let worktree = temp.path().join("feat-x");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_gitdir.display()),
        )
        .unwrap();

        assert_eq!(detect(&worktree).as_deref(), Some("main-repo"));
        assert_eq!(detect(&worktree.join("src")).as_deref(), Some("main-repo"));
    }

    #[test]
    fn submodule_resolves_to_its_own_folder_name() {
        let temp = tempfile::tempdir().unwrap();
        let superproject = temp.path().join("super");
        std::fs::create_dir_all(superproject.join(".git/modules/sub")).unwrap();

        let submodule = superproject.join("sub-module");
        std::fs::create_dir_all(&submodule).unwrap();
        std::fs::write(submodule.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();

        assert_eq!(detect(&submodule).as_deref(), Some("sub-module"));
    }

    #[test]
    fn malformed_git_file_falls_back_to_its_own_directory() {
        let temp = tempfile::tempdir().unwrap();
        let broken = temp.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(".git"), "not a gitdir line\n").unwrap();

        assert_eq!(detect(&broken).as_deref(), Some("broken"));
    }
}
