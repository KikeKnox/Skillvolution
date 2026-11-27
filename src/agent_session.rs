//! What a session start does about agents: refresh the stale managed agent files,
//! retire the project copies of promoted agents, promote heavily used global agents to a
//! per-user install (at most once a day), and the catalog block that lists the agents.
//! Nothing here runs a client's CLI or touches the network.

use crate::{
    agents::{self, AgentChange, InstallOptions, Target},
    skill_file,
    vault::{Candidate, Event, Vault},
};
use anyhow::{Error, Result};
use serde_json::json;
use std::{collections::BTreeMap, fmt::Write as _, path::Path};

/// Most agents the catalog block lists.
const AGENT_CATALOG_LIMIT: usize = 15;

/// An agent file a session start rewrote, and its scope (`project` or `global`).
pub type Refreshed = (AgentChange, &'static str);

/// Records the `agent_*` events of `changes` at `scope`, with the project only for a
/// project install.
fn log_changes(vault: &Vault, changes: &[AgentChange], scope: &str, project: Option<&str>) {
    let project = (scope == "project").then_some(project).flatten();
    agents::log_agent_events(vault, changes, scope, project);
}

/// Session-start agent upkeep, in order: retire the managed project copies of agents
/// promoted earlier (first, so a stale copy about to go is not rewritten; copies of an
/// agent promoted now are left until the next session there), rewrite outdated managed
/// files, then the daily promotion run. Never fails: each error goes to `report` and the
/// rest still runs. Returns the files rewritten.
pub fn upkeep(
    vault: &Vault,
    project: Option<&str>,
    project_dir: Option<&Path>,
    report: &mut dyn FnMut(Error),
) -> Vec<Refreshed> {
    if let Some(dir) = project_dir
        && let Err(error) = retire(vault, project, dir)
    {
        report(error);
    }
    let refreshed = match agents::refresh_outdated(vault, project, project_dir) {
        Ok(refreshed) => refreshed,
        Err(error) => {
            report(error);
            Vec::new()
        }
    };
    for (change, scope) in &refreshed {
        log_changes(vault, std::slice::from_ref(change), scope, project);
    }
    match vault.claim_promotion_run() {
        Ok(true) => match vault.promotion_status() {
            Ok(status) => {
                promote(vault, &status.candidates, report);
            }
            Err(error) => report(error),
        },
        Ok(false) => {}
        Err(error) => report(error),
    }
    refreshed
}

/// Removes the managed project copies of promoted agents (see
/// `agents::retire_project_copies`), recording `agent_remove`.
fn retire(vault: &Vault, project: Option<&str>, project_dir: &Path) -> Result<()> {
    let promoted = vault.promoted_agents()?;
    if promoted.is_empty() {
        return Ok(());
    }
    let changes = agents::retire_project_copies(&promoted, project_dir)?;
    log_changes(vault, &changes, "project", project);
    Ok(())
}

/// Installs each of `candidates` in the per-user directory of every client set up per
/// user, through the same path as `agents add --global`, recording the `agent_*` events
/// and `agent_promote`, and marks it promoted. Project copies are left alone. A failed
/// install goes to `report` and the agent stays a candidate. Returns the agents
/// promoted; none when no client with agents is set up per user.
pub fn promote(
    vault: &Vault,
    candidates: &[Candidate],
    report: &mut dyn FnMut(Error),
) -> Vec<String> {
    if candidates.is_empty() || agents::configured_targets(None).is_empty() {
        return Vec::new();
    }
    let mut promoted = Vec::new();
    for candidate in candidates {
        let ids = [candidate.id.clone()];
        let installed = match agents::install(vault, &ids, None, None, &InstallOptions::default()) {
            Ok(installed) => installed,
            Err(error) => {
                report(error.context(format!("promote agent {}", candidate.id)));
                continue;
            }
        };
        let Some(version) = installed.files.first().map(|f| f.version) else {
            continue;
        };
        log_changes(vault, &installed.changes, "global", None);
        let clients: Vec<&str> = installed.files.iter().map(|f| f.target.token()).collect();
        vault.log_event(
            Event::new("agent_promote")
                .subject(&candidate.id, Some(version))
                .data(json!({
                    "projects": candidate.projects,
                    "active": candidate.active,
                    "delegations": candidate.delegations,
                    "clients": clients,
                })),
        );
        match vault.mark_promoted(&candidate.id) {
            Ok(()) => promoted.push(candidate.id.clone()),
            Err(error) => report(error),
        }
    }
    promoted
}

/// Where an agent is installed for one client: the marker version of its local
/// (project) and global (per-user) copy.
#[derive(Default)]
struct Installed {
    local: Option<i64>,
    global: Option<i64>,
    /// The description in the first copy found, for an agent the vault no longer has.
    description: Option<String>,
}

/// The start of a catalog line: `- id vN (tier)`.
fn agent_line(id: &str, version: Option<i64>, tier: Option<&str>) -> String {
    let mut line = format!("- {id}");
    if let Some(version) = version {
        write!(line, " v{version}").unwrap();
    }
    if let Some(tier) = tier {
        write!(line, " ({tier})").unwrap();
    }
    line
}

/// The catalog block for a session of `target`: the Skillvolution agents installed for it
/// (local and global, with versions) and the vault agents visible from `project` that
/// are not, at most `AGENT_CATALOG_LIMIT`, then one line naming `target`'s files that
/// `refreshed` rewrote. Ids, versions, tiers and descriptions only, never bodies. `None`
/// when there are no agents and nothing of `target`'s was refreshed.
pub fn catalog_block(
    vault: &Vault,
    project: Option<&str>,
    project_dir: Option<&Path>,
    target: Target,
    refreshed: &[Refreshed],
) -> Result<Option<String>> {
    let mut installed: BTreeMap<String, Installed> = BTreeMap::new();
    for (dir, level) in agents::session_levels(target, project_dir) {
        for (path, state) in agents::managed_files(&dir) {
            let Some(marker) = state.marker() else {
                continue;
            };
            let entry = installed.entry(marker.id.clone()).or_default();
            let slot = if level.is_some() {
                &mut entry.local
            } else {
                &mut entry.global
            };
            *slot = Some(marker.version);
            if entry.description.is_none() {
                entry.description = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| skill_file::parse(&text).description);
            }
        }
    }
    let page = vault.search_kind("", project, "agent", 100, 0)?;
    let available: Vec<_> = page
        .skills
        .iter()
        .filter(|agent| !installed.contains_key(&agent.id))
        .collect();
    let beyond_page = (page.total as usize).saturating_sub(page.skills.len());
    let total = installed.len() + available.len() + beyond_page;
    let mut lines = Vec::new();
    for (id, copies) in &installed {
        if lines.len() == AGENT_CATALOG_LIMIT {
            break;
        }
        let (version, tier, description) = match agents::builtin(id) {
            Some(builtin) => (
                Some(builtin.version),
                Some(builtin.meta.tier),
                builtin.description,
            ),
            None => match vault.get(id, None, project) {
                Ok(view) if view.kind == "agent" => (
                    Some(view.version),
                    view.meta.map(|meta| meta.tier),
                    view.description,
                ),
                _ => (None, None, copies.description.clone().unwrap_or_default()),
            },
        };
        let level = |name: &str, at: Option<i64>| {
            at.map(|at| match version {
                Some(v) if v == at => name.to_owned(),
                _ => format!("{name} v{at}"),
            })
        };
        let levels: Vec<String> = [level("local", copies.local), level("global", copies.global)]
            .into_iter()
            .flatten()
            .collect();
        let shown = version.or(copies.local).or(copies.global);
        lines.push(format!(
            "{} [installed: {}]: {description}",
            agent_line(id, shown, tier.as_deref()),
            levels.join(", ")
        ));
    }
    for agent in available {
        if lines.len() == AGENT_CATALOG_LIMIT {
            break;
        }
        let tier = vault
            .get(&agent.id, None, project)
            .ok()
            .and_then(|view| view.meta)
            .map(|meta| meta.tier);
        lines.push(format!(
            "{} [not installed]: {}",
            agent_line(&agent.id, Some(agent.version), tier.as_deref()),
            agent.description
        ));
    }
    let mut text = String::new();
    if !lines.is_empty() {
        writeln!(
            text,
            "Skillvolution agents: before delegating to {}'s own agents, check these; delegate \
             to an installed one by its id, or install one with install_agent (search_agents \
             finds more).",
            target.display_name()
        )?;
        for line in &lines {
            writeln!(text, "{line}")?;
        }
        if total > lines.len() {
            writeln!(
                text,
                "- ...and {} more; use search_agents",
                total - lines.len()
            )?;
        }
    }
    // Only this client's files: the others are not this session's concern.
    let updated: Vec<String> = refreshed
        .iter()
        .filter(|(change, _)| change.target == target)
        .map(|(change, scope)| {
            let level = if *scope == "project" {
                "local"
            } else {
                "global"
            };
            format!(
                "{} v{} ({level})",
                change.id,
                change.version.unwrap_or_default()
            )
        })
        .collect();
    if !updated.is_empty() {
        // A client without hot reload read its agent files before this hook ran.
        let restart = !vault.client_caps(target.kind())?.hot_reload;
        writeln!(
            text,
            "Agent files updated to the vault's current version: {}{}.",
            updated.join(", "),
            if restart {
                format!("; restart {} to load them", target.display_name())
            } else {
                String::new()
            }
        )?;
    }
    Ok((!text.is_empty()).then_some(text))
}
