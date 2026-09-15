use super::{Vault, split_tags};
use anyhow::{Result, ensure};
use rusqlite::named_params;

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
}

/// Reads a metadata row plus the `COUNT(*) OVER ()` total carried on every row.
fn metadata_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(SkillMetadata, i64)> {
    Ok((
        SkillMetadata {
            id: row.get(0)?,
            version: row.get(1)?,
            description: row.get(2)?,
            tags: split_tags(&row.get::<_, String>(3)?),
            scope: row.get(4)?,
            helped: row.get(5)?,
            failed: row.get(6)?,
        },
        row.get(7)?,
    ))
}

/// The total is carried on every row by the window function, so an empty
/// result (correctly) reports a total of 0.
fn split_hits(hits: Vec<(SkillMetadata, i64)>) -> (Vec<SkillMetadata>, i64) {
    let total = hits.first().map_or(0, |(_, total)| *total);
    (hits.into_iter().map(|(skill, _)| skill).collect(), total)
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
    pub fn search(&self, query: &str, project: Option<&str>, limit: i64) -> Result<SearchPage> {
        ensure!((1..=100).contains(&limit), "limit must be 1..100");
        ensure!(
            query.len() <= 512 && !query.contains('\0'),
            "query must be at most 512 UTF-8 bytes without NUL"
        );
        let (skills, total) = if query.trim().is_empty() {
            let hits = self
                .conn
                .prepare(
                    "SELECT c.id, c.version, c.description, c.tags, c.scope, c.helped, c.failed,
                            COUNT(*) OVER () AS total
                     FROM current_skills c
                     WHERE c.deprecated = 0 AND (c.scope IS NULL OR c.scope = :project)
                     ORDER BY c.helped - c.failed DESC, c.id
                     LIMIT :limit",
                )?
                .query_map(
                    named_params! {":project": project, ":limit": limit},
                    metadata_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            split_hits(hits)
        } else if let Some(expression) = match_expression(query) {
            let hits = self
                .conn
                .prepare(
                    "WITH hits AS (
                         SELECT id, bm25(skills_fts, 4.0, 3.0, 2.0, 1.0) AS rank
                         FROM skills_fts WHERE skills_fts MATCH :expression
                     )
                     SELECT c.id, c.version, c.description, c.tags, c.scope, c.helped, c.failed,
                            COUNT(*) OVER () AS total
                     FROM hits h JOIN current_skills c ON c.id = h.id
                     WHERE c.deprecated = 0 AND (c.scope IS NULL OR c.scope = :project)
                     ORDER BY h.rank, c.helped - c.failed DESC, c.id
                     LIMIT :limit",
                )?
                .query_map(
                    named_params! {":project": project, ":expression": expression, ":limit": limit},
                    metadata_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            split_hits(hits)
        } else {
            (Vec::new(), 0)
        };
        Ok(SearchPage { skills, total })
    }
}
