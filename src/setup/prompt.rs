//! Interactive client selection for global setup: when a terminal is attached we
//! ask which detected clients to configure. `/dev/tty` is opened for both the
//! questions and the answers so prompting still works under `curl | sh`, where
//! stdin is the download pipe. With no terminal — CI, scripts, piped output —
//! every detected client is configured, matching the pre-prompt behavior.
//! `SKILLVOLUTION_NO_INPUT` forces that non-interactive path explicitly.

use super::Clients;
use std::io::{BufRead, BufReader, IsTerminal, Write};

/// Returns the subset of `detected` the user confirmed, or `detected` unchanged
/// when no terminal is available to ask.
pub fn choose(detected: Clients) -> Clients {
    if std::env::var_os("SKILLVOLUTION_NO_INPUT").is_some_and(|v| !v.is_empty()) {
        return detected;
    }
    let Some(mut tty) = Tty::open() else {
        return detected;
    };
    let mut chosen = Clients::default();
    let _ = writeln!(tty.writer, "Detected AI clients:");
    for (present, label, _) in detected.list() {
        if present && tty.ask(&format!("  Configure {label}? [Y/n] ")) {
            match label {
                "Claude Code" => chosen.claude_code = true,
                "OpenCode" => chosen.opencode = true,
                _ => chosen.devin = true,
            }
        }
    }
    chosen
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

    /// A yes/no question defaulting to yes: blank input, EOF, or a read error all
    /// count as yes, matching the previous non-interactive behavior.
    fn ask(&mut self, question: &str) -> bool {
        let _ = write!(self.writer, "{question}");
        let _ = self.writer.flush();
        let mut answer = String::new();
        if self.reader.read_line(&mut answer).is_err() {
            return true;
        }
        !matches!(answer.trim().to_lowercase().as_str(), "n" | "no")
    }
}
