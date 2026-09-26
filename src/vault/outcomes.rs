use super::{Vault, validate_id, validate_no_secrets, validate_text};
use anyhow::{Result, ensure};
use rusqlite::params;

const RESULTS: [&str; 3] = ["helped", "failed", "not_applicable"];

#[derive(Debug, serde::Serialize)]
pub struct OutcomeRecord {
    pub id: String,
    pub version: i64,
    pub result: String,
    pub note: String,
    pub project: Option<String>,
    pub created_at: String,
}

#[derive(Debug, serde::Serialize)]
pub struct OutcomeSummary {
    pub id: String,
    pub version: i64,
    pub scope: Option<String>,
    pub helped: i64,
    pub failed: i64,
    pub not_applicable: i64,
}

impl Vault {
    /// Records an outcome in one statement: the INSERT only fires when the
    /// revision is published and visible to `project`, so `changed == 1` is
    /// both the write and the visibility check.
    ///
    /// A revision keeps at most one report per project and UTC day; a repeat
    /// report that day replaces the earlier one's result and note (last report
    /// wins) instead of failing, so retries and changed minds are harmless.
    pub fn record_outcome(
        &self,
        id: &str,
        version: i64,
        result: &str,
        note: &str,
        project: Option<&str>,
    ) -> Result<()> {
        validate_id(id)?;
        ensure!(version > 0, "version must be positive");
        ensure!(
            RESULTS.contains(&result),
            "result must be one of {}",
            RESULTS.join(", ")
        );
        validate_text("note", note, 2_048)?;
        validate_no_secrets("note", note)?;
        let changed = self.conn.execute(
            "INSERT INTO outcomes (id, version, result, note, project)
             SELECT ?1, ?2, ?3, ?4, ?5
             WHERE EXISTS (
                 SELECT 1 FROM revisions r JOIN skills s ON s.id = r.id
                 WHERE r.id = ?1 AND r.version = ?2 AND r.status = 'published'
                   AND (s.scope IS NULL OR s.scope = ?5)
             )
             ON CONFLICT DO UPDATE SET
                 result = excluded.result, note = excluded.note, created_at = excluded.created_at",
            params![id, version, result, note, project],
        )?;
        ensure!(
            changed == 1,
            "no published revision {id} version {version} visible to this project"
        );
        Ok(())
    }

    pub fn outcome_summaries(&self, failing_only: bool) -> Result<Vec<OutcomeSummary>> {
        let mut statement = self.conn.prepare(
            "SELECT id, version, scope, helped, failed, not_applicable FROM current_skills
             WHERE ?1 = 0 OR failed > 0
             ORDER BY failed DESC, helped DESC, id",
        )?;
        Ok(statement
            .query_map([failing_only], |row| {
                Ok(OutcomeSummary {
                    id: row.get(0)?,
                    version: row.get(1)?,
                    scope: row.get(2)?,
                    helped: row.get(3)?,
                    failed: row.get(4)?,
                    not_applicable: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn outcomes(&self, id: &str) -> Result<Vec<OutcomeRecord>> {
        validate_id(id)?;
        let known: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM skills WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )?;
        ensure!(known, "unknown skill: {id}");
        let mut statement = self.conn.prepare(
            "SELECT id, version, result, note, project, created_at FROM outcomes
             WHERE id = ?1 ORDER BY rowid DESC",
        )?;
        Ok(statement
            .query_map([id], outcome_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn outcome_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutcomeRecord> {
    Ok(OutcomeRecord {
        id: row.get(0)?,
        version: row.get(1)?,
        result: row.get(2)?,
        note: row.get(3)?,
        project: row.get(4)?,
        created_at: row.get(5)?,
    })
}
