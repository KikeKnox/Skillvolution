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
        /// Print the outcome log as JSON instead of the readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Hook entry points for Claude Code, Devin CLI, Codex CLI, Gemini CLI, and Cursor.
    #[command(subcommand)]
    Hook(HookEvent),
    /// Set up the Skillvolution MCP server for a project or globally, detect clients,
    /// configure them, or remove Skillvolution from them. Global setup is the default and
    /// is shared by every project. Use --project for project-specific setup, --remove to
    /// undo, or --dry-run to preview.
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
    },
    /// Move the vault database to a new local path and repoint configured clients.
    Relocate {
        /// New database path.
        path: PathBuf,
    },
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
            Self::ClaudeCode => context,
            Self::Devin | Self::Codex | Self::Gemini => serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": context,
                }
            })
            .to_string(),
            Self::Cursor => serde_json::json!({ "additional_context": context }).to_string(),
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

/// Resolves the project key for `serve` and `hook session-start`: an explicit
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
        Command::Doctor { json } => {
            if !skillvolution::doctor::run(&db_path(&cli.db)?, json)? {
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
            json,
        } => {
            let summaries = open(&cli.db)?.outcome_summaries(failing)?;
            if json {
                print_json(&summaries)?;
            } else {
                for s in summaries {
                    println!(
                        "{} v{}: helped {}, failed {}, not applicable {}",
                        s.id, s.version, s.helped, s.failed, s.not_applicable
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
