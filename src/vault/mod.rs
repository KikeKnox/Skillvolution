mod outcomes;
mod revisions;
mod search;

pub use outcomes::{OutcomeRecord, OutcomeSummary};
pub use revisions::{Proposal, Revision, SkillView};
pub use search::{SearchPage, SkillMetadata};

use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = include_str!("schema.sql");
const MAX_TAGS: usize = 8;
const MAX_TAG_BYTES: usize = 32;

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
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&conn)?;
        Ok(Self { conn })
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

    pub fn set_deprecated(&self, id: &str, deprecated: bool) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE skills SET deprecated = ?2 WHERE id = ?1",
            rusqlite::params![id, deprecated],
        )?;
        ensure!(changed == 1, "unknown skill: {id}");
        Ok(())
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => {
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        SCHEMA_VERSION => {}
        other => bail!("unsupported database schema version {other}; upgrade skillvolution"),
    }
    tx.commit()?;
    Ok(())
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
