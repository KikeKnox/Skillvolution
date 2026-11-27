use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use skillvolution::{
    clients::{ClientKind, ContinueOutput, HookCommand, HookCommands, SessionOutput, StopJudgment},
    hook, setup,
    vault::Vault,
};
use std::{io::Read, path::PathBuf, sync::LazyLock};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Vault database path; defaults to SKILLVOLUTION_DB, else the platform data directory.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum TelemetryCommand {
    /// Summarize usage: searches without results, skills loaded without a report, usage by
    /// client, agent and model, and errors, over all projects.
    ///
    /// --json prints one object: since (UTC start of the window), events, searches,
    /// searches_empty, searches_without_results [{name, count}], loaded_without_report
    /// [{id, version, unreported, loaded}], usage {sessions_by_client, loads_by_client,
    /// delegations_by_agent, delegations_by_model, delegations_by_client} (each
    /// [{name, count}]), and errors [{kind, source, class, count}]. Each list keeps its top 10.
    Summary {
        /// Window start: YYYY-MM-DD, or a number of days such as 7d.
        #[arg(long, default_value = skillvolution::vault::DEFAULT_SINCE, value_name = "DATE|Nd")]
        since: String,
        /// Print the summary as JSON instead of the readable form.
        #[arg(long)]
        json: bool,
    },
    /// List recorded events, newest first.
    ///
    /// --json prints an array of {at, kind, client, client_version, project, session,
    /// subject, version, data}.
    Events {
        /// Only events of this kind (search, suggest, load, delegation, mcp_error, ...).
        #[arg(long)]
        kind: Option<String>,
        /// Only events since YYYY-MM-DD or a number of days such as 7d.
        #[arg(long, value_name = "DATE|Nd")]
        since: Option<String>,
        /// Most events to list.
        #[arg(long, default_value_t = 100)]
        limit: i64,
        /// Print the events as JSON instead of one line each.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Start the MCP server on stdio.
    Serve {
        /// Project key used for project-scoped skills and outcome records.
        /// Detected automatically from the working directory's git root when omitted,
        /// or from CLAUDE_PROJECT_DIR.
        #[arg(long)]
        project: Option<String>,
        /// The client that runs this server (setup passes it): the one `install_agent`
        /// and `list_models` act for, and the client recorded on its usage events.
        #[arg(long, value_enum)]
        client: Option<AnyClient>,
    },
    /// Show one revision: its status, evidence, and a diff against its base.
    Show {
        /// Skill id.
        id: String,
        /// Revision to show; defaults to the skill's latest revision.
        #[arg(long, value_name = "VERSION")]
        version: Option<i64>,
        /// Print the full revision as JSON instead of the readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Hide a skill from search without deleting its history.
    Deprecate {
        /// Skill id.
        id: String,
    },
    /// Make a deprecated skill searchable again.
    Undeprecate {
        /// Skill id.
        id: String,
    },
    /// Summarize skill outcomes, or list the outcome log of one skill.
    Outcomes {
        /// Skill id; omit to summarize all skills.
        id: Option<String>,
        /// Only skills whose current version has failures.
        #[arg(long, conflicts_with = "id")]
        failing: bool,
        /// Only skills whose current version was loaded with get_skill in a
        /// project and day that never reported an outcome for it.
        #[arg(long, conflicts_with = "id")]
        unreported: bool,
        /// Print the outcome log as JSON instead of the readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Read the local usage telemetry (nothing is ever sent anywhere).
    #[command(subcommand)]
    Telemetry(TelemetryCommand),
    /// Hook entry points for Claude Code, Devin CLI, Codex CLI, Gemini CLI, and Cursor.
    #[command(subcommand)]
    Hook(HookEvent),
    /// Configure AI clients to use the vault (per user by default), or remove it.
    ///
    /// Global setup is the default: it detects the installed clients, asks which to
    /// configure, and is shared by every project. Use --project for one project's own
    /// files, --remove to undo, or --dry-run to preview the changes.
    Setup(setup::SetupArgs),
    /// Permanently delete a skill, or one of its revisions, with its outcomes.
    Purge {
        /// Skill id.
        id: String,
        /// Delete only this revision instead of the whole skill.
        #[arg(long, value_name = "VERSION")]
        version: Option<i64>,
        /// Purge even if published agents list this skill in `skills`.
        #[arg(long)]
        force: bool,
    },
    /// Write every skill, revision, and outcome as JSON.
    Export {
        /// Output file; defaults to stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Merge a JSON document written by `export` into the vault.
    Import {
        /// File written by `export`.
        path: PathBuf,
    },
    /// Write a consistent copy of the vault database to a new file.
    Backup {
        /// Destination file; must not exist.
        path: PathBuf,
    },
    /// Check the vault and the client configurations setup wrote.
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
        /// Repair what can be repaired safely, then check again.
        #[arg(long)]
        fix: bool,
    },
    /// Publish a SKILL.md or AGENT.md file to the vault yourself (you are the evaluator).
    Publish {
        /// A SKILL.md or AGENT.md file, a directory containing one, or a directory of
        /// subdirectories (every PATH/*/SKILL.md and PATH/*/AGENT.md is published;
        /// e.g. ~/.claude/skills).
        path: PathBuf,
        /// Skill id; overrides the `name` in the frontmatter.
        #[arg(long)]
        id: Option<String>,
        /// `global` or `project`; overrides the frontmatter `scope:` (default global).
        #[arg(long, value_enum)]
        scope: Option<PublishScope>,
        /// Project key for `--scope project`; detected like `serve` when omitted.
        #[arg(long)]
        project: Option<String>,
        /// Evidence recorded with the revision.
        #[arg(
            long,
            default_value = "Published manually with `skillvolution publish` by the user."
        )]
        evidence: String,
        /// Allow publishing a new version of an existing skill; also overrides the
        /// proven-version guard (the replaced version keeps its record at half weight in ranking).
        #[arg(long)]
        replace: bool,
    },
    /// Search the vault (like the MCP search_skills tool).
    Search {
        /// Keywords.
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
        /// Project key; detected like `serve` when omitted.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the skills (or agents) visible from the project.
    List {
        /// `skill` (default) or `agent`.
        #[arg(long, value_enum, default_value_t = ListKind::Skill)]
        kind: ListKind,
        /// Project key; detected like `serve` when omitted.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a skill as SKILL.md (an agent as AGENT.md) that `publish --replace` accepts; records nothing.
    Get {
        /// Skill id.
        id: String,
        /// Version to print; defaults to the current one.
        #[arg(long, value_name = "VERSION")]
        version: Option<i64>,
        /// Project key; detected like `serve` when omitted.
        #[arg(long)]
        project: Option<String>,
    },
    /// Generate subagent files for Claude Code and OpenCode from vault agents.
    #[command(subcommand)]
    Agents(AgentsCommand),
    /// Move the vault database to a new local path and repoint configured clients.
    Relocate {
        /// New database path.
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum AgentsCommand {
    /// Show the tier-to-model mapping, or change it with `models set`.
    Models {
        #[command(subcommand)]
        action: Option<ModelsAction>,
    },
    /// Write agent files (vault agents or built-in sv-high, sv-mid, sv-low) for the
    /// configured clients.
    Add {
        /// Agent ids.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Write to the per-user directories instead of the current project's.
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Project key for project-scoped agents; detected like `serve` when omitted. Must
        /// be the project of the directory you run this from.
        #[arg(long)]
        project: Option<String>,
        /// Overwrite a file that was not generated by skillvolution, or a generated one
        /// edited by hand, after saving a numbered `.skillvolution.bak` backup.
        #[arg(long)]
        force: bool,
    },
    /// Delete generated agent files that were not edited by hand.
    Remove {
        /// Agent ids.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Look in the per-user directories instead of the current project's.
        #[arg(long)]
        global: bool,
    },
    /// Regenerate installed agent files from the vault (a dry run unless --apply).
    Sync {
        /// Write the changes.
        #[arg(long)]
        apply: bool,
        /// Use the per-user directories instead of the current project's.
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Project key for project-scoped agents; detected like `serve` when omitted.
        #[arg(long)]
        project: Option<String>,
        /// With --apply: also regenerate files edited by hand, after a backup.
        #[arg(long, requires = "apply")]
        force: bool,
    },
    /// Show or change when a global-scope vault agent is promoted to a per-user install,
    /// and list the current candidates and the agents already promoted. Within the
    /// window, an agent is promoted when it was used (delegated to after its install in
    /// that project) in at least --min-share percent of the active projects (those with
    /// any Skillvolution event), in at least --min-projects projects, with a mean of at
    /// least --min-delegations delegations per using project. Checked once a day at
    /// session start, and by `doctor` (which promotes only with --fix). Without options,
    /// shows the current values.
    Promotion {
        /// Window: a number of days such as `30d` (1 to 180). Default 30d.
        #[arg(long)]
        window: Option<String>,
        /// Percent of the active projects that must use the agent (1 to 100). Default 75.
        #[arg(long)]
        min_share: Option<i64>,
        /// Distinct projects that must use the agent. Default 6 (more than 5).
        #[arg(long)]
        min_projects: Option<i64>,
        /// Mean delegations per using project. Default 3.
        #[arg(long)]
        min_delegations: Option<i64>,
    },
    /// Turn a Claude Code agent file (or a directory of them) into AGENT.md for `publish`.
    Import {
        /// A Claude Code agent file, or a directory of *.md files.
        path: PathBuf,
        /// Write DIR/<id>/AGENT.md files (required for a directory); never overwrites.
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ModelsAction {
    /// Map a tier to a model for a client.
    Set {
        /// `claude-code` or `opencode`.
        client: String,
        /// `high`, `mid` or `low`.
        tier: String,
        /// Model name as the client expects it, e.g. `opus` (Claude Code) or
        /// `anthropic/claude-sonnet-4-5` (OpenCode).
        model: String,
        /// `low`, `medium`, `high`, `xhigh` or `max`. Without it the current effort of
        /// the mapping (stored, else the tier default) is kept.
        #[arg(long)]
        effort: Option<String>,
    },
    /// Delete the priority-0 mapping so the tier reverts to the next ranked model, its
    /// default, or none.
    Unset {
        /// `claude-code` or `opencode`.
        client: String,
        /// `high`, `mid` or `low`.
        tier: String,
    },
    /// Add a ranked model for a tier (lower priority is preferred).
    Add {
        /// `claude-code` or `opencode`.
        client: String,
        /// `high`, `mid` or `low`.
        tier: String,
        /// Model as written to the file, e.g. `opus` or `anthropic/claude-sonnet-4-5`.
        model: String,
        /// Provider; `anthropic` for Claude Code, else the `provider/` of an OpenCode model.
        #[arg(long)]
        provider: Option<String>,
        /// `low`, `medium`, `high`, `xhigh` or `max`.
        #[arg(long)]
        effort: Option<String>,
        /// Lower is preferred. Default: after the tier's last model.
        #[arg(long)]
        priority: Option<i64>,
    },
    /// Delete the stored model rows matching CLIENT TIER MODEL.
    Remove(ModelRef),
    /// Let a disabled model be chosen again.
    Enable(ModelRef),
    /// Stop choosing a model without deleting it.
    Disable(ModelRef),
}

/// Names the stored rows `models remove`, `enable` and `disable` act on.
#[derive(clap::Args)]
struct ModelRef {
    /// `claude-code` or `opencode`.
    client: String,
    /// `high`, `mid` or `low`.
    tier: String,
    /// The model exactly as stored.
    model: String,
    /// Only the row with this provider.
    #[arg(long)]
    provider: Option<String>,
    /// Only the row with this effort.
    #[arg(long)]
    effort: Option<String>,
}

impl ModelRef {
    fn selector(&self) -> skillvolution::vault::ModelSelector<'_> {
        skillvolution::vault::ModelSelector {
            client: &self.client,
            tier: &self.tier,
            model: &self.model,
            provider: self.provider.as_deref(),
            effort: self.effort.as_deref(),
        }
    }
}

/// The vault project key and the directory agent files go in: `--global` is the per-user
/// directories (no project at all), otherwise the enclosing git repo (or the working
/// directory) of the current directory.
fn agent_scope(global: bool, project: Option<String>) -> Result<(Option<String>, Option<PathBuf>)> {
    if global {
        return Ok((None, None));
    }
    validate_project(&project)?;
    let key = resolve_project(project)?;
    let cwd = std::env::current_dir().context("determine current directory")?;
    let dir = skillvolution::project::repo_dir(&cwd).unwrap_or(cwd);
    Ok((key, Some(dir)))
}

fn scope_name(global: bool) -> &'static str {
    if global { "global" } else { "project" }
}

/// The line `models remove/enable/disable` print; no matching row is a failure.
fn model_rows_report(
    reference: &ModelRef,
    verb: &str,
    count: usize,
    failed: &mut bool,
) -> Vec<String> {
    if count == 0 {
        *failed = true;
        return vec![format!(
            "No stored model `{}` for {} {}.",
            reference.model, reference.client, reference.tier
        )];
    }
    vec![format!(
        "{verb} {count} row{} for {} {} `{}`.",
        if count == 1 { "" } else { "s" },
        reference.client,
        reference.tier,
        reference.model
    )]
}

/// `30d` or `30` as a number of days.
fn parse_days(text: &str) -> Result<i64> {
    let digits = text.trim().strip_suffix('d').unwrap_or(text.trim());
    digits
        .parse()
        .ok()
        .with_context(|| format!("invalid --window {text:?}: use a number of days like 30d"))
}

/// What `agents promotion` prints.
fn promotion_report(status: &skillvolution::vault::PromotionStatus, changed: bool) -> Vec<String> {
    let s = &status.settings;
    let mut lines = Vec::new();
    if changed {
        lines.push("Saved the promotion thresholds.".to_owned());
    }
    lines.push(format!(
        "Promotion thresholds: window {}d, min share {}%, min projects {}, min delegations {} (mean per using project).",
        s.window_days, s.min_share, s.min_projects, s.min_delegations
    ));
    lines.push(format!("Active projects in the window: {}.", status.active));
    let list = |items: Vec<String>| {
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    };
    lines.push(format!(
        "Candidates: {}.",
        list(status.candidates.iter().map(|c| c.render()).collect())
    ));
    lines.push(format!("Promoted: {}.", list(status.promoted.clone())));
    lines
}

fn run_agents(db: &Option<PathBuf>, command: AgentsCommand) -> Result<()> {
    use skillvolution::agents;
    let mut failed = false;
    let lines = match command {
        AgentsCommand::Models { action: None } => agents::models_table(&open(db)?.client_models()?),
        AgentsCommand::Models {
            action:
                Some(ModelsAction::Set {
                    client,
                    tier,
                    model,
                    effort,
                }),
        } => {
            let vault = open(db)?;
            let stored = vault.client_models()?;
            let kept = effort.is_none();
            let effort = effort.or_else(|| agents::current_effort(&stored, &client, &tier));
            vault.set_agent_model(&client, &tier, &model, effort.as_deref())?;
            let mut lines = vec![format!(
                "Mapped {client} {tier} to {model}{}.",
                match (&effort, kept) {
                    (Some(e), true) => format!(" (effort {e} kept)"),
                    (Some(e), false) => format!(" (effort {e})"),
                    (None, _) => String::new(),
                }
            )];
            lines.extend(agents::model_warning(&client, &model).map(|w| format!("warning: {w}")));
            if client == "opencode" && effort.is_some() {
                lines.push(
                    "warning: effort is stored, but OpenCode files do not carry effort".into(),
                );
            }
            lines
        }
        AgentsCommand::Models {
            action: Some(ModelsAction::Unset { client, tier }),
        } => {
            let removed = open(db)?.unset_agent_model(&client, &tier)?;
            vec![if removed {
                format!(
                    "Removed the stored mapping for {client} {tier}; it reverts to the default."
                )
            } else {
                format!("No stored mapping for {client} {tier}; nothing to remove.")
            }]
        }
        AgentsCommand::Models {
            action:
                Some(ModelsAction::Add {
                    client,
                    tier,
                    model,
                    provider,
                    effort,
                    priority,
                }),
        } => {
            let row = open(db)?.add_client_model(
                &client,
                &tier,
                &model,
                provider.as_deref(),
                effort.as_deref(),
                priority,
            )?;
            let mut lines = vec![format!(
                "Added {client} {tier} {} `{model}` (priority {}).",
                row.provider, row.priority
            )];
            lines.extend(agents::model_warning(&client, &model).map(|w| format!("warning: {w}")));
            lines
        }
        AgentsCommand::Models {
            action: Some(ModelsAction::Remove(r)),
        } => {
            let count = open(db)?.remove_client_model(&r.selector())?;
            model_rows_report(&r, "Removed", count, &mut failed)
        }
        AgentsCommand::Models {
            action: Some(ModelsAction::Enable(r)),
        } => {
            let count = open(db)?.set_client_model_enabled(&r.selector(), true)?;
            model_rows_report(&r, "Enabled", count, &mut failed)
        }
        AgentsCommand::Models {
            action: Some(ModelsAction::Disable(r)),
        } => {
            let count = open(db)?.set_client_model_enabled(&r.selector(), false)?;
            model_rows_report(&r, "Disabled", count, &mut failed)
        }
        AgentsCommand::Add {
            ids,
            global,
            project,
            force,
        } => {
            let (key, dir) = agent_scope(global, project)?;
            let vault = open(db)?;
            let (lines, changes) =
                agents::add_with_changes(&vault, &ids, key.as_deref(), dir.as_deref(), force)?;
            agents::log_agent_events(&vault, &changes, scope_name(global), key.as_deref());
            lines
        }
        AgentsCommand::Remove { ids, global } => {
            let (key, dir) = agent_scope(global, None)?;
            let (lines, missing, changes) = agents::remove_with_changes(&ids, dir.as_deref())?;
            failed = missing > 0;
            // Recording is best effort: removing needs no vault, so a vault that
            // cannot be opened must not fail it.
            if let Ok(vault) = open(db) {
                agents::log_agent_events(&vault, &changes, scope_name(global), key.as_deref());
            }
            lines
        }
        AgentsCommand::Sync {
            apply,
            global,
            project,
            force,
        } => {
            let (key, dir) = agent_scope(global, project)?;
            let vault = open(db)?;
            let (lines, changes) =
                agents::sync_with_changes(&vault, key.as_deref(), dir.as_deref(), apply, force)?;
            agents::log_agent_events(&vault, &changes, scope_name(global), key.as_deref());
            lines
        }
        AgentsCommand::Promotion {
            window,
            min_share,
            min_projects,
            min_delegations,
        } => {
            let vault = open(db)?;
            let mut settings = vault.promotion_settings()?;
            let changed = window.is_some()
                || min_share.is_some()
                || min_projects.is_some()
                || min_delegations.is_some();
            if let Some(window) = window {
                settings.window_days = parse_days(&window)?;
            }
            settings.min_share = min_share.unwrap_or(settings.min_share);
            settings.min_projects = min_projects.unwrap_or(settings.min_projects);
            settings.min_delegations = min_delegations.unwrap_or(settings.min_delegations);
            if changed {
                vault.set_promotion_settings(&settings)?;
            }
            promotion_report(&vault.promotion_status()?, changed)
        }
        AgentsCommand::Import { path, output } => {
            anyhow::ensure!(
                output.is_some() || !path.is_dir(),
                "importing a directory needs -o DIR"
            );
            let mut rejected = false;
            for (file, item) in agents::import(&open(db)?, &path)? {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => {
                        // Without -o the AGENT.md goes to stdout, often redirected.
                        let line = format!("[rejected] {}: {e:#}", file.display());
                        if output.is_some() {
                            println!("{line}");
                        } else {
                            eprintln!("{line}");
                        }
                        rejected = true;
                        continue;
                    }
                };
                for note in &item.notes {
                    eprintln!("note: {}: {note}", item.id);
                }
                match &output {
                    None => print!("{}", item.text),
                    Some(dir) => {
                        use std::io::Write;
                        let target = dir.join(&item.id).join("AGENT.md");
                        if target.symlink_metadata().is_ok() {
                            println!("[skipped] {}: exists", target.display());
                            continue;
                        }
                        std::fs::create_dir_all(target.parent().unwrap())?;
                        let written = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&target)
                            .and_then(|mut f| f.write_all(item.text.as_bytes()));
                        match written {
                            Ok(()) => println!("Wrote {}", target.display()),
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                                println!("[skipped] {}: exists", target.display())
                            }
                            Err(e) => {
                                return Err(e)
                                    .with_context(|| format!("write {}", target.display()));
                            }
                        }
                    }
                }
            }
            failed = rejected;
            Vec::new()
        }
    };
    for line in lines {
        println!("{line}");
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PublishScope {
    Global,
    Project,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ListKind {
    Skill,
    Agent,
}

impl ListKind {
    fn as_str(self) -> &'static str {
        match self {
            ListKind::Skill => "skill",
            ListKind::Agent => "agent",
        }
    }
}

const AGENT_TEMPLATE: &str = "---
name: my-agent-id
description: Use when <task the orchestrator should delegate>
tier: mid              # high | mid | low
tools: [read, edit, shell, mcp]   # optional; omit it to allow all (read, edit, shell, web, mcp)
skills: [some-skill]   # optional vault skills to load; needs mcp in tools
---
## Role
...
## Procedure
...
## Output contract
...
## Limits
...";

const SKILL_TEMPLATE: &str = "---
name: my-skill-id
description: Use when <situation or symptom an agent would search for>
tags: [tool, topic]
---
## When to use
...
## Procedure
1. ...
## Pitfalls
- ...
## Verification
...";

/// The outcome of publishing one skill file.
enum Published {
    Done {
        id: String,
        version: i64,
        scope: &'static str,
        agent: bool,
    },
    Skipped {
        id: String,
        version: i64,
    },
}

/// How a skill file is published: the CLI flags shared by every file.
struct PublishOptions<'a> {
    scope: Option<PublishScope>,
    project: &'a Option<String>,
    evidence: &'a str,
    replace: bool,
    /// An existing id without `replace` is `Skipped` rather than an error.
    skip_existing: bool,
}

/// Publishes the SKILL.md at `path`. `Err(problems)` lists everything wrong with the file.
fn publish_file(
    vault: &mut Vault,
    path: &std::path::Path,
    id: Option<String>,
    opts: &PublishOptions,
) -> Result<std::result::Result<Published, Vec<String>>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file = skillvolution::skill_file::parse(&text);
    let named_agent = path.file_name().is_some_and(|n| n == "AGENT.md");
    let is_agent = file.is_agent() || named_agent;
    // A file named SKILL.md or AGENT.md without a `name` takes its directory's name.
    let fallback = path
        .file_name()
        .is_some_and(|n| n == "SKILL.md" || n == "AGENT.md")
        .then(|| std::path::absolute(path).ok())
        .flatten()
        .and_then(|p| {
            p.parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        });
    let id = id.or(file.name.clone()).or(fallback);
    let meta = file.agent_meta();
    let mut problems = if is_agent {
        skillvolution::vault::agent_problems(
            id.as_deref(),
            file.description.as_deref(),
            &file.tags,
            &file.content,
            &meta,
        )
    } else {
        skillvolution::vault::skill_problems(
            id.as_deref(),
            file.description.as_deref(),
            &file.tags,
            &file.content,
        )
    };
    problems.extend(file.problems.iter().cloned());
    let file_scope = match file.scope.as_deref() {
        Some("project") => Some(PublishScope::Project),
        Some(_) => Some(PublishScope::Global),
        None => None,
    };
    let scope = opts.scope.or(file_scope).unwrap_or(PublishScope::Global);
    let project = if scope == PublishScope::Project {
        let project = resolve_project(opts.project.clone())?;
        if project.is_none() {
            problems.push(
                "scope project needs a project key: run inside a git repository or pass --project KEY"
                    .into(),
            );
        }
        project
    } else {
        None
    };
    if !problems.is_empty() {
        return Ok(Err(problems));
    }
    let (id, description) = (id.unwrap(), file.description.unwrap());
    let current = vault.published_version(&id)?;
    if current > 0 && !opts.replace {
        if let Some(stored) = vault.kind(&id)? {
            let want = if is_agent { "agent" } else { "skill" };
            if stored != want {
                return Ok(Err(vec![skillvolution::vault::kind_conflict(&id, &stored)]));
            }
        }
        if opts.skip_existing {
            return Ok(Ok(Published::Skipped {
                id,
                version: current,
            }));
        }
        anyhow::bail!(
            "{} {id} already exists at v{current}; pass --replace to publish v{}",
            if is_agent { "agent" } else { "skill" },
            current + 1
        );
    }
    let project_scope = project.as_deref();
    let proposal = skillvolution::vault::Proposal {
        id: &id,
        description: &description,
        tags: &file.tags,
        content: &file.content,
        evidence: opts.evidence,
        expected_version: current,
        scope: project_scope,
        verdict: if project_scope.is_some() {
            "keep project"
        } else {
            "keep global"
        },
        verdict_reason: "published manually from the CLI by the user",
        replaces_proven: opts.replace,
    };
    let revision = if is_agent {
        vault.propose_agent(&proposal, &meta)?
    } else {
        vault.propose(&proposal)?
    };
    Ok(Ok(Published::Done {
        id: revision.id,
        version: revision.version,
        scope: project_scope.map_or("global", |_| "project"),
        agent: is_agent,
    }))
}

/// Records a `publish` event for a published file, or a `publish_rejected` one with
/// the (scrubbed) problems for a rejected one. A skipped file is neither.
fn log_publish(vault: &Vault, result: &std::result::Result<Published, Vec<String>>) {
    use skillvolution::vault::{Event, MAX_REASON_CHARS, scrub_text};
    match result {
        Ok(Published::Done {
            id,
            version,
            scope,
            agent,
        }) => vault.log_event(Event::new("publish").subject(id, Some(*version)).data(
            serde_json::json!({
                "scope": scope,
                "agent": agent,
            }),
        )),
        Ok(Published::Skipped { .. }) => {}
        Err(problems) => vault.log_event(Event::new("publish_rejected").data(serde_json::json!({
            "reasons": problems
                .iter()
                .take(5)
                .map(|problem| scrub_text(problem, MAX_REASON_CHARS))
                .collect::<Vec<_>>(),
        }))),
    }
}

fn publish(
    db: &Option<PathBuf>,
    path: PathBuf,
    id: Option<String>,
    scope: Option<PublishScope>,
    project: Option<String>,
    evidence: String,
    replace: bool,
) -> Result<()> {
    validate_project(&project)?;
    let skill_template = format!(
        "A SKILL.md looks like this:\n\n{SKILL_TEMPLATE}\n\nSee 'Publishing a skill manually' in README.md."
    );
    let agent_template = format!("An AGENT.md looks like this:\n\n{AGENT_TEMPLATE}");
    let template_for = |file: &std::path::Path| {
        if file.file_name().is_some_and(|n| n == "AGENT.md") {
            agent_template.clone()
        } else {
            skill_template.clone()
        }
    };
    let mut opts = PublishOptions {
        scope,
        project: &project,
        evidence: &evidence,
        replace,
        skip_existing: false,
    };
    if path.is_dir() && !path.join("SKILL.md").is_file() && !path.join("AGENT.md").is_file() {
        anyhow::ensure!(
            id.is_none(),
            "--id cannot be used when publishing a directory of skills"
        );
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .with_context(|| format!("read {}", path.display()))?
            .filter_map(|e| e.ok())
            .flat_map(|e| [e.path().join("SKILL.md"), e.path().join("AGENT.md")])
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        anyhow::ensure!(
            !files.is_empty(),
            "no SKILL.md or AGENT.md in {} or in its subdirectories",
            path.display()
        );
        opts.skip_existing = true;
        let mut vault = open(db)?;
        let (mut done, mut skipped, mut rejected) = (0, 0, 0);
        let mut templates = Vec::new();
        for file in &files {
            let result = publish_file(&mut vault, file, None, &opts)
                .unwrap_or_else(|e| Err(vec![format!("{e:#}")]));
            log_publish(&vault, &result);
            match result {
                Ok(Published::Done {
                    id,
                    version,
                    scope,
                    agent,
                }) => {
                    done += 1;
                    let kind = if agent { ", agent" } else { "" };
                    println!("[published] {id} v{version} ({scope}{kind})");
                }
                Ok(Published::Skipped { id, version }) => {
                    skipped += 1;
                    println!("[skipped] {id}: already at v{version}; pass --replace");
                }
                Err(problems) => {
                    rejected += 1;
                    let template = template_for(file);
                    if !templates.contains(&template) {
                        templates.push(template);
                    }
                    println!("[rejected] {}:", file.display());
                    for problem in problems {
                        println!("  - {problem}");
                    }
                }
            }
        }
        for template in &templates {
            println!("\n{template}");
        }
        println!("\n{done} published, {skipped} skipped, {rejected} rejected");
        if rejected > 0 {
            std::process::exit(1);
        }
        return Ok(());
    }
    let path = if path.is_dir() {
        let skill = path.join("SKILL.md");
        if skill.is_file() {
            skill
        } else {
            path.join("AGENT.md")
        }
    } else {
        path
    };
    let mut vault = open(db)?;
    let result = publish_file(&mut vault, &path, id, &opts)?;
    log_publish(&vault, &result);
    match result {
        Ok(Published::Done {
            id,
            version,
            scope,
            agent,
        }) => {
            let kind = if agent { ", agent" } else { "" };
            println!("Published {id} v{version} ({scope}{kind}).")
        }
        Ok(Published::Skipped { .. }) => unreachable!("single-file publish never skips"),
        Err(problems) => {
            let list: String = problems.iter().map(|p| format!("- {p}\n")).collect();
            anyhow::bail!(
                "{} cannot be published:\n{list}\n{}",
                path.display(),
                template_for(&path)
            );
        }
    }
    Ok(())
}

/// Prints an agent as an AGENT.md that `publish --replace` accepts.
fn print_agent_file(agent: &skillvolution::vault::SkillView) {
    let meta = agent
        .meta
        .clone()
        .unwrap_or_else(|| skillvolution::skill_file::parse("").agent_meta());
    let scope = agent.scope.is_some().then_some("project");
    println!(
        "{}",
        skillvolution::skill_file::agent_md(
            &agent.id,
            &agent.description,
            &agent.tags,
            scope,
            &meta,
            &agent.content
        )
    );
}

fn print_skills(page: &skillvolution::vault::SearchPage, json: bool) -> Result<()> {
    if json {
        return print_json(page);
    }
    for s in &page.skills {
        println!(
            "{} v{} [{}] (helped {}, failed {}): {}",
            s.id,
            s.version,
            s.scope.as_deref().unwrap_or("global"),
            s.helped,
            s.failed,
            s.description
        );
    }
    if page.has_more {
        println!("{} of {}", page.skills.len(), page.total);
    }
    Ok(())
}

/// The client emitting a hook payload: payloads and blocking conventions differ.
/// `claude-code` scans a transcript and makes Claude continue via
/// `hookSpecificOutput.additionalContext` JSON on stdout; every other client
/// tracks per-session flags from `tool-use` and asks to continue with its own
/// JSON on stdout (`{"decision":"block"}` for Devin and Codex, `{"decision":"deny"}`
/// for Gemini CLI's AfterAgent, `{"followup_message"}` for Cursor). Each of these
/// conventions is read from the client's `HookSpec`; the accepted `--client` tokens
/// are those of the clients whose hooks are `hook` commands (every client but
/// OpenCode, whose plugin handles its own events).
#[derive(Clone, Copy)]
struct HookClient(ClientKind);

impl HookClient {
    const CLAUDE_CODE: Self = Self(ClientKind::ClaudeCode);
    const DEVIN: Self = Self(ClientKind::Devin);

    /// The `hook` commands of this client; every `HookClient` has them.
    fn commands(self) -> &'static HookCommands {
        self.0
            .spec()
            .hooks
            .commands
            .as_ref()
            .expect("a HookClient's hooks are hook commands")
    }

    /// The file-editing tool names `tool-use` counts as work for this client.
    fn work_tools(self) -> &'static [&'static str] {
        self.0.spec().hooks.work_tools
    }

    /// The SessionStart output that injects `context` into this client's session.
    fn session_context(self, context: String) -> String {
        match self.commands().session_output {
            SessionOutput::HookSpecificOutput => serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": context,
                }
            })
            .to_string(),
            SessionOutput::AdditionalContext => {
                serde_json::json!({ "additional_context": context }).to_string()
            }
        }
    }

    /// The prompt-submit event whose `hookSpecificOutput.additionalContext` this client
    /// adds to the model's context: UserPromptSubmit for Claude Code, Codex, and Devin,
    /// BeforeAgent for Gemini CLI. `None` for Cursor, whose beforeSubmitPrompt can only
    /// allow or block the prompt, not add context to it.
    fn prompt_event(self) -> Option<&'static str> {
        self.commands().event(HookCommand::Prompt)
    }
}

/// The Stop output asking a flag-tracked client to continue with `reason`.
fn continue_with(output: ContinueOutput, reason: &str) -> serde_json::Value {
    match output {
        ContinueOutput::Block => serde_json::json!({"decision": "block", "reason": reason}),
        ContinueOutput::Deny => serde_json::json!({"decision": "deny", "reason": reason}),
        ContinueOutput::FollowupMessage => serde_json::json!({ "followup_message": reason }),
    }
}

/// Any client's `--client` token (`serve`, `hook session-start`).
#[derive(Clone, Copy)]
struct AnyClient(ClientKind);

impl AnyClient {
    const CLAUDE_CODE: Self = Self(ClientKind::ClaudeCode);

    /// The SessionStart output that injects `context`: the client's own hook convention,
    /// or `hookSpecificOutput` for a client whose hooks are not `hook` commands (the
    /// OpenCode plugin reads that form).
    fn session_context(self, context: String) -> String {
        if self.0.spec().hooks.commands.is_some() {
            return HookClient(self.0).session_context(context);
        }
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": context,
            }
        })
        .to_string()
    }

    /// The prompt-submit event `hook prompt` names in its output: the client's own
    /// (`HookClient::prompt_event`), or `UserPromptSubmit` for a client whose hooks are not
    /// `hook` commands (the OpenCode plugin reads only `additionalContext`).
    fn prompt_event(self) -> Option<&'static str> {
        if self.0.spec().hooks.commands.is_some() {
            return HookClient(self.0).prompt_event();
        }
        Some("UserPromptSubmit")
    }
}

impl clap::ValueEnum for AnyClient {
    fn value_variants<'a>() -> &'a [Self] {
        static VARIANTS: LazyLock<Vec<AnyClient>> =
            LazyLock::new(|| ClientKind::ALL.into_iter().map(AnyClient).collect());
        &VARIANTS
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.0.token()))
    }
}

impl clap::ValueEnum for HookClient {
    fn value_variants<'a>() -> &'a [Self] {
        static VARIANTS: LazyLock<Vec<HookClient>> = LazyLock::new(|| {
            ClientKind::ALL
                .into_iter()
                .filter(|kind| kind.spec().hooks.commands.is_some())
                .map(HookClient)
                .collect()
        });
        &VARIANTS
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.0.token()))
    }
}

#[derive(Subcommand)]
enum HookEvent {
    /// Print the skill catalog (and, for clients with subagents, the agents block) as
    /// session context, after updating stale managed agent files, retiring project copies
    /// of promoted agents and, once a day, promoting heavily used agents to per-user.
    SessionStart {
        /// Project key for project-scoped skills.
        /// Detected automatically from the working directory's git root when omitted,
        /// or from CLAUDE_PROJECT_DIR.
        #[arg(long)]
        project: Option<String>,
        /// The client emitting the hook: claude-code, devin, codex, gemini, cursor, or
        /// opencode (its plugin). Defaults to claude-code.
        #[arg(long, value_enum, default_value_t = AnyClient::CLAUDE_CODE)]
        client: AnyClient,
        /// The session is a subagent's (the OpenCode plugin passes it for child
        /// sessions): print the skills catalog only, with no agents block and no agent
        /// upkeep.
        #[arg(long)]
        subagent: bool,
    },
    /// Suggest the vault skills matching the submitted prompt (UserPromptSubmit, or
    /// Gemini CLI's BeforeAgent); prints nothing when none is relevant.
    Prompt {
        /// Project key for project-scoped skills.
        /// Detected automatically from the working directory's git root when omitted,
        /// or from CLAUDE_PROJECT_DIR.
        #[arg(long)]
        project: Option<String>,
        /// The client emitting the hook: claude-code, devin, codex, gemini, or opencode
        /// (its plugin); cursor cannot inject prompt context. Defaults to claude-code.
        #[arg(long, value_enum, default_value_t = AnyClient::CLAUDE_CODE)]
        client: AnyClient,
    },
    /// Record a delegation a client's plugin saw (the OpenCode plugin): reads
    /// `{"agent", "model"?, "session"?}` JSON on stdin and prints nothing.
    Delegation {
        /// Project key. Detected like `hook prompt` when omitted.
        #[arg(long)]
        project: Option<String>,
        /// The client whose plugin saw the delegation (opencode).
        #[arg(long, value_enum)]
        client: AnyClient,
    },
    /// Record a review reminder a client's plugin added (the OpenCode plugin): reads
    /// `{"session"?}` JSON on stdin and prints nothing.
    Reminder {
        /// Project key. Detected like `hook prompt` when omitted.
        #[arg(long)]
        project: Option<String>,
        /// The client whose plugin added the reminder (opencode).
        #[arg(long, value_enum)]
        client: AnyClient,
    },
    /// Record work/review tool calls for a session (PostToolUse of the flag-tracked
    /// clients: Devin, Codex, Gemini CLI, Cursor).
    ToolUse {
        /// The client emitting the hook: claude-code, devin, codex, gemini, or cursor.
        /// Defaults to devin.
        #[arg(long, value_enum, default_value_t = HookClient::DEVIN)]
        client: HookClient,
    },
    /// Ask for an evolution review after unreviewed work.
    Stop {
        /// Project key, used by the claude-code client to tell which delegated agents are
        /// vault agents. Detected like `hook prompt` when omitted.
        #[arg(long)]
        project: Option<String>,
        /// The client emitting the hook: claude-code, devin, codex, gemini, or cursor.
        /// Defaults to claude-code.
        #[arg(long, value_enum, default_value_t = HookClient::CLAUDE_CODE)]
        client: HookClient,
    },
    /// Drop a session's hook state (Devin SessionEnd).
    SessionEnd,
    /// Approve the tools the evolution flow needs (Devin PermissionRequest).
    Approve,
}

impl HookEvent {
    /// The hook's name and the client that emitted it, for `hook_error` events.
    fn origin(&self) -> (&'static str, Option<ClientKind>) {
        match self {
            HookEvent::SessionStart { client, .. } => ("session-start", Some(client.0)),
            HookEvent::Prompt { client, .. } => ("prompt", Some(client.0)),
            HookEvent::Delegation { client, .. } => ("delegation", Some(client.0)),
            HookEvent::Reminder { client, .. } => ("reminder", Some(client.0)),
            HookEvent::ToolUse { client } => ("tool-use", Some(client.0)),
            HookEvent::Stop { client, .. } => ("stop", Some(client.0)),
            HookEvent::SessionEnd => ("session-end", None),
            HookEvent::Approve => ("approve", None),
        }
    }
}

/// Records a `hook_error` event (hook and error class, never the message). Best effort:
/// when the vault itself cannot be opened there is nowhere to record it.
fn log_hook_error(
    db: &Option<PathBuf>,
    hook: &str,
    client: Option<ClientKind>,
    error: &anyhow::Error,
) {
    if let Ok(vault) = open(db) {
        vault.log_event(
            skillvolution::vault::Event::new("hook_error")
                .client(client)
                .data(serde_json::json!({
                    "hook": hook,
                    "class": skillvolution::vault::error_class(error),
                })),
        );
    }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn validate_project(project: &Option<String>) -> Result<()> {
    if let Some(project) = project {
        skillvolution::vault::validate_id(project).context("invalid --project key")?;
    }
    Ok(())
}

/// Resolves the project key for `serve`, `hook session-start`, and `hook prompt`: an explicit
/// `--project` wins (already validated by `validate_project`); otherwise the
/// current directory is tried first, since that is where OpenCode and a
/// plain terminal start these processes. If the current directory is not a
/// repo, fall back to `CLAUDE_PROJECT_DIR` (set by Claude Code) — this covers
/// Claude Code starting a user-scope MCP server with its own config dir as
/// cwd, which is never a repo.
fn resolve_project(explicit: Option<String>) -> Result<Option<String>> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    let cwd = std::env::current_dir().context("determine current directory")?;
    if let Some(project) = skillvolution::project::detect(&cwd) {
        return Ok(Some(project));
    }
    Ok(match std::env::var("CLAUDE_PROJECT_DIR") {
        Ok(dir) if !dir.is_empty() => skillvolution::project::detect(&PathBuf::from(dir)),
        _ => None,
    })
}

/// The directory `serve` installs project agents in: the git repository holding the
/// current directory, else that of `CLAUDE_PROJECT_DIR` (or that directory itself), else
/// the current directory; the same order as `resolve_project`.
fn resolve_project_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("determine current directory")?;
    if let Some(repo) = skillvolution::project::repo_dir(&cwd) {
        return Ok(repo);
    }
    Ok(match std::env::var("CLAUDE_PROJECT_DIR") {
        Ok(dir) if !dir.is_empty() => {
            let dir = PathBuf::from(dir);
            skillvolution::project::repo_dir(&dir).unwrap_or(dir)
        }
        _ => cwd,
    })
}

/// The project directory a hook works in: the git repository holding the current
/// directory, else `CLAUDE_PROJECT_DIR`.
fn hook_project_dir() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .as_deref()
        .and_then(skillvolution::project::repo_dir)
        .or_else(|| {
            std::env::var_os("CLAUDE_PROJECT_DIR")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
        })
}

/// The database at `--db`, or the default location.
fn db_path(db: &Option<PathBuf>) -> Result<PathBuf> {
    db.clone()
        .map(Ok)
        .unwrap_or_else(skillvolution::vault::default_database)
}

/// Opens the database at `--db`, or the default location (setup and every
/// command create it on first open, so there is no separate init step).
fn open(db: &Option<PathBuf>) -> Result<Vault> {
    let path = db_path(db)?;
    Vault::open(&path).with_context(|| format!("open {}", path.display()))
}

/// The hook payload the client wrote to stdin.
fn read_stdin() -> Result<String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("read hook input from stdin")?;
    Ok(input)
}

fn run_hook(event: HookEvent, db: &Option<PathBuf>) -> Result<()> {
    match event {
        HookEvent::SessionStart {
            project,
            client,
            subagent,
        } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            let project_dir = hook_project_dir();
            let context = hook::session_start_recorded(
                &open(db)?,
                project.as_deref(),
                project_dir.as_deref(),
                client.0,
                subagent,
            )?;
            print!("{}", client.session_context(context));
        }
        HookEvent::Prompt { project, client } => {
            // Drain the payload before anything can fail: exiting with it unread breaks
            // the pipe under a client (or test) that is still writing it.
            let input = read_stdin()?;
            validate_project(&project)?;
            let event = client
                .prompt_event()
                .context("this client's prompt hook cannot inject context")?;
            let project = resolve_project(project)?;
            if let Some(context) =
                hook::prompt_recorded(&open(db)?, project.as_deref(), &input, client.0)?
            {
                println!(
                    "{}",
                    serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": event,
                            "additionalContext": context,
                        }
                    })
                );
            }
        }
        HookEvent::Delegation { project, client } => {
            let input = read_stdin()?;
            validate_project(&project)?;
            let project = resolve_project(project)?;
            hook::plugin_delegation(&open(db)?, project.as_deref(), &input, client.0)?;
        }
        HookEvent::Reminder { project, client } => {
            let input = read_stdin()?;
            validate_project(&project)?;
            let project = resolve_project(project)?;
            hook::plugin_reminder(&open(db)?, project.as_deref(), &input, client.0)?;
        }
        HookEvent::ToolUse { client } => {
            hook::tool_use(&open(db)?, &read_stdin()?, client.work_tools())?
        }
        HookEvent::Stop { project, client } => {
            let input = read_stdin()?;
            validate_project(&project)?;
            match client.commands().stop {
                StopJudgment::Transcript => {
                    let project = resolve_project(project)?;
                    let project_dir = hook_project_dir();
                    let dirs = skillvolution::agents::claude_agent_dirs(project_dir.as_deref());
                    let hooks = &client.0.spec().hooks;
                    if let Some(reason) = hook::transcript_stop_as(
                        &open(db)?,
                        &input,
                        project.as_deref(),
                        &dirs,
                        hooks,
                        Some(client.0),
                    )? {
                        // additionalContext on stdout (exit 0) makes Claude continue the
                        // same turn, like decision:block, and the continued stop arrives
                        // with stop_hook_active=true; unlike decision:block, it shows no
                        // "Stop hook error" notification.
                        println!(
                            "{}",
                            serde_json::json!({
                                "hookSpecificOutput": {
                                    "hookEventName": "Stop",
                                    "additionalContext": reason
                                }
                            })
                        );
                    }
                }
                StopJudgment::Flags(output) => {
                    let project = resolve_project(project).ok().flatten();
                    if let Some(reason) =
                        hook::devin_stop_as(&open(db)?, &input, project.as_deref(), Some(client.0))?
                    {
                        println!("{}", continue_with(output, reason));
                    }
                }
            }
        }
        HookEvent::SessionEnd => hook::devin_session_end(&open(db)?, &read_stdin()?)?,
        HookEvent::Approve => {
            if let Some(reason) = hook::devin_approve(&read_stdin()?)? {
                println!(
                    "{}",
                    serde_json::json!({"decision": "approve", "reason": reason})
                );
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Purge { id, version, force } => {
            let mut vault = open(&cli.db)?;
            let report = if force {
                vault.purge_forced(&id, version)?
            } else {
                vault.purge(&id, version)?
            };
            println!(
                "Purged {id}: {} revision(s), {} outcome(s){}.",
                report.revisions,
                report.outcomes,
                if report.skill_removed {
                    ", skill removed"
                } else {
                    ""
                }
            );
            if !report.agents_listing.is_empty() {
                if !report.still_published {
                    println!(
                        "warning: agent(s) {} still list {id} in `skills`; their generated files will fail to load it. Republish them without it (`skillvolution agents sync --apply` then updates the files).",
                        report.agents_listing.join(", ")
                    );
                } else {
                    println!(
                        "warning: agent(s) {} list {id} in `skills`; `skillvolution agents sync --apply` repins them to the remaining version.",
                        report.agents_listing.join(", ")
                    );
                }
            }
            for line in &report.agent_warnings {
                println!("{line}");
            }
            if !report.compacted {
                eprintln!(
                    "warning: the data was deleted, but the vault file could not be compacted \
                     while other connections are open, so purged content may linger on disk; \
                     close running AI client sessions and run another purge later to compact it."
                );
            }
        }
        Command::Export { output } => {
            let json = open(&cli.db)?.export_json()?;
            match output {
                Some(path) => std::fs::write(&path, json)
                    .with_context(|| format!("write {}", path.display()))?,
                None => println!("{json}"),
            }
        }
        Command::Import { path } => {
            let json = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let report = open(&cli.db)?.import_json(&json)?;
            print_json(&report)?;
            if !report.agents_imported.is_empty() {
                eprintln!(
                    "note: agent(s) {} were imported or updated; review them, then run \
                     `skillvolution agents sync` (a dry run) before `--apply`, since the \
                     generated files can now change.",
                    report.agents_imported.join(", ")
                );
            }
        }
        Command::Backup { path } => {
            open(&cli.db)?.backup(&path)?;
            println!("Backed up to {}.", path.display());
        }
        Command::Doctor { json, fix } => {
            if !skillvolution::doctor::run(&db_path(&cli.db)?, json, fix)? {
                std::process::exit(1);
            }
        }
        Command::Agents(command) => run_agents(&cli.db, command)?,
        Command::Relocate { path } => skillvolution::relocate::run(&db_path(&cli.db)?, &path)?,
        Command::Setup(args) => setup::run(args.with_default_db(cli.db))?,
        Command::Serve { project, client } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            eprintln!(
                "skillvolution: project = {}",
                project.as_deref().unwrap_or("none")
            );
            skillvolution::mcp::serve(
                open(&cli.db)?,
                skillvolution::mcp::ServerContext {
                    project,
                    project_dir: resolve_project_dir()?,
                    client: client.map(|c| c.0),
                },
            )?;
        }
        Command::Show { id, version, json } => {
            let vault = open(&cli.db)?;
            let version = match version {
                Some(version) => version,
                None => vault.latest_version(&id)?,
            };
            let revision = vault.inspect(&id, version)?;
            if json {
                print_json(&revision)?;
            } else {
                println!(
                    "{} v{} ({}{}, base v{}, {})",
                    revision.id,
                    revision.version,
                    if revision.kind == "agent" {
                        "agent, "
                    } else {
                        ""
                    },
                    revision.status,
                    revision.expected_version,
                    revision.scope.as_deref().unwrap_or("global")
                );
                if let Some(meta) = &revision.meta {
                    println!("tier: {}", meta.tier);
                    println!("role: {}", meta.role);
                    if let Some(effort) = &meta.effort {
                        println!("effort: {effort}");
                    }
                    println!(
                        "tools: {}",
                        meta.tools
                            .as_ref()
                            .map_or("all".to_owned(), |tools| tools.join(", "))
                    );
                    if !meta.skills.is_empty() {
                        println!("skills: {}", meta.skills.join(", "));
                    }
                }
                if let Some(reviewed_at) = &revision.reviewed_at {
                    println!("reviewed: {reviewed_at}");
                }
                if let Some(note) = &revision.review_note {
                    println!("note: {note}");
                }
                println!("\nevidence:\n{}", revision.evidence);
                let diff = vault.diff(&id, version)?;
                // The meta JSON line is shown above as readable lines.
                let diff: Vec<&str> = diff
                    .lines()
                    .filter(|line| {
                        revision.meta.is_none()
                            || !line
                                .get(1..)
                                .is_some_and(|rest| rest.starts_with("meta: {"))
                    })
                    .collect();
                println!("\n{}", diff.join("\n"));
            }
        }
        Command::Publish {
            path,
            id,
            scope,
            project,
            evidence,
            replace,
        } => publish(&cli.db, path, id, scope, project, evidence, replace)?,
        Command::Search {
            query,
            limit,
            project,
            json,
        } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            let vault = open(&cli.db)?;
            let page = vault.search(&query, project.as_deref(), limit, 0)?;
            vault.log_event(
                skillvolution::vault::Event::new("search")
                    .project(project.as_deref())
                    .data(serde_json::json!({
                        "query": skillvolution::vault::search_query_for_event(&query),
                        "total": page.total,
                        "ids": page.skills.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
                    })),
            );
            print_skills(&page, json)?;
        }
        Command::List {
            kind,
            project,
            json,
        } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            let vault = open(&cli.db)?;
            let page = vault.search_kind("", project.as_deref(), kind.as_str(), 100, 0)?;
            if json {
                print_skills(&page, json)?;
            } else if kind == ListKind::Agent && page.skills.is_empty() {
                println!("no agents");
            } else if kind == ListKind::Agent {
                for s in &page.skills {
                    let tier = vault
                        .get(&s.id, None, project.as_deref())
                        .ok()
                        .and_then(|v| v.meta)
                        .map_or("?".to_owned(), |m| m.tier);
                    println!(
                        "{} v{} [{}] (tier {tier}, helped {}, failed {}): {}",
                        s.id,
                        s.version,
                        s.scope.as_deref().unwrap_or("global"),
                        s.helped,
                        s.failed,
                        s.description
                    );
                }
                if page.has_more {
                    println!("{} of {}", page.skills.len(), page.total);
                }
            } else {
                print_skills(&page, json)?;
            }
        }
        Command::Get {
            id,
            version,
            project,
        } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            if let Some(text) = skillvolution::agents::builtin_source(&id) {
                print!("{text}");
                return Ok(());
            }
            let skill = open(&cli.db)?.get(&id, version, project.as_deref())?;
            if skill.kind == "agent" {
                print_agent_file(&skill);
                return Ok(());
            }
            println!(
                "---\nname: {}\ndescription: {}\ntags: [{}]\n{}---\n{}",
                skill.id,
                skillvolution::skill_file::yaml_scalar(&skill.description),
                skill.tags.join(", "),
                if skill.scope.is_some() {
                    "scope: project\n"
                } else {
                    ""
                },
                skill.content.trim_end()
            );
        }
        Command::Deprecate { id } => open(&cli.db)?.set_deprecated(&id, true)?,
        Command::Undeprecate { id } => open(&cli.db)?.set_deprecated(&id, false)?,
        Command::Outcomes {
            id: Some(id), json, ..
        } => {
            let records = open(&cli.db)?.outcomes(&id)?;
            if json {
                print_json(&records)?;
            } else {
                for r in records {
                    println!(
                        "{} {} v{} {}: {}",
                        r.created_at, r.id, r.version, r.result, r.note
                    );
                }
            }
        }
        Command::Outcomes {
            id: None,
            failing,
            unreported,
            json,
        } => {
            let summaries = open(&cli.db)?.outcome_summaries(failing, unreported)?;
            if json {
                print_json(&summaries)?;
            } else {
                for s in summaries {
                    println!(
                        "{} v{}: helped {}, failed {}, not applicable {}, loaded {}, unreported {}",
                        s.id,
                        s.version,
                        s.helped,
                        s.failed,
                        s.not_applicable,
                        s.loaded,
                        s.unreported
                    );
                }
            }
        }
        Command::Telemetry(TelemetryCommand::Summary { since, json }) => {
            let report = open(&cli.db)?.usage_report(&since)?;
            if json {
                print_json(&report)?;
            } else {
                print!("{}", report.render());
            }
        }
        Command::Telemetry(TelemetryCommand::Events {
            kind,
            since,
            limit,
            json,
        }) => {
            let events = open(&cli.db)?.list_events(kind.as_deref(), since.as_deref(), limit)?;
            if json {
                print_json(&events)?;
            } else {
                for event in events {
                    println!("{}", event.render());
                }
            }
        }
        // Hooks fail open: a hook error must never block or break the client's
        // session, so it is reported on stderr and the process still exits 0.
        Command::Hook(event) => {
            let (hook, client) = event.origin();
            if let Err(error) = run_hook(event, &cli.db) {
                eprintln!("skillvolution hook: {error:#}");
                log_hook_error(&cli.db, hook, client, &error);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::HookClient;
    use clap::ValueEnum;

    #[test]
    fn hook_clients_are_the_registry_clients_with_hook_commands() {
        let tokens: Vec<String> = HookClient::value_variants()
            .iter()
            .map(|client| client.to_possible_value().unwrap().get_name().to_owned())
            .collect();
        assert_eq!(
            tokens,
            ["claude-code", "devin", "codex", "gemini", "cursor"]
        );
    }
}
