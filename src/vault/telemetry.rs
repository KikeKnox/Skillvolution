//! Read access to the local usage telemetry: the aggregate `UsageReport` behind
//! `telemetry summary` and the MCP `usage_report` tool, and the raw `events` listing.
//! Read-only: nothing here writes to the vault.

use super::{Vault, outcomes::UNREPORTED_LOAD};
use anyhow::{Result, bail};
use rusqlite::{Connection, ToSql, params};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write;

/// Default window of `telemetry summary` and `usage_report`.
pub const DEFAULT_SINCE: &str = "30d";
/// How many rows each ranked list of the report keeps.
const TOP: i64 = 10;
/// Label for events that lack the grouped field (no model known, no client known).
const UNKNOWN: &str = "unknown";

/// A name and how many times it occurred.
#[derive(Debug, Serialize, PartialEq)]
pub struct Count {
    pub name: String,
    pub count: i64,
}

/// A skill version loaded in a project and day that never reported an outcome for it.
#[derive(Debug, Serialize, PartialEq)]
pub struct UnreportedSkill {
    pub id: String,
    pub version: i64,
    pub unreported: i64,
    pub loaded: i64,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub sessions_by_client: Vec<Count>,
    pub loads_by_client: Vec<Count>,
    pub delegations_by_agent: Vec<Count>,
    pub delegations_by_model: Vec<Count>,
    pub delegations_by_client: Vec<Count>,
}

/// An error kind (`mcp_error` or `hook_error`), what raised it (tool or hook) and its class.
#[derive(Debug, Serialize, PartialEq)]
pub struct ErrorCount {
    pub kind: String,
    pub source: String,
    pub class: String,
    pub count: i64,
}

/// The aggregate telemetry summary. Its JSON form is the documented, stable output of
/// `skillvolution telemetry summary --json` and of the MCP `usage_report` tool.
#[derive(Debug, Serialize)]
pub struct UsageReport {
    /// UTC start of the window (`YYYY-MM-DDTHH:MM:SSZ`).
    pub since: String,
    /// Events recorded in the window.
    pub events: i64,
    /// Most frequent search queries that returned nothing.
    pub searches_without_results: Vec<Count>,
    /// Total searches in the window, and how many of them returned nothing.
    pub searches: i64,
    pub searches_empty: i64,
    /// Skill versions loaded with `get_skill` and never reported (same rule as
    /// `outcomes --unreported`, limited to loads inside the window).
    pub loaded_without_report: Vec<UnreportedSkill>,
    pub usage: Usage,
    pub errors: Vec<ErrorCount>,
}

/// One row of the `events` table, with `data` parsed.
#[derive(Debug, Serialize)]
pub struct EventRecord {
    pub at: String,
    pub kind: String,
    pub client: Option<String>,
    pub client_version: Option<String>,
    pub project: Option<String>,
    pub session: Option<String>,
    pub subject: Option<String>,
    pub version: Option<i64>,
    pub data: Value,
}

fn counts(conn: &Connection, sql: &str, args: &[&dyn ToSql]) -> Result<Vec<Count>> {
    let mut statement = conn.prepare(sql)?;
    Ok(statement
        .query_map(args, |row| {
            Ok(Count {
                name: row.get(0)?,
                count: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

impl Vault {
    /// The UTC timestamp a `--since` value stands for: `YYYY-MM-DD` (that day, 00:00Z)
    /// or `Nd` (N days ago).
    pub fn since_cutoff(&self, since: &str) -> Result<String> {
        let since = since.trim();
        let cutoff: Option<String> = if let Some(days) = since.strip_suffix('d')
            && !days.is_empty()
            && days.len() <= 5
            && days.bytes().all(|b| b.is_ascii_digit())
        {
            self.conn.query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)",
                [format!("-{days} days")],
                |row| row.get(0),
            )?
        } else if since.len() == 10 && since.bytes().filter(|b| *b == b'-').count() == 2 {
            self.conn.query_row(
                "SELECT strftime('%Y-%m-%dT00:00:00Z', ?1)",
                [since],
                |row| row.get(0),
            )?
        } else {
            None
        };
        match cutoff {
            Some(cutoff) => Ok(cutoff),
            None => bail!("invalid --since {since:?}: use YYYY-MM-DD or a number of days like 30d"),
        }
    }

    /// The usage summary since `since` (see `since_cutoff`) over all projects.
    pub fn usage_report(&self, since: &str) -> Result<UsageReport> {
        let cutoff = self.since_cutoff(since)?;
        let conn = &self.conn;
        let at: &dyn ToSql = &cutoff;
        let events = conn.query_row("SELECT COUNT(*) FROM events WHERE at >= ?1", [at], |row| {
            row.get(0)
        })?;
        let (searches, searches_empty) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(json_extract(data, '$.total') = 0), 0)
             FROM events WHERE kind = 'search' AND at >= ?1",
            [at],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let searches_without_results = counts(
            conn,
            "SELECT json_extract(data, '$.query'), COUNT(*) AS n FROM events
             WHERE kind = 'search' AND at >= ?1 AND json_extract(data, '$.total') = 0
               AND json_extract(data, '$.query') IS NOT NULL
             GROUP BY 1 ORDER BY n DESC, 1 LIMIT ?2",
            &[at, &TOP],
        )?;
        let loaded_without_report = {
            let mut statement = conn.prepare(&format!(
                "SELECT l.id, l.version, COUNT(*) AS unreported,
                     (SELECT COUNT(*) FROM loads x WHERE x.id = l.id AND x.version = l.version
                        AND x.created_at >= ?1)
                 FROM loads l
                 WHERE l.created_at >= ?1 AND {UNREPORTED_LOAD}
                 GROUP BY l.id, l.version ORDER BY unreported DESC, l.id LIMIT ?2"
            ))?;
            statement
                .query_map(params![cutoff, TOP], |row| {
                    Ok(UnreportedSkill {
                        id: row.get(0)?,
                        version: row.get(1)?,
                        unreported: row.get(2)?,
                        loaded: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let by = |expr: &str, kind: &str| -> Result<Vec<Count>> {
            counts(
                conn,
                &format!(
                    "SELECT COALESCE({expr}, '{UNKNOWN}'), COUNT(*) AS n FROM events
                     WHERE kind = '{kind}' AND at >= ?1 GROUP BY 1 ORDER BY n DESC, 1 LIMIT ?2"
                ),
                &[at, &TOP],
            )
        };
        let usage = Usage {
            sessions_by_client: by("client", "session_start")?,
            loads_by_client: by("client", "load")?,
            delegations_by_agent: by("subject", "delegation")?,
            delegations_by_model: by("json_extract(data, '$.model')", "delegation")?,
            delegations_by_client: by("client", "delegation")?,
        };
        let errors = {
            let mut statement = conn.prepare(
                "SELECT kind,
                     COALESCE(json_extract(data, '$.tool'), json_extract(data, '$.hook'), ?3),
                     COALESCE(json_extract(data, '$.class'), ?3), COUNT(*) AS n
                 FROM events WHERE kind IN ('mcp_error', 'hook_error') AND at >= ?1
                 GROUP BY 1, 2, 3 ORDER BY n DESC, 1, 2, 3 LIMIT ?2",
            )?;
            statement
                .query_map(params![cutoff, TOP, UNKNOWN], |row| {
                    Ok(ErrorCount {
                        kind: row.get(0)?,
                        source: row.get(1)?,
                        class: row.get(2)?,
                        count: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(UsageReport {
            since: cutoff,
            events,
            searches_without_results,
            searches,
            searches_empty,
            loaded_without_report,
            usage,
            errors,
        })
    }

    /// Raw events, newest first, optionally of one `kind` and since `since`.
    pub fn list_events(
        &self,
        kind: Option<&str>,
        since: Option<&str>,
        limit: i64,
    ) -> Result<Vec<EventRecord>> {
        let cutoff = match since {
            Some(since) => self.since_cutoff(since)?,
            None => String::new(),
        };
        let mut statement = self.conn.prepare(
            "SELECT at, kind, client, client_version, project, session, subject, version, data
             FROM events WHERE (?1 IS NULL OR kind = ?1) AND at >= ?2
             ORDER BY at DESC, id DESC LIMIT ?3",
        )?;
        Ok(statement
            .query_map(params![kind, cutoff, limit], |row| {
                let data: String = row.get(8)?;
                Ok(EventRecord {
                    at: row.get(0)?,
                    kind: row.get(1)?,
                    client: row.get(2)?,
                    client_version: row.get(3)?,
                    project: row.get(4)?,
                    session: row.get(5)?,
                    subject: row.get(6)?,
                    version: row.get(7)?,
                    data: serde_json::from_str(&data).unwrap_or(Value::Null),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

impl UsageReport {
    /// The readable form: one short section per list, empty sections omitted.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Usage since {} ({} events)", self.since, self.events);
        let _ = writeln!(
            out,
            "\nSearches: {}, without results: {}",
            self.searches, self.searches_empty
        );
        section(
            &mut out,
            "Top queries without results",
            &self.searches_without_results,
        );
        if !self.loaded_without_report.is_empty() {
            let _ = writeln!(out, "\nSkills loaded without report:");
            for s in &self.loaded_without_report {
                let _ = writeln!(
                    out,
                    "  {} v{}: {} of {} loads",
                    s.id, s.version, s.unreported, s.loaded
                );
            }
        }
        section(
            &mut out,
            "Sessions by client",
            &self.usage.sessions_by_client,
        );
        section(
            &mut out,
            "Skill loads by client",
            &self.usage.loads_by_client,
        );
        section(
            &mut out,
            "Delegations by agent",
            &self.usage.delegations_by_agent,
        );
        section(
            &mut out,
            "Delegations by model",
            &self.usage.delegations_by_model,
        );
        section(
            &mut out,
            "Delegations by client",
            &self.usage.delegations_by_client,
        );
        if !self.errors.is_empty() {
            let _ = writeln!(out, "\nErrors:");
            for e in &self.errors {
                let _ = writeln!(out, "  {} {} {}: {}", e.kind, e.source, e.class, e.count);
            }
        }
        out
    }
}

fn section(out: &mut String, title: &str, rows: &[Count]) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title}:");
    for row in rows {
        let _ = writeln!(out, "  {}: {}", row.name, row.count);
    }
}

impl EventRecord {
    /// One line: time, kind, client, subject, then the data as compact JSON.
    pub fn render(&self) -> String {
        let mut line = format!("{} {}", self.at, self.kind);
        if let Some(client) = &self.client {
            let _ = write!(line, " [{client}]");
        }
        if let Some(subject) = &self.subject {
            let _ = write!(line, " {subject}");
            if let Some(version) = self.version {
                let _ = write!(line, " v{version}");
            }
        }
        if self.data.as_object().is_none_or(|data| !data.is_empty()) {
            let _ = write!(line, " {}", self.data);
        }
        line
    }
}
