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
    /// Distinct (project, UTC day) pairs in which `get_skill` loaded this version.
    pub loaded: i64,
    /// Those load pairs with no outcome report for this version from the same
    /// project and day.
    pub unreported: i64,
}

impl Vault {
    /// Records that `get_skill` returned `id` v`version` to `project`, at most
    /// once per revision, project and UTC day (later loads that day are
    /// ignored). Local telemetry for `outcomes`; never exported.
    pub fn record_load(&self, id: &str, version: i64, project: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO loads (id, version, project) VALUES (?1, ?2, ?3)",
            params![id, version, project],
        )?;
        Ok(())
    }

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

    /// One summary per current skill. `failing_only` keeps skills whose
    /// current version has failures, `unreported_only` those with loads that
    /// were never reported (both may be combined).
    pub fn outcome_summaries(
        &self,
        failing_only: bool,
        unreported_only: bool,
    ) -> Result<Vec<OutcomeSummary>> {
        let mut statement = self.conn.prepare(
            "SELECT id, version, scope, helped, failed, not_applicable, loaded, unreported FROM (
                 SELECT c.id, c.version, c.scope, c.helped, c.failed, c.not_applicable,
                     (SELECT COUNT(*) FROM loads l
                      WHERE l.id = c.id AND l.version = c.version) AS loaded,
                     (SELECT COUNT(*) FROM loads l
                      WHERE l.id = c.id AND l.version = c.version
                        AND NOT EXISTS (
                            SELECT 1 FROM outcomes o
                            WHERE o.id = l.id AND o.version = l.version
                              AND COALESCE(o.project, '') = COALESCE(l.project, '')
                              AND substr(o.created_at, 1, 10) = substr(l.created_at, 1, 10)
                        )) AS unreported
                 FROM current_skills c
             )
             WHERE (?1 = 0 OR failed > 0) AND (?2 = 0 OR unreported > 0)
             ORDER BY failed DESC, helped DESC, id",
        )?;
        Ok(statement
            .query_map(params![failing_only, unreported_only], |row| {
                Ok(OutcomeSummary {
                    id: row.get(0)?,
                    version: row.get(1)?,
                    scope: row.get(2)?,
                    helped: row.get(3)?,
                    failed: row.get(4)?,
                    not_applicable: row.get(5)?,
                    loaded: row.get(6)?,
                    unreported: row.get(7)?,
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
