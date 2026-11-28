//! Local usage telemetry (`events`): what was searched, suggested, loaded, delegated to,
//! and where hooks or the MCP server failed. Nothing here is ever sent anywhere, and
//! nothing stored may be prompt text, file contents, or an absolute path.

use super::{Vault, validate_no_secrets};
use crate::clients::ClientKind;
use anyhow::Result;
use rusqlite::params;
use serde_json::{Value, json};

/// Longest search query kept, in characters.
const MAX_QUERY_CHARS: usize = 120;
/// Longest free-text reason kept (a rejection reason), in characters.
pub const MAX_REASON_CHARS: usize = 200;

/// One usage event. `client_version` is not part of it: it is the version stored at the
/// last setup, looked up when the event is recorded (never by running the client's CLI).
pub struct Event<'a> {
    pub kind: &'a str,
    pub client: Option<ClientKind>,
    pub project: Option<&'a str>,
    pub session: Option<&'a str>,
    pub subject: Option<&'a str>,
    pub version: Option<i64>,
    pub data: Value,
}

impl<'a> Event<'a> {
    pub fn new(kind: &'a str) -> Self {
        Self {
            kind,
            client: None,
            project: None,
            session: None,
            subject: None,
            version: None,
            data: json!({}),
        }
    }

    pub fn client(mut self, client: Option<ClientKind>) -> Self {
        self.client = client;
        self
    }

    pub fn project(mut self, project: Option<&'a str>) -> Self {
        self.project = project;
        self
    }

    pub fn session(mut self, session: Option<&'a str>) -> Self {
        self.session = session;
        self
    }

    pub fn subject(mut self, id: &'a str, version: Option<i64>) -> Self {
        self.subject = Some(id);
        self.version = version;
        self
    }

    pub fn data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }
}

impl Vault {
    /// Records `event`; see `log_event` for the form callers on a main path use.
    pub fn record_event(&self, event: Event<'_>) -> Result<()> {
        let client_version = match event.client {
            Some(kind) => self
                .client_version(kind)
                .ok()
                .flatten()
                .map(|stored| stored.version.to_string()),
            None => None,
        };
        self.conn.execute(
            "INSERT INTO events (client, client_version, project, session, kind, subject, version, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.client.map(ClientKind::token),
                client_version,
                event.project,
                event.session,
                event.kind,
                event.subject,
                event.version,
                event.data.to_string(),
            ],
        )?;
        Ok(())
    }

    /// Records `event`, never failing the operation that produced it: a failure is
    /// reported on stderr and otherwise ignored.
    pub fn log_event(&self, event: Event<'_>) {
        let kind = event.kind;
        if let Err(error) = self.record_event(event) {
            eprintln!("skillvolution: could not record {kind} event: {error:#}");
        }
    }

    /// Deletes events older than `older_than_days`, at most once per UTC day (the day is
    /// kept in `vault_meta`): it runs on every open, and the day check is a primary-key
    /// read, while a DELETE would take the write lock.
    pub(super) fn prune_events(&self, older_than_days: i64) -> Result<()> {
        self.prune_events_on(older_than_days, None)
    }

    /// `prune_events` as if the UTC day were `today` (`YYYY-MM-DD`; `None` is the real
    /// one), so tests do not depend on when they run.
    fn prune_events_on(&self, older_than_days: i64, today: Option<&str>) -> Result<()> {
        let done_today: bool = self.conn.query_row(
            "SELECT COALESCE((SELECT value FROM vault_meta WHERE key = 'events_pruned_on'), '')
                    = COALESCE(?1, strftime('%Y-%m-%d', 'now'))",
            [today],
            |row| row.get(0),
        )?;
        if done_today {
            return Ok(());
        }
        self.conn.execute(
            "DELETE FROM events WHERE at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)",
            [format!("-{older_than_days} days")],
        )?;
        self.conn.execute(
            "INSERT INTO vault_meta (key, value)
             VALUES ('events_pruned_on', COALESCE(?1, strftime('%Y-%m-%d', 'now')))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [today],
        )?;
        Ok(())
    }
}

/// Whether `token` holds an absolute path: the whole token, or any part of it after an
/// `=` or `:` (`--db=/home/u/x`, `path:/home/u`, `file:///home/u`).
fn looks_like_path(token: &str) -> bool {
    is_absolute_path(token) || token.split(['=', ':']).any(is_absolute_path)
}

/// Whether `token` looks like an absolute path (POSIX, home-relative, Windows drive or
/// UNC), ignoring the quotes and brackets it may be wrapped in.
fn is_absolute_path(token: &str) -> bool {
    let token = token.trim_start_matches(['(', '[', '{', '"', '\'', '`', '<', '=']);
    let mut chars = token.chars();
    let drive = matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('/' | '\\')) if letter.is_ascii_alphabetic()
    );
    token.starts_with('/') || token.starts_with("~/") || token.starts_with('\\') || drive
}

/// `text` for storage: whitespace collapsed, absolute-path tokens dropped, cut to
/// `max_chars` characters.
pub fn scrub_text(text: &str, max_chars: usize) -> String {
    let joined = text
        .split_whitespace()
        .filter(|token| !looks_like_path(token))
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(max_chars).collect()
}

/// A search query as stored in an event: scrubbed and trimmed, or `None` when it is
/// empty or the credential detector flags it (the query is then dropped entirely).
pub fn search_query_for_event(query: &str) -> Option<String> {
    if validate_no_secrets("query", query).is_err() {
        return None;
    }
    let query = scrub_text(query, MAX_QUERY_CHARS);
    // Trimming can cut a credential in half; check what is actually stored too.
    (!query.is_empty() && validate_no_secrets("query", &query).is_ok()).then_some(query)
}

/// A coarse class for an error, for `mcp_error` and `hook_error` events: never its
/// message, which can quote user input.
pub fn error_class(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<rusqlite::Error>().is_some() {
        "database"
    } else if error.downcast_ref::<serde_json::Error>().is_some() {
        "json"
    } else if error.downcast_ref::<std::io::Error>().is_some() {
        "io"
    } else if error.to_string().starts_with("invalid arguments") {
        "invalid_arguments"
    } else {
        "rejected"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(&dir.path().join("vault.db")).unwrap();
        (dir, vault)
    }

    #[test]
    fn scrubbing_drops_absolute_paths_and_cuts_length() {
        assert_eq!(
            scrub_text("fix  /home/ana/proj/x.rs  (C:\\Users\\a\\b) ~/z now", 100),
            "fix now"
        );
        assert_eq!(scrub_text("abcdef", 3), "abc");
        assert_eq!(scrub_text("relative/path ok", 100), "relative/path ok");
    }

    #[test]
    fn scrubbing_drops_paths_after_an_equals_sign_or_a_colon() {
        assert_eq!(
            scrub_text(
                "run --db=/home/u/x path:/home/u/y file:///home/u/z key=C:\\Users\\u ok",
                200
            ),
            "run ok"
        );
        // Not paths: kept.
        assert_eq!(
            scrub_text("std::fs::read at 12:30 mode=fast", 200),
            "std::fs::read at 12:30 mode=fast"
        );
    }

    #[test]
    fn a_query_with_a_credential_is_dropped() {
        assert_eq!(
            search_query_for_event("sqlite busy timeout").as_deref(),
            Some("sqlite busy timeout")
        );
        assert_eq!(
            search_query_for_event("deploy with ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            None
        );
        assert_eq!(search_query_for_event("   "), None);
        assert_eq!(
            search_query_for_event(&"a ".repeat(500)).map(|q| q.chars().count()),
            Some(MAX_QUERY_CHARS)
        );
    }

    #[test]
    fn events_older_than_the_retention_are_pruned() {
        let (_dir, vault) = vault();
        vault
            .record_event(Event::new("search").data(json!({"results": 0})))
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO events (at, kind) VALUES ('2000-01-01T00:00:00Z', 'search')",
                [],
            )
            .unwrap();
        vault.conn.execute("DELETE FROM vault_meta", []).unwrap();
        vault.prune_events(180).unwrap();
        let count: i64 = vault
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn pruning_runs_at_most_once_per_day() {
        let (_dir, vault) = vault();
        // A pinned day: the test does not depend on midnight UTC passing while it runs.
        let day = Some("2000-01-01");
        vault.prune_events_on(180, day).unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO events (at, kind) VALUES ('2000-01-01T00:00:00Z', 'search')",
                [],
            )
            .unwrap();
        vault.prune_events_on(180, day).unwrap();
        let count = || -> i64 {
            vault
                .conn
                .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(count(), 1, "already pruned today: left alone");
        vault.prune_events_on(180, Some("2000-01-02")).unwrap();
        assert_eq!(count(), 0, "the next day prunes again");
    }
}
