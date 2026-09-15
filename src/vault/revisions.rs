use super::{
    Vault, join_tags, split_tags, validate_id, validate_single_line, validate_tags, validate_text,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

const REVISION_SELECT: &str = "SELECT r.id, r.version, s.scope, r.description, r.tags, r.content,
    r.evidence, r.expected_version, r.status, r.client, r.created_at, r.reviewed_at, r.review_note
    FROM revisions r JOIN skills s ON s.id = r.id";
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%SZ', 'now')";

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Revision {
    pub id: String,
    pub version: i64,
    pub scope: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub content: String,
    pub evidence: String,
    pub expected_version: i64,
    pub status: String,
    pub client: Option<String>,
    pub created_at: String,
    pub reviewed_at: Option<String>,
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SkillView {
    pub id: String,
    pub version: i64,
    pub scope: Option<String>,
    pub deprecated: bool,
    pub description: String,
    pub tags: Vec<String>,
    pub content: String,
}

pub struct Proposal<'a> {
    pub id: &'a str,
    pub description: &'a str,
    pub tags: &'a [String],
    pub content: &'a str,
    pub evidence: &'a str,
    pub expected_version: i64,
    pub scope: Option<&'a str>,
    pub client: Option<&'a str>,
}

fn revision_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Revision> {
    Ok(Revision {
        id: row.get(0)?,
        version: row.get(1)?,
        scope: row.get(2)?,
        description: row.get(3)?,
        tags: split_tags(&row.get::<_, String>(4)?),
        content: row.get(5)?,
        evidence: row.get(6)?,
        expected_version: row.get(7)?,
        status: row.get(8)?,
        client: row.get(9)?,
        created_at: row.get(10)?,
        reviewed_at: row.get(11)?,
        review_note: row.get(12)?,
    })
}

fn current_version(conn: &Connection, id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM revisions WHERE id = ?1 AND status = 'published'",
        [id],
        |row| row.get(0),
    )?)
}

fn load(conn: &Connection, id: &str, version: i64) -> Result<Revision> {
    validate_id(id)?;
    ensure!(version > 0, "version must be positive");
    conn.query_row(
        &format!("{REVISION_SELECT} WHERE r.id = ?1 AND r.version = ?2"),
        params![id, version],
        revision_row,
    )
    .optional()?
    .with_context(|| format!("no revision {id} version {version}"))
}

fn render(revision: &Revision) -> String {
    let mut text = format!(
        "description: {}\ntags: {}\n\n{}",
        revision.description,
        revision.tags.join(", "),
        revision.content
    );
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

impl Vault {
    pub fn propose(&mut self, proposal: &Proposal<'_>) -> Result<Revision> {
        let Proposal {
            id,
            description,
            tags,
            content,
            evidence,
            expected_version,
            scope,
            client,
        } = *proposal;
        validate_id(id)?;
        validate_single_line("description", description, 280)?;
        validate_tags(tags)?;
        validate_text("content", content, 65_536)?;
        validate_text("evidence", evidence, 16_384)?;
        ensure!(expected_version >= 0, "expected_version must be nonnegative");
        if let Some(scope) = scope {
            validate_id(scope).context("invalid project scope")?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = current_version(&tx, id)?;
        let existing: Option<Option<String>> = tx
            .query_row("SELECT scope FROM skills WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        match existing {
            None => {
                tx.execute(
                    "INSERT INTO skills (id, scope) VALUES (?1, ?2)",
                    params![id, scope],
                )?;
            }
            Some(existing) if existing.as_deref() != scope => {
                ensure!(
                    current == 0,
                    "skill {id} already exists with scope {}; choose a different id",
                    existing.as_deref().unwrap_or("global")
                );
                tx.execute(
                    "UPDATE skills SET scope = ?2 WHERE id = ?1",
                    params![id, scope],
                )?;
            }
            Some(_) => {}
        }
        ensure!(
            current == expected_version,
            "stale base: expected {expected_version}, current published version is {current}"
        );
        let version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM revisions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version, client)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, version, description, join_tags(tags), content, evidence, expected_version, client],
        )?;
        tx.commit()?;
        self.inspect(id, version)
    }

    pub fn publish(&mut self, id: &str, version: i64) -> Result<Revision> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = load(&tx, id, version)?;
        ensure!(
            revision.status == "draft",
            "revision {id} version {version} is {}, not a draft",
            revision.status
        );
        let current = current_version(&tx, id)?;
        ensure!(
            current == revision.expected_version,
            "stale base: draft expects {}, current published version is {current}",
            revision.expected_version
        );
        tx.execute(
            &format!("UPDATE revisions SET status = 'published', reviewed_at = {NOW} WHERE id = ?1 AND version = ?2"),
            params![id, version],
        )?;
        tx.execute(
            &format!(
                "UPDATE revisions SET status = 'superseded', reviewed_at = {NOW}, review_note = ?2
                 WHERE id = ?1 AND status = 'draft'"
            ),
            params![id, format!("superseded by published version {version}")],
        )?;
        tx.execute("DELETE FROM skills_fts WHERE id = ?1", [id])?;
        tx.execute(
            "INSERT INTO skills_fts (id, description, tags, content) VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                revision.description,
                revision.tags.join(" "),
                revision.content
            ],
        )?;
        tx.commit()?;
        self.inspect(id, version)
    }

    pub fn reject(&mut self, id: &str, version: i64, note: Option<&str>) -> Result<Revision> {
        if let Some(note) = note {
            validate_text("note", note, 2_048)?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = load(&tx, id, version)?;
        ensure!(
            revision.status == "draft",
            "revision {id} version {version} is {}, not a draft",
            revision.status
        );
        tx.execute(
            &format!("UPDATE revisions SET status = 'rejected', reviewed_at = {NOW}, review_note = ?3 WHERE id = ?1 AND version = ?2"),
            params![id, version, note],
        )?;
        tx.commit()?;
        self.inspect(id, version)
    }

    pub fn get(&self, id: &str, version: Option<i64>, project: Option<&str>) -> Result<SkillView> {
        validate_id(id)?;
        ensure!(version.is_none_or(|v| v > 0), "version must be positive");
        let version = match version {
            Some(version) => version,
            None => current_version(&self.conn, id)?,
        };
        self.conn
            .query_row(
                "SELECT r.id, r.version, s.scope, s.deprecated, r.description, r.tags, r.content
                 FROM revisions r JOIN skills s ON s.id = r.id
                 WHERE r.id = ?1 AND r.version = ?2 AND r.status = 'published'
                   AND (s.scope IS NULL OR s.scope = ?3)",
                params![id, version, project],
                |row| {
                    Ok(SkillView {
                        id: row.get(0)?,
                        version: row.get(1)?,
                        scope: row.get(2)?,
                        deprecated: row.get(3)?,
                        description: row.get(4)?,
                        tags: split_tags(&row.get::<_, String>(5)?),
                        content: row.get(6)?,
                    })
                },
            )
            .optional()?
            .with_context(|| format!("no published skill {id} is visible from this project"))
    }

    pub fn inspect(&self, id: &str, version: i64) -> Result<Revision> {
        load(&self.conn, id, version)
    }

    pub fn drafts(&self) -> Result<Vec<Revision>> {
        let mut statement = self.conn.prepare(&format!(
            "{REVISION_SELECT} WHERE r.status = 'draft' ORDER BY r.id, r.version"
        ))?;
        Ok(statement
            .query_map([], revision_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn diff(&self, id: &str, version: i64) -> Result<String> {
        let revision = self.inspect(id, version)?;
        let base = match revision.expected_version {
            0 => String::new(),
            base => render(&self.inspect(id, base)?),
        };
        let new = render(&revision);
        if base == new {
            return Ok(format!("{id} v{version}: no changes from its base\n"));
        }
        Ok(similar::TextDiff::from_lines(&base, &new)
            .unified_diff()
            .context_radius(3)
            .header(
                &format!("{id} v{}", revision.expected_version),
                &format!("{id} v{version}"),
            )
            .to_string())
    }
}
