use super::{
    Vault, join_tags, split_tags, validate_id, validate_no_secrets, validate_sections,
    validate_single_line, validate_tags, validate_text,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

const REVISION_SELECT: &str = "SELECT r.id, r.version, s.scope, r.description, r.tags, r.content,
    r.evidence, r.expected_version, r.status, r.created_at, r.reviewed_at, r.review_note
    FROM revisions r JOIN skills s ON s.id = r.id";
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%SZ', 'now')";
/// The only evaluator verdicts that may be published; `discard` and anything
/// else is refused.
const KEEP_VERDICTS: [&str; 2] = ["keep global", "keep project"];

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
    /// The fresh-context evaluator's verdict line, verbatim.
    pub verdict: &'a str,
    /// Its one-line reason, verbatim.
    pub verdict_reason: &'a str,
    /// Acknowledges that the version being replaced has more helped than
    /// failed reports and that its content was merged, not rewritten.
    pub replaces_proven: bool,
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
        created_at: row.get(9)?,
        reviewed_at: row.get(10)?,
        review_note: row.get(11)?,
    })
}

fn current_version(conn: &Connection, id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM revisions WHERE id = ?1 AND status = 'published'",
        [id],
        |row| row.get(0),
    )?)
}

/// The helped/failed reports recorded against `version` — the history a
/// wholesale replacement resets, since outcome counts are per revision.
fn outcome_score(conn: &Connection, id: &str, version: i64) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(result = 'helped'), 0), COALESCE(SUM(result = 'failed'), 0)
         FROM outcomes WHERE id = ?1 AND version = ?2",
        params![id, version],
        |row| Ok((row.get(0)?, row.get(1)?)),
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
            verdict,
            verdict_reason,
            replaces_proven,
        } = *proposal;
        validate_id(id)?;
        validate_single_line("description", description, 280)?;
        validate_tags(tags)?;
        validate_text("content", content, 65_536)?;
        validate_sections(content)?;
        validate_text("evidence", evidence, 16_384)?;
        validate_single_line("verdict_reason", verdict_reason, 280)?;
        let verdict = verdict.trim().to_ascii_lowercase();
        ensure!(
            verdict != "discard",
            "the evaluator discarded this candidate; do not publish it — tell the user the lesson was evaluated and dropped"
        );
        ensure!(
            KEEP_VERDICTS.contains(&verdict.as_str()),
            "verdict must be exactly {} — relay the evaluator's verdict line, do not paraphrase it",
            KEEP_VERDICTS.join(" or ")
        );
        ensure!(
            expected_version >= 0,
            "expected_version must be nonnegative"
        );
        if let Some(scope) = scope {
            validate_id(scope).context("invalid project scope")?;
        }
        ensure!(
            (verdict == "keep project") == scope.is_some(),
            "verdict {verdict} does not match scope {}; keep project requires scope project and keep global requires scope global",
            scope.unwrap_or("global")
        );
        validate_no_secrets("description", description)?;
        validate_no_secrets("content", content)?;
        validate_no_secrets("evidence", evidence)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO skills (id, scope) VALUES (?1, ?2)",
            params![id, scope],
        )?;
        let stored_scope: Option<String> =
            tx.query_row("SELECT scope FROM skills WHERE id = ?1", [id], |row| {
                row.get(0)
            })?;
        ensure!(
            stored_scope.as_deref() == scope,
            "skill {id} already exists with scope {}; choose a different id",
            stored_scope.as_deref().unwrap_or("global")
        );
        let current = current_version(&tx, id)?;
        ensure!(
            current == expected_version,
            "stale base: expected {expected_version}, current published version is {current}"
        );
        let (helped, failed) = outcome_score(&tx, id, expected_version)?;
        ensure!(
            replaces_proven || helped <= failed,
            "version {expected_version} of {id} is proven (helped {helped}, failed {failed}) and publishing over it resets those counts; \
             merge it into your lesson instead of rewriting it and set replaces_proven true, or publish the new lesson under its own id"
        );
        let version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM revisions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let review_note = format!("{verdict}: {verdict_reason}");
        let revision = tx.query_row(
            &format!(
                "INSERT INTO revisions (id, version, description, tags, content, evidence, expected_version, status, reviewed_at, review_note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'published', {NOW}, ?8)
                 RETURNING description, tags, content, evidence, status, created_at, reviewed_at, review_note"
            ),
            params![id, version, description, join_tags(tags), content, evidence, expected_version, review_note],
            |row| {
                Ok(Revision {
                    id: id.to_owned(),
                    version,
                    scope: stored_scope,
                    description: row.get(0)?,
                    tags: split_tags(&row.get::<_, String>(1)?),
                    content: row.get(2)?,
                    evidence: row.get(3)?,
                    expected_version,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                    reviewed_at: row.get(6)?,
                    review_note: row.get(7)?,
                })
            },
        )?;
        // Drafts can no longer be created, but a database from before that
        // change may still hold some; publishing over their shared base
        // supersedes them so they never linger as publishable candidates.
        tx.execute(
            &format!(
                "UPDATE revisions SET status = 'superseded', reviewed_at = {NOW}, review_note = ?2
                 WHERE id = ?1 AND status = 'draft' AND expected_version = ?3"
            ),
            params![
                id,
                format!("superseded by published version {version}"),
                expected_version
            ],
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
        Ok(revision)
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
