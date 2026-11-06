//! `skillvolution doctor`: health checks of the vault and of the client
//! configurations setup wrote. Read-only unless `--fix`: the checks open the
//! vault read-only (see `open_readonly`) and only compare client files against
//! what `setup` would write, via `ClientKind::changes`, which itself only reads.
//! With `--fix`, each check that carries a `Fix` is repaired through the same
//! machinery the rest of the CLI uses (`Vault::open` to migrate,
//! `Vault::rebuild_search_index`, setup's `write_all` and MCP registration),
//! then every check runs again.

use crate::agents::{self, FileState, Target};
use crate::setup::{self, Change, ClientKind, Edit, Scope};
use crate::vault::{self, Vault};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
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
    /// What `--fix` does about this check, if anything.
    #[serde(skip)]
    fix: Option<Fix>,
}

/// A repair `--fix` applies for one check.
enum Fix {
    /// Open the vault with `Vault::open`, which migrates an older schema.
    Migrate,
    RebuildSearchIndex,
    /// Re-apply global setup for a configured client: its files as computed by
    /// `ClientKind::changes` for `bin` (plus, for Claude Code, the MCP
    /// registration), exactly what `setup --client <kind>` writes.
    Client {
        kind: ClientKind,
        bin: PathBuf,
        changes: Vec<Change>,
    },
    RegisterClaudeMcp(PathBuf),
    /// Not auto-fixable; `--fix` prints this guidance instead.
    Manual(&'static str),
}

impl Check {
    fn new(status: Status, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: Option<Fix>) -> Self {
        self.fix = fix;
        self
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
/// `json`). With `fix`, first applies every available repair (one line each;
/// on stderr when `json`, so stdout stays a single JSON document) and then
/// checks again. Returns whether the final report is healthy (no failing
/// check; warnings are fine).
pub fn run(db: &Path, json: bool, fix: bool) -> Result<bool> {
    // Resolved exactly as setup resolves it, so the paths setup wrote into
    // client configs compare equal to what doctor recomputes.
    let db = &setup::absolutize(db)?;
    let mut checks = all_checks(db)?;
    if fix {
        for line in repair(db, &checks) {
            if json {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
        checks = all_checks(db)?;
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

fn all_checks(db: &Path) -> Result<Vec<Check>> {
    let mut checks = vault_checks(db)?;
    let (client_checks, globally_configured) = client_checks(db)?;
    checks.extend(client_checks);
    if let Some(detail) = setup::double_install_warning(&globally_configured) {
        checks.push(Check::warn("legacy install", detail));
    }
    checks.extend(agent_checks(db));
    Ok(checks)
}

/// The `agents sync --apply` command for level `at` (0 is the per-user directory).
fn sync_command(at: usize) -> &'static str {
    if at == 0 {
        "skillvolution agents sync --apply --global"
    } else {
        "skillvolution agents sync --apply"
    }
}

/// Warn-only checks of the generated agent files in the per-user directories and the
/// current project's. Reads the vault only when it is already at the current schema
/// (never `Vault::open`, which migrates); says nothing when no managed file exists.
fn agent_checks(db: &Path) -> Vec<Check> {
    let cwd = std::env::current_dir().ok();
    let project_dir = cwd.as_deref().and_then(crate::project::repo_dir);
    let key = cwd.as_deref().and_then(crate::project::detect);
    let conn = db
        .is_file()
        .then(|| open_readonly(db).ok())
        .flatten()
        .filter(|conn| {
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .is_ok_and(|v| v == vault::schema_version())
        });
    let (mut checks, mut seen) = (Vec::new(), Vec::new());
    let mut catalog = Vec::new();
    for target in Target::ALL {
        let name = format!("agents: {}", target.display_name());
        let mut no_model: Vec<(usize, String, Vec<String>)> = Vec::new();
        let mut user_ids: Vec<String> = Vec::new();
        let mut project_ids: Vec<String> = Vec::new();
        let dirs = [
            target.dir(None).ok(),
            project_dir
                .as_deref()
                .and_then(|d| target.dir(Some(d)).ok()),
        ];
        let (mut count, mut tokens) = (0, 0);
        for (at, dir) in dirs.into_iter().enumerate() {
            let Some(dir) = dir else { continue };
            if seen.contains(&dir) {
                continue;
            }
            seen.push(dir.clone());
            for (path, state) in agents::managed_files(&dir) {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let file = crate::skill_file::parse(&text);
                count += 1;
                tokens += file.description.as_deref().map_or(0, str::len) / 4;
                let (FileState::Managed(marker) | FileState::Edited(marker)) = &state else {
                    continue;
                };
                let (id, shown) = (&marker.id, path.display());
                if at == 0 {
                    user_ids.push(id.clone());
                } else {
                    project_ids.push(id.clone());
                }
                if matches!(state, FileState::Edited(_)) {
                    let (advice, manual) = if agents::builtin(id).is_some() {
                        (
                            format!(
                                "delete the file and re-run `skillvolution agents add {id}` \
                                 (built-in ids are reserved, so it cannot be published)"
                            ),
                            "delete the file, then `skillvolution agents add <id>`",
                        )
                    } else {
                        (
                            "run `skillvolution agents import` on it and `skillvolution publish` \
                             the result, or delete it"
                                .to_owned(),
                            "`skillvolution agents import <file> -o <dir>`, then `skillvolution publish <dir>`",
                        )
                    };
                    checks.push(
                        Check::warn(
                            &name,
                            format!(
                                "{id} ({shown}) was edited by hand and is skipped by `agents sync`; {advice}"
                            ),
                        )
                        .with_fix(Some(Fix::Manual(manual))),
                    );
                    continue;
                }
                let builtin = agents::builtin(id);
                // One lookup of the current published row visible from this project:
                // (version, kind, deprecated, meta). Outer `None`: no readable vault.
                let found = conn.as_ref().and_then(|conn| {
                    conn.query_row(
                        "SELECT version, kind, deprecated, meta FROM current_skills \
                         WHERE id = ?1 AND (scope IS NULL OR scope = ?2)",
                        rusqlite::params![id, key],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, bool>(2)?,
                                row.get::<_, Option<String>>(3)?,
                            ))
                        },
                    )
                    .optional()
                    .ok()
                });
                let meta = match &builtin {
                    Some(agent) => Some(agent.meta.clone()),
                    None => found.clone().flatten().and_then(|(_, _, _, meta)| {
                        serde_json::from_str::<vault::AgentMeta>(&meta?).ok()
                    }),
                };
                if file.model.is_none() {
                    let tier = meta
                        .as_ref()
                        .map_or_else(|| "unknown".to_owned(), |m| m.tier.clone());
                    match no_model
                        .iter_mut()
                        .find(|(level, t, _)| *level == at && *t == tier)
                    {
                        Some((_, _, ids)) => ids.push(id.clone()),
                        None => no_model.push((at, tier, vec![id.clone()])),
                    }
                }
                let is_agent = found
                    .as_ref()
                    .and_then(|row| row.as_ref())
                    .is_some_and(|(_, kind, _, _)| kind == "agent");
                let current = if builtin.is_some() {
                    Some((agents::BUILTIN_VERSION, true))
                } else {
                    found.map(|row| match row {
                        Some((version, kind, deprecated, _)) => {
                            (version, kind == "agent" && !deprecated)
                        }
                        None => (0, false),
                    })
                };
                if matches!(current, Some((_, true)))
                    && let (Some(conn), Some(meta)) = (&conn, &meta)
                {
                    for skill in &meta.skills {
                        if let Ok(Some(reason)) =
                            vault::skill_unavailable(conn, skill, key.as_deref())
                        {
                            checks.push(Check::warn(
                                &name,
                                format!("agent {id}: skill {skill} is not available ({reason})"),
                            ));
                        } else if vault::skill_deprecated(conn, skill).unwrap_or(false) {
                            checks.push(Check::warn(
                                &name,
                                format!("agent {id}: skill {skill} is deprecated"),
                            ));
                        }
                    }
                }
                match current {
                    Some((version, false)) => {
                        let why = if version == 0 {
                            "is missing from the vault (or scoped to another project)"
                        } else if is_agent {
                            "is deprecated in the vault"
                        } else {
                            "is a skill in the vault, not an agent"
                        };
                        checks.push(Check::warn(&name, format!("{id} ({shown}) {why}")))
                    }
                    Some((version, true)) if marker.version < version => {
                        let sync = sync_command(at);
                        checks.push(
                            Check::warn(
                                &name,
                                format!(
                                    "{id} ({shown}) is outdated (v{} vs v{version} in the vault); \
                                 run `{sync}`",
                                    marker.version
                                ),
                            )
                            .with_fix(Some(Fix::Manual(if at == 0 {
                                "run `skillvolution agents sync --apply --global`"
                            } else {
                                "run `skillvolution agents sync --apply`"
                            }))),
                        )
                    }
                    _ => {}
                }
            }
        }
        for (at, tier, ids) in &no_model {
            let level = if *at == 0 { "user-level" } else { "project" };
            let sync = sync_command(*at);
            let (tier_text, set_tier) = if tier == "unknown" {
                ("an unknown tier".to_owned(), "<tier>")
            } else {
                (format!("tier {tier}"), tier.as_str())
            };
            checks.push(Check::warn(
                &name,
                format!(
                    "no model for {tier_text} ({}) in {} {level} files; the client inherits its own. \
                     Set one with `skillvolution agents models set {} {set_tier} <model>`, then \
                     `{sync}`",
                    ids.join(", "),
                    target.display_name(),
                    target.token()
                ),
            ));
        }
        let both: Vec<&str> = project_ids
            .iter()
            .filter(|id| user_ids.contains(id))
            .map(String::as_str)
            .collect();
        if !both.is_empty() {
            checks.push(Check::warn(
                &name,
                format!(
                    "{} installed at both project and user level; {} merges the two files, so \
                     the project one may inherit settings from the user one. Keep one \
                     (`skillvolution agents remove <id>` or with `--global`)",
                    both.join(", "),
                    target.display_name()
                ),
            ));
        }
        if count > 0 {
            catalog.push(format!(
                "{}: {count} agent(s), ~{tokens} tokens",
                target.token()
            ));
        }
    }
    if !catalog.is_empty() {
        checks.push(Check::ok("agents: catalog", catalog.join("; ")));
    }
    checks
}

/// Applies every check's `Fix`, returning one line per repair. A failed repair is
/// reported and the rest still run.
fn repair(db: &Path, checks: &[Check]) -> Vec<String> {
    let mut lines = Vec::new();
    for check in checks {
        let Some(fix) = &check.fix else { continue };
        lines.push(match fix {
            Fix::Manual(guidance) => format!("[not fixable] {}: {guidance}", check.name),
            fix => match apply(fix, db) {
                Ok(done) => format!("[fixed] {}: {done}", check.name),
                Err(error) => format!("[fix failed] {}: {error:#}", check.name),
            },
        });
    }
    lines
}

fn apply(fix: &Fix, db: &Path) -> Result<String> {
    match fix {
        Fix::Migrate => {
            Vault::open(db).with_context(|| format!("open {}", db.display()))?;
            Ok(format!("migrated to v{}", vault::schema_version()))
        }
        Fix::RebuildSearchIndex => {
            let mut vault = Vault::open(db).with_context(|| format!("open {}", db.display()))?;
            let indexed = vault.rebuild_search_index()?;
            Ok(format!("rebuilt, {indexed} skill(s) indexed"))
        }
        Fix::Client { kind, bin, changes } => {
            setup::write_all(
                changes
                    .iter()
                    .map(|(path, content)| Edit::Write(path.clone(), content.clone()))
                    .collect(),
            )?;
            let mut done = format!(
                "re-applied `skillvolution setup --client {}` (bin {})",
                kind.token(),
                bin.display()
            );
            if *kind == ClientKind::ClaudeCode {
                done = format!("{done}; {}", setup::register_claude_code_mcp(bin, db)?);
            }
            Ok(done)
        }
        Fix::RegisterClaudeMcp(bin) => setup::register_claude_code_mcp(bin, db),
        Fix::Manual(guidance) => Ok((*guidance).to_owned()),
    }
}

/// Opens `db` read-only: doctor must never create or migrate it, unlike
/// `Vault::open`.
fn open_readonly(db: &Path) -> Result<Connection> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {} read-only", db.display()))
}

fn vault_checks(db: &Path) -> Result<Vec<Check>> {
    if !db.is_file() {
        let mut checks = vec![Check::warn(
            "vault: database",
            "will be created on first use",
        )];
        let backup = crate::relocate::backup_path(db);
        if backup.exists() {
            checks.push(Check::warn(
                "vault: relocated",
                format!(
                    "{} was relocated (old copy at {}); set SKILLVOLUTION_DB to the new path \
                     in your shell profile, or pass --db, so CLI commands use the relocated vault",
                    db.display(),
                    backup.display()
                ),
            ));
        }
        return Ok(checks);
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
    // Repairs write to the vault, so none is offered for a database that fails
    // its integrity check.
    let quick_check: String = conn.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    let intact = quick_check == "ok";
    checks.push(match user_version.cmp(&current) {
        Ordering::Equal => Check::ok("vault: schema", format!("up to date (v{user_version})")),
        Ordering::Less => Check::warn(
            "vault: schema",
            format!("v{user_version}; will be upgraded to v{current} on next use"),
        )
        .with_fix(intact.then_some(Fix::Migrate)),
        Ordering::Greater => Check::fail(
            "vault: schema",
            format!(
                "v{user_version} is newer than this build supports (v{current}); upgrade skillvolution"
            ),
        ),
    });

    checks.push(if intact {
        Check::ok("vault: integrity", "PRAGMA quick_check: ok")
    } else {
        Check::fail("vault: integrity", quick_check).with_fix(Some(Fix::Manual(
            "not auto-fixable; restore a copy made with `skillvolution backup`, or \
             `skillvolution export` what is still readable and `import` it into a fresh vault",
        )))
    });

    // The search index and stats queries assume the current schema's tables
    // and columns, so they only run once the vault is actually at that
    // version; an older or newer vault is left to the schema check above.
    if user_version == current {
        let check = search_index_check(&conn)?;
        let rebuild = check.status == Status::Fail && intact;
        checks.push(check.with_fix(rebuild.then_some(Fix::RebuildSearchIndex)));
        checks.push(stats_check(&conn)?);
    }
    Ok(checks)
}

/// Every skill (not agent: agents are never indexed) with a published revision, by id. Deprecated skills included:
/// deprecating keeps the search row and `search` hides it at query time.
const INDEXABLE_SKILLS: &str = "SELECT s.id FROM skills s \
    WHERE s.kind = 'skill' \
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
/// A client whose config can't even be read or parsed gets a failing check of
/// its own instead of aborting the whole report.
fn client_checks(db: &Path) -> Result<(Vec<Check>, Vec<ClientKind>)> {
    let bin = std::path::absolute(std::env::current_exe().context("determine current executable")?)
        .context("resolve current executable path")?;
    let mut checks = Vec::new();
    let mut globally_configured = Vec::new();
    for kind in ClientKind::ALL {
        match client_check(kind, &bin, db) {
            Ok((client_checks, configured)) => {
                checks.extend(client_checks);
                if configured {
                    globally_configured.push(kind);
                }
            }
            Err(error) => checks.push(Check::fail(kind.display_name(), format!("{error:#}"))),
        }
    }
    Ok((checks, globally_configured))
}

/// The checks for one client, and whether it is configured (see `client_checks`).
fn client_check(kind: ClientKind, bin: &Path, db: &Path) -> Result<(Vec<Check>, bool)> {
    let display = kind.display_name();
    if !kind.detect() {
        return Ok((vec![Check::ok(display, "not installed")], false));
    }
    let mut checks = Vec::new();
    // The binary the client stays configured with when it still exists, else this
    // one (global setup's own default), so a fix never points at a missing binary.
    let mut bin = bin.to_owned();
    let mut changes = kind.changes(Scope::Global, &bin, db)?;
    if let Some(configured_bin) = setup::configured_bin(kind, &changes)
        && configured_bin != bin
    {
        changes = kind.changes(Scope::Global, &configured_bin, db)?;
        bin = configured_bin;
    }
    let mismatched = diff_changes(&changes)?;
    let configured = setup::is_configured(&changes);
    // Only a client that is already configured gets re-applied: configuring one
    // that is merely installed is the user's call. This one fix also covers the
    // outdated-skill and missing-binary checks below, which imply a mismatch.
    let reapply = configured && !mismatched.is_empty();
    if mismatched.is_empty() {
        checks.push(Check::ok(display, "configured, up to date"));
    } else if !configured {
        checks.push(Check::ok(display, "installed but not configured"));
    } else {
        checks.push(
            Check::warn(
                display,
                format!(
                    "differs from what `skillvolution setup --client {}` would write: {}",
                    kind.token(),
                    mismatched.join(", ")
                ),
            )
            .with_fix(Some(Fix::Client {
                kind,
                bin: bin.clone(),
                changes: changes.clone(),
            })),
        );
    }

    for (name, content) in setup::SKILLS {
        if let Some(installed) = installed_skill_version(&changes, name)?
            && let Some(current) = setup::skill_marker_version(content, name)
            && installed < current
        {
            checks.push(Check::warn(
                format!("{display} skill"),
                format!(
                    "rerun setup to update the {name} skill (installed v{installed}, current v{current})"
                ),
            ));
        }
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
        // Re-applying the client already registers it, like setup does.
        checks.push(
            Check::warn(
                "Claude Code MCP registration",
                "`claude mcp get skillvolution` reports no registration; rerun setup",
            )
            .with_fix((configured && !reapply).then(|| Fix::RegisterClaudeMcp(bin.clone()))),
        );
    }
    Ok((checks, configured || mismatched.is_empty()))
}

/// Compares every `changes()` path against the file on disk. Returns the paths
/// that differ (missing or with different content) as display strings.
fn diff_changes(changes: &[(PathBuf, String)]) -> Result<Vec<String>> {
    let mut mismatched = Vec::new();
    for (path, expected) in changes {
        if read_optional(path)?.is_none_or(|actual| actual != expected.as_bytes()) {
            mismatched.push(path.display().to_string());
        }
    }
    Ok(mismatched)
}

/// The version embedded in the on-disk file of the managed skill `name` among
/// `changes`, if any.
fn installed_skill_version(changes: &[(PathBuf, String)], name: &str) -> Result<Option<u32>> {
    let Some((path, _)) = changes
        .iter()
        .find(|(path, _)| path.ends_with(Path::new("skills").join(name).join("SKILL.md")))
    else {
        return Ok(None);
    };
    let Some(actual) = read_optional(path)? else {
        return Ok(None);
    };
    Ok(setup::skill_marker_version(
        &String::from_utf8_lossy(&actual),
        name,
    ))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}
