//! Filesystem and JSON helpers shared by the Claude Code and OpenCode installers.
//! Every write goes through `write`, which backs up the previous content first.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
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
        Some(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} must be valid JSON", path.display()))?,
        None => json!({}),
    };
    ensure!(
        config.is_object(),
        "{} must contain a JSON object",
        path.display()
    );
    Ok(config)
}

/// Refuses an existing skill file that doesn't carry our managed marker, so we never
/// clobber a user's own file at that path.
pub fn check_skill(path: &Path) -> Result<()> {
    if let Some(text) = read_optional(path)? {
        ensure!(
            text.contains(SKILL_OWNER_PREFIX),
            "unowned Evolution skill conflict: {}",
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

/// Writes `content` to `path`, backing up any differing existing content first.
/// Skips the write entirely when the content is already up to date.
pub fn write(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        ensure!(path.is_file(), "not a regular file: {}", path.display());
        let old = fs::read(path)?;
        if old == content.as_bytes() {
            return Ok(());
        }
        backup(path)?;
    }
    fs::create_dir_all(path.parent().context("missing parent")?)?;
    fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

/// Copies `path` to `path.skillvolution.bak` unless that backup already exists,
/// so the file preserves whatever it held before Skillvolution ever touched it.
fn backup(path: &Path) -> Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(".skillvolution.bak");
    let backup = PathBuf::from(name);
    if !backup.exists() {
        fs::copy(path, &backup).with_context(|| format!("backup {}", path.display()))?;
    }
    Ok(())
}
