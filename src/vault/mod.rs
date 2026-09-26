mod outcomes;
mod revisions;
mod search;
mod transfer;

pub use outcomes::{OutcomeRecord, OutcomeSummary};
pub use revisions::{Proposal, Revision, SkillView};
pub use search::{SearchPage, SkillMetadata};
pub use transfer::{ImportReport, PurgeReport};

use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, ErrorCode, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The full current schema, run on new databases.
const SCHEMA: &str = include_str!("schema.sql");
/// Upgrade steps for existing databases: `MIGRATIONS[n]` moves a database from
/// schema version `n + 1` to `n + 2`. Append a step (and update `SCHEMA`) to
/// change the schema; never edit a shipped one.
const MIGRATIONS: &[&str] = &[
    // v2: Devin hook state.
    "CREATE TABLE devin_hook_state (
        session_id TEXT PRIMARY KEY,
        worked INTEGER NOT NULL DEFAULT 0 CHECK(worked IN (0, 1)),
        reviewed INTEGER NOT NULL DEFAULT 0 CHECK(reviewed IN (0, 1))
    );",
    // v3: search entries addressed by rowid, one outcome per revision, project
    // and day, legacy drafts retired, and hook state dated for pruning.
    "ALTER TABLE skills ADD COLUMN fts_rowid INTEGER;
    DELETE FROM skills_fts;
    INSERT INTO skills_fts (rowid, id, description, tags, content)
        SELECT s.rowid, c.id, c.description, c.tags, c.content
        FROM current_skills c JOIN skills s ON s.id = c.id;
    UPDATE skills SET fts_rowid = rowid WHERE id IN (SELECT id FROM current_skills);

    DELETE FROM outcomes WHERE rowid NOT IN (
        SELECT MAX(rowid) FROM outcomes
        GROUP BY id, version, COALESCE(project, ''), substr(created_at, 1, 10)
    );
    CREATE UNIQUE INDEX outcomes_once_per_day
        ON outcomes(id, version, COALESCE(project, ''), substr(created_at, 1, 10));

    UPDATE revisions SET status = 'superseded',
        reviewed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
        review_note = 'legacy draft retired by schema v3'
    WHERE status = 'draft';

    -- ADD COLUMN can't take a non-constant default, so rebuild the hook tables.
    CREATE TABLE hook_state_v3 (
        session_id TEXT PRIMARY KEY,
        transcript_offset INTEGER NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
    );
    INSERT INTO hook_state_v3 (session_id, transcript_offset)
        SELECT session_id, transcript_offset FROM hook_state;
    DROP TABLE hook_state;
    ALTER TABLE hook_state_v3 RENAME TO hook_state;
    CREATE TABLE devin_hook_state_v3 (
        session_id TEXT PRIMARY KEY,
        worked INTEGER NOT NULL DEFAULT 0 CHECK(worked IN (0, 1)),
        reviewed INTEGER NOT NULL DEFAULT 0 CHECK(reviewed IN (0, 1)),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
    );
    INSERT INTO devin_hook_state_v3 (session_id, worked, reviewed)
        SELECT session_id, worked, reviewed FROM devin_hook_state;
    DROP TABLE devin_hook_state;
    ALTER TABLE devin_hook_state_v3 RENAME TO devin_hook_state;
    CREATE TRIGGER hook_state_touch AFTER UPDATE OF transcript_offset ON hook_state
    BEGIN
        UPDATE hook_state SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE session_id = NEW.session_id;
    END;
    CREATE TRIGGER devin_hook_state_touch AFTER UPDATE OF worked, reviewed ON devin_hook_state
    BEGIN
        UPDATE devin_hook_state SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE session_id = NEW.session_id;
    END;",
];
const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64 + 1;
/// `PRAGMA application_id` of a Skillvolution vault ("SKV1"), so another
/// program's database is refused instead of migrated.
const APPLICATION_ID: i64 = 0x534B_5631;
/// Hook state for sessions idle this long is dropped on open.
const HOOK_STATE_RETENTION_DAYS: i64 = 30;
const MAX_TAGS: usize = 8;
const MAX_TAG_BYTES: usize = 32;
const CONTENT_SECTIONS: [&str; 4] = [
    "## When to use",
    "## Procedure",
    "## Pitfalls",
    "## Verification",
];

pub struct Vault {
    conn: Connection,
}

impl Vault {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // Several processes can race to create the same not-yet-existing database.
        // busy_timeout doesn't cover this window (the WAL pragma and the first
        // migration transaction can still see SQLITE_BUSY/LOCKED here), so retry.
        retry_on_busy(|| {
            conn.pragma_update(None, "journal_mode", "WAL")?;
            migrate(&conn)
        })?;
        let vault = Self { conn };
        vault.prune_hook_state(HOOK_STATE_RETENTION_DAYS)?;
        Ok(vault)
    }

    /// Deletes hook state for sessions not written in `older_than_days`. Reads
    /// first because it runs on every open and a DELETE takes the write lock
    /// even when nothing matches.
    pub fn prune_hook_state(&self, older_than_days: i64) -> Result<()> {
        let age = format!("-{older_than_days} days");
        let stale: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM hook_state
                    WHERE updated_at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1))
                 OR EXISTS (SELECT 1 FROM devin_hook_state
                    WHERE updated_at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1))",
            [&age],
            |row| row.get(0),
        )?;
        if !stale {
            return Ok(());
        }
        for table in ["hook_state", "devin_hook_state"] {
            self.conn.execute(
                &format!(
                    "DELETE FROM {table} WHERE updated_at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)"
                ),
                [&age],
            )?;
        }
        Ok(())
    }

    pub fn transcript_offset(&self, session_id: &str) -> Result<u64> {
        let offset: Option<i64> = self
            .conn
            .query_row(
                "SELECT transcript_offset FROM hook_state WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(offset.unwrap_or(0).max(0) as u64)
    }

    pub fn set_transcript_offset(&self, session_id: &str, offset: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO hook_state (session_id, transcript_offset) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET transcript_offset = excluded.transcript_offset",
            rusqlite::params![session_id, offset as i64],
        )?;
        Ok(())
    }

    // The Devin review flags say whether a session did work since its last stop
    // and whether a review tool ran after that work. Devin runs PostToolUse
    // hooks in parallel, so each update is a single statement: a separate read
    // and write would let concurrent hooks overwrite each other's flag.

    /// Marks work done: any earlier review no longer covers it.
    pub fn mark_devin_work(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO devin_hook_state (session_id, worked, reviewed) VALUES (?1, 1, 0)
             ON CONFLICT(session_id) DO UPDATE SET worked = 1, reviewed = 0",
            [session_id],
        )?;
        Ok(())
    }

    pub fn mark_devin_review(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO devin_hook_state (session_id, worked, reviewed) VALUES (?1, 0, 1)
             ON CONFLICT(session_id) DO UPDATE SET reviewed = 1",
            [session_id],
        )?;
        Ok(())
    }

    /// Returns the session's (worked, reviewed) flags and resets them, so the
    /// next stop only sees what happens after this one. Missing row means
    /// (false, false).
    pub fn take_devin_hook_state(&self, session_id: &str) -> Result<(bool, bool)> {
        let row: Option<(bool, bool)> = self
            .conn
            .query_row(
                "DELETE FROM devin_hook_state WHERE session_id = ?1 RETURNING worked, reviewed",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row.unwrap_or((false, false)))
    }

    pub fn clear_devin_hook_state(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM devin_hook_state WHERE session_id = ?1",
            [session_id],
        )?;
        Ok(())
    }

    pub fn set_deprecated(&self, id: &str, deprecated: bool) -> Result<()> {
        validate_id(id)?;
        let changed = self.conn.execute(
            "UPDATE skills SET deprecated = ?2 WHERE id = ?1",
            rusqlite::params![id, deprecated],
        )?;
        ensure!(changed == 1, "unknown skill: {id}");
        Ok(())
    }
}

/// Retries `f` while it fails with SQLITE_BUSY/SQLITE_LOCKED, up to a bounded
/// total wait; any other error (or a busy error past the deadline) is returned
/// immediately.
fn retry_on_busy<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut backoff = Duration::from_millis(5);
    loop {
        match f() {
            Err(err) if is_busy(&err) && Instant::now() < deadline => {
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_millis(200));
            }
            result => return result,
        }
    }
}

fn is_busy(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(inner, _))
            if matches!(inner.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn migrate(conn: &Connection) -> Result<()> {
    // Checked outside a transaction first: an up-to-date vault, the common case,
    // must open without the write lock or it would queue behind every writer.
    if schema_stamp(conn)? == (SCHEMA_VERSION, APPLICATION_ID) {
        return Ok(());
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    // Re-read under the lock: another process may have migrated meanwhile.
    let (version, application_id) = schema_stamp(&tx)?;
    ensure!(
        (0..=SCHEMA_VERSION).contains(&version),
        "unsupported database schema version {version}; upgrade skillvolution"
    );
    // Vaults from before the application id was stamped carry 0.
    ensure!(
        application_id == APPLICATION_ID || application_id == 0,
        "not a skillvolution database (application_id {application_id:#x}); choose another path"
    );
    if version == 0 {
        let count = |sql| tx.query_row(sql, [], |row| row.get::<_, i64>(0));
        ensure!(
            count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'revisions'")? == 0,
            "database uses the pre-1 schema; move it aside and create a new one"
        );
        ensure!(
            count("SELECT COUNT(*) FROM sqlite_master")? == 0,
            "not a skillvolution database (it already holds other tables); choose another path"
        );
        tx.execute_batch(SCHEMA)?;
    } else {
        for migration in &MIGRATIONS[version as usize - 1..] {
            tx.execute_batch(migration)?;
        }
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.commit()?;
    Ok(())
}

/// The database's `(user_version, application_id)`.
fn schema_stamp(conn: &Connection) -> Result<(i64, i64)> {
    let read = |name| conn.pragma_query_value(None, name, |row| row.get(0));
    Ok((read("user_version")?, read("application_id")?))
}

/// `$XDG_DATA_HOME/skillvolution/skills.db` when `XDG_DATA_HOME` is an absolute, nonempty path;
/// otherwise `$HOME/.local/share/skillvolution/skills.db` (`$USERPROFILE` if `$HOME` is unset).
pub fn default_database() -> Result<PathBuf> {
    let nonempty_env = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = nonempty_env("XDG_DATA_HOME")
        .filter(|path| path.is_absolute())
        .or_else(|| {
            nonempty_env("HOME")
                .or_else(|| nonempty_env("USERPROFILE"))
                .map(|home| home.join(".local/share"))
        })
        .ok_or_else(|| anyhow::anyhow!("set --db, XDG_DATA_HOME, or HOME"))?;
    Ok(base.join("skillvolution/skills.db"))
}

pub fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            }),
        "id must be 1..64 bytes of lowercase letters or digits separated by single hyphens"
    );
    Ok(())
}

fn validate_text(name: &str, text: &str, maximum: usize) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.len() <= maximum && !text.contains('\0'),
        "{name} must be nonempty, at most {maximum} UTF-8 bytes, and contain no NUL"
    );
    Ok(())
}

fn validate_single_line(name: &str, text: &str, maximum: usize) -> Result<()> {
    validate_text(name, text, maximum)?;
    ensure!(
        !text
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')),
        "{name} must be a single line without control characters"
    );
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<()> {
    ensure!(
        tags.len() <= MAX_TAGS,
        "at most {MAX_TAGS} tags are allowed"
    );
    for tag in tags {
        ensure!(
            tag.len() <= MAX_TAG_BYTES && validate_id(tag).is_ok(),
            "tag {tag:?} must be at most {MAX_TAG_BYTES} bytes of lowercase letters or digits separated by single hyphens"
        );
    }
    Ok(())
}

/// The four sections the evolution skill prescribes, in order. Extra `##`
/// sections and prose before the first heading are fine; the order is enforced
/// by advancing one heading iterator across all four searches.
fn validate_sections(content: &str) -> Result<()> {
    let mut headings = content
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("## "));
    ensure!(
        CONTENT_SECTIONS
            .iter()
            .all(|section| headings.any(|heading| heading.eq_ignore_ascii_case(section))),
        "content must contain the headings {}, each on its own line and in that order",
        CONTENT_SECTIONS.join(", ")
    );
    Ok(())
}

/// Credential shapes with a fixed, high-signal prefix: (prefix, minimum body
/// length, label). Case-sensitive on purpose: real credentials have fixed case,
/// so `GHP_` or `Akia` in prose never matches.
const SECRET_SHAPES: [(&str, usize, &str); 22] = [
    ("AKIA", 16, "an AWS access key id"),
    ("ASIA", 16, "an AWS temporary access key id"),
    ("ghp_", 30, "a GitHub token"),
    ("gho_", 30, "a GitHub token"),
    ("ghu_", 30, "a GitHub token"),
    ("ghs_", 30, "a GitHub token"),
    ("ghr_", 30, "a GitHub token"),
    ("github_pat_", 30, "a GitHub token"),
    ("sk-", 20, "an OpenAI API key"),
    ("AIza", 35, "a Google API key"),
    ("xoxb-", 24, "a Slack bot token"),
    ("xoxp-", 24, "a Slack user token"),
    ("xoxa-", 24, "a Slack workspace app token"),
    ("xoxr-", 24, "a Slack refresh token"),
    ("xapp-", 24, "a Slack app-level token"),
    ("sk_live_", 24, "a Stripe secret key"),
    ("rk_live_", 24, "a Stripe restricted key"),
    ("glpat-", 20, "a GitLab personal access token"),
    ("npm_", 30, "an npm token"),
    ("hf_", 30, "a Hugging Face token"),
    ("pypi-", 30, "a PyPI token"),
    ("ya29.", 24, "a Google OAuth access token"),
];
const BEARER_MINIMUM: usize = 32;

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// The credential-shaped run at the start of `rest`: real credentials are long
/// and mix letters with digits, which placeholders (`ghp_xxx`, `<token>`,
/// `$GITHUB_TOKEN`, `sk-...`) never are.
fn credential_body(rest: &str, minimum: usize) -> bool {
    let body = rest
        .split(|c: char| !is_token_char(c))
        .next()
        .unwrap_or_default();
    body.len() >= minimum
        && body.bytes().any(|b| b.is_ascii_digit())
        && body.bytes().any(|b| b.is_ascii_alphabetic())
}

/// `prefix` must start the token or follow a character that cannot be part of a
/// credential, so `task-1a2b3c4d5e6f7a8b` is never read as an OpenAI key.
fn has_credential(token: &str, prefix: &str, minimum: usize) -> bool {
    token.match_indices(prefix).any(|(at, _)| {
        (at == 0 || !token[..at].chars().next_back().is_some_and(is_token_char))
            && credential_body(&token[at + prefix.len()..], minimum)
    })
}

/// Placeholder-shaped stand-ins for a credential: an environment variable
/// reference (`$PASS`, `${PASS}`), an angle-bracketed name (`<password>`), a
/// redaction marker (`***`), the literal word `password`, or empty.
fn is_placeholder_value(value: &str) -> bool {
    value.is_empty()
        || value.starts_with('$')
        || (value.starts_with('<') && value.ends_with('>'))
        || value.chars().all(|c| c == '*')
        || value.eq_ignore_ascii_case("password")
}

/// A `scheme://user:password@host` URL whose password looks real: not one of
/// the placeholder forms `is_placeholder_value` accepts, not a repeat of the user
/// name (`postgres:postgres`, `guest:guest`), and at least 8 characters mixing two
/// or more character classes, so conventional local-development defaults such as
/// `root:secret@localhost` stay publishable.
fn url_credential(token: &str) -> bool {
    let Some((_, after_scheme)) = token.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let Some((userinfo, _host)) = authority.rsplit_once('@') else {
        return false;
    };
    let Some((user, password)) = userinfo.split_once(':') else {
        return false;
    };
    let classes = [
        password.bytes().any(|b| b.is_ascii_lowercase()),
        password.bytes().any(|b| b.is_ascii_uppercase()),
        password.bytes().any(|b| b.is_ascii_digit()),
        password.bytes().any(|b| !b.is_ascii_alphanumeric()),
    ];
    !is_placeholder_value(password)
        && password != user
        && password.len() >= 8
        && classes.iter().filter(|&&present| present).count() >= 2
}

/// A Slack incoming-webhook URL (`hooks.slack.com/services/<team>/<bot>/<secret>`).
/// The URL itself is a usable credential, so unlike the prefix shapes above
/// there is no placeholder carve-out: any three nonempty segments count.
fn is_slack_webhook(token: &str) -> bool {
    let Some((_, rest)) = token.split_once("hooks.slack.com/services/") else {
        return false;
    };
    let mut segments = rest.split('/');
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(team), Some(bot), Some(secret))
            if team.starts_with('T') && bot.starts_with('B') && !secret.is_empty()
    )
}

/// Whether `tokens[at]` is the value half of a `Bearer <token>` pair, allowing
/// for a leading/trailing quote or trailing colon on the `Bearer` marker (as in
/// `'Bearer <tok>'` or `Authorization: Bearer <tok>`) and around the token
/// itself.
fn bearer_label(tokens: &[&str], at: usize) -> Option<&'static str> {
    const QUOTES: [char; 2] = ['\'', '"'];
    if at == 0 {
        return None;
    }
    let marker = tokens[at - 1].trim_matches(|c| QUOTES.contains(&c) || c == ':');
    if !marker.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let candidate = tokens[at].trim_matches(|c| QUOTES.contains(&c));
    credential_body(candidate, BEARER_MINIMUM).then_some("a bearer token")
}

/// A complete three-segment JWT; truncated teaching examples
/// (`eyJhbGciOiJIUzI1NiJ9.<payload>.<sig>`) don't match.
fn is_jwt(token: &str) -> bool {
    let token = token.trim_matches(|c: char| !is_token_char(c) && c != '.');
    let segments: Vec<&str> = token.split('.').collect();
    matches!(segments.as_slice(), [header, payload, signature]
        if header.starts_with("eyJ") && header.len() >= 16
            && payload.len() >= 24 && signature.len() >= 16
            && segments.iter().all(|s| s.chars().all(is_token_char)))
}

/// Refuses credential-shaped tokens before anything is written to a vault that
/// publishes immediately. Line- and token-local: no entropy scoring, and no
/// generic `key=value` rule (deliberately — see the false-positive discussion
/// in docs/architecture.md).
fn validate_no_secrets(name: &str, text: &str) -> Result<()> {
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        // `contains`, not a line-start check: a key can be embedded mid-line,
        // e.g. as an escaped `\n`-joined value inside a JSON string.
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            bail!(
                "{name} must not contain credentials: a private key block appears on line {number}; \
                 describe the key instead of pasting it, then publish again"
            );
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for (at, token) in tokens.iter().enumerate() {
            let label = SECRET_SHAPES
                .iter()
                .find(|(prefix, minimum, _)| has_credential(token, prefix, *minimum))
                .map(|(_, _, label)| *label)
                .or_else(|| is_jwt(token).then_some("a JSON Web Token"))
                .or_else(|| bearer_label(&tokens, at))
                .or_else(|| url_credential(token).then_some("a URL with an embedded password"))
                .or_else(|| is_slack_webhook(token).then_some("a Slack webhook URL"));
            if let Some(label) = label {
                bail!(
                    "{name} must not contain credentials: {label} appears on line {number}; \
                     replace the value with a placeholder like <token> or $ENV_VAR and publish again"
                );
            }
        }
    }
    Ok(())
}

fn join_tags(tags: &[String]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for tag in tags {
        if !unique.contains(&tag.as_str()) {
            unique.push(tag);
        }
    }
    unique.join(" ")
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split_whitespace().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40 bytes, mixes letters and digits, contains none of the fixed
    /// prefixes below — a generic stand-in for "a long random credential".
    const REAL_BODY: &str = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0";

    #[test]
    fn secret_shapes_detect_a_real_credential_shaped_value() {
        for (prefix, minimum, label) in SECRET_SHAPES {
            let token = format!("{prefix}{REAL_BODY}");
            assert!(
                has_credential(&token, prefix, minimum),
                "expected {label} ({prefix:?}) to match {token:?}"
            );
        }
    }

    #[test]
    fn secret_shapes_do_not_flag_their_placeholder_forms() {
        let placeholders = [
            ("AKIA", "AKIAEXAMPLE"),
            ("ASIA", "ASIAEXAMPLE"),
            ("ghp_", "ghp_xxx"),
            ("gho_", "gho_xxx"),
            ("ghu_", "ghu_xxx"),
            ("ghs_", "ghs_xxx"),
            ("ghr_", "ghr_xxx"),
            ("github_pat_", "github_pat_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
            ("sk-", "sk-..."),
            ("AIza", "AIza..."),
            ("xoxb-", "xoxb-your-token"),
            ("xoxp-", "xoxp-your-token"),
            ("xoxa-", "xoxa-your-token"),
            ("xoxr-", "xoxr-your-token"),
            ("xapp-", "xapp-your-token"),
            ("sk_live_", "sk_live_xxx"),
            ("rk_live_", "rk_live_xxx"),
            ("glpat-", "glpat-xxx"),
            ("npm_", "npm_xxx"),
            ("hf_", "hf_xxx"),
            ("pypi-", "pypi-xxx"),
            ("ya29.", "ya29.xxx"),
        ];
        for (prefix, placeholder) in placeholders {
            let (_, minimum, label) = SECRET_SHAPES
                .into_iter()
                .find(|(p, _, _)| *p == prefix)
                .unwrap();
            assert!(
                !has_credential(placeholder, prefix, minimum),
                "expected placeholder {placeholder:?} not to match {label}"
            );
        }
    }

    #[test]
    fn credential_prefix_inside_a_larger_identifier_is_not_flagged() {
        // `sk-` is preceded by a token character (`a` of `task`, `i` of
        // `disk`) in both cases, so it is part of a larger word, not the
        // start of a credential.
        assert!(!has_credential("task-1a2b3c4d5e6f7a8b", "sk-", 20));
        assert!(!has_credential("disk-abc123456789012345", "sk-", 20));
    }

    #[test]
    fn private_key_block_is_detected() {
        let text = format!("before\n-----BEGIN RSA PRIVATE KEY-----\n{REAL_BODY}\nafter");
        let error = validate_no_secrets("content", &text)
            .unwrap_err()
            .to_string();
        assert!(error.contains("private key"), "{error}");
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn full_jwt_is_detected_but_a_truncated_teaching_example_is_not() {
        let full = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.\
SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert!(is_jwt(full), "{full}");
        assert!(!is_jwt("eyJhbGciOiJIUzI1NiJ9.<payload>.<sig>"));
    }

    #[test]
    fn bearer_prefixed_long_token_is_detected_but_env_var_placeholder_is_not() {
        let real = format!("Authorization: Bearer {REAL_BODY}");
        let error = validate_no_secrets("content", &real)
            .unwrap_err()
            .to_string();
        assert!(error.contains("bearer token"), "{error}");
        assert!(validate_no_secrets("content", "Authorization: Bearer $TOKEN").is_ok());
    }

    #[test]
    fn quoted_bearer_token_is_detected() {
        let single = format!("'Bearer {REAL_BODY}'");
        let error = validate_no_secrets("content", &single)
            .unwrap_err()
            .to_string();
        assert!(error.contains("bearer token"), "{error}");

        let double = format!("\"Bearer {REAL_BODY}\"");
        let error = validate_no_secrets("content", &double)
            .unwrap_err()
            .to_string();
        assert!(error.contains("bearer token"), "{error}");
    }

    #[test]
    fn private_key_embedded_mid_line_is_detected() {
        // A JSON string carrying a key as an escaped `\n`-joined value, not on
        // a line by itself.
        let text = format!(
            r#"{{"key": "-----BEGIN RSA PRIVATE KEY-----\n{REAL_BODY}\n-----END RSA PRIVATE KEY-----"}}"#
        );
        let error = validate_no_secrets("content", &text)
            .unwrap_err()
            .to_string();
        assert!(error.contains("private key"), "{error}");
    }

    #[test]
    fn url_with_a_real_password_is_detected() {
        let real = format!("postgres://admin:{REAL_BODY}@db.example.com:5432/app");
        let error = validate_no_secrets("content", &real)
            .unwrap_err()
            .to_string();
        assert!(error.contains("URL with an embedded password"), "{error}");
        assert!(!error.contains(REAL_BODY), "{error}");
    }

    #[test]
    fn url_credential_placeholder_forms_are_not_flagged() {
        for safe in [
            "postgres://user:<password>@host/db",
            "https://$USER:$TOKEN@host/path",
            "https://user:${PASS}@host/path",
            "https://user:***@host/path",
            "https://user:password@host/path",
            "https://user@host/path",
            "postgres://postgres:postgres@localhost:5432/app",
            "amqp://guest:guest@localhost:5672",
            "mysql://root:secret@localhost/db",
        ] {
            assert!(validate_no_secrets("content", safe).is_ok(), "{safe}");
        }
    }

    #[test]
    fn slack_webhook_url_is_detected() {
        // Split so no contiguous webhook URL appears in the source for secret
        // scanners (e.g. GitHub push protection) to flag.
        let webhook = concat!(
            "https://hooks.slack.com/services/T0123456789/",
            "B0123456789/abcdefghijklmnopqrstuvwx"
        );
        let error = validate_no_secrets("content", webhook)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Slack webhook"), "{error}");
    }
}
