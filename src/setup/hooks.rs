//! SessionStart/Stop hook merging shared by project (`.claude/settings.local.json`) and
//! global (`settings.json`) Claude Code setup. Both own their hook entries by matching on
//! the command text, so a rerun (even with a changed `--bin`/`--db`) replaces the old
//! entry instead of duplicating it, and any foreign hook is left untouched.

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::path::Path;

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

/// A Skillvolution-owned hook command: one whose first shell word is a path named
/// `skillvolution` and whose remaining words end with `hook stop` or contain `hook
/// session-start` as whole words. Independent of the bin path, db path, or project key,
/// so a changed one replaces the old entry instead of duplicating it; independent of the
/// command text otherwise, so a foreign command that merely contains the substring
/// " hook stop" (e.g. another tool's own `hook stop`, or a `hook stopwatch`) is left alone.
fn owned_command(command: &str) -> bool {
    let words = shell_words(command);
    let Some(first) = words.first() else {
        return false;
    };
    if Path::new(first).file_name() != Some(std::ffi::OsStr::new("skillvolution")) {
        return false;
    }
    let ends_with_hook_stop =
        words.len() >= 2 && words[words.len() - 2] == "hook" && words[words.len() - 1] == "stop";
    let has_hook_session_start = words
        .windows(2)
        .any(|pair| pair[0] == "hook" && pair[1] == "session-start");
    ends_with_hook_stop || has_hook_session_start
}

fn strip_owned(hooks: &mut Map<String, Value>, event: &str) -> Result<()> {
    let Some(value) = hooks.get_mut(event) else {
        return Ok(());
    };
    let groups = value
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    for group in groups.iter_mut() {
        let group = group
            .as_object_mut()
            .with_context(|| format!("hooks.{event} entries must be objects"))?;
        if let Some(entries) = group.get_mut("hooks") {
            let entries = entries
                .as_array_mut()
                .with_context(|| format!("hooks.{event}[].hooks must be an array"))?;
            entries.retain(|entry| {
                !entry
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(owned_command)
            });
        }
    }
    groups.retain(|group| !matches!(group.get("hooks"), Some(Value::Array(e)) if e.is_empty()));
    Ok(())
}

fn append_group(hooks: &mut Map<String, Value>, event: &str, command: String) -> Result<()> {
    let groups = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("hooks.{event} must be an array"))?;
    groups.push(json!({"hooks": [{"type": "command", "command": command, "timeout": 10}]}));
    Ok(())
}

/// Replaces our SessionStart/Stop entries in `config["hooks"]` with `session_start`/`stop`,
/// preserving every other event and every foreign hook.
pub fn merge(config: &mut Value, session_start: String, stop: String) -> Result<()> {
    let hooks = config
        .as_object_mut()
        .context("config must be an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be an object")?;
    strip_owned(hooks, "SessionStart")?;
    strip_owned(hooks, "Stop")?;
    append_group(hooks, "SessionStart", session_start)?;
    append_group(hooks, "Stop", stop)?;
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
    }

    #[test]
    fn a_skillvolution_named_binary_with_unrelated_args_is_not_owned() {
        assert!(!owned_command("/opt/skillvolution hook stopwatch"));
        assert!(!owned_command("/opt/skillvolution serve"));
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
