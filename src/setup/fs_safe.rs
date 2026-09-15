//! Filesystem and JSON helpers shared by the Claude Code and OpenCode installers.
//! Every write goes through `write`, which backs up the previous content first.

use anyhow::{Context, Result, bail, ensure};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

/// Any skill file carrying this prefix (any version) is ours to overwrite on upgrade.
const SKILL_OWNER_PREFIX: &str = "<!-- skillvolution-managed:evolution:";

pub fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

pub fn load_json(path: &Path) -> Result<Value> {
    let config: Value = match read_optional(path)? {
        Some(text) => parse_strict_object(&text).with_context(|| {
            format!(
                "{} must be a strict JSON object with unique keys (no comments, trailing commas, or duplicate keys)",
                path.display()
            )
        })?,
        None => json!({}),
    };
    ensure!(
        config.is_object(),
        "{} must contain a JSON object",
        path.display()
    );
    Ok(config)
}

/// Parses `text` as a JSON object, rejecting duplicate top-level keys instead of letting
/// the last one silently win.
pub fn parse_strict_object(text: &str) -> Result<Value> {
    struct StrictObjectVisitor;

    impl<'de> Visitor<'de> for StrictObjectVisitor {
        type Value = Value;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a JSON object with unique keys")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut entries: Vec<(String, Value)> = Vec::new();
            while let Some((key, value)) = access.next_entry::<String, Value>()? {
                if !seen.insert(key.clone()) {
                    return Err(<M::Error as serde::de::Error>::custom(format!(
                        "duplicate key {key:?}"
                    )));
                }
                entries.push((key, value));
            }
            let map: serde_json::Map<String, Value> = entries.into_iter().collect();
            Ok(Value::Object(map))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = deserializer.deserialize_map(StrictObjectVisitor)?;
    deserializer.end()?;
    Ok(value)
}

/// Refuses an existing skill file that doesn't carry our managed marker, so we never
/// clobber a user's own file at that path.
pub fn check_skill(path: &Path) -> Result<()> {
    check_owner(path, SKILL_OWNER_PREFIX, "Evolution skill")
}

/// Refuses an existing file at `path` that doesn't carry `marker`, so we never clobber
/// a user's own file there. `what` names the file kind in the error message.
pub fn check_owner(path: &Path, marker: &str, what: &str) -> Result<()> {
    if let Some(text) = read_optional(path)? {
        ensure!(
            text.contains(marker),
            "unowned {what} conflict: {}",
            path.display()
        );
    }
    Ok(())
}

/// Inserts/overwrites `config[key].skillvolution`. Any prior entry survives in the
/// `.skillvolution.bak` backup made before the write.
pub fn merge_server(config: &mut Value, key: &str, entry: Value) -> Result<()> {
    let servers = config
        .as_object_mut()
        .context("config must be an object")?
        .entry(key)
        .or_insert(json!({}));
    let servers = servers
        .as_object_mut()
        .with_context(|| format!("{key} must be an object"))?;
    if let Some(old) = servers.get("skillvolution") {
        ensure!(old.is_object(), "{key}.skillvolution must be an object");
    }
    servers.insert("skillvolution".to_owned(), entry);
    Ok(())
}

/// Finds the single well-formed `start..end` marker block, if any.
/// Missing markers return `Ok(None)`; malformed or duplicate markers are refused.
fn find_marker_block(text: &str, start: &str, end: &str) -> Result<Option<(usize, usize)>> {
    let starts: Vec<_> = text.match_indices(start).map(|(i, _)| i).collect();
    let ends: Vec<_> = text.match_indices(end).map(|(i, _)| i).collect();
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([s], [e]) if *s < *e => Ok(Some((*s, e + end.len()))),
        _ => bail!("malformed or duplicate Skillvolution markers; repair them manually"),
    }
}

/// Rewrites the marker block in place, or appends it if absent.
pub fn merge_marker_block(mut text: String, start: &str, end: &str, block: &str) -> Result<String> {
    match find_marker_block(&text, start, end)? {
        Some((s, e)) => text.replace_range(s..e, block),
        None => {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(block);
            text.push('\n');
        }
    }
    Ok(text)
}

/// Removes a legacy marker block if present. `Ok(None)` means the file is untouched.
pub fn remove_marker_block(mut text: String, start: &str, end: &str) -> Result<Option<String>> {
    match find_marker_block(&text, start, end)? {
        Some((s, e)) => {
            text.replace_range(s..e, "");
            Ok(Some(text))
        }
        None => Ok(None),
    }
}

/// Refuses `path` if it exists as a symlink or as anything other than a regular file.
/// A path that doesn't exist at all is fine to write; a dangling symlink is still caught
/// here because `symlink_metadata` reports the link itself, not its (missing) target.
/// Ancestor directories are never inspected, so a symlinked ancestor (a stowed `~/.local/bin`,
/// a symlinked project directory, ...) is left alone.
pub fn check_target(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                !meta.file_type().is_symlink(),
                "symlink refused: {}",
                path.display()
            );
            ensure!(meta.is_file(), "not a regular file: {}", path.display());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("inspect {}", path.display())),
    }
}

/// Writes `content` to `path` atomically: it lands fully or not at all, even if the
/// process is killed mid-write, because it's assembled in a temp file next to `path`
/// and only then renamed over it (a rename is a single filesystem operation, unlike
/// `fs::write`'s truncate-then-write).
pub fn write(path: &Path, content: &str) -> Result<()> {
    check_target(path)?;
    let mut permissions = None;
    if path.exists() {
        let old = fs::read(path)?;
        if old == content.as_bytes() {
            return Ok(());
        }
        let backup = backup_path(path)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)?;
        file.set_permissions(fs::metadata(path)?.permissions())?;
        file.write_all(&old)?;
        file.sync_all()?;
        permissions = Some(fs::metadata(path)?.permissions());
    }
    fs::create_dir_all(path.parent().context("missing parent")?)?;

    let tmp_path = tmp_path_for(path);
    let mut tmp_file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_path)
        .with_context(|| format!("create {}", tmp_path.display()))?;
    if let Some(permissions) = permissions {
        tmp_file.set_permissions(permissions)?;
    }
    tmp_file
        .write_all(content.as_bytes())
        .with_context(|| format!("write {}", tmp_path.display()))?;
    tmp_file.sync_all()?;
    drop(tmp_file);

    fs::rename(&tmp_path, path).with_context(|| format!("write {}", path.display()))
}

/// The scratch file `write` assembles new content in before renaming it over `path`.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(path.file_name().unwrap_or_default());
    tmp_name.push(".skillvolution.tmp");
    path.with_file_name(tmp_name)
}

/// Picks the first available numbered backup path (`.skillvolution.bak`, then `.bak.1`, ...),
/// refusing a symlinked candidate instead of silently skipping past it.
pub fn backup_path(path: &Path) -> Result<PathBuf> {
    for index in 0..10_000 {
        let suffix = if index == 0 {
            ".skillvolution.bak".to_string()
        } else {
            format!(".skillvolution.bak.{index}")
        };
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let backup = PathBuf::from(name);
        check_target(&backup)?;
        if !backup.exists() {
            return Ok(backup);
        }
    }
    bail!("too many backups for {}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strict_object_refuses_trailing_content_after_the_object() {
        let err = parse_strict_object(r#"{"a":1}{"trailing":true}"#).unwrap_err();
        // The malformed input must be refused outright, not silently truncated to the
        // first object with the rest dropped.
        assert!(!err.to_string().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_replaces_existing_file_atomically_and_preserves_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "old content").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let original_inode = fs::metadata(&path).unwrap().ino();

        write(&path, "new content").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // A rename over the target leaves a new inode; an in-place truncate-and-write
        // (the non-atomic form) would keep the original one.
        assert_ne!(fs::metadata(&path).unwrap().ino(), original_inode);

        let leftover_tmp = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp"));
        assert!(!leftover_tmp, "temp file left behind");
    }
}
