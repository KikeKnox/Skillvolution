//! Whole-vault operations: hard deletion (`purge`), JSON export/import, and
//! online backups.

use super::{
    Vault, join_tags, revisions, split_tags, validate_id, validate_no_secrets,
    validate_single_line, validate_tags, validate_text,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

const EXPORT_FORMAT: &str = "skillvolution-export";
const EXPORT_VERSION: i64 = 1;

/// What `purge` removed.
#[derive(Debug, Serialize)]
pub struct PurgeReport {
    pub revisions: usize,
    pub outcomes: usize,
    /// True when the skill itself (no revisions left) was removed.
    pub skill_removed: bool,
    /// False when `VACUUM` or the WAL checkpoint could not finish (other
    /// connections still open), so freed content may linger on disk until a
    /// later purge compacts the file.
    pub compacted: bool,
}

/// What `import_json` merged into the vault.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub revisions_imported: usize,
    pub revisions_skipped: usize,
    pub outcomes_imported: usize,
    pub outcomes_skipped: usize,
}

/// One revision, exactly as stored in the `revisions` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportRevision {
    id: String,
    version: i64,
    description: String,
    tags: Vec<String>,
    content: String,
    evidence: String,
    expected_version: i64,
    status: String,
    created_at: String,
    reviewed_at: Option<String>,
    review_note: Option<String>,
}

/// One outcome, exactly as stored in the `outcomes` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportOutcome {
    id: String,
    version: i64,
    result: String,
    note: String,
    project: Option<String>,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportSkill {
    id: String,
    scope: Option<String>,
    deprecated: bool,
    revisions: Vec<ExportRevision>,
    outcomes: Vec<ExportOutcome>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExportDocument {
    format: String,
    version: i64,
    exported_at: String,
    skills: Vec<ExportSkill>,
}

/// Repoints `id`'s search row at its current latest published revision, or
/// clears the row when none remains. Used after `purge` removes a revision
/// and after `import_json` merges new ones, mirroring the indexing `propose`
/// does for the revision it just published.
fn refresh_fts(tx: &Connection, id: &str) -> Result<()> {
    let latest: Option<(String, String, String)> = tx
        .query_row(
            "SELECT description, tags, content FROM revisions
             WHERE id = ?1 AND status = 'published'
             ORDER BY version DESC LIMIT 1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match latest {
        Some((description, tags, content)) => {
            revisions::replace_fts_row(tx, id, &description, &tags, &content)
        }
        None => revisions::clear_fts_row(tx, id),
    }
}

impl Vault {
    /// Permanently deletes one revision (`version = Some`) or a whole skill,
    /// including its outcomes and search-index row, in one IMMEDIATE
    /// transaction. `secure_delete` is turned on for this connection first so
    /// SQLite overwrites the freed pages instead of merely unlinking them,
    /// then `VACUUM` (and a WAL checkpoint) after commit rewrites the file so
    /// the purged content does not linger in either the main database file or
    /// the `-wal` file. That compaction is best effort, reported in
    /// `compacted`: the deletion is already committed by then.
    pub fn purge(&mut self, id: &str, version: Option<i64>) -> Result<PurgeReport> {
        validate_id(id)?;
        self.conn.pragma_update(None, "secure_delete", "ON")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM skills WHERE id = ?1)",
                [id],
                |row| row.get::<_, bool>(0)
            )?,
            "unknown skill: {id}"
        );
        let mut report = match version {
            None => {
                let outcomes = tx.execute("DELETE FROM outcomes WHERE id = ?1", [id])?;
                let revisions = tx.execute("DELETE FROM revisions WHERE id = ?1", [id])?;
                revisions::clear_fts_row(&tx, id)?;
                tx.execute("DELETE FROM skills WHERE id = ?1", [id])?;
                PurgeReport {
                    revisions,
                    outcomes,
                    skill_removed: true,
                    compacted: false,
                }
            }
            Some(version) => {
                ensure!(version > 0, "version must be positive");
                ensure!(
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM revisions WHERE id = ?1 AND version = ?2)",
                        params![id, version],
                        |row| row.get::<_, bool>(0)
                    )?,
                    "unknown version: {id} v{version}"
                );
                let outcomes = tx.execute(
                    "DELETE FROM outcomes WHERE id = ?1 AND version = ?2",
                    params![id, version],
                )?;
                let revisions = tx.execute(
                    "DELETE FROM revisions WHERE id = ?1 AND version = ?2",
                    params![id, version],
                )?;
                let remaining: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM revisions WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )?;
                let skill_removed = remaining == 0;
                if skill_removed {
                    revisions::clear_fts_row(&tx, id)?;
                    tx.execute("DELETE FROM skills WHERE id = ?1", [id])?;
                } else {
                    refresh_fts(&tx, id)?;
                }
                PurgeReport {
                    revisions,
                    outcomes,
                    skill_removed,
                    compacted: false,
                }
            }
        };
        tx.commit()?;
        report.compacted = self.compact().unwrap_or(false);
        Ok(report)
    }

    /// Rewrites the database file and truncates the WAL. Returns whether the
    /// checkpoint completed; other connections' open read transactions make
    /// it report busy (and can make `VACUUM` fail outright).
    fn compact(&self) -> Result<bool> {
        self.conn.execute("VACUUM", [])?;
        let busy: i64 = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        Ok(busy == 0)
    }

    /// Serializes every skill, revision, and outcome as a versioned JSON
    /// document (excluding the hook-state tables, which are per-machine
    /// session bookkeeping, not vault content). Skills are ordered by id,
    /// revisions by version, and outcomes by version then creation time.
    pub fn export_json(&self) -> Result<String> {
        let exported_at: String =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now')", [], |row| {
                    row.get(0)
                })?;
        let mut skill_statement = self
            .conn
            .prepare("SELECT id, scope, deprecated FROM skills ORDER BY id")?;
        let mut revision_statement = self.conn.prepare(
            "SELECT id, version, description, tags, content, evidence, expected_version,
                    status, created_at, reviewed_at, review_note
             FROM revisions WHERE id = ?1 ORDER BY version",
        )?;
        let mut outcome_statement = self.conn.prepare(
            "SELECT id, version, result, note, project, created_at
             FROM outcomes WHERE id = ?1 ORDER BY version, created_at",
        )?;
        let skill_rows: Vec<(String, Option<String>, bool)> = skill_statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut skills = Vec::with_capacity(skill_rows.len());
        for (id, scope, deprecated) in skill_rows {
            let revisions = revision_statement
                .query_map([&id], |row| {
                    Ok(ExportRevision {
                        id: row.get(0)?,
                        version: row.get(1)?,
                        description: row.get(2)?,
                        tags: split_tags(&row.get::<_, String>(3)?),
                        content: row.get(4)?,
                        evidence: row.get(5)?,
                        expected_version: row.get(6)?,
                        status: row.get(7)?,
                        created_at: row.get(8)?,
                        reviewed_at: row.get(9)?,
                        review_note: row.get(10)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            let outcomes = outcome_statement
                .query_map([&id], |row| {
                    Ok(ExportOutcome {
                        id: row.get(0)?,
                        version: row.get(1)?,
                        result: row.get(2)?,
                        note: row.get(3)?,
                        project: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            skills.push(ExportSkill {
                id,
                scope,
                deprecated,
                revisions,
                outcomes,
            });
        }
        Ok(serde_json::to_string_pretty(&ExportDocument {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at,
            skills,
        })?)
    }

    /// Merges a document produced by `export_json` into this vault, in one
    /// IMMEDIATE transaction: any conflict aborts the whole import with no
    /// partial writes.
    ///
    /// A skill absent locally is created with its exported scope; a skill
    /// present locally must share that scope, or its whole import is
    /// refused, as is a skill listing a revision or outcome of another id.
    /// `deprecated` becomes the OR of the local and imported values.
    /// A revision that doesn't exist yet is validated exactly as `propose`
    /// validates a new one (sections, secrets) and inserted; one that exists
    /// with identical description/tags/content/evidence is skipped; one that
    /// exists but differs is a conflict that aborts the import. Outcomes are
    /// inserted, skipping duplicates under the vault's one-report-per-day
    /// index. Every imported skill's search row is then refreshed to its
    /// latest published revision.
    pub fn import_json(&mut self, json: &str) -> Result<ImportReport> {
        let document: ExportDocument =
            serde_json::from_str(json).context("parse export document")?;
        ensure!(
            document.format == EXPORT_FORMAT && document.version == EXPORT_VERSION,
            "unsupported export document (format {:?}, version {})",
            document.format,
            document.version
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = ImportReport {
            revisions_imported: 0,
            revisions_skipped: 0,
            outcomes_imported: 0,
            outcomes_skipped: 0,
        };
        for skill in &document.skills {
            validate_id(&skill.id)?;
            // A row filed under another id would bypass that skill's scope
            // check and leave its search row stale.
            for (kind, id) in skill
                .revisions
                .iter()
                .map(|r| ("revision", &r.id))
                .chain(skill.outcomes.iter().map(|o| ("outcome", &o.id)))
            {
                ensure!(
                    *id == skill.id,
                    "{kind} for skill {id} is listed under skill {}; aborting import",
                    skill.id
                );
            }
            let existing_scope: Option<Option<String>> = tx
                .query_row(
                    "SELECT scope FROM skills WHERE id = ?1",
                    [&skill.id],
                    |row| row.get(0),
                )
                .optional()?;
            match existing_scope {
                None => {
                    tx.execute(
                        "INSERT INTO skills (id, scope, deprecated) VALUES (?1, ?2, ?3)",
                        params![skill.id, skill.scope, skill.deprecated],
                    )?;
                }
                Some(scope) => {
                    ensure!(
                        scope == skill.scope,
                        "skill {} exists with a different scope; aborting import",
                        skill.id
                    );
                    if skill.deprecated {
                        tx.execute(
                            "UPDATE skills SET deprecated = 1 WHERE id = ?1",
                            [&skill.id],
                        )?;
                    }
                }
            }
            for revision in &skill.revisions {
                let existing: Option<(String, String, String, String)> = tx
                    .query_row(
                        "SELECT description, tags, content, evidence FROM revisions
                         WHERE id = ?1 AND version = ?2",
                        params![revision.id, revision.version],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()?;
                match existing {
                    None => {
                        validate_single_line("description", &revision.description, 280)?;
                        validate_tags(&revision.tags)?;
                        validate_text("content", &revision.content, 65_536)?;
                        // No validate_sections: revisions published before the heading
                        // rule existed are legitimate history and must round-trip.
                        validate_text("evidence", &revision.evidence, 16_384)?;
                        validate_no_secrets("description", &revision.description)?;
                        validate_no_secrets("content", &revision.content)?;
                        validate_no_secrets("evidence", &revision.evidence)?;
                        tx.execute(
                            "INSERT INTO revisions (id, version, description, tags, content, evidence,
                                expected_version, status, created_at, reviewed_at, review_note)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                            params![
                                revision.id,
                                revision.version,
                                revision.description,
                                join_tags(&revision.tags),
                                revision.content,
                                revision.evidence,
                                revision.expected_version,
                                revision.status,
                                revision.created_at,
                                revision.reviewed_at,
                                revision.review_note,
                            ],
                        )?;
                        report.revisions_imported += 1;
                    }
                    Some((description, tags, content, evidence)) => {
                        ensure!(
                            description == revision.description
                                && split_tags(&tags) == revision.tags
                                && content == revision.content
                                && evidence == revision.evidence,
                            "revision {} v{} conflicts with an existing revision with different content",
                            revision.id,
                            revision.version
                        );
                        report.revisions_skipped += 1;
                    }
                }
            }
            for outcome in &skill.outcomes {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO outcomes (id, version, result, note, project, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        outcome.id,
                        outcome.version,
                        outcome.result,
                        outcome.note,
                        outcome.project,
                        outcome.created_at,
                    ],
                )?;
                if inserted == 1 {
                    report.outcomes_imported += 1;
                } else {
                    report.outcomes_skipped += 1;
                }
            }
            refresh_fts(&tx, &skill.id)?;
        }
        tx.commit()?;
        Ok(report)
    }

    /// Writes a consistent copy of the live database to `dest` via
    /// `VACUUM INTO`, which snapshots the database (including any content
    /// still in the WAL) without needing exclusive access.
    pub fn backup(&self, dest: &Path) -> Result<()> {
        ensure!(!dest.exists(), "{} already exists", dest.display());
        if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let dest = dest
            .to_str()
            .context("destination path must be valid UTF-8")?;
        self.conn.execute("VACUUM INTO ?1", [dest])?;
        Ok(())
    }
}
