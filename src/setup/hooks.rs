//! Hook command building and merging shared by the Claude Code and Devin setups, project
//! and global. Our hook entries are recognized by their command text, so a rerun (even
//! with a changed `--bin`/`--db`) replaces the old entry instead of duplicating it, and
//! any foreign hook is left untouched.

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::path::Path;

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
const HOOK_EVENTS: [&str; 5] = [
    "session-start",
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
fn owned_command(command: &str) -> bool {
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

/// Whether `config["hooks"]` holds a Skillvolution-owned command under any event.
pub fn has_owned(config: &Value) -> bool {
    let Some(hooks) = config.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|entry| entry.get("command").and_then(Value::as_str))
        .any(owned_command)
}

/// Removes our entries from `hooks[event]`, and any group they leave empty. Returns
/// whether anything was removed.
fn strip_owned(hooks: &mut Map<String, Value>, event: &str) -> Result<bool> {
    let Some(value) = hooks.get_mut(event) else {
        return Ok(false);
    };
    let groups = value
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    let mut removed = false;
    for group in groups.iter_mut() {
        let group = group
            .as_object_mut()
            .with_context(|| format!("hooks.{event} entries must be objects"))?;
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
            removed |= entries.len() != count;
        }
    }
    groups.retain(|group| !matches!(group.get("hooks"), Some(Value::Array(e)) if e.is_empty()));
    Ok(removed)
}

fn append_group(
    hooks: &mut Map<String, Value>,
    event: &str,
    matcher: Option<&str>,
    command: String,
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
    groups.push(group);
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
    let events: Vec<String> = hooks.keys().cloned().collect();
    let mut emptied = Vec::new();
    for event in events {
        if strip_owned(hooks, &event)? && hooks[&event].as_array().is_some_and(Vec::is_empty) {
            emptied.push(event);
        }
    }
    for (event, matcher, command) in entries {
        append_group(hooks, event, matcher.as_deref(), command.clone())?;
    }
    // Checked after appending, so an event we re-add keeps its place in the file.
    hooks.retain(|event, groups| {
        !(emptied.contains(event) && groups.as_array().is_some_and(Vec::is_empty))
    });
    Ok(())
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
