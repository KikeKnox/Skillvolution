use std::path::Path;

/// Project key for `dir`: the sanitized name of the enclosing git repository root,
/// or `None` outside a git repository.
pub fn detect(dir: &Path) -> Option<String> {
    let root = dir
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())?;
    key_from_dir(root)
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
}
