use anyhow::{Result, ensure};
use rusqlite::Connection;
use std::{path::Path, time::Duration};

pub struct Vault {
    conn: Connection,
}

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct Revision {
    pub id: String,
    pub version: i64,
    pub description: String,
    pub content: String,
    pub evidence: String,
    pub expected_version: i64,
    pub published: bool,
}

fn revision_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Revision> {
    Ok(Revision {
        id: row.get(0)?,
        version: row.get(1)?,
        description: row.get(2)?,
        content: row.get(3)?,
        evidence: row.get(4)?,
        expected_version: row.get(5)?,
        published: row.get(6)?,
    })
}

fn check_base(conn: &Connection, id: &str, expected_version: i64) -> Result<()> {
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM revisions WHERE id = ?1 AND published = 1",
        [id], |row| row.get(0),
    )?;
    ensure!(current == expected_version, "stale base: expected {expected_version}, current published version is {current}");
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 64
            && id.split('-').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())),
        "id must be 1..64 bytes of lowercase letters or digits separated by single hyphens"
    );
    Ok(())
}

fn validate_text(name: &str, text: &str, maximum: usize) -> Result<()> {
    ensure!(!text.trim().is_empty() && text.len() <= maximum && !text.contains('\0'),
        "{name} must be nonempty, at most {maximum} UTF-8 bytes, and contain no NUL");
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct SkillMetadata {
    pub id: String,
    pub version: i64,
    pub description: String,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchPage {
    pub skills: Vec<SkillMetadata>,
    pub total: i64,
    pub has_more: bool,
}

impl Vault {
    pub fn drafts(&self) -> Result<Vec<Revision>> {
        let mut statement = self.conn.prepare(
            "SELECT id, version, description, content, evidence, expected_version, published
             FROM revisions WHERE published = 0 ORDER BY id, version",
        )?;
        Ok(statement.query_map([], revision_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn search(&self, query: &str, limit: i64, offset: i64) -> Result<SearchPage> {
        ensure!((1..=100).contains(&limit), "limit must be 1..100");
        ensure!(offset >= 0, "offset must be nonnegative");
        ensure!(query.len() <= 512 && !query.contains('\0'), "query must be at most 512 UTF-8 bytes without NUL");
        let tx = self.conn.unchecked_transaction()?;
        let total: i64 = tx.query_row(
            "SELECT COUNT(*) FROM latest_published WHERE instr(lower(id), lower(?1)) > 0 OR instr(lower(description), lower(?1)) > 0",
            [query], |row| row.get(0),
        )?;
        let skills = {
            let mut statement = tx.prepare(
                "SELECT id, version, description FROM latest_published
                 WHERE instr(lower(id), lower(?1)) > 0 OR instr(lower(description), lower(?1)) > 0
                 ORDER BY id LIMIT ?2 OFFSET ?3",
            )?;
            statement.query_map(rusqlite::params![query, limit, offset], |row| Ok(SkillMetadata {
                id: row.get(0)?, version: row.get(1)?, description: row.get(2)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.commit()?;
        let has_more = total.saturating_sub(offset) > skills.len() as i64;
        Ok(SearchPage { skills, total, has_more })
    }

    pub fn get(&self, id: &str, version: Option<i64>) -> Result<Revision> {
        validate_id(id)?;
        ensure!(version.is_none_or(|v| v > 0), "version must be positive");
        let version = match version {
            Some(version) => version,
            None => self.conn.query_row("SELECT COALESCE(MAX(version), 0) FROM revisions WHERE id = ?1 AND published = 1", [id], |row| row.get(0))?,
        };
        let revision = self.inspect(id, version)?;
        ensure!(revision.published, "revision is not published");
        Ok(revision)
    }

    pub fn publish(&mut self, id: &str, version: i64) -> Result<Revision> {
        validate_id(id)?;
        ensure!(version > 0, "version must be positive");
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let revision = tx.query_row(
            "SELECT id, version, description, content, evidence, expected_version, published FROM revisions WHERE id = ?1 AND version = ?2",
            rusqlite::params![id, version], revision_row,
        )?;
        ensure!(!revision.published, "revision is already published");
        check_base(&tx, id, revision.expected_version)?;
        tx.execute("UPDATE revisions SET published = 1 WHERE id = ?1 AND version = ?2", rusqlite::params![id, version])?;
        tx.commit()?;
        self.inspect(id, version)
    }

    pub fn propose(
        &mut self,
        id: &str,
        description: &str,
        content: &str,
        evidence: &str,
        expected_version: i64,
    ) -> Result<Revision> {
        validate_id(id)?;
        validate_text("description", description, 280)?;
        ensure!(!description.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')), "description must be a single line without control characters");
        validate_text("content", content, 65_536)?;
        validate_text("evidence", evidence, 16_384)?;
        ensure!(expected_version >= 0, "expected_version must be nonnegative");
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        check_base(&tx, id, expected_version)?;
        let version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM revisions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO revisions (id, version, description, content, evidence, expected_version, published)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            rusqlite::params![id, version, description, content, evidence, expected_version],
        )?;
        tx.commit()?;
        self.inspect(id, version)
    }

    pub fn inspect(&self, id: &str, version: i64) -> Result<Revision> {
        validate_id(id)?;
        ensure!(version > 0, "version must be positive");
        Ok(self.conn.query_row(
            "SELECT id, version, description, content, evidence, expected_version, published
             FROM revisions WHERE id = ?1 AND version = ?2",
            rusqlite::params![id, version],
            revision_row,
        )?)
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let vault = Self { conn };
        vault.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS revisions (
            id TEXT NOT NULL,
            version INTEGER NOT NULL CHECK(version > 0),
            description TEXT NOT NULL,
            content TEXT NOT NULL,
            evidence TEXT NOT NULL,
            expected_version INTEGER NOT NULL CHECK(expected_version >= 0),
            published INTEGER NOT NULL DEFAULT 0 CHECK(published IN (0, 1)),
            PRIMARY KEY (id, version)
        );
        CREATE VIEW IF NOT EXISTS latest_published AS
        SELECT r.id, r.version, r.description FROM revisions r
        WHERE r.published = 1 AND r.version = (
            SELECT MAX(p.version) FROM revisions p WHERE p.id = r.id AND p.published = 1
        );",
        )?;
        Ok(vault)
    }
}
