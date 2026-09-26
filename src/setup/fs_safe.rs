//! Filesystem and JSON helpers shared by all client setups.
//! Every write goes through `write`, which backs up the previous content first (unless the
//! file is entirely ours) and replaces the file atomically.

use anyhow::{Context, Result, bail, ensure};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

/// Any skill file carrying this prefix (any version) is ours to overwrite on upgrade.
const SKILL_OWNER_PREFIX: &str = "<!-- skillvolution-managed:evolution:";

/// Shared prefix of every managed-file marker (the Evolution skill's, the OpenCode
/// plugin's). A file whose old and new content both carry it is entirely ours, so
/// replacing it needs no backup.
const MANAGED_MARKER: &str = "skillvolution-managed:";

/// Backups kept per file: `.skillvolution.bak` (newest), `.bak.1`, `.bak.2` (oldest).
const KEPT_BACKUPS: usize = 3;

pub fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Loads a JSON config object. A missing, empty, or whitespace-only file is `{}`, and a
/// leading UTF-8 BOM (which some Windows editors add) is ignored.
pub fn load_json(path: &Path) -> Result<Value> {
    let text = read_optional(path)?.unwrap_or_default();
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let config = parse_strict_object(text).with_context(|| {
        format!(
            "{} must be a strict JSON object with unique keys (no comments, trailing commas, or duplicate keys)",
            path.display()
        )
    })?;
    ensure!(
        config.is_object(),
        "{} must contain a JSON object",
        path.display()
    );
    Ok(config)
}

/// Parses `text` as JSON, rejecting a duplicate key in any object at any depth instead
/// of letting the last one silently win.
pub fn parse_strict_object(text: &str) -> Result<Value> {
    serde_json::from_str::<UniqueKeys>(text)?;
    Ok(serde_json::from_str(text)?)
}

/// Deserializes to nothing: it only walks a JSON document and fails on the first object
/// that repeats a key.
struct UniqueKeys;

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueKeysVisitor)
    }
}

struct UniqueKeysVisitor;

impl<'de> Visitor<'de> for UniqueKeysVisitor {
    type Value = UniqueKeys;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("JSON with unique object keys")
    }

    fn visit_bool<E>(self, _: bool) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_i64<E>(self, _: i64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_u64<E>(self, _: u64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_f64<E>(self, _: f64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_str<E>(self, _: &str) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_unit<E>(self) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueKeys, A::Error> {
        while seq.next_element::<UniqueKeys>()?.is_some() {}
        Ok(UniqueKeys)
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<UniqueKeys, M::Error> {
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
            }
            map.next_value::<UniqueKeys>()?;
        }
        Ok(UniqueKeys)
    }
}

/// Refuses an existing skill file that doesn't carry our managed marker, so we never
/// clobber a user's own file at that path.
pub fn check_skill(path: &Path) -> Result<()> {
    check_owner(path, SKILL_OWNER_PREFIX, "Evolution skill")
}

/// Whether the managed skill file at `path` is ours to delete (any version). A missing
/// file is `false`, same as one without our marker.
pub fn is_our_skill(path: &Path) -> Result<bool> {
    is_marked(path, SKILL_OWNER_PREFIX)
}

/// Whether `path` exists and its content carries `marker`. A missing file is `false`.
pub fn is_marked(path: &Path, marker: &str) -> Result<bool> {
    Ok(read_optional(path)?.is_some_and(|text| text.contains(marker)))
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

/// Inserts `config[key].skillvolution`, or, when it already exists, overwrites only the
/// keys `entry` sets (how to launch the server) and keeps everything else the user added
/// there (`env`, `timeout`, `environment`, ...). An existing `enabled` is never touched,
/// so a server the user disabled stays disabled across reruns.
pub fn merge_server(config: &mut Value, key: &str, entry: Value) -> Result<()> {
    let servers = config
        .as_object_mut()
        .context("config must be an object")?
        .entry(key)
        .or_insert(json!({}));
    let servers = servers
        .as_object_mut()
        .with_context(|| format!("{key} must be an object"))?;
    let Some(old) = servers.get_mut("skillvolution") else {
        servers.insert("skillvolution".to_owned(), entry);
        return Ok(());
    };
    let old = old
        .as_object_mut()
        .with_context(|| format!("{key}.skillvolution must be an object"))?;
    let Value::Object(entry) = entry else {
        bail!("server entry must be an object");
    };
    for (name, value) in entry {
        if name != "enabled" {
            old.insert(name, value);
        }
    }
    Ok(())
}

/// Removes `config[container][name]`, dropping `container` entirely if that empties it
/// (the removal counterpart of `merge_server`, which may have created it). Returns
/// whether anything was removed.
pub fn remove_server(config: &mut Value, container: &str, name: &str) -> Result<bool> {
    let Some(root) = config.as_object_mut() else {
        return Ok(false);
    };
    let Some(value) = root.get_mut(container) else {
        return Ok(false);
    };
    let entries = value
        .as_object_mut()
        .with_context(|| format!("{container} must be an object"))?;
    if entries.remove(name).is_none() {
        return Ok(false);
    }
    if entries.is_empty() {
        root.remove(container);
    }
    Ok(true)
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

/// Removes a marker block if present, along with the one newline right after it that
/// `merge_marker_block` adds when it appends a block (so removing a freshly-appended
/// block is its exact inverse, not a block plus a stray blank line). `Ok(None)` means the
/// file is untouched.
pub fn remove_marker_block(mut text: String, start: &str, end: &str) -> Result<Option<String>> {
    match find_marker_block(&text, start, end)? {
        Some((s, e)) => {
            let e = if text[e..].starts_with('\n') {
                e + 1
            } else {
                e
            };
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

/// Writes `content` to `path` atomically (see `replace`), backing up the previous
/// content first unless the file is entirely ours (see `MANAGED_MARKER`). An unchanged
/// file is left alone.
pub fn write(path: &Path, content: &str) -> Result<()> {
    check_target(path)?;
    if let Some(old) = read_optional_bytes(path)? {
        if old == content.as_bytes() {
            return Ok(());
        }
        let managed = contains(&old, MANAGED_MARKER) && content.contains(MANAGED_MARKER);
        if !managed {
            backup(path, &old)?;
        }
    }
    replace(path, content.as_bytes())
}

/// Puts `content` back at `path` without a backup: how a failed setup undoes its
/// earlier writes.
pub fn restore(path: &Path, content: &[u8]) -> Result<()> {
    check_target(path)?;
    replace(path, content)
}

pub fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Replaces `path` with `content` so it lands fully or not at all, even if the process
/// is killed mid-write: the content is assembled in a temp file next to `path` and only
/// then renamed over it. The temp file has a random name and is created exclusively, so
/// a symlink planted in the directory (e.g. committed to a cloned repo) can't redirect
/// the write. An existing file keeps its permissions; a new one gets what `fs::write`
/// would give it.
fn replace(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let permissions = match fs::metadata(path) {
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("inspect {}", path.display())),
    };

    let mut builder = tempfile::Builder::new();
    builder.prefix(".skillvolution-").suffix(".tmp");
    #[cfg(unix)]
    if permissions.is_none() {
        use std::os::unix::fs::PermissionsExt;
        // 0o666 minus the umask: the mode `fs::write` creates new files with.
        builder.permissions(fs::Permissions::from_mode(0o666));
    }
    let mut tmp = builder
        .tempfile_in(parent)
        .with_context(|| format!("create a temp file in {}", parent.display()))?;
    tmp.write_all(content)
        .with_context(|| format!("write {}", tmp.path().display()))?;
    if let Some(permissions) = permissions {
        tmp.as_file().set_permissions(permissions)?;
    }
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("write {}", path.display()))?;
    sync_dir(parent);
    Ok(())
}

/// Flushes a rename in `dir` to disk. Best-effort: some filesystems refuse to fsync a
/// directory, and the rename itself has already happened.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The backup slots for `path`, newest first: `.skillvolution.bak`, `.bak.1`, `.bak.2`.
fn backup_paths(path: &Path) -> Vec<PathBuf> {
    (0..KEPT_BACKUPS)
        .map(|index| {
            let mut name = path.as_os_str().to_owned();
            name.push(".skillvolution.bak");
            if index > 0 {
                name.push(format!(".{index}"));
            }
            PathBuf::from(name)
        })
        .collect()
}

/// Refuses any backup slot for `path` that is a symlink or not a regular file, so
/// `backup` never rotates through or writes into one.
pub fn check_backups(path: &Path) -> Result<()> {
    backup_paths(path)
        .iter()
        .try_for_each(|slot| check_target(slot))
}

/// Saves `old` (the current content of `path`) as the newest backup, shifting the older
/// ones down a slot and dropping the oldest. The backup is created with `path`'s mode,
/// so a private file's copy is never readable by others, even briefly.
fn backup(path: &Path, old: &[u8]) -> Result<()> {
    check_backups(path)?;
    let slots = backup_paths(path);
    for index in (1..slots.len()).rev() {
        if slots[index - 1].exists() {
            fs::rename(&slots[index - 1], &slots[index])
                .with_context(|| format!("rotate {}", slots[index - 1].display()))?;
        }
    }

    let newest = &slots[0];
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(fs::metadata(path)?.permissions().mode());
    }
    let mut file = options
        .open(newest)
        .with_context(|| format!("create {}", newest.display()))?;
    file.write_all(old)?;
    file.sync_all()?;
    Ok(())
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

    #[cfg(unix)]
    #[test]
    fn write_ignores_a_symlink_planted_at_the_old_predictable_temp_path() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("bashrc");
        fs::write(&victim, "precious").unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "old").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join(".config.json.skillvolution.tmp"))
            .unwrap();

        write(&path, "new").unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn new_files_get_the_mode_fs_write_would_give_them() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference");
        fs::write(&reference, "x").unwrap();
        let path = dir.path().join("new.json");

        write(&path, "x").unwrap();

        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), mode(&reference));
    }

    #[cfg(unix)]
    #[test]
    fn backup_keeps_the_source_files_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        write(&path, "new").unwrap();

        let backup = dir.path().join("settings.json.skillvolution.bak");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old");
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn managed_files_are_replaced_without_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        write(&path, "<!-- skillvolution-managed:evolution:1 -->\nv1").unwrap();
        write(&path, "<!-- skillvolution-managed:evolution:2 -->\nv2").unwrap();

        assert!(!dir.path().join("SKILL.md.skillvolution.bak").exists());
    }

    #[test]
    fn only_the_three_most_recent_backups_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for version in 1..=5 {
            write(&path, &format!("v{version}")).unwrap();
        }

        let backup = |suffix: &str| {
            dir.path()
                .join(format!("config.json.skillvolution.{suffix}"))
        };
        assert_eq!(fs::read_to_string(backup("bak")).unwrap(), "v4");
        assert_eq!(fs::read_to_string(backup("bak.1")).unwrap(), "v3");
        assert_eq!(fs::read_to_string(backup("bak.2")).unwrap(), "v2");
        assert!(!backup("bak.3").exists());
    }

    #[test]
    fn merge_server_updates_only_the_launch_keys_of_an_existing_entry() {
        let mut config = json!({"mcp": {"skillvolution": {
            "type": "local",
            "command": ["/old/skillvolution", "serve"],
            "enabled": false,
            "environment": {"X": "1"},
        }}});
        let entry = json!({
            "type": "local",
            "command": ["/new/skillvolution", "serve"],
            "enabled": true,
        });

        merge_server(&mut config, "mcp", entry).unwrap();

        assert_eq!(
            config["mcp"]["skillvolution"],
            json!({
                "type": "local",
                "command": ["/new/skillvolution", "serve"],
                "enabled": false,
                "environment": {"X": "1"},
            })
        );
    }

    #[test]
    fn remove_server_drops_the_entry_and_an_emptied_container() {
        let mut config = json!({"mcp": {"skillvolution": {"type": "local"}, "other": {}}});
        assert!(remove_server(&mut config, "mcp", "skillvolution").unwrap());
        assert_eq!(config, json!({"mcp": {"other": {}}}));

        let mut config = json!({"mcp": {"skillvolution": {"type": "local"}}});
        assert!(remove_server(&mut config, "mcp", "skillvolution").unwrap());
        assert_eq!(config, json!({}));
    }

    #[test]
    fn remove_server_is_a_no_op_when_absent() {
        let mut config = json!({"mcp": {"other": {}}});
        assert!(!remove_server(&mut config, "mcp", "skillvolution").unwrap());
        assert_eq!(config, json!({"mcp": {"other": {}}}));

        let mut config = json!({});
        assert!(!remove_server(&mut config, "mcp", "skillvolution").unwrap());
        assert_eq!(config, json!({}));
    }

    #[test]
    fn merge_server_inserts_the_whole_entry_when_absent() {
        let mut config = json!({});
        let entry = json!({"type": "local", "command": ["x"], "enabled": true});
        merge_server(&mut config, "mcp", entry.clone()).unwrap();
        assert_eq!(config["mcp"]["skillvolution"], entry);
    }

    #[test]
    fn duplicate_keys_are_refused_at_any_depth() {
        assert!(parse_strict_object(r#"{"a":1,"a":2}"#).is_err());
        assert!(parse_strict_object(r#"{"mcp":{"x":{"k":1,"k":2}}}"#).is_err());
        assert!(parse_strict_object(r#"{"list":[{"k":1,"k":2}]}"#).is_err());
        let value =
            parse_strict_object(r#"{"a":{"b":[1,{"c":null}]},"big":12345678901234567890123}"#)
                .unwrap();
        assert_eq!(value["a"]["b"][1], json!({"c": null}));
        assert_eq!(value["big"].to_string(), "12345678901234567890123");
    }

    #[test]
    fn is_our_skill_recognizes_any_marker_version_and_rejects_a_users_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        assert!(!is_our_skill(&path).unwrap(), "missing file");

        fs::write(&path, "# my own notes").unwrap();
        assert!(!is_our_skill(&path).unwrap());

        fs::write(&path, "<!-- skillvolution-managed:evolution:3 -->\nv3").unwrap();
        assert!(is_our_skill(&path).unwrap());
    }

    #[test]
    fn empty_or_bom_prefixed_config_files_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for text in ["", "  \n\t", "\u{feff}", "\u{feff}\n"] {
            fs::write(&path, text).unwrap();
            assert_eq!(load_json(&path).unwrap(), json!({}), "{text:?}");
        }
        fs::write(&path, "\u{feff}{\"a\":1}").unwrap();
        assert_eq!(load_json(&path).unwrap(), json!({"a": 1}));
    }
}
