use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use skillvolution::{hook, setup, vault::Vault};
use std::{io::Read, path::PathBuf};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
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
        #[arg(long)]
        project: Option<String>,
    },
    /// List drafts awaiting review.
    Drafts {
        #[arg(long)]
        json: bool,
    },
    /// Show one revision: its status, evidence, and a diff against its base.
    Show {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
        /// Print the full revision as JSON instead of the readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Publish a draft revision.
    Publish {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
    },
    /// Reject a draft revision.
    Reject {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
        #[arg(long)]
        note: Option<String>,
    },
    /// Hide a skill from search without deleting its history.
    Deprecate { id: String },
    /// Make a deprecated skill searchable again.
    Undeprecate { id: String },
    /// Summarize skill outcomes, or list the outcome log of one skill.
    Outcomes {
        id: Option<String>,
        /// Only skills whose current version has failures.
        #[arg(long, conflicts_with = "id")]
        failing: bool,
        #[arg(long)]
        json: bool,
    },
    /// Claude Code hook entry points.
    #[command(subcommand)]
    Hook(HookEvent),
    /// Configure a project to use the Skillvolution MCP.
    Setup(setup::SetupArgs),
}

#[derive(Subcommand)]
enum HookEvent {
    /// Print the skill catalog as session context.
    SessionStart {
        #[arg(long)]
        project: Option<String>,
    },
    /// Ask for an evolution review after unreviewed work (exit 2 blocks the stop).
    Stop,
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

/// Opens the database at `--db`, or the default location (setup and every
/// command create it on first open, so there is no separate init step).
fn open(db: &Option<PathBuf>) -> Result<Vault> {
    let path = db
        .clone()
        .map(Ok)
        .unwrap_or_else(skillvolution::vault::default_database)?;
    Vault::open(&path).with_context(|| format!("open {}", path.display()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Setup(args) => setup::run(args.with_default_db(cli.db))?,
        Command::Serve { project } => {
            validate_project(&project)?;
            skillvolution::mcp::serve(open(&cli.db)?, project)?;
        }
        Command::Drafts { json } => {
            let drafts = open(&cli.db)?.drafts()?;
            if json {
                print_json(&drafts)?;
            } else if drafts.is_empty() {
                println!("No drafts.");
            } else {
                for draft in drafts {
                    println!(
                        "{} v{} (base v{}, {}): {}",
                        draft.id,
                        draft.version,
                        draft.expected_version,
                        draft.scope.as_deref().unwrap_or("global"),
                        draft.description
                    );
                }
            }
        }
        Command::Show { id, version, json } => {
            let vault = open(&cli.db)?;
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
        Command::Publish { id, version } => {
            let version = open(&cli.db)?.publish(&id, version)?;
            println!("Published {id} v{version}");
        }
        Command::Reject { id, version, note } => {
            let version = open(&cli.db)?.reject(&id, version, note.as_deref())?;
            println!("Rejected {id} v{version}");
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
        Command::Hook(HookEvent::SessionStart { project }) => {
            validate_project(&project)?;
            print!(
                "{}",
                hook::session_start(&open(&cli.db)?, project.as_deref())?
            );
        }
        Command::Hook(HookEvent::Stop) => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            if let Some(reason) = hook::stop(&open(&cli.db)?, &input)? {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
    }
    Ok(())
}
