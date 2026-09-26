//! `skillvolution doctor`: read-only health checks of the vault and of the
//! client configurations setup wrote. Never writes to the database or to any
//! client config: the vault is opened read-only (see `open_readonly`), and
//! every client check only compares against what `setup` would write, via
//! `ClientKind::changes`, which itself only reads.

use crate::setup::{self, ClientKind, Scope};
use crate::vault;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

#[derive(Serialize)]
struct Check {
    name: String,
    status: Status,
    detail: String,
}

impl Check {
    fn new(status: Status, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }

    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(Status::Ok, name, detail)
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(Status::Warn, name, detail)
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(Status::Fail, name, detail)
    }
}

/// Runs every check against the database at `db`, printing a report (JSON when
/// `json`). Returns whether everything is healthy (no failing check; warnings
/// are fine).
pub fn run(db: &Path, json: bool) -> Result<bool> {
    let mut checks = vault_checks(db)?;
    let (client_checks, globally_configured) = client_checks(db)?;
    checks.extend(client_checks);
    if let Some(detail) = setup::double_install_warning(&globally_configured) {
        checks.push(Check::warn("legacy install", detail));
    }

    let healthy = checks.iter().all(|check| check.status != Status::Fail);
    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            healthy: bool,
            checks: &'a [Check],
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&Report {
                healthy,
                checks: &checks
            })?
        );
    } else {
        for check in &checks {
            println!("[{}] {}: {}", check.status.tag(), check.name, check.detail);
        }
    }
    Ok(healthy)
}

/// Opens `db` read-only: doctor must never create or migrate it, unlike
/// `Vault::open`.
fn open_readonly(db: &Path) -> Result<Connection> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {} read-only", db.display()))
}

fn vault_checks(db: &Path) -> Result<Vec<Check>> {
    if !db.is_file() {
        return Ok(vec![Check::warn(
            "vault: database",
            "will be created on first use",
        )]);
    }
    let conn = open_readonly(db)?;
    let mut checks = Vec::new();

    let user_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let application_id: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let current = vault::schema_version();
    // A vault from before the application id was stamped carries 0, same as
    // `Vault::open`'s own migration check.
    if application_id != vault::application_id() && application_id != 0 {
        checks.push(Check::fail(
            "vault: schema",
            format!("not a skillvolution database (application_id {application_id:#x})"),
        ));
        return Ok(checks);
    }
    checks.push(match user_version.cmp(&current) {
        Ordering::Equal => Check::ok("vault: schema", format!("up to date (v{user_version})")),
        Ordering::Less => Check::warn(
            "vault: schema",
            format!("v{user_version}; will be upgraded to v{current} on next use"),
        ),
        Ordering::Greater => Check::fail(
            "vault: schema",
            format!(
                "v{user_version} is newer than this build supports (v{current}); upgrade skillvolution"
            ),
        ),
    });

    let quick_check: String = conn.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    checks.push(if quick_check == "ok" {
        Check::ok("vault: integrity", "PRAGMA quick_check: ok")
    } else {
        Check::fail("vault: integrity", quick_check)
    });

    // The search index and stats queries assume the current schema's tables
    // and columns, so they only run once the vault is actually at that
    // version; an older or newer vault is left to the schema check above.
    if user_version == current {
        checks.push(search_index_check(&conn)?);
        checks.push(stats_check(&conn)?);
    }
    Ok(checks)
}

/// Every non-deprecated skill with a published revision, by id.
const INDEXABLE_SKILLS: &str = "SELECT s.id FROM skills s WHERE s.deprecated = 0 \
    AND EXISTS (SELECT 1 FROM revisions r WHERE r.id = s.id AND r.status = 'published')";

/// Checks that `skills_fts` has exactly one row per indexable skill, addressed
/// by that skill's `fts_rowid`, and no orphan rows left over from a purge or a
/// bug.
fn search_index_check(conn: &Connection) -> Result<Check> {
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM ({INDEXABLE_SKILLS})"),
        [],
        |row| row.get(0),
    )?;
    let missing: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM skills s WHERE s.id IN ({INDEXABLE_SKILLS}) \
             AND (s.fts_rowid IS NULL OR NOT EXISTS (SELECT 1 FROM skills_fts f WHERE f.rowid = s.fts_rowid))"
        ),
        [],
        |row| row.get(0),
    )?;
    let orphaned: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM skills_fts f WHERE NOT EXISTS \
             (SELECT 1 FROM skills s WHERE s.fts_rowid = f.rowid AND s.id IN ({INDEXABLE_SKILLS}))"
        ),
        [],
        |row| row.get(0),
    )?;
    Ok(if missing == 0 && orphaned == 0 {
        Check::ok(
            "vault: search index",
            format!("{total} skill(s) indexed, no orphans"),
        )
    } else {
        Check::fail(
            "vault: search index",
            format!(
                "{missing} skill(s) missing from the index, {orphaned} orphaned row(s) (of {total} expected)"
            ),
        )
    })
}

fn stats_check(conn: &Connection) -> Result<Check> {
    let skills: i64 = conn.query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))?;
    let published: i64 = conn.query_row(
        "SELECT COUNT(*) FROM revisions WHERE status = 'published'",
        [],
        |row| row.get(0),
    )?;
    let outcomes: i64 = conn.query_row("SELECT COUNT(*) FROM outcomes", [], |row| row.get(0))?;
    let deprecated: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skills WHERE deprecated = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(Check::ok(
        "vault: stats",
        format!(
            "{skills} skill(s), {published} published revision(s), {outcomes} outcome(s), {deprecated} deprecated"
        ),
    ))
}

/// One check per `ClientKind`, plus the list of clients found configured
/// (up to date or differing, as opposed to not installed or installed but
/// never configured) — what the legacy-install check below scopes itself to.
fn client_checks(db: &Path) -> Result<(Vec<Check>, Vec<ClientKind>)> {
    let bin = std::path::absolute(std::env::current_exe().context("determine current executable")?)
        .context("resolve current executable path")?;
    let mut checks = Vec::new();
    let mut globally_configured = Vec::new();
    let current_skill_version = setup::skill_marker_version(setup::SKILL);

    for kind in ClientKind::ALL {
        let display = kind.display_name();
        if !kind.detect() {
            checks.push(Check::ok(display, "not installed"));
            continue;
        }
        let mut changes = kind.changes(Scope::Global, &bin, db)?;
        if let Some(configured_bin) = setup::configured_bin(kind, &changes)
            && configured_bin != bin
        {
            changes = kind.changes(Scope::Global, &configured_bin, db)?;
        }
        let (configured, mismatched) = diff_changes(&changes)?;
        if mismatched.is_empty() {
            checks.push(Check::ok(display, "configured, up to date"));
            globally_configured.push(kind);
        } else if !configured {
            checks.push(Check::ok(display, "installed but not configured"));
        } else {
            checks.push(Check::warn(
                display,
                format!(
                    "differs from what `skillvolution setup --client {}` would write: {}",
                    kind.token(),
                    mismatched.join(", ")
                ),
            ));
            globally_configured.push(kind);
        }

        if let Some(installed) = installed_skill_version(&changes)?
            && let Some(current) = current_skill_version
            && installed < current
        {
            checks.push(Check::warn(
                format!("{display} skill"),
                format!("rerun setup to update the evolution skill (installed v{installed}, current v{current})"),
            ));
        }

        if let Some(missing) = setup::missing_configured_bin(kind, &changes) {
            checks.push(Check::fail(
                format!("{display} binary"),
                format!(
                    "configured binary {} does not exist; rerun setup",
                    missing.display()
                ),
            ));
        }

        if kind == ClientKind::ClaudeCode
            && let Some(registered) = setup::claude_mcp_registered()?
            && !registered
        {
            checks.push(Check::warn(
                "Claude Code MCP registration",
                "`claude mcp get skillvolution` reports no registration; rerun setup",
            ));
        }
    }
    Ok((checks, globally_configured))
}

/// Compares every `changes()` path against the file on disk. Returns whether
/// any of them already carries something of ours, and the paths that differ
/// (missing or with different content) as display strings.
fn diff_changes(changes: &[(PathBuf, String)]) -> Result<(bool, Vec<String>)> {
    let mut configured = false;
    let mut mismatched = Vec::new();
    for (path, expected) in changes {
        match read_optional(path)? {
            None => mismatched.push(path.display().to_string()),
            Some(actual) => {
                if actual != expected.as_bytes() {
                    mismatched.push(path.display().to_string());
                }
                if String::from_utf8_lossy(&actual)
                    .to_lowercase()
                    .contains("skillvolution")
                {
                    configured = true;
                }
            }
        }
    }
    Ok((configured, mismatched))
}

/// The version embedded in the on-disk evolution skill file among `changes`,
/// if any.
fn installed_skill_version(changes: &[(PathBuf, String)]) -> Result<Option<u32>> {
    let Some((path, _)) = changes.iter().find(|(path, _)| path.ends_with("SKILL.md")) else {
        return Ok(None);
    };
    let Some(actual) = read_optional(path)? else {
        return Ok(None);
    };
    Ok(setup::skill_marker_version(&String::from_utf8_lossy(
        &actual,
    )))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}
