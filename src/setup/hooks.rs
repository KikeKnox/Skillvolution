//! Hook command building and merging shared by all client setups, project and global.
//! Our hook entries are recognized by their command text, so a rerun (even with a changed
//! `--bin`/`--db`) replaces the old entry instead of duplicating it, and any foreign hook
//! is left untouched.

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

// POSIX single-quoting, used for every client's hook command. On Windows, Claude Code
// runs hook commands through Git Bash, so this quoting is still correct there; Devin's
// Windows behavior is unverified (see TRACK J report), so it keeps the same quoting.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Returns `path` as `&str`, failing clearly instead of silently dropping non-UTF-8
/// bytes the way `Path::display` would: a hook command embeds the path as shell text,
/// so a byte it can't represent must stop setup before anything is written, not corrupt
/// the command silently.
pub fn require_utf8<'a>(path: &'a Path, what: &str) -> Result<&'a str> {
    path.to_str()
        .with_context(|| format!("{what} is not valid UTF-8: {}", path.display()))
}

/// A hook command line, `<bin> --db <db> hook <event><extra>`, with `bin` and `db`
/// shell-quoted. `extra` is appended as-is, so it must already be shell-safe (see
/// `project_flag`).
pub fn hook_cmd(bin: &Path, db: &Path, event: &str, extra: &str) -> Result<String> {
    let bin = shell_quote(require_utf8(bin, "--bin")?);
    let db = shell_quote(require_utf8(db, "--db")?);
    Ok(format!("{bin} --db {db} hook {event}{extra}"))
}

/// ` --project '<key>'` for a project's hooks; empty for global ones.
pub fn project_flag(key: Option<&str>) -> String {
    key.map(|key| format!(" --project {}", shell_quote(key)))
        .unwrap_or_default()
}

/// Splits `command` into shell words: single-quoted segments (including the
/// `'\''`-style embedded quote our own `shell_quote` produces) and plain
/// whitespace-separated runs. Not a general shell parser, just enough to read back the
/// commands this module builds and to tell them apart from a foreign one.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = command.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            match c {
                '\'' => {
                    for c in chars.by_ref() {
                        if c == '\'' {
                            break;
                        }
                        word.push(c);
                    }
                }
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        word.push(escaped);
                    }
                }
                other => word.push(other),
            }
        }
        words.push(word);
    }
    words
}

/// Hook subcommand names that mark a command as ours, in their CLI spelling.
const HOOK_EVENTS: [&str; 6] = [
    "session-start",
    "prompt",
    "tool-use",
    "stop",
    "session-end",
    "approve",
];

/// A Skillvolution-owned hook command: one whose first shell word is a path named
/// `skillvolution` (any extension, any case, so `skillvolution.exe` on Windows counts)
/// and whose remaining words contain `hook <event>` as whole words
/// (`hook session-start`, `hook stop`, `hook tool-use --client devin`, ...).
/// Independent of the bin path, db path, or project key, so a changed one replaces
/// the old entry instead of duplicating it; independent of the command text
/// otherwise, so a foreign command that merely contains the substring " hook stop"
/// (e.g. another tool's own `hook stop`, or a `hook stopwatch`) is left alone.
pub(super) fn owned_command(command: &str) -> bool {
    let words = shell_words(command);
    let Some(first) = words.first() else {
        return false;
    };
    let stem = Path::new(first).file_stem().and_then(|stem| stem.to_str());
    if !stem.is_some_and(|stem| stem.eq_ignore_ascii_case("skillvolution")) {
        return false;
    }
    words
        .windows(2)
        .any(|pair| pair[0] == "hook" && HOOK_EVENTS.contains(&pair[1].as_str()))
}

/// The Skillvolution binary path read back from the first owned hook command
/// found in `config["hooks"]`, if any. `doctor` uses this to check that the
/// binary a client's hooks point at still exists.
pub(crate) fn owned_bin(config: &Value) -> Option<PathBuf> {
    let hooks = config.get("hooks")?.as_object()?;
    let command = hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|entry| entry.get("command").and_then(Value::as_str))
        .find(|command| owned_command(command))?;
    shell_words(command).into_iter().next().map(PathBuf::from)
}

/// Whether `config["hooks"]` holds a Skillvolution-owned command under any event, in
/// either the nested `{event: [{hooks: [{command}]}]}` shape or Cursor's flat
/// `{event: [{command}]}` one.
pub fn has_owned(config: &Value) -> bool {
    let Some(hooks) = config.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .flat_map(|group| {
            let nested = group.get("hooks").and_then(Value::as_array);
            std::iter::once(group).chain(nested.into_iter().flatten())
        })
        .filter_map(|entry| entry.get("command").and_then(Value::as_str))
        .any(owned_command)
}

/// Removes our entries from `hooks[event]`, and any group that removing them leaves
/// empty (a foreign group that was already empty is kept as is). Returns `None` when
/// nothing was removed, else the slot where our first group was, so a rerun can write
/// its replacement back in place instead of reordering the event (which made setup and
/// other tools appending hooks fight over the order).
fn strip_owned(hooks: &mut Map<String, Value>, event: &str) -> Result<Option<usize>> {
    let Some(value) = hooks.get_mut(event) else {
        return Ok(None);
    };
    let groups = value
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    let mut first_owned = None;
    // Per group: whether to keep it, i.e. it didn't become empty by losing ours.
    let mut keep = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter_mut().enumerate() {
        let group = group
            .as_object_mut()
            .with_context(|| format!("hooks.{event} entries must be objects"))?;
        let mut emptied = false;
        if let Some(entries) = group.get_mut("hooks") {
            let entries = entries
                .as_array_mut()
                .with_context(|| format!("hooks.{event}[].hooks must be an array"))?;
            let count = entries.len();
            entries.retain(|entry| {
                !entry
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(owned_command)
            });
            if entries.len() != count {
                first_owned.get_or_insert(index);
                emptied = entries.is_empty();
            }
        }
        keep.push(!emptied);
    }
    let mut keep_iter = keep.iter();
    groups.retain(|_| *keep_iter.next().unwrap_or(&true));
    // A group shared with foreign entries survives, so ours goes right after it.
    Ok(first_owned.map(|index| keep[..=index].iter().filter(|kept| **kept).count()))
}

fn append_group(
    hooks: &mut Map<String, Value>,
    event: &str,
    matcher: Option<&str>,
    command: String,
    slot: Option<&mut usize>,
) -> Result<()> {
    let groups = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    let mut group = json!({"hooks": [{"type": "command", "command": command, "timeout": 10}]});
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    match slot {
        Some(slot) => {
            groups.insert((*slot).min(groups.len()), group);
            *slot += 1;
        }
        None => groups.push(group),
    }
    Ok(())
}

/// Replaces our entries in `config["hooks"]` with `entries` (event, optional
/// `matcher` regex, command), preserving every foreign hook. Our entries are stripped
/// from every event, not just the ones merged now, so a hook an older version wrote
/// under an event it no longer uses doesn't linger; an event that stripping leaves
/// empty is removed.
pub fn merge(config: &mut Value, entries: &[(&str, Option<String>, String)]) -> Result<()> {
    let hooks = config
        .as_object_mut()
        .context("config must be an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be an object")?;
    merge_entries(hooks, entries)
}

fn merge_entries(
    hooks: &mut Map<String, Value>,
    entries: &[(&str, Option<String>, String)],
) -> Result<()> {
    let (mut slots, emptied) = strip_owned_from_all(hooks)?;
    for (event, matcher, command) in entries {
        append_group(
            hooks,
            event,
            matcher.as_deref(),
            command.clone(),
            slots.get_mut(*event),
        )?;
    }
    // Checked after appending, so an event we re-add keeps its place in the file.
    hooks.retain(|event, groups| {
        !(emptied.contains(event) && groups.as_array().is_some_and(Vec::is_empty))
    });
    Ok(())
}

/// Strips our commands from every event in `hooks`. Returns, per event we were removed
/// from, the slot our first group occupied (see `strip_owned`), and which events that
/// stripping alone left with an empty group array (before any new entries are
/// appended back).
fn strip_owned_from_all(
    hooks: &mut Map<String, Value>,
) -> Result<(HashMap<String, usize>, Vec<String>)> {
    let events: Vec<String> = hooks.keys().cloned().collect();
    let mut slots = HashMap::new();
    let mut emptied = Vec::new();
    for event in events {
        if let Some(slot) = strip_owned(hooks, &event)? {
            if hooks[&event].as_array().is_some_and(Vec::is_empty) {
                emptied.push(event.clone());
            }
            slots.insert(event, slot);
        }
    }
    Ok((slots, emptied))
}

/// The removal counterpart of `merge`: strips our commands from every event of
/// `config["hooks"]` without adding anything back, dropping any event that leaves empty
/// and the whole `hooks` object if that in turn empties it. A config with no `hooks` key
/// at all, or none of ours, is left untouched. Returns whether anything was removed.
pub fn remove_owned(config: &mut Value) -> Result<bool> {
    let Some(root) = config.as_object_mut() else {
        return Ok(false);
    };
    let Some(hooks_value) = root.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks_value
        .as_object_mut()
        .context("hooks must be an object")?;
    let (slots, emptied) = strip_owned_from_all(hooks)?;
    let removed = !slots.is_empty();
    hooks.retain(|event, _| !emptied.contains(event));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_commands_that_merely_contain_our_substrings_are_not_owned() {
        assert!(!owned_command("/usr/bin/other-tool hook stop"));
        assert!(!owned_command("x hook stopwatch"));
        assert!(!owned_command("/usr/bin/other-tool hook session-start"));
    }

    #[test]
    fn our_commands_in_quoted_and_unquoted_form_are_owned() {
        assert!(owned_command(&format!(
            "{} --db {} hook stop",
            shell_quote("/opt/skillvolution"),
            shell_quote("/home/user/vault.sqlite3")
        )));
        assert!(owned_command(
            "/opt/skillvolution --db /home/user/vault.sqlite3 hook session-start --project key"
        ));
        // A quoted path containing a literal single quote, via our own escaping idiom.
        assert!(owned_command(&format!(
            "{} hook stop",
            shell_quote("/opt/it's-a-path/skillvolution")
        )));
        // Devin-format commands carry a --client flag after the event name.
        for event in ["tool-use", "session-end", "approve"] {
            assert!(owned_command(&format!("/opt/skillvolution hook {event}")));
        }
        assert!(owned_command("/opt/skillvolution hook stop --client devin"));
        assert!(owned_command(
            "/opt/skillvolution hook session-start --client devin"
        ));
        // Gemini CLI's commands carry the same --client flag, so a rerun replaces
        // rather than duplicates its hook entries too.
        for event in ["session-start", "tool-use", "stop", "session-end"] {
            assert!(owned_command(&format!(
                "/opt/skillvolution hook {event} --client gemini"
            )));
        }
        // The prompt-submit hook, with and without a client or project key.
        assert!(owned_command("/opt/skillvolution hook prompt"));
        assert!(owned_command(
            "/opt/skillvolution hook prompt --client gemini --project key"
        ));
        assert!(!owned_command("/opt/skillvolution hook prompts"));
    }

    #[test]
    fn a_skillvolution_named_binary_with_unrelated_args_is_not_owned() {
        assert!(!owned_command("/opt/skillvolution hook stopwatch"));
        assert!(!owned_command("/opt/skillvolution serve"));
    }

    #[test]
    fn windows_style_binary_names_are_owned_case_insensitively() {
        assert!(owned_command("/opt/skillvolution.exe hook stop"));
        assert!(owned_command("/opt/Skillvolution.EXE hook session-start"));
        assert!(!owned_command("/opt/skillvolution-helper.exe hook stop"));
    }

    #[test]
    fn merge_strips_owned_hooks_under_every_event_and_keeps_foreign_ones() {
        let ours = |event: &str| json!({"type": "command", "command": format!("/old/skillvolution hook {event}")});
        let foreign = json!({"type": "command", "command": "echo foreign"});
        let mut config = json!({"hooks": {
            // A stale event from an older version that this merge doesn't write.
            "SessionEnd": [{"hooks": [ours("session-end")]}],
            // Our hook sharing a group with a foreign one.
            "PreToolUse": [{"matcher": "Bash", "hooks": [ours("tool-use"), foreign.clone()]}],
            "Stop": [{"hooks": [ours("stop")]}],
        }});

        merge(
            &mut config,
            &[("Stop", None, "/new/skillvolution hook stop".to_owned())],
        )
        .unwrap();

        let hooks = &config["hooks"];
        assert!(hooks.get("SessionEnd").is_none(), "{hooks}");
        assert_eq!(
            hooks["PreToolUse"],
            json!([{"matcher": "Bash", "hooks": [foreign]}])
        );
        assert_eq!(
            hooks["Stop"],
            json!([{"hooks": [{"type": "command", "command": "/new/skillvolution hook stop", "timeout": 10}]}])
        );
    }

    #[test]
    fn merge_rewrites_our_hook_in_place_instead_of_moving_it_last() {
        let foreign = json!({"hooks": [{"type": "command", "command": "codegraph prompt-hook"}]});
        let mut config = json!({"hooks": {"UserPromptSubmit": [
            {"hooks": [{"type": "command", "command": "/old/skillvolution hook prompt"}]},
            foreign.clone(),
        ]}});
        let entries = [(
            "UserPromptSubmit",
            None,
            "/new/skillvolution hook prompt".to_owned(),
        )];
        merge(&mut config, &entries).unwrap();
        let groups = config["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(
            groups[0]["hooks"][0]["command"],
            "/new/skillvolution hook prompt"
        );
        assert_eq!(groups[1], foreign);
        let once = config.clone();
        merge(&mut config, &entries).unwrap();
        assert_eq!(config, once, "a rerun must not reorder anything");
    }

    #[test]
    fn merge_keeps_an_event_the_user_left_empty() {
        let mut config = json!({"hooks": {"Notification": []}});
        merge(
            &mut config,
            &[("Stop", None, "/x/skillvolution hook stop".to_owned())],
        )
        .unwrap();
        assert_eq!(config["hooks"]["Notification"], json!([]));
    }

    #[test]
    fn has_owned_reads_nested_and_flat_hook_shapes() {
        let ours = "/opt/skillvolution hook stop";
        assert!(has_owned(
            &json!({"hooks": {"Stop": [{"hooks": [{"command": ours}]}]}})
        ));
        assert!(has_owned(&json!({"hooks": {"stop": [{"command": ours}]}})));
        assert!(!has_owned(
            &json!({"hooks": {"stop": [{"command": "echo skillvolution"}]}})
        ));
    }

    #[test]
    fn merge_keeps_a_foreign_group_that_was_already_empty() {
        let empty = json!({"matcher": "Bash", "hooks": []});
        let mut config = json!({"hooks": {"Stop": [
            empty.clone(),
            {"hooks": [{"type": "command", "command": "/old/skillvolution hook stop"}]},
        ]}});
        let entries = [("Stop", None, "/new/skillvolution hook stop".to_owned())];
        merge(&mut config, &entries).unwrap();
        let groups = config["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0], empty);
        assert_eq!(
            groups[1]["hooks"][0]["command"], "/new/skillvolution hook stop",
            "ours stays in its slot, after the empty group"
        );

        let mut config = json!({"hooks": {"Stop": [
            empty.clone(),
            {"hooks": [{"type": "command", "command": "/old/skillvolution hook stop"}]},
        ]}});
        assert!(remove_owned(&mut config).unwrap());
        assert_eq!(config, json!({"hooks": {"Stop": [empty]}}));
    }

    #[test]
    fn remove_owned_strips_our_hooks_and_drops_emptied_events_and_the_hooks_object() {
        let ours = |event: &str| json!({"type": "command", "command": format!("/old/skillvolution hook {event}")});
        let foreign = json!({"type": "command", "command": "echo foreign"});
        let mut config = json!({"model": "existing", "hooks": {
            "SessionStart": [{"hooks": [ours("session-start")]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [ours("tool-use"), foreign.clone()]}],
            "Stop": [{"hooks": [ours("stop")]}],
        }});

        assert!(remove_owned(&mut config).unwrap());

        assert_eq!(
            config,
            json!({
                "model": "existing",
                "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [foreign]}]},
            })
        );
    }

    #[test]
    fn remove_owned_drops_the_hooks_object_entirely_when_only_ours_was_there() {
        let mut config = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/x/skillvolution hook stop"}]}]}});
        assert!(remove_owned(&mut config).unwrap());
        assert_eq!(config, json!({}));
    }

    #[test]
    fn remove_owned_is_a_no_op_without_a_hooks_key_or_our_entries() {
        let mut config = json!({"model": "existing"});
        assert!(!remove_owned(&mut config).unwrap());
        assert_eq!(config, json!({"model": "existing"}));

        let mut config = json!({"hooks": {"Notification": [{"hooks": [{"type": "command", "command": "echo foreign"}]}]}});
        let before = config.clone();
        assert!(!remove_owned(&mut config).unwrap());
        assert_eq!(config, before);
    }

    #[test]
    fn require_utf8_rejects_non_utf8_paths() {
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let bad = std::path::PathBuf::from(OsStr::from_bytes(&[0x66, 0x6f, 0xff]));
            assert!(require_utf8(&bad, "--db").is_err());
        }
        assert!(require_utf8(Path::new("/valid/path"), "--db").is_ok());
    }
}
