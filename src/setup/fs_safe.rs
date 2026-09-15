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
    check_path(path)?;
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
    Ok(value)
}

/// Returns whether `path` already holds a skill we installed (any managed version),
/// so a differing sibling MCP entry may be rewritten instead of treated as a conflict.
pub fn check_skill(path: &Path) -> Result<bool> {
    if let Some(text) = read_optional(path)? {
        ensure!(
            text.contains(SKILL_OWNER_PREFIX),
            "unowned Evolution skill conflict: {}",
            path.display()
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Inserts/overwrites `config[key].skillvolution`, refusing a foreign entry unless `owned`.
pub fn merge_server(config: &mut Value, key: &str, entry: Value, owned: bool) -> Result<()> {
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
        ensure!(
            old == &entry || owned,
            "unowned {key}.skillvolution conflict; review and remove the conflicting entry manually"
        );
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

pub fn write(path: &Path, content: &str) -> Result<()> {
    check_path(path)?;
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
    }
    fs::create_dir_all(path.parent().context("missing parent")?)?;
    fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

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
        check_path(&backup)?;
        if !backup.exists() {
            return Ok(backup);
        }
        ensure!(
            backup.is_file(),
            "backup is not a regular file: {}",
            backup.display()
        );
    }
    bail!("too many backups for {}", path.display())
}

pub fn check_path(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "path must not be empty");
    ensure!(
        !path
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "parent traversal (..) is unsupported: {}",
        path.display()
    );
    let absolute = std::path::absolute(path)?;
    for item in absolute.ancestors() {
        match fs::symlink_metadata(item) {
            Ok(meta) => {
                let linked = meta.file_type().is_symlink();
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    linked || meta.file_attributes() & 0x400 != 0
                };
                ensure!(
                    !linked,
                    "symlink or reparse point refused: {}",
                    item.display()
                );
                if item != absolute {
                    ensure!(meta.is_dir(), "not a directory: {}", item.display());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("inspect {}", item.display())),
        }
    }
    Ok(())
}
