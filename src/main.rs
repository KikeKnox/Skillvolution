use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use skillvolution::{setup, vault::Vault};
use std::path::PathBuf;

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
    Serve,
    /// List unpublished drafts.
    Drafts,
    /// Show one revision by id and integer version.
    Show {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
    },
    /// Publish a previously proposed draft revision.
    Publish {
        id: String,
        #[arg(long, value_name = "VERSION")]
        version: i64,
    },
    /// Configure a project to use the Skillvolution MCP.
    Setup(setup::SetupArgs),
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = cli.db.map(Ok).unwrap_or_else(default_database)?;
    match cli.command {
        Command::Init => {
            Vault::open(&db).with_context(|| format!("initialize {}", db.display()))?;
            println!("Initialized {}", db.display());
            Ok(())
        }
        Command::Serve => skillvolution::mcp::serve(db),
        Command::Drafts => {
            let vault = Vault::open(&db)?;
            let drafts = vault.drafts()?;
            println!("{}", serde_json::to_string_pretty(&drafts)?);
            Ok(())
        }
        Command::Show { id, version } => {
            let vault = Vault::open(&db)?;
            let revision = vault.inspect(&id, version)?;
            println!("{}", serde_json::to_string_pretty(&revision)?);
            Ok(())
        }
        Command::Publish { id, version } => {
            let mut vault = Vault::open(&db)?;
            let revision = vault.publish(&id, version)?;
            println!("{}", serde_json::to_string_pretty(&revision)?);
            Ok(())
        }
        Command::Setup(args) => setup::run(args),
    }
}
