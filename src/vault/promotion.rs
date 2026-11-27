//! When a global-scope vault agent has earned a per-user install: the thresholds (compiled
//! defaults, overridable in `vault_meta` with `agents promotion`), the pure evaluation
//! over usage events, and the agents already promoted.
//!
//! An agent is a candidate when, within the window, it was used in at least `min_share`
//! percent of the active projects (projects with any event in the window), in at least
//! `min_projects` distinct projects, with a mean of at least `min_delegations`
//! delegations per using project. In a project with an `agent_install` record of that
//! agent, only the delegations after its first install record count; in a project with
//! none (its install predates the events, or the record was pruned) every delegation in
//! the window counts, since a client cannot delegate to a file that does not exist. A
//! project "uses" an agent when at least one of its delegations counts.

use super::Vault;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const WINDOW_KEY: &str = "promotion_window_days";
const SHARE_KEY: &str = "promotion_min_share";
const PROJECTS_KEY: &str = "promotion_min_projects";
const DELEGATIONS_KEY: &str = "promotion_min_delegations";
/// The agents promoted so far, a JSON array of ids.
const PROMOTED_KEY: &str = "agents_promoted";
/// The UTC day of the last automatic evaluation.
const EVALUATED_KEY: &str = "agents_promotion_on";

/// Longest window accepted: events are kept 180 days.
pub const MAX_WINDOW_DAYS: i64 = 180;

/// The promotion thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PromotionSettings {
    pub window_days: i64,
    /// Percent of the active projects that must use the agent.
    pub min_share: i64,
    pub min_projects: i64,
    /// Mean delegations per using project.
    pub min_delegations: i64,
}

impl Default for PromotionSettings {
    fn default() -> Self {
        Self {
            window_days: 30,
            min_share: 75,
            min_projects: 6,
            min_delegations: 3,
        }
    }
}

impl PromotionSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_WINDOW_DAYS).contains(&self.window_days),
            "the window must be 1 to {MAX_WINDOW_DAYS} days (events are kept {MAX_WINDOW_DAYS} days)"
        );
        ensure!(
            (1..=100).contains(&self.min_share),
            "--min-share must be a percentage from 1 to 100"
        );
        ensure!(self.min_projects >= 1, "--min-projects must be at least 1");
        ensure!(
            self.min_delegations >= 1,
            "--min-delegations must be at least 1"
        );
        Ok(())
    }
}

/// One event, as the evaluation reads it. Only events with a project matter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRow {
    pub id: i64,
    /// UTC, `%Y-%m-%dT%H:%M:%SZ` (compares as text).
    pub at: String,
    pub kind: String,
    pub project: String,
    pub subject: Option<String>,
}

/// An agent that meets every threshold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub id: String,
    /// Projects that delegated to it after installing it.
    pub projects: usize,
    /// Active projects in the window.
    pub active: usize,
    /// Delegations counted over those projects.
    pub delegations: usize,
}

impl Candidate {
    pub fn mean_delegations(&self) -> f64 {
        self.delegations as f64 / self.projects.max(1) as f64
    }

    /// `reviewer (6/7 active projects, 4.2 delegations each)`.
    pub fn render(&self) -> String {
        format!(
            "{} ({}/{} active projects, {:.1} delegations each)",
            self.id,
            self.projects,
            self.active,
            self.mean_delegations()
        )
    }
}

/// The evaluation: active projects in the window and the agents that pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub active: usize,
    pub candidates: Vec<Candidate>,
}

/// Evaluates `rows` against `settings`: `since` is the window's UTC start, `eligible`
/// says which agent ids may be promoted (global-scope vault agents not promoted yet).
/// `agent_install` rows count whenever they happened; every other row only within the
/// window. A delegation counts when it follows the project's first install record of the
/// agent, or when the project has none. Candidates are sorted by id.
pub fn evaluate(
    rows: &[UsageRow],
    since: &str,
    settings: &PromotionSettings,
    eligible: &dyn Fn(&str) -> bool,
) -> Evaluation {
    let in_window = |row: &UsageRow| row.at.as_str() >= since;
    let active: BTreeSet<&str> = rows
        .iter()
        .filter(|row| in_window(row))
        .map(|row| row.project.as_str())
        .collect();
    // The first install of each agent in each project, as (at, id) so ties keep order.
    let mut installs: HashMap<(&str, &str), (&str, i64)> = HashMap::new();
    for row in rows.iter().filter(|row| row.kind == "agent_install") {
        let Some(agent) = row.subject.as_deref() else {
            continue;
        };
        let key = (agent, row.project.as_str());
        let at = (row.at.as_str(), row.id);
        installs
            .entry(key)
            .and_modify(|first| *first = (*first).min(at))
            .or_insert(at);
    }
    // agent -> project -> delegations after that project's install
    let mut usage: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| row.kind == "delegation" && in_window(row))
    {
        let Some(agent) = row.subject.as_deref().filter(|id| eligible(id)) else {
            continue;
        };
        let project = row.project.as_str();
        let after_install = installs
            .get(&(agent, project))
            .is_none_or(|installed| (row.at.as_str(), row.id) > *installed);
        if after_install {
            *usage.entry(agent).or_default().entry(project).or_default() += 1;
        }
    }
    let active_count = active.len();
    let candidates = usage
        .into_iter()
        .filter_map(|(agent, projects)| {
            let candidate = Candidate {
                id: agent.to_owned(),
                projects: projects.len(),
                active: active_count,
                delegations: projects.values().sum(),
            };
            passes(&candidate, settings).then_some(candidate)
        })
        .collect();
    Evaluation {
        active: active_count,
        candidates,
    }
}

/// Integer arithmetic, so exactly the threshold share and mean pass.
fn passes(candidate: &Candidate, settings: &PromotionSettings) -> bool {
    let projects = candidate.projects as i64;
    projects >= settings.min_projects
        && projects * 100 >= settings.min_share * candidate.active as i64
        && candidate.delegations as i64 >= settings.min_delegations * projects
}

/// Everything `agents promotion` and `doctor` show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionStatus {
    pub settings: PromotionSettings,
    pub active: usize,
    pub candidates: Vec<Candidate>,
    /// Promoted agents, sorted.
    pub promoted: Vec<String>,
}

fn meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM vault_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// The stored thresholds over the compiled defaults; an unparsable value is ignored.
pub fn promotion_settings(conn: &Connection) -> Result<PromotionSettings> {
    let mut settings = PromotionSettings::default();
    for (key, field) in [
        (WINDOW_KEY, &mut settings.window_days),
        (SHARE_KEY, &mut settings.min_share),
        (PROJECTS_KEY, &mut settings.min_projects),
        (DELEGATIONS_KEY, &mut settings.min_delegations),
    ] {
        if let Some(value) = meta(conn, key)?.and_then(|v| v.parse().ok()) {
            *field = value;
        }
    }
    Ok(settings)
}

/// The promoted agent ids, sorted.
pub fn promoted_agents(conn: &Connection) -> Result<Vec<String>> {
    let mut ids: Vec<String> = meta(conn, PROMOTED_KEY)?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The evaluation now, from a connection (`doctor` reads a database it must not write).
pub fn promotion_status(conn: &Connection) -> Result<PromotionStatus> {
    let settings = promotion_settings(conn)?;
    let promoted = promoted_agents(conn)?;
    let since: String = conn.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)",
        [format!("-{} days", settings.window_days)],
        |row| row.get(0),
    )?;
    let global_agents: BTreeSet<String> = conn
        .prepare(
            "SELECT id FROM current_skills WHERE kind = 'agent' AND scope IS NULL AND deprecated = 0",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    // Installs whenever they happened; delegations from vault agents and one row per
    // active project within the window.
    let rows: Vec<UsageRow> = conn
        .prepare(
            "SELECT id, at, kind, project, subject FROM events
             WHERE project IS NOT NULL AND (
                 (kind = 'agent_install' AND subject IS NOT NULL)
                 OR (at >= ?1 AND kind = 'delegation' AND subject IS NOT NULL
                     AND json_extract(data, '$.source') = 'vault')
                 OR id IN (SELECT MIN(id) FROM events
                           WHERE at >= ?1 AND project IS NOT NULL GROUP BY project))",
        )?
        .query_map([&since], |row| {
            Ok(UsageRow {
                id: row.get(0)?,
                at: row.get(1)?,
                kind: row.get(2)?,
                project: row.get(3)?,
                subject: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let eligible = |id: &str| {
        global_agents.contains(id) && promoted.binary_search_by(|p| p.as_str().cmp(id)).is_err()
    };
    let evaluation = evaluate(&rows, &since, &settings, &eligible);
    Ok(PromotionStatus {
        settings,
        active: evaluation.active,
        candidates: evaluation.candidates,
        promoted,
    })
}

impl Vault {
    pub fn promotion_settings(&self) -> Result<PromotionSettings> {
        promotion_settings(&self.conn)
    }

    /// Stores `settings` after validating them.
    pub fn set_promotion_settings(&self, settings: &PromotionSettings) -> Result<()> {
        settings.validate()?;
        for (key, value) in [
            (WINDOW_KEY, settings.window_days),
            (SHARE_KEY, settings.min_share),
            (PROJECTS_KEY, settings.min_projects),
            (DELEGATIONS_KEY, settings.min_delegations),
        ] {
            set_meta(&self.conn, key, &value.to_string())?;
        }
        Ok(())
    }

    pub fn promotion_status(&self) -> Result<PromotionStatus> {
        promotion_status(&self.conn)
    }

    pub fn promoted_agents(&self) -> Result<Vec<String>> {
        promoted_agents(&self.conn)
    }

    /// Adds `id` to the promoted agents.
    pub fn mark_promoted(&self, id: &str) -> Result<()> {
        let mut ids = self.promoted_agents()?;
        if !ids.iter().any(|p| p == id) {
            ids.push(id.to_owned());
            ids.sort();
            set_meta(&self.conn, PROMOTED_KEY, &serde_json::to_string(&ids)?)?;
        }
        Ok(())
    }

    /// Whether the automatic evaluation has yet to run today (UTC), claiming today's run
    /// when it has: at most one evaluation a day, like events pruning. A primary-key read
    /// answers most calls without the write lock; the claim itself is one conditional
    /// upsert, so of two concurrent sessions only the one that changed the row runs it.
    pub fn claim_promotion_run(&self) -> Result<bool> {
        let done_today: bool = self.conn.query_row(
            "SELECT COALESCE((SELECT value FROM vault_meta WHERE key = ?1), '')
                    = strftime('%Y-%m-%d', 'now')",
            [EVALUATED_KEY],
            |row| row.get(0),
        )?;
        if done_today {
            return Ok(false);
        }
        self.claim_today()
    }

    /// Sets the evaluation day to today in one statement, `true` only when this call
    /// changed it.
    fn claim_today(&self) -> Result<bool> {
        let claimed = self.conn.execute(
            "INSERT INTO vault_meta (key, value) VALUES (?1, strftime('%Y-%m-%d', 'now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value
             WHERE vault_meta.value <> excluded.value",
            [EVALUATED_KEY],
        )?;
        Ok(claimed == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINCE: &str = "2026-11-01T00:00:00Z";

    fn row(id: i64, at: &str, kind: &str, project: &str, subject: Option<&str>) -> UsageRow {
        UsageRow {
            id,
            at: at.to_owned(),
            kind: kind.to_owned(),
            project: project.to_owned(),
            subject: subject.map(str::to_owned),
        }
    }

    /// `projects` projects that install `agent` and then delegate `delegations` times
    /// each, plus `idle` active projects that never use it.
    fn usage(projects: usize, delegations: usize, idle: usize) -> Vec<UsageRow> {
        let mut rows = Vec::new();
        let mut id = 0;
        let mut next = || {
            id += 1;
            id
        };
        for p in 0..projects {
            let project = format!("p{p}");
            rows.push(row(
                next(),
                "2026-11-02T00:00:00Z",
                "agent_install",
                &project,
                Some("reviewer"),
            ));
            for _ in 0..delegations {
                rows.push(row(
                    next(),
                    "2026-11-03T00:00:00Z",
                    "delegation",
                    &project,
                    Some("reviewer"),
                ));
            }
        }
        for p in 0..idle {
            rows.push(row(
                next(),
                "2026-11-03T00:00:00Z",
                "session_start",
                &format!("idle{p}"),
                None,
            ));
        }
        rows
    }

    fn ids(rows: &[UsageRow]) -> Vec<String> {
        evaluate(rows, SINCE, &PromotionSettings::default(), &|_| true)
            .candidates
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    #[test]
    fn exactly_the_minimum_share_passes_and_just_below_does_not() {
        // 6 of 8 active projects = 75%.
        assert_eq!(ids(&usage(6, 3, 2)), ["reviewer"]);
        // 6 of 9 = 66.7%.
        assert!(ids(&usage(6, 3, 3)).is_empty());
        // 9 of 12 = 75% again.
        assert_eq!(ids(&usage(9, 3, 3)), ["reviewer"]);
    }

    #[test]
    fn five_projects_are_not_enough_and_six_are() {
        assert!(ids(&usage(5, 3, 0)).is_empty(), "more than 5 are required");
        assert_eq!(ids(&usage(6, 3, 0)), ["reviewer"]);
    }

    #[test]
    fn the_mean_delegations_threshold_is_inclusive() {
        assert!(ids(&usage(6, 2, 0)).is_empty());
        assert_eq!(ids(&usage(6, 3, 0)), ["reviewer"]);
        // 17 delegations over 6 projects: mean 2.83.
        let mut rows = usage(6, 3, 0);
        let last = rows.iter().rposition(|r| r.kind == "delegation").unwrap();
        rows.remove(last);
        assert!(ids(&rows).is_empty());
    }

    #[test]
    fn delegations_before_the_install_are_not_counted() {
        let mut rows = usage(6, 2, 0);
        // A third delegation per project, but before that project's install.
        for p in 0..6 {
            rows.push(row(
                100 + p,
                "2026-11-01T12:00:00Z",
                "delegation",
                &format!("p{p}"),
                Some("reviewer"),
            ));
        }
        assert!(ids(&rows).is_empty());
        // In the same second, the event order decides.
        let mut rows = usage(6, 2, 0);
        for p in 0..6 {
            rows.push(row(
                200 + p,
                "2026-11-02T00:00:00Z",
                "delegation",
                &format!("p{p}"),
                Some("reviewer"),
            ));
        }
        assert_eq!(ids(&rows), ["reviewer"]);
    }

    #[test]
    fn a_project_without_an_install_record_counts_every_delegation_in_the_window() {
        // No install record at all (installed before v0.7, or the record was pruned).
        let mut rows = usage(6, 3, 0);
        rows.retain(|r| r.kind != "agent_install");
        assert_eq!(ids(&rows), ["reviewer"]);
        // One project without a record, its delegations early in the window.
        let mut rows = usage(6, 3, 0);
        rows.retain(|r| !(r.kind == "agent_install" && r.project == "p0"));
        for r in rows.iter_mut().filter(|r| r.project == "p0") {
            r.at = "2026-11-01T00:00:00Z".to_owned();
        }
        let evaluation = evaluate(&rows, SINCE, &PromotionSettings::default(), &|_| true);
        assert_eq!(evaluation.candidates[0].projects, 6);
        assert_eq!(evaluation.candidates[0].delegations, 18);
        // Delegations before the window still do not count.
        for r in rows.iter_mut().filter(|r| r.project == "p0") {
            r.at = "2026-10-01T00:00:00Z".to_owned();
        }
        assert!(ids(&rows).is_empty(), "p0 no longer uses it");
    }

    #[test]
    fn a_project_with_an_install_record_counts_only_later_delegations() {
        let mut rows = usage(6, 3, 0);
        // p0's delegations all happened before its (re)install record.
        for r in rows
            .iter_mut()
            .filter(|r| r.project == "p0" && r.kind == "delegation")
        {
            r.at = "2026-11-01T12:00:00Z".to_owned();
        }
        let evaluation = evaluate(&rows, SINCE, &PromotionSettings::default(), &|_| true);
        assert!(evaluation.candidates.is_empty(), "only 5 projects count");
        assert_eq!(evaluation.active, 6, "p0 is still an active project");
        // A later install record does not move the first one.
        let mut rows = usage(6, 3, 0);
        rows.push(row(
            900,
            "2026-11-20T00:00:00Z",
            "agent_install",
            "p0",
            Some("reviewer"),
        ));
        assert_eq!(ids(&rows), ["reviewer"]);
    }

    #[test]
    fn rows_outside_the_window_are_ignored_except_installs() {
        let mut rows = usage(6, 3, 0);
        for r in rows.iter_mut().filter(|r| r.kind == "agent_install") {
            r.at = "2026-01-01T00:00:00Z".to_owned();
        }
        assert_eq!(ids(&rows), ["reviewer"], "an old install still counts");
        let mut rows = usage(6, 3, 2);
        for r in rows.iter_mut().filter(|r| r.kind == "session_start") {
            r.at = "2026-01-01T00:00:00Z".to_owned();
        }
        let evaluation = evaluate(&rows, SINCE, &PromotionSettings::default(), &|_| true);
        assert_eq!(evaluation.active, 6, "old activity is not activity");
    }

    #[test]
    fn ineligible_agents_never_pass() {
        let rows = usage(6, 3, 0);
        let evaluation = evaluate(&rows, SINCE, &PromotionSettings::default(), &|_| false);
        assert!(evaluation.candidates.is_empty());
    }

    #[test]
    fn candidates_report_their_numbers() {
        let evaluation = evaluate(
            &usage(6, 4, 1),
            SINCE,
            &PromotionSettings::default(),
            &|_| true,
        );
        assert_eq!(
            evaluation.candidates,
            [Candidate {
                id: "reviewer".into(),
                projects: 6,
                active: 7,
                delegations: 24
            }]
        );
        assert_eq!(
            evaluation.candidates[0].render(),
            "reviewer (6/7 active projects, 4.0 delegations each)"
        );
    }

    #[test]
    fn settings_round_trip_and_reject_out_of_range_values() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(&dir.path().join("vault.db")).unwrap();
        assert_eq!(
            vault.promotion_settings().unwrap(),
            PromotionSettings::default()
        );
        let custom = PromotionSettings {
            window_days: 14,
            min_share: 50,
            min_projects: 3,
            min_delegations: 2,
        };
        vault.set_promotion_settings(&custom).unwrap();
        assert_eq!(vault.promotion_settings().unwrap(), custom);
        for bad in [
            PromotionSettings {
                window_days: 0,
                ..custom
            },
            PromotionSettings {
                window_days: 181,
                ..custom
            },
            PromotionSettings {
                min_share: 101,
                ..custom
            },
            PromotionSettings {
                min_projects: 0,
                ..custom
            },
            PromotionSettings {
                min_delegations: 0,
                ..custom
            },
        ] {
            assert!(vault.set_promotion_settings(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(vault.promotion_settings().unwrap(), custom);
    }

    #[test]
    fn the_automatic_run_is_claimed_once_a_day_and_promotions_persist() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(&dir.path().join("vault.db")).unwrap();
        assert!(vault.claim_promotion_run().unwrap());
        assert!(!vault.claim_promotion_run().unwrap());
        vault
            .conn
            .execute(
                "UPDATE vault_meta SET value = '2000-01-01' WHERE key = ?1",
                [EVALUATED_KEY],
            )
            .unwrap();
        assert!(vault.claim_promotion_run().unwrap());
        // Two sessions that both read an old day race to claim it: one wins.
        vault
            .conn
            .execute(
                "UPDATE vault_meta SET value = '2000-01-01' WHERE key = ?1",
                [EVALUATED_KEY],
            )
            .unwrap();
        let other = Vault::open(&dir.path().join("vault.db")).unwrap();
        let claims = [vault.claim_today().unwrap(), other.claim_today().unwrap()];
        assert_eq!(claims, [true, false], "only one claim per day");
        vault.mark_promoted("b").unwrap();
        vault.mark_promoted("a").unwrap();
        vault.mark_promoted("b").unwrap();
        assert_eq!(vault.promoted_agents().unwrap(), ["a", "b"]);
    }
}
