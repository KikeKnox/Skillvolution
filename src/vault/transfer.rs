//! Whole-vault operations: hard deletion (`purge`), JSON export/import, and
//! online backups.

use super::agent::{AgentMeta, agent_content_problems, parse_meta};
use super::{
    Vault, join_tags, revisions, split_tags, validate_id, validate_no_secrets,
    validate_single_line, validate_tags, validate_text,
};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

const EXPORT_FORMAT: &str = "skillvolution-export";
/// Skill-only exports stay version 1 (readable by older binaries); an export
/// containing any agent is version 2 so older binaries refuse it.
const EXPORT_VERSION: i64 = 1;
const EXPORT_VERSION_AGENTS: i64 = 2;

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
    /// Agents (any project, deprecated or not) whose current revision lists the purged
    /// skill in `skills:`; only filled when the skill stops being published.
    pub agents_listing: Vec<String>,
    /// `[warning]` lines for an agent whose current revision changed because its
    /// version was purged and whose new current revision lists unavailable skills.
    pub agent_warnings: Vec<String>,
    /// True when a published version of the purged skill remains.
    pub still_published: bool,
}

/// What `import_json` merged into the vault.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub revisions_imported: usize,
    pub revisions_skipped: usize,
    pub outcomes_imported: usize,
    pub outcomes_skipped: usize,
    /// Agents that gained at least one revision; their generated files are not touched,
    /// but `agents sync` would now rewrite them.
    pub agents_imported: Vec<String>,
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
    /// An agent revision's metadata; absent for skills and in older exports.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "strict_meta"
    )]
    meta: Option<serde_json::Value>,
}

const META_KEYS: [&str; 5] = ["role", "tier", "effort", "tools", "skills"];

/// `AgentMeta` tolerates unknown keys when read from the vault (rows from a newer
/// version), but an import must not silently drop what it does not understand.
fn strict_meta<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<serde_json::Value>, D::Error> {
    use serde::de::Error;
    let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if let Some(map) = value.as_object()
        && let Some(key) = map.keys().find(|k| !META_KEYS.contains(&k.as_str()))
    {
        return Err(D::Error::custom(format!(
            "unknown agent meta key `{key}` (this version knows {}); the export comes from a newer skillvolution",
            META_KEYS.join(", ")
        )));
    }
    serde_json::from_value::<AgentMeta>(value.clone()).map_err(D::Error::custom)?;
    Ok(Some(value))
}

fn skill_kind() -> String {
    "skill".to_owned()
}

fn is_skill(kind: &str) -> bool {
    kind == "skill"
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
    /// `skill` (left out, and the default for older exports) or `agent`.
    #[serde(default = "skill_kind", skip_serializing_if = "is_skill")]
    kind: String,
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
    // Agents are never indexed: they would skew the skills' bm25 statistics.
    let kind: Option<String> = tx
        .query_row("SELECT kind FROM skills WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    if kind.as_deref() == Some("agent") {
        return revisions::clear_fts_row(tx, id);
    }
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
    /// Rebuilds `skills_fts` from scratch in one IMMEDIATE transaction: clears it,
    /// then indexes every skill's latest published revision (deprecated skills
    /// included; `search` hides them at query time) and repoints `fts_rowid`.
    /// Returns how many skills ended up indexed. Used by `doctor --fix`.
    pub fn rebuild_search_index(&mut self) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM skills_fts", [])?;
        tx.execute("UPDATE skills SET fts_rowid = NULL", [])?;
        let ids: Vec<String> = tx
            .prepare("SELECT id FROM skills ORDER BY id")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in &ids {
            refresh_fts(&tx, id)?;
        }
        let indexed: i64 = tx.query_row(
            "SELECT COUNT(*) FROM skills WHERE fts_rowid IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(indexed as usize)
    }

    /// Permanently deletes one revision (`version = Some`) or a whole skill,
    /// including its outcomes, loads, and search-index row, in one IMMEDIATE
    /// transaction. `secure_delete` is turned on for this connection first so
    /// SQLite overwrites the freed pages instead of merely unlinking them,
    /// then `VACUUM` (and a WAL checkpoint) after commit rewrites the file so
    /// the purged content does not linger in either the main database file or
    /// the `-wal` file. That compaction is best effort, reported in
    /// `compacted`: the deletion is already committed by then.
    ///
    /// Refuses to remove a skill that a current agent revision lists in
    /// `skills:`; `purge_forced` skips that check.
    pub fn purge(&mut self, id: &str, version: Option<i64>) -> Result<PurgeReport> {
        self.purge_checked(id, version, false)
    }

    pub fn purge_forced(&mut self, id: &str, version: Option<i64>) -> Result<PurgeReport> {
        self.purge_checked(id, version, true)
    }

    fn purge_checked(
        &mut self,
        id: &str,
        version: Option<i64>,
        force: bool,
    ) -> Result<PurgeReport> {
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
        let removes_skill = version.is_none()
            || tx.query_row(
                "SELECT COUNT(*) FROM revisions
                 WHERE id = ?1 AND status = 'published' AND version != ?2",
                params![id, version],
                |row| row.get::<_, i64>(0),
            )? == 0;
        // Computed whenever the skill stops being published (even if superseded rows
        // remain) or its current version goes away: agents pin the current version.
        let removes_current = version.is_some_and(|v| {
            tx.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM revisions WHERE id = ?1 AND status = 'published'",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .is_ok_and(|current| current == v)
        });
        let users: Vec<String> = if removes_skill || removes_current {
            tx.prepare(
                "SELECT c.id FROM current_skills c, json_each(c.meta, '$.skills') j
                 WHERE c.kind = 'agent' AND j.value = ?1 ORDER BY c.id",
            )?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
        } else {
            Vec::new()
        };
        if !force {
            ensure!(
                users.is_empty(),
                "{id} is used by agent(s) {}; purging it (or its current version) breaks or re-pins them (pass --force to purge anyway)",
                users.join(", ")
            );
        }
        let mut report = match version {
            None => {
                tx.execute("DELETE FROM loads WHERE id = ?1", [id])?;
                let outcomes = tx.execute("DELETE FROM outcomes WHERE id = ?1", [id])?;
                let revisions = tx.execute("DELETE FROM revisions WHERE id = ?1", [id])?;
                revisions::clear_fts_row(&tx, id)?;
                tx.execute("DELETE FROM skills WHERE id = ?1", [id])?;
                PurgeReport {
                    revisions,
                    outcomes,
                    skill_removed: true,
                    compacted: false,
                    agents_listing: users,
                    agent_warnings: Vec::new(),
                    still_published: false,
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
                tx.execute(
                    "DELETE FROM loads WHERE id = ?1 AND version = ?2",
                    params![id, version],
                )?;
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
                let mut agent_warnings = Vec::new();
                if removes_current && !skill_removed {
                    let current: Option<(String, Option<String>, Option<String>)> = tx
                        .query_row(
                            "SELECT kind, scope, meta FROM current_skills WHERE id = ?1",
                            [id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?;
                    if let Some((kind, scope, meta)) = current
                        && kind == "agent"
                        && let Some(meta) = parse_meta(meta)?
                    {
                        for skill in &meta.skills {
                            if let Some(reason) =
                                super::agent::skill_unavailable(&tx, skill, scope.as_deref())?
                            {
                                agent_warnings.push(format!(
                                    "[warning] agent {id}: skill {skill} is not available ({reason})"
                                ));
                            }
                        }
                    }
                }
                PurgeReport {
                    revisions,
                    outcomes,
                    skill_removed,
                    compacted: false,
                    agents_listing: users,
                    agent_warnings,
                    still_published: !removes_skill,
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
    /// document (excluding the hook-state tables and `loads`, which are
    /// per-machine session bookkeeping and telemetry, not vault content). Skills are ordered by id,
    /// revisions by version, and outcomes by version then creation time.
    pub fn export_json(&self) -> Result<String> {
        let exported_at: String =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now')", [], |row| {
                    row.get(0)
                })?;
        let mut skill_statement = self
            .conn
            .prepare("SELECT id, scope, deprecated, kind FROM skills ORDER BY id")?;
        let mut revision_statement = self.conn.prepare(
            "SELECT id, version, description, tags, content, evidence, expected_version,
                    status, created_at, reviewed_at, review_note, meta
             FROM revisions WHERE id = ?1 ORDER BY version",
        )?;
        let mut outcome_statement = self.conn.prepare(
            "SELECT id, version, result, note, project, created_at
             FROM outcomes WHERE id = ?1 ORDER BY version, created_at",
        )?;
        let skill_rows: Vec<(String, Option<String>, bool, String)> = skill_statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut skills = Vec::with_capacity(skill_rows.len());
        for (id, scope, deprecated, kind) in skill_rows {
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
                        meta: row
                            .get::<_, Option<String>>(11)?
                            .map(|text| serde_json::from_str(&text))
                            .transpose()
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    11,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
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
                kind,
                revisions,
                outcomes,
            });
        }
        Ok(serde_json::to_string_pretty(&ExportDocument {
            format: EXPORT_FORMAT.to_owned(),
            version: if skills.iter().any(|s| s.kind == "agent") {
                EXPORT_VERSION_AGENTS
            } else {
                EXPORT_VERSION
            },
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
    /// exists but differs is a conflict that aborts the import. A `draft` or
    /// `rejected` revision (exports from before schema v5) is inserted as
    /// `superseded` with its note prefixed `legacy <status>`. Outcomes are
    /// inserted, skipping duplicates under the vault's one-report-per-day
    /// index. Every imported skill's search row is then refreshed to its
    /// latest published revision.
    pub fn import_json(&mut self, json: &str) -> Result<ImportReport> {
        let document: ExportDocument =
            serde_json::from_str(json).context("parse export document")?;
        ensure!(
            document.format == EXPORT_FORMAT
                && (document.version == EXPORT_VERSION
                    || document.version == EXPORT_VERSION_AGENTS),
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
            agents_imported: Vec::new(),
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
            ensure!(
                matches!(skill.kind.as_str(), "skill" | "agent"),
                "skill {} has unknown kind {:?}; aborting import",
                skill.id,
                skill.kind
            );
            if skill.kind == "agent"
                && let Some(reason) = super::reserved_agent_id(&skill.id)
            {
                bail!(
                    "agent id {} is reserved: {reason}; aborting import",
                    skill.id
                );
            }
            let existing: Option<(Option<String>, String)> = tx
                .query_row(
                    "SELECT scope, kind FROM skills WHERE id = ?1",
                    [&skill.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match existing {
                None => {
                    tx.execute(
                        "INSERT INTO skills (id, scope, deprecated, kind) VALUES (?1, ?2, ?3, ?4)",
                        params![skill.id, skill.scope, skill.deprecated, skill.kind],
                    )?;
                }
                Some((scope, kind)) => {
                    ensure!(
                        scope == skill.scope,
                        "skill {} exists with a different scope; aborting import",
                        skill.id
                    );
                    let article = |kind: &str| {
                        if kind == "agent" {
                            "an agent"
                        } else {
                            "a skill"
                        }
                    };
                    ensure!(
                        kind == skill.kind,
                        "{} already exists as {}; aborting import (the import file has it as {})",
                        skill.id,
                        article(&kind),
                        article(&skill.kind)
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
                ensure!(
                    (skill.kind == "agent") == revision.meta.is_some(),
                    "revision {} v{}: agent revisions need meta and skill revisions must not have it; aborting import",
                    revision.id,
                    revision.version
                );
                let meta: Option<AgentMeta> = revision
                    .meta
                    .clone()
                    .map(serde_json::from_value)
                    .transpose()?;
                let existing: Option<(String, String, String, String, Option<String>)> = tx
                    .query_row(
                        "SELECT description, tags, content, evidence, meta FROM revisions
                         WHERE id = ?1 AND version = ?2",
                        params![revision.id, revision.version],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
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
                        if let Some(meta) = &meta {
                            let problems = agent_content_problems(&revision.content, meta);
                            ensure!(
                                problems.is_empty(),
                                "revision {} v{}: {}",
                                revision.id,
                                revision.version,
                                problems.join("; ")
                            );
                        }
                        // Exports from before schema v5 can carry draft or rejected
                        // revisions; they land superseded, as the v5 migration maps them.
                        let (status, review_note) = match revision.status.as_str() {
                            legacy @ ("draft" | "rejected") => (
                                "superseded",
                                Some(match &revision.review_note {
                                    Some(note) => format!("legacy {legacy}: {note}"),
                                    None => format!("legacy {legacy}"),
                                }),
                            ),
                            status => (status, revision.review_note.clone()),
                        };
                        tx.execute(
                            "INSERT INTO revisions (id, version, description, tags, content, evidence,
                                expected_version, status, created_at, reviewed_at, review_note, meta)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                            params![
                                revision.id,
                                revision.version,
                                revision.description,
                                join_tags(&revision.tags),
                                revision.content,
                                revision.evidence,
                                revision.expected_version,
                                status,
                                revision.created_at,
                                revision.reviewed_at,
                                review_note,
                                meta.as_ref().map(serde_json::to_string).transpose()?,
                            ],
                        )?;
                        report.revisions_imported += 1;
                        if skill.kind == "agent" && !report.agents_imported.contains(&skill.id) {
                            report.agents_imported.push(skill.id.clone());
                        }
                    }
                    Some((description, tags, content, evidence, stored_meta)) => {
                        ensure!(
                            description == revision.description
                                && split_tags(&tags) == revision.tags
                                && content == revision.content
                                && evidence == revision.evidence
                                && parse_meta(stored_meta)? == meta,
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
