//! The MCP agent tools behind `search_agents`, `install_agent` and `list_models`. None of
//! them returns an agent's body: the orchestrator delegates to the installed file, so the
//! prompt never passes through its context.

use crate::{
    agents::{self, FileState, InstallOptions, Target, TierModel},
    clients::{ClientKind, ModelRefFormat},
    vault::{ClientModel, TIERS, Vault},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::Path;

/// What the server knows about where it runs.
pub struct Scope<'a> {
    /// Project key for vault visibility and events.
    pub project: Option<&'a str>,
    /// The directory project agent files go in.
    pub project_dir: &'a Path,
    /// The client running the server (`serve --client`), if setup said.
    pub client: Option<ClientKind>,
}

/// The subagent client for `kind`, or why it has none.
fn target_of(kind: ClientKind) -> Result<Target> {
    Target::from_kind(kind).with_context(|| {
        format!(
            "{} has no subagents; agent tools work for {}",
            kind.display_name(),
            Target::all()
                .map(Target::display_name)
                .collect::<Vec<_>>()
                .join(" and ")
        )
    })
}

/// The agents visible from the project (vault ones, then the built-in tier agents) whose
/// id, description or tier match a word of `query` (all of them for an empty query),
/// best match first, with where each is installed and whether that copy is outdated.
pub fn search(vault: &Vault, scope: &Scope, query: &str, limit: i64, offset: i64) -> Result<Value> {
    ensure!((1..=100).contains(&limit), "limit must be 1..100");
    ensure!(offset >= 0, "offset must be nonnegative");
    ensure!(
        query.len() <= 512 && !query.contains('\0'),
        "query must be at most 512 UTF-8 bytes without NUL"
    );
    let (limit, offset) = (limit as usize, offset as usize);
    let targets: Vec<Target> = match scope.client {
        Some(kind) => vec![target_of(kind)?],
        None => Target::all().collect(),
    };
    let mut ids = Vec::new();
    let mut page = 0;
    loop {
        let found = vault.search_kind("", scope.project, "agent", 100, page)?;
        ids.extend(found.skills.into_iter().map(|s| s.id));
        if !found.has_more {
            break;
        }
        page += 100;
    }
    ids.extend(agents::BUILTINS.iter().map(|(id, _)| (*id).to_owned()));
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut hits = Vec::new();
    for id in ids {
        let Ok(agent) = agents::resolve(vault, &id, scope.project) else {
            continue;
        };
        let text = format!("{} {} {}", agent.id, agent.description, agent.meta.tier).to_lowercase();
        let score = terms.iter().filter(|t| text.contains(t.as_str())).count();
        if terms.is_empty() || score > 0 {
            hits.push((score, agent));
        }
    }
    // Stable: equal scores keep vault order, then the built-ins.
    hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let total = hits.len();
    let models = vault.client_models()?;
    let agents: Vec<Value> = hits
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, agent)| {
            let (installed, outdated) = install_state(&models, &targets, scope.project_dir, &agent);
            json!({
                "id": agent.id,
                "version": agent.version,
                "tier": agent.meta.tier,
                "description": agent.description,
                "installed": installed,
                "outdated": outdated,
            })
        })
        .collect();
    Ok(json!({
        "has_more": offset + agents.len() < total,
        "agents": agents,
        "total": total,
    }))
}

/// Where `agent` has a generated file for `targets`: `local` (the project), `global`
/// (per user), `both` or `no`; and whether any of them is outdated (`agents::is_outdated`:
/// a managed file `sync` would rewrite, a hand-edited one of an older version).
fn install_state(
    models: &[ClientModel],
    targets: &[Target],
    project_dir: &Path,
    agent: &agents::Agent,
) -> (&'static str, bool) {
    let (mut local, mut global, mut outdated) = (false, false, false);
    for &target in targets {
        let Ok(user_dir) = target.dir(None) else {
            continue;
        };
        let project = target
            .dir(Some(project_dir))
            .ok()
            .filter(|dir| !agents::same_dir(dir, &user_dir));
        for (dir, is_local) in [(project, true), (Some(user_dir), false)] {
            let Some(dir) = dir else { continue };
            let path = dir.join(format!("{}.md", agent.id));
            let Ok(state @ (FileState::Managed(_) | FileState::Edited(_))) =
                agents::file_state(&path)
            else {
                continue;
            };
            if is_local {
                local = true;
            } else {
                global = true;
            }
            let at = is_local.then_some(project_dir);
            outdated |= agents::is_outdated(models, target, agent, at, &state);
        }
    }
    let installed = match (local, global) {
        (true, true) => "both",
        (true, false) => "local",
        (false, true) => "global",
        (false, false) => "no",
    };
    (installed, outdated)
}

/// Installs agent `id` in the project, for the server's client (every configured client
/// without one), as `agents add` does, and records the `agent_*` events. One line per
/// client: path, version, how to delegate, and whether the client must restart.
pub fn install(vault: &Vault, scope: &Scope, id: &str, model: Option<&str>) -> Result<Value> {
    let options = InstallOptions {
        force: false,
        client: scope.client.map(target_of).transpose()?,
        model,
    };
    let ids = [id.to_owned()];
    let report = agents::install(
        vault,
        &ids,
        scope.project,
        Some(scope.project_dir),
        &options,
    )?;
    // Files can land in a per-user directory (a server started in $HOME): those are
    // global installs, recorded without a project like `agents add --global`.
    for target in Target::all() {
        let changes: Vec<_> = report
            .changes
            .iter()
            .filter(|c| c.target == target)
            .cloned()
            .collect();
        let scope_name = agents::install_scope(target, Some(scope.project_dir));
        let project = (scope_name == "project").then_some(scope.project).flatten();
        agents::log_agent_events(vault, &changes, scope_name, project);
    }
    if report.files.is_empty() {
        bail!("{id} was not installed: {}", report.lines.join("; "));
    }
    let mut parts = Vec::new();
    let mut restart_needed = false;
    for file in &report.files {
        let name = file.target.display_name();
        let caps = vault.client_caps(file.target.kind())?;
        let path = file
            .path
            .strip_prefix(scope.project_dir)
            .unwrap_or(&file.path)
            .display()
            .to_string();
        let delegate = caps.delegation.first().map_or_else(
            || format!("delegate to \"{}\"", file.id),
            |tool| {
                format!(
                    "delegate with {} ({}: \"{}\")",
                    tool.tool, tool.agent_field, file.id
                )
            },
        );
        parts.push(match file.status {
            "edited" => format!(
                "{name}: {path} was edited by hand and was left as is (the user can overwrite it with `skillvolution agents add --force {}`); {delegate}",
                file.id
            ),
            "unchanged" => format!(
                "{name}: {} v{} already at {path} (unchanged); {delegate}",
                file.id, file.version
            ),
            status => {
                let restart = if caps.hot_reload {
                    "no restart needed".to_owned()
                } else {
                    restart_needed = true;
                    format!("restart {name} to load it")
                };
                format!(
                    "{name}: {status} {} v{} at {path}{}; {delegate}; {restart}",
                    file.id,
                    file.version,
                    file.model
                        .as_deref()
                        .map(|m| format!(" (model {m})"))
                        .unwrap_or_default()
                )
            }
        });
    }
    Ok(json!({
        "result": parts.join(" | "),
        "restart_needed": restart_needed,
        "notes": report.notes,
    }))
}

/// The enabled models of `tier` (every tier when `None`) for `client` (else the server's),
/// best first, each with how to pass it per delegation call on that client's version.
pub fn list_models(
    vault: &Vault,
    scope: &Scope,
    client: Option<ClientKind>,
    tier: Option<&str>,
) -> Result<Value> {
    let kind = client.or(scope.client).context(
        "this server was started without --client; pass `client` (claude-code or opencode)",
    )?;
    let target = target_of(kind)?;
    let caps = vault.client_caps(kind)?;
    let version = vault.client_version(kind)?.map(|v| v.version);
    let stored = vault.client_models()?;
    let tiers: Vec<&str> = match tier {
        Some(tier) if !TIERS.contains(&tier) => bail!("tier must be one of {}", TIERS.join(", ")),
        Some(tier) => vec![tier],
        None => TIERS.to_vec(),
    };
    let mut models = Vec::new();
    for tier in tiers {
        for model in agents::tier_models(&stored, target, tier) {
            let per_call = per_call(caps.per_call_model, caps.hot_reload, kind, &model);
            models.push(json!({
                "tier": tier,
                "model": model.model,
                "provider": model.provider.as_deref().unwrap_or("unknown"),
                "effort": model.effort,
                "priority": model.priority,
                "per_call": per_call,
            }));
        }
    }
    Ok(json!({
        "client": kind.token(),
        "version": version.map(|v| v.to_string()),
        "models": models,
    }))
}

/// How to use `model` for one delegation on a client whose calls take `format`.
fn per_call(
    format: ModelRefFormat,
    hot_reload: bool,
    kind: ClientKind,
    model: &TierModel,
) -> String {
    let reinstall = format!(
        "reinstall with install_agent (model: \"{}\"){}",
        model.model,
        if hot_reload {
            String::new()
        } else {
            format!(" and restart {}", kind.display_name())
        }
    );
    let pass = |value: &str| format!("pass model: \"{value}\" when delegating");
    match format {
        ModelRefFormat::None => format!(
            "not supported: {} at this version takes no model per call; {reinstall}",
            kind.display_name()
        ),
        ModelRefFormat::Alias if model.model.chars().all(|c| c.is_ascii_lowercase()) => {
            pass(&model.model)
        }
        ModelRefFormat::Alias => format!(
            "not passable per call ({} takes only an alias such as opus); {reinstall}",
            kind.display_name()
        ),
        ModelRefFormat::ProviderModel => match &model.provider {
            None => format!("not passable per call (unknown provider); {reinstall}"),
            Some(provider) => {
                let mut value = if model.model.contains('/') {
                    model.model.clone()
                } else {
                    format!("{provider}/{}", model.model)
                };
                if let Some(effort) = &model.effort {
                    value = format!("{value}#{effort}");
                }
                pass(&value)
            }
        },
    }
}
