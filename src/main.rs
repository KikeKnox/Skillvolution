use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use skillvolution::{hook, setup, vault::Vault};
use std::{io::Read, path::PathBuf};

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
enum Command {
    /// Start the MCP server on stdio.
    Serve {
        /// Project key used for project-scoped skills and outcome records.
        /// Detected automatically from the working directory's git root when omitted,
        /// or from CLAUDE_PROJECT_DIR.
        #[arg(long)]
        project: Option<String>,
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
    /// Publish a SKILL.md file to the vault yourself (you are the evaluator).
    Publish {
        /// A SKILL.md file, a directory containing one, or a directory of skill directories
        /// (every PATH/*/SKILL.md is published; e.g. ~/.claude/skills).
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
    /// List the skills visible from the project.
    List {
        /// Project key; detected like `serve` when omitted.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a skill as a SKILL.md that `publish --replace` accepts; records nothing.
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
    /// Move the vault database to a new local path and repoint configured clients.
    Relocate {
        /// New database path.
        path: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PublishScope {
    Global,
    Project,
}

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

/// The skill file's frontmatter fields and body.
#[derive(Default)]
struct SkillFile {
    name: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    scope: Option<PublishScope>,
    content: String,
    problems: Vec<String>,
}

fn unquote(value: &str) -> &str {
    let v = value.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|v| v.strip_suffix(q))
            && !inner.contains(q)
        {
            return inner;
        }
    }
    v
}

/// `value` without a YAML-style trailing comment: a `#` at the start or after
/// whitespace, outside a quoted value ("C#" and "a#b" keep theirs).
fn strip_comment(value: &str) -> &str {
    let v = value.trim();
    // A leading quoted string can hold a " #" of its own: look past its closing quote.
    let from = v
        .strip_prefix(['"', '\''])
        .and_then(|rest| rest.find(&v[..1]))
        .map_or(0, |end| end + 2);
    v.match_indices('#')
        .find(|&(i, _)| i >= from && (i == 0 || v[..i].ends_with(char::is_whitespace)))
        .map_or(v, |(i, _)| v[..i].trim_end())
}

fn parse_skill_file(text: &str) -> SkillFile {
    let text = text.replace("\r\n", "\n");
    let text = text.as_str();
    let mut file = SkillFile::default();
    let mut body = text;
    let mut lines = text.lines();
    if lines.next().is_some_and(|l| l.trim_end() == "---") {
        let mut consumed = text.lines().next().unwrap().len() + 1;
        let mut closed = false;
        for line in lines {
            consumed += line.len() + 1;
            if line.trim_end() == "---" {
                body = text.get(consumed..).unwrap_or("");
                closed = true;
                break;
            }
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let (key, value) = (key.trim_end(), strip_comment(value));
                if matches!(key, "name" | "description" | "tags")
                    && matches!(value, "" | ">" | "|" | ">-" | "|-")
                {
                    file.problems.push(if key == "tags" {
                        "tags: multi-line YAML values are not supported; write tags inline as [a, b]"
                            .into()
                    } else {
                        format!("{key}: multi-line YAML values are not supported; write it on one line")
                    });
                    continue;
                }
                match key {
                    "name" => file.name = Some(unquote(value).to_owned()),
                    "description" => file.description = Some(unquote(value).to_owned()),
                    "tags" => {
                        file.tags = value
                            .trim_start_matches('[')
                            .trim_end_matches(']')
                            .split(',')
                            .map(|t| unquote(t).to_owned())
                            .filter(|t| !t.is_empty())
                            .collect()
                    }
                    "scope" => match unquote(value) {
                        "global" => file.scope = Some(PublishScope::Global),
                        "project" => file.scope = Some(PublishScope::Project),
                        other => file
                            .problems
                            .push(format!("scope: `{other}` is not global or project")),
                    },
                    _ => {}
                }
            }
        }
        if !closed {
            file.problems
                .push("frontmatter starts with --- but is never closed by a --- line".into());
        }
    }
    file.content = body
        .lines()
        .filter(|l| !(l.trim_start().starts_with("<!-- skillvolution-managed:")))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start_matches('\n')
        .to_owned();
    file
}

/// The outcome of publishing one skill file.
enum Published {
    Done {
        id: String,
        version: i64,
        scope: &'static str,
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
    let file = parse_skill_file(&text);
    // A file named SKILL.md without a `name` takes its directory's name.
    let fallback = path
        .file_name()
        .is_some_and(|n| n == "SKILL.md")
        .then(|| std::path::absolute(path).ok())
        .flatten()
        .and_then(|p| {
            p.parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        });
    let id = id.or(file.name.clone()).or(fallback);
    let mut problems = skillvolution::vault::skill_problems(
        id.as_deref(),
        file.description.as_deref(),
        &file.tags,
        &file.content,
    );
    problems.extend(file.problems.iter().cloned());
    let scope = opts.scope.or(file.scope).unwrap_or(PublishScope::Global);
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
        if opts.skip_existing {
            return Ok(Ok(Published::Skipped {
                id,
                version: current,
            }));
        }
        anyhow::bail!(
            "skill {id} already exists at v{current}; pass --replace to publish v{}",
            current + 1
        );
    }
    let project_scope = project.as_deref();
    let revision = vault.propose(&skillvolution::vault::Proposal {
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
    })?;
    Ok(Ok(Published::Done {
        id: revision.id,
        version: revision.version,
        scope: project_scope.map_or("global", |_| "project"),
    }))
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
    let template = format!(
        "A SKILL.md looks like this:\n\n{SKILL_TEMPLATE}\n\nSee 'Publishing a skill manually' in README.md."
    );
    let mut opts = PublishOptions {
        scope,
        project: &project,
        evidence: &evidence,
        replace,
        skip_existing: false,
    };
    if path.is_dir() && !path.join("SKILL.md").is_file() {
        anyhow::ensure!(
            id.is_none(),
            "--id cannot be used when publishing a directory of skills"
        );
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .with_context(|| format!("read {}", path.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path().join("SKILL.md"))
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        anyhow::ensure!(
            !files.is_empty(),
            "no SKILL.md in {} or in its subdirectories",
            path.display()
        );
        opts.skip_existing = true;
        let mut vault = open(db)?;
        let (mut done, mut skipped, mut rejected) = (0, 0, 0);
        for file in &files {
            let result = publish_file(&mut vault, file, None, &opts)
                .unwrap_or_else(|e| Err(vec![format!("{e:#}")]));
            match result {
                Ok(Published::Done { id, version, scope }) => {
                    done += 1;
                    println!("[published] {id} v{version} ({scope})");
                }
                Ok(Published::Skipped { id, version }) => {
                    skipped += 1;
                    println!("[skipped] {id}: already at v{version}; pass --replace");
                }
                Err(problems) => {
                    rejected += 1;
                    println!("[rejected] {}:", file.display());
                    for problem in problems {
                        println!("  - {problem}");
                    }
                }
            }
        }
        if rejected > 0 {
            println!("\n{template}");
        }
        println!("\n{done} published, {skipped} skipped, {rejected} rejected");
        if rejected > 0 {
            std::process::exit(1);
        }
        return Ok(());
    }
    let path = if path.is_dir() {
        path.join("SKILL.md")
    } else {
        path
    };
    let mut vault = open(db)?;
    match publish_file(&mut vault, &path, id, &opts)? {
        Ok(Published::Done { id, version, scope }) => {
            println!("Published {id} v{version} ({scope}).")
        }
        Ok(Published::Skipped { .. }) => unreachable!("single-file publish never skips"),
        Err(problems) => {
            let list: String = problems.iter().map(|p| format!("- {p}\n")).collect();
            anyhow::bail!(
                "{} cannot be published:\n{list}\n{template}",
                path.display()
            );
        }
    }
    Ok(())
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
/// for Gemini CLI's AfterAgent, `{"followup_message"}` for Cursor).
#[derive(Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
enum HookClient {
    #[default]
    ClaudeCode,
    Devin,
    Codex,
    Gemini,
    Cursor,
}

impl HookClient {
    /// The file-editing tool names `tool-use` counts as work for this client.
    fn work_tools(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeCode => hook::WORK_TOOLS,
            Self::Devin => hook::DEVIN_WORK_TOOLS,
            Self::Codex => hook::CODEX_WORK_TOOLS,
            Self::Gemini => hook::GEMINI_WORK_TOOLS,
            Self::Cursor => hook::CURSOR_WORK_TOOLS,
        }
    }

    /// The SessionStart output that injects `context` into this client's session.
    fn session_context(self, context: String) -> String {
        match self {
            Self::ClaudeCode | Self::Devin | Self::Codex | Self::Gemini => serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": context,
                }
            })
            .to_string(),
            Self::Cursor => serde_json::json!({ "additional_context": context }).to_string(),
        }
    }

    /// The prompt-submit event whose `hookSpecificOutput.additionalContext` this client
    /// adds to the model's context: UserPromptSubmit for Claude Code, Codex, and Devin,
    /// BeforeAgent for Gemini CLI. `None` for Cursor, whose beforeSubmitPrompt can only
    /// allow or block the prompt, not add context to it.
    fn prompt_event(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode | Self::Devin | Self::Codex => Some("UserPromptSubmit"),
            Self::Gemini => Some("BeforeAgent"),
            Self::Cursor => None,
        }
    }

    /// The Stop output asking a flag-tracked client to continue with `reason`.
    fn continue_with(self, reason: &str) -> serde_json::Value {
        match self {
            Self::ClaudeCode | Self::Devin | Self::Codex => {
                serde_json::json!({"decision": "block", "reason": reason})
            }
            Self::Gemini => serde_json::json!({"decision": "deny", "reason": reason}),
            Self::Cursor => serde_json::json!({ "followup_message": reason }),
        }
    }
}

#[derive(Subcommand)]
enum HookEvent {
    /// Print the skill catalog as session context.
    SessionStart {
        /// Project key for project-scoped skills.
        /// Detected automatically from the working directory's git root when omitted,
        /// or from CLAUDE_PROJECT_DIR.
        #[arg(long)]
        project: Option<String>,
        /// The client emitting the hook: claude-code, devin, codex, gemini, or cursor.
        /// Defaults to claude-code.
        #[arg(long, value_enum, default_value_t = HookClient::ClaudeCode)]
        client: HookClient,
    },
    /// Suggest the vault skills matching the submitted prompt (UserPromptSubmit, or
    /// Gemini CLI's BeforeAgent); prints nothing when none is relevant.
    Prompt {
        /// Project key for project-scoped skills.
        /// Detected automatically from the working directory's git root when omitted,
        /// or from CLAUDE_PROJECT_DIR.
        #[arg(long)]
        project: Option<String>,
        /// The client emitting the hook: claude-code, devin, codex, or gemini (cursor
        /// cannot inject prompt context). Defaults to claude-code.
        #[arg(long, value_enum, default_value_t = HookClient::ClaudeCode)]
        client: HookClient,
    },
    /// Record work/review tool calls for a session (PostToolUse of the flag-tracked
    /// clients: Devin, Codex, Gemini CLI, Cursor).
    ToolUse {
        /// The client emitting the hook: claude-code, devin, codex, gemini, or cursor.
        /// Defaults to devin.
        #[arg(long, value_enum, default_value_t = HookClient::Devin)]
        client: HookClient,
    },
    /// Ask for an evolution review after unreviewed work.
    Stop {
        /// The client emitting the hook: claude-code, devin, codex, gemini, or cursor.
        /// Defaults to claude-code.
        #[arg(long, value_enum, default_value_t = HookClient::ClaudeCode)]
        client: HookClient,
    },
    /// Drop a session's hook state (Devin SessionEnd).
    SessionEnd,
    /// Approve the tools the evolution flow needs (Devin PermissionRequest).
    Approve,
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
        HookEvent::SessionStart { project, client } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            let context = hook::session_start(&open(db)?, project.as_deref())?;
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
            if let Some(context) = hook::prompt(&open(db)?, project.as_deref(), &input)? {
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
        HookEvent::ToolUse { client } => {
            hook::tool_use(&open(db)?, &read_stdin()?, client.work_tools())?
        }
        HookEvent::Stop { client } => {
            let input = read_stdin()?;
            match client {
                HookClient::ClaudeCode => {
                    if let Some(reason) = hook::stop(&open(db)?, &input)? {
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
                flagged => {
                    if let Some(reason) = hook::devin_stop(&open(db)?, &input)? {
                        println!("{}", flagged.continue_with(reason));
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
        Command::Purge { id, version } => {
            let report = open(&cli.db)?.purge(&id, version)?;
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
            print_json(&open(&cli.db)?.import_json(&json)?)?;
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
        Command::Relocate { path } => skillvolution::relocate::run(&db_path(&cli.db)?, &path)?,
        Command::Setup(args) => setup::run(args.with_default_db(cli.db))?,
        Command::Serve { project } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            eprintln!(
                "skillvolution: project = {}",
                project.as_deref().unwrap_or("none")
            );
            skillvolution::mcp::serve(open(&cli.db)?, project)?;
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
                    "{} v{} ({}, base v{}, {})",
                    revision.id,
                    revision.version,
                    revision.status,
                    revision.expected_version,
                    revision.scope.as_deref().unwrap_or("global")
                );
                if let Some(reviewed_at) = &revision.reviewed_at {
                    println!("reviewed: {reviewed_at}");
                }
                if let Some(note) = &revision.review_note {
                    println!("note: {note}");
                }
                println!("\nevidence:\n{}", revision.evidence);
                println!("\n{}", vault.diff(&id, version)?);
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
            let page = open(&cli.db)?.search(&query, project.as_deref(), limit, 0)?;
            print_skills(&page, json)?;
        }
        Command::List { project, json } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            let page = open(&cli.db)?.search("", project.as_deref(), 100, 0)?;
            print_skills(&page, json)?;
        }
        Command::Get {
            id,
            version,
            project,
        } => {
            validate_project(&project)?;
            let project = resolve_project(project)?;
            let skill = open(&cli.db)?.get(&id, version, project.as_deref())?;
            println!(
                "---\nname: {}\ndescription: {}\ntags: [{}]\n{}---\n{}",
                skill.id,
                skill.description,
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
        // Hooks fail open: a hook error must never block or break the client's
        // session, so it is reported on stderr and the process still exits 0.
        Command::Hook(event) => {
            if let Err(error) = run_hook(event, &cli.db) {
                eprintln!("skillvolution hook: {error:#}");
            }
        }
    }
    Ok(())
}
