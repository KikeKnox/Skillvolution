use super::{Vault, split_tags};
use anyhow::{Result, ensure};
use rusqlite::named_params;

const METADATA: &str = "c.id, c.version, c.description, c.tags, c.scope, c.helped, c.failed";
const VISIBLE: &str = "c.deprecated = 0 AND (c.scope IS NULL OR c.scope = :project)";
const HITS: &str = "WITH hits AS MATERIALIZED (
    SELECT id, bm25(skills_fts, 4.0, 3.0, 2.0, 1.0) AS rank
    FROM skills_fts WHERE skills_fts MATCH :expression
)";

#[derive(Debug, serde::Serialize)]
pub struct SkillMetadata {
    pub id: String,
    pub version: i64,
    pub description: String,
    pub tags: Vec<String>,
    pub scope: Option<String>,
    pub helped: i64,
    pub failed: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchPage {
    pub skills: Vec<SkillMetadata>,
    pub total: i64,
    pub has_more: bool,
}

fn metadata_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SkillMetadata> {
    Ok(SkillMetadata {
        id: row.get(0)?,
        version: row.get(1)?,
        description: row.get(2)?,
        tags: split_tags(&row.get::<_, String>(3)?),
        scope: row.get(4)?,
        helped: row.get(5)?,
        failed: row.get(6)?,
    })
}

/// Turns free text into an FTS5 expression of quoted prefix terms joined by OR,
/// so user input can never inject FTS syntax.
fn match_expression(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"*", term.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

impl Vault {
    pub fn search(
        &self,
        query: &str,
        project: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<SearchPage> {
        ensure!((1..=100).contains(&limit), "limit must be 1..100");
        ensure!(offset >= 0, "offset must be nonnegative");
        ensure!(
            query.len() <= 512 && !query.contains('\0'),
            "query must be at most 512 UTF-8 bytes without NUL"
        );
        let tx = self.conn.unchecked_transaction()?;
        let (total, skills): (i64, Vec<SkillMetadata>) = if query.trim().is_empty() {
            let total = tx.query_row(
                &format!("SELECT COUNT(*) FROM current_skills c WHERE {VISIBLE}"),
                named_params! {":project": project},
                |row| row.get(0),
            )?;
            let skills = tx
                .prepare(&format!(
                    "SELECT {METADATA} FROM current_skills c WHERE {VISIBLE}
                     ORDER BY c.helped - c.failed DESC, c.id LIMIT :limit OFFSET :offset"
                ))?
                .query_map(
                    named_params! {":project": project, ":limit": limit, ":offset": offset},
                    metadata_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (total, skills)
        } else if let Some(expression) = match_expression(query) {
            let total = tx.query_row(
                &format!(
                    "{HITS} SELECT COUNT(*) FROM hits h JOIN current_skills c ON c.id = h.id
                     WHERE {VISIBLE}"
                ),
                named_params! {":project": project, ":expression": expression},
                |row| row.get(0),
            )?;
            let skills = tx
                .prepare(&format!(
                    "{HITS} SELECT {METADATA} FROM hits h JOIN current_skills c ON c.id = h.id
                     WHERE {VISIBLE}
                     ORDER BY h.rank, c.helped - c.failed DESC, c.id LIMIT :limit OFFSET :offset"
                ))?
                .query_map(
                    named_params! {
                        ":project": project,
                        ":expression": expression,
                        ":limit": limit,
                        ":offset": offset,
                    },
                    metadata_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (total, skills)
        } else {
            (0, Vec::new())
        };
        tx.commit()?;
        let has_more = total.saturating_sub(offset) > skills.len() as i64;
        Ok(SearchPage {
            skills,
            total,
            has_more,
        })
    }
}
