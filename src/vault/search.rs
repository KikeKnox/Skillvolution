use super::{Vault, split_tags};
use anyhow::{Result, ensure};
use rusqlite::named_params;

const VISIBLE: &str = "c.deprecated = 0 AND (c.scope IS NULL OR c.scope = :project)";
/// Text relevance as a nonnegative number: `bm25` scores are negative (more
/// negative = better match), so the sign is flipped once, here, and every
/// query below reads "larger strength is a better match". The clamp makes the
/// direction independent of that sign convention, so scaling by the outcome
/// factor can never invert the ranking.
const STRENGTH: &str = "MAX(-bm25(skills_fts, 4.0, 3.0, 2.0, 1.0), 0.0)";
/// Relevance scaled by the current version's track record. With
/// `net = helped - failed`, the factor `1 + net / (ABS(net) + 2.0)` stays in
/// (0, 2): exactly 1.0 when `helped = failed` (an unreported skill ranks purely
/// on text), 0.5 at two net failures, and at most 2.0 for a proven skill, so
/// history can demote a match but never replace relevance. The `2.0` also
/// forces real division: with `2` this would be integer division and the whole
/// penalty would silently truncate to zero.
const RANKED: &str =
    "h.strength * (1.0 + (c.helped - c.failed) / (ABS(c.helped - c.failed) + 2.0))";

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
/// page only tells us the true total when it's genuinely empty: an offset
/// past the last row also yields zero rows, hiding the real total, so the
/// caller falls back to a plain COUNT(*) whenever the page came back empty
/// with a nonzero offset.
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
        let (skills, total) = if query.trim().is_empty() {
            let hits = self
                .conn
                .prepare(&format!(
                    "SELECT c.id, c.version, c.description, c.tags, c.scope, c.helped, c.failed,
                            COUNT(*) OVER () AS total
                     FROM current_skills c
                     WHERE {VISIBLE}
                     ORDER BY c.helped - c.failed DESC, c.id
                     LIMIT :limit OFFSET :offset"
                ))?
                .query_map(
                    named_params! {":project": project, ":limit": limit, ":offset": offset},
                    metadata_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let (skills, total) = split_hits(hits);
            if skills.is_empty() && offset > 0 {
                let total = self.conn.query_row(
                    &format!("SELECT COUNT(*) FROM current_skills c WHERE {VISIBLE}"),
                    named_params! {":project": project},
                    |row| row.get(0),
                )?;
                (skills, total)
            } else {
                (skills, total)
            }
        } else if let Some(expression) = match_expression(query) {
            let hits = self
                .conn
                .prepare(&format!(
                    "WITH hits AS (
                         SELECT id, {STRENGTH} AS strength
                         FROM skills_fts WHERE skills_fts MATCH :expression
                     )
                     SELECT c.id, c.version, c.description, c.tags, c.scope, c.helped, c.failed,
                            COUNT(*) OVER () AS total
                     FROM hits h JOIN current_skills c ON c.id = h.id
                     WHERE {VISIBLE}
                     ORDER BY {RANKED} DESC, c.helped - c.failed DESC, c.id
                     LIMIT :limit OFFSET :offset"
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
            let (skills, total) = split_hits(hits);
            if skills.is_empty() && offset > 0 {
                let total = self.conn.query_row(
                    &format!(
                        "WITH hits AS (
                             SELECT id, {STRENGTH} AS strength
                             FROM skills_fts WHERE skills_fts MATCH :expression
                         )
                         SELECT COUNT(*) FROM hits h JOIN current_skills c ON c.id = h.id
                         WHERE {VISIBLE}"
                    ),
                    named_params! {":project": project, ":expression": expression},
                    |row| row.get(0),
                )?;
                (skills, total)
            } else {
                (skills, total)
            }
        } else {
            (Vec::new(), 0)
        };
        let has_more = total.saturating_sub(offset) > skills.len() as i64;
        Ok(SearchPage {
            skills,
            total,
            has_more,
        })
    }
}
