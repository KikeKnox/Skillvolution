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
    /// Initialize the shared database (idempotent).
    Init,
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
    /// Show one revision, including its evidence.
    Show {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
    },
    /// Show a draft as a unified diff against its base revision.
    Diff {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
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

fn default_database() -> Result<PathBuf> {
    let nonempty_env = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = nonempty_env("XDG_DATA_HOME")
        .filter(|path| path.is_absolute())
        .or_else(|| {
            nonempty_env("HOME")
                .or_else(|| nonempty_env("USERPROFILE"))
                .map(|home| home.join(".local/share"))
        })
        .ok_or_else(|| anyhow::anyhow!("set --db, XDG_DATA_HOME, or HOME"))?;
    Ok(base.join("skillvolution/skills.db"))
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Setup(args) = cli.command {
        return setup::run(args);
    }
    let db = cli.db.map(Ok).unwrap_or_else(default_database)?;
    let open = || Vault::open(&db).with_context(|| format!("open {}", db.display()));
    match cli.command {
        Command::Init => {
            open()?;
            println!("Initialized {}", db.display());
        }
        Command::Serve { project } => {
            validate_project(&project)?;
            skillvolution::mcp::serve(open()?, project)?;
        }
        Command::Drafts { json } => {
            let drafts = open()?.drafts()?;
            if json {
                print_json(&drafts)?;
            } else if drafts.is_empty() {
                println!("No drafts.");
            } else {
                for draft in drafts {
                    println!(
                        "{} v{} (base v{}, {}, by {}): {}",
                        draft.id,
                        draft.version,
                        draft.expected_version,
                        draft.scope.as_deref().unwrap_or("global"),
                        draft.client.as_deref().unwrap_or("unknown"),
                        draft.description
                    );
                }
            }
        }
        Command::Show { id, version } => print_json(&open()?.inspect(&id, version)?)?,
        Command::Diff { id, version } => print!("{}", open()?.diff(&id, version)?),
        Command::Publish { id, version } => print_json(&open()?.publish(&id, version)?)?,
        Command::Reject { id, version, note } => {
            print_json(&open()?.reject(&id, version, note.as_deref())?)?
        }
        Command::Deprecate { id } => open()?.set_deprecated(&id, true)?,
        Command::Undeprecate { id } => open()?.set_deprecated(&id, false)?,
        Command::Outcomes {
            id: Some(id), json, ..
        } => {
            let records = open()?.outcomes(&id)?;
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
            let summaries = open()?.outcome_summaries(failing)?;
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
            print!("{}", hook::session_start(&open()?, project.as_deref())?);
        }
        Command::Hook(HookEvent::Stop) => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            if let Some(reason) = hook::stop(&open()?, &input)? {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
        Command::Setup(_) => unreachable!(),
    }
    Ok(())
}
