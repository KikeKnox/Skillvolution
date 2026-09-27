//! Interactive client selection for global setup: when a terminal is attached we
//! ask which detected clients to configure. `/dev/tty` is opened for both the
//! questions and the answers so prompting still works under `curl | sh`, where
//! stdin is the download pipe. With no terminal — CI, scripts, piped output —
//! every detected client is configured, matching the pre-prompt behavior.
//! `SKILLVOLUTION_NO_INPUT` forces that non-interactive path explicitly.

use super::{Clients, absolutize, common};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Returns the subset of `detected` the user confirmed, or `detected` unchanged
/// when no terminal is available to ask.
pub fn choose_on_terminal(detected: Clients) -> Clients {
    if std::env::var_os("SKILLVOLUTION_NO_INPUT").is_some_and(|v| !v.is_empty()) {
        return detected;
    }
    let Some(mut tty) = Tty::open() else {
        return detected;
    };
    choose(detected, &mut tty.reader, &mut tty.writer)
}

/// Asks on `output` whether to configure each detected client, reading the answers
/// from `input`, and returns the confirmed ones.
fn choose(detected: Clients, input: &mut dyn BufRead, output: &mut dyn Write) -> Clients {
    let _ = writeln!(output, "Detected AI clients:");
    detected
        .iter()
        .filter(|kind| {
            let question = format!("  Configure {}? [Y/n] ", kind.display_name());
            ask(&question, input, output)
        })
        .collect()
}

/// A yes/no question defaulting to yes: `y`/`yes`, blank input, EOF, or a read
/// error count as yes (matching the non-interactive behavior), `n`/`no` as no. Any
/// other answer asks once more, then counts as yes, so a stray keystroke can neither
/// loop forever nor silently skip a client.
fn ask(question: &str, input: &mut dyn BufRead, output: &mut dyn Write) -> bool {
    for _ in 0..2 {
        let _ = write!(output, "{question}");
        let _ = output.flush();
        let mut answer = String::new();
        if input.read_line(&mut answer).unwrap_or(0) == 0 {
            return true;
        }
        match answer.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => {}
        }
    }
    true
}

/// Asks `Vault database path [<default>]: ` on a terminal when one is attached, right
/// after the client-selection prompt; returns `default` unchanged (without asking)
/// under `SKILLVOLUTION_NO_INPUT` or with no terminal available.
pub fn choose_db_on_terminal(default: &Path) -> PathBuf {
    if std::env::var_os("SKILLVOLUTION_NO_INPUT").is_some_and(|v| !v.is_empty()) {
        return default.to_owned();
    }
    let Some(mut tty) = Tty::open() else {
        return default.to_owned();
    };
    choose_db(
        default,
        common::home().as_deref(),
        &mut tty.reader,
        &mut tty.writer,
    )
}

/// Asks for the database path on `output`, reading the answer from `input`: EOF or a
/// blank line accepts `default`; any other answer is resolved by `resolve_db_answer`.
fn choose_db(
    default: &Path,
    home: Option<&Path>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> PathBuf {
    let _ = write!(output, "Vault database path [{}]: ", default.display());
    let _ = output.flush();
    let mut answer = String::new();
    if input.read_line(&mut answer).unwrap_or(0) == 0 {
        return default.to_owned();
    }
    resolve_db_answer(&answer, default, home)
}

/// A blank answer keeps `default`; otherwise a leading `~/` is expanded against `home`
/// and the result is absolutized (falling back to the expanded-but-not-absolutized path
/// if that somehow fails, rather than losing the user's answer).
fn resolve_db_answer(answer: &str, default: &Path, home: Option<&Path>) -> PathBuf {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return default.to_owned();
    }
    let expanded = match (trimmed.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(trimmed),
    };
    absolutize(&expanded).unwrap_or(expanded)
}

struct Tty {
    reader: Box<dyn BufRead>,
    writer: Box<dyn Write>,
}

impl Tty {
    fn open() -> Option<Self> {
        // Never prompt when nothing is a terminal: that covers CI, `Command`ed
        // subprocesses, and piped runs. `/dev/tty` then still answers for the
        // `curl | sh` case, where stdin is the pipe but a terminal exists.
        if !(std::io::stdin().is_terminal()
            || std::io::stdout().is_terminal()
            || std::io::stderr().is_terminal())
        {
            return None;
        }
        #[cfg(unix)]
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            && let Ok(reader) = file.try_clone()
        {
            return Some(Self {
                reader: Box::new(BufReader::new(reader)),
                writer: Box::new(file),
            });
        }
        if std::io::stdin().is_terminal() {
            return Some(Self {
                reader: Box::new(BufReader::new(std::io::stdin())),
                writer: Box::new(std::io::stderr()),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::ClientKind::{self, *};

    /// Runs `choose` over three detected clients with `answers` as the typed input.
    fn choose_with(answers: &str) -> (Vec<ClientKind>, String) {
        let mut output = Vec::new();
        let detected: Clients = [ClaudeCode, OpenCode, Devin].into_iter().collect();
        let chosen = choose(detected, &mut answers.as_bytes(), &mut output);
        (chosen.iter().collect(), String::from_utf8(output).unwrap())
    }

    #[test]
    fn n_and_no_decline_and_a_blank_line_accepts() {
        assert_eq!(choose_with("n\nNO\n\n").0, [Devin]);
        assert_eq!(choose_with("y\n no \nYes\n").0, [ClaudeCode, Devin]);
    }

    #[test]
    fn end_of_input_accepts_every_remaining_client() {
        assert_eq!(choose_with("").0, [ClaudeCode, OpenCode, Devin]);
        assert_eq!(choose_with("n\n").0, [OpenCode, Devin]);
    }

    #[test]
    fn an_invalid_answer_asks_again_once() {
        let (chosen, output) = choose_with("maybe\nn\nn\nn\n");
        assert_eq!(chosen, []);
        assert_eq!(output.matches("Configure Claude Code?").count(), 2);
        assert_eq!(output.matches("Configure OpenCode?").count(), 1);
    }

    #[test]
    fn a_second_invalid_answer_counts_as_yes() {
        let (chosen, output) = choose_with("maybe\nsure\nn\nn\n");
        assert_eq!(chosen, [ClaudeCode]);
        assert_eq!(output.matches("Configure Claude Code?").count(), 2);
    }

    fn choose_db_with(answer: &str, home: Option<&Path>) -> (PathBuf, String) {
        let mut output = Vec::new();
        let default = Path::new("/default/vault.db");
        let path = choose_db(default, home, &mut answer.as_bytes(), &mut output);
        (path, String::from_utf8(output).unwrap())
    }

    #[test]
    fn a_blank_line_keeps_the_default() {
        let (path, output) = choose_db_with("\n", None);
        assert_eq!(path, Path::new("/default/vault.db"));
        assert!(output.starts_with("Vault database path [/default/vault.db]: "));
    }

    #[test]
    fn end_of_input_keeps_the_default() {
        assert_eq!(choose_db_with("", None).0, Path::new("/default/vault.db"));
    }

    // Unix-style absolute paths; Windows resolves them against the current drive.
    #[cfg(unix)]
    #[test]
    fn a_custom_absolute_path_is_kept() {
        let path = choose_db_with("/somewhere/else/vault.db\n", None).0;
        assert_eq!(path, Path::new("/somewhere/else/vault.db"));
    }

    // Unix-style absolute paths; Windows resolves them against the current drive.
    #[cfg(unix)]
    #[test]
    fn a_tilde_prefixed_path_expands_against_home() {
        let path = choose_db_with("~/x.db\n", Some(Path::new("/home/user"))).0;
        assert_eq!(path, Path::new("/home/user/x.db"));
    }
}
