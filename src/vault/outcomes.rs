use super::{Vault, validate_id, validate_single_line, validate_text};
use anyhow::{Result, ensure};
use rusqlite::params;

pub const RESULTS: [&str; 3] = ["helped", "failed", "not_applicable"];

#[derive(Debug, serde::Serialize)]
pub struct OutcomeRecord {
    pub id: String,
    pub version: i64,
    pub result: String,
    pub note: String,
    pub client: Option<String>,
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
    pub fn record_outcome(
        &self,
        id: &str,
        version: i64,
        result: &str,
        note: &str,
        client: Option<&str>,
        project: Option<&str>,
    ) -> Result<OutcomeRecord> {
        validate_id(id)?;
        ensure!(version > 0, "version must be positive");
        ensure!(
            RESULTS.contains(&result),
            "result must be one of {}",
            RESULTS.join(", ")
        );
        validate_text("note", note, 2_048)?;
        if let Some(client) = client {
            validate_single_line("client", client, 128)?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let published: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM revisions WHERE id = ?1 AND version = ?2 AND status = 'published')",
            params![id, version],
            |row| row.get(0),
        )?;
        ensure!(published, "no published revision {id} version {version}");
        tx.execute(
            "INSERT INTO outcomes (id, version, result, note, client, project) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, version, result, note, client, project],
        )?;
        let record = tx.query_row(
            "SELECT id, version, result, note, client, project, created_at FROM outcomes WHERE rowid = last_insert_rowid()",
            [],
            outcome_row,
        )?;
        tx.commit()?;
        Ok(record)
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
        let mut statement = self.conn.prepare(
            "SELECT id, version, result, note, client, project, created_at FROM outcomes
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
        client: row.get(4)?,
        project: row.get(5)?,
        created_at: row.get(6)?,
    })
}
