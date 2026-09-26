//! The AI clients setup can configure, sets of them, and where their files go.

use super::{Change, Edit, claude, claude_cli, devin, opencode};
use anyhow::{Result, bail, ensure};
use std::path::{Path, PathBuf};

/// One AI coding client setup knows how to configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientKind {
    ClaudeCode,
    OpenCode,
    Devin,
}

impl ClientKind {
    /// Every client, in the stable order setup lists, prompts for, and configures them.
    pub(crate) const ALL: [ClientKind; 3] = [Self::ClaudeCode, Self::OpenCode, Self::Devin];

    /// The `--client` token.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
            Self::Devin => "devin",
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Devin => "Devin CLI",
        }
    }

    /// Whether the client looks installed: its CLI is on `PATH` or its global config
    /// directory already exists.
    pub(crate) fn detect(self) -> bool {
        let cli = match self {
            Self::ClaudeCode => "claude",
            Self::OpenCode => "opencode",
            Self::Devin => "devin",
        };
        claude_cli::find_on_path(cli).is_some() || self.global_dir().is_ok_and(|dir| dir.exists())
    }

    /// The client's per-user config directory, where global setup writes.
    pub(crate) fn global_dir(self) -> Result<PathBuf> {
        match self {
            Self::ClaudeCode => claude::global_dir(),
            Self::OpenCode => opencode::global_dir(),
            Self::Devin => devin::global_dir(),
        }
    }

    /// Every file this client's setup writes in `scope`, with its new content. Only
    /// reads: nothing is written until the caller passes the result to `write_all`.
    pub(crate) fn changes(self, scope: Scope, bin: &Path, db: &Path) -> Result<Vec<Change>> {
        match self {
            Self::ClaudeCode => claude::changes(scope, bin, db),
            Self::OpenCode => opencode::changes(scope, bin, db),
            Self::Devin => devin::changes(scope, bin, db),
        }
    }

    /// Every edit that undoes this client's `changes` in `scope` (see `remove`'s module
    /// doc comment for what a client's `removals` must do). `notes` collects anything
    /// worth telling the user that isn't a file edit.
    pub(crate) fn removals(self, scope: Scope, notes: &mut Vec<String>) -> Result<Vec<Edit>> {
        match self {
            Self::ClaudeCode => claude::removals(scope, notes),
            Self::OpenCode => opencode::removals(scope, notes),
            Self::Devin => devin::removals(scope, notes),
        }
    }
}

/// A set of clients. Iterates in `ClientKind::ALL` order, whatever order they were
/// added in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Clients(u8);

impl Clients {
    pub(crate) fn all() -> Self {
        ClientKind::ALL.into_iter().collect()
    }

    pub(crate) fn insert(&mut self, kind: ClientKind) {
        self.0 |= 1 << kind as u8;
    }

    pub(crate) fn contains(self, kind: ClientKind) -> bool {
        self.0 & (1 << kind as u8) != 0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = ClientKind> {
        ClientKind::ALL
            .into_iter()
            .filter(move |kind| self.contains(*kind))
    }

    /// Maps `--client` tokens to clients: `all` selects every client, `both` keeps its
    /// original meaning (Claude Code + OpenCode).
    pub(crate) fn parse(tokens: &[String]) -> Result<Self> {
        let mut clients = Clients::default();
        for token in tokens {
            match token.as_str() {
                "all" => clients = Clients::all(),
                "both" => {
                    clients.insert(ClientKind::ClaudeCode);
                    clients.insert(ClientKind::OpenCode);
                }
                other => match ClientKind::ALL
                    .into_iter()
                    .find(|kind| kind.token() == other)
                {
                    Some(kind) => clients.insert(kind),
                    None => bail!("unknown --client {other}"),
                },
            }
        }
        ensure!(!clients.is_empty(), "--client selects no clients");
        Ok(clients)
    }
}

impl FromIterator<ClientKind> for Clients {
    fn from_iter<I: IntoIterator<Item = ClientKind>>(kinds: I) -> Self {
        let mut clients = Clients::default();
        for kind in kinds {
            clients.insert(kind);
        }
        clients
    }
}

/// Where setup writes a client's files.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Scope<'a> {
    /// A project directory, and the vault scope key its MCP server and hooks pass.
    Project { dir: &'a Path, key: &'a str },
    /// The current user's per-client config directories, shared by every project.
    Global,
}

impl<'a> Scope<'a> {
    /// The project key; `None` for global setup, whose server and hooks serve any project.
    pub(crate) fn key(self) -> Option<&'a str> {
        match self {
            Scope::Project { key, .. } => Some(key),
            Scope::Global => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(tokens: &[&str]) -> Result<Vec<ClientKind>> {
        let tokens: Vec<String> = tokens.iter().map(|token| token.to_string()).collect();
        Ok(Clients::parse(&tokens)?.iter().collect())
    }

    #[test]
    fn client_tokens_parse_into_a_set_in_stable_order() {
        use ClientKind::*;
        assert_eq!(parse(&["all"]).unwrap(), [ClaudeCode, OpenCode, Devin]);
        assert_eq!(parse(&["both"]).unwrap(), [ClaudeCode, OpenCode]);
        assert_eq!(
            parse(&["devin", "claude-code"]).unwrap(),
            [ClaudeCode, Devin]
        );
        assert_eq!(parse(&["opencode", "opencode"]).unwrap(), [OpenCode]);
        assert!(parse(&["cursor"]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn every_client_round_trips_through_its_token() {
        for kind in ClientKind::ALL {
            assert_eq!(parse(&[kind.token()]).unwrap(), [kind]);
        }
    }
}
