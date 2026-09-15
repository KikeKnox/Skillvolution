mod claude;
mod fs_safe;
mod opencode;

use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const SKILL: &str = include_str!("../../assets/evolution/SKILL.md");

#[derive(clap::Args)]
pub struct SetupArgs {
    #[arg(long)]
    project: PathBuf,
    #[arg(long, default_value = "both", value_parser = ["both", "opencode", "claude-code"])]
    client: String,
    #[arg(long)]
    bin: PathBuf,
    #[arg(long)]
    db: PathBuf,
    /// Project scope key for the MCP server; defaults to the project directory name.
    #[arg(long)]
    project_key: Option<String>,
}

pub fn run(args: SetupArgs) -> Result<()> {
    for path in [&args.project, &args.bin, &args.db] {
        fs_safe::check_path(path)?;
    }
    let project = fs::canonicalize(&args.project).context("project must exist")?;
    let bin = fs::canonicalize(&args.bin).context("binary must exist")?;
    let db = std::path::absolute(&args.db)?;
    ensure!(project.is_dir(), "project must be a directory");
    ensure!(bin.is_file(), "binary must be a regular file");
    ensure!(
        !db.exists() || db.is_file(),
        "database must be a regular file or a new path"
    );
    let key = resolve_project_key(args.project_key.as_deref(), &project)?;

    let mut changes = Vec::new();
    if args.client != "claude-code" {
        changes.extend(opencode::changes(&project, &bin, &db, &key)?);
    }
    if args.client != "opencode" {
        changes.extend(claude::changes(&project, &bin, &db, &key)?);
    }

    for (path, _) in &changes {
        fs_safe::check_path(path)?;
        if path.exists() {
            fs_safe::backup_path(path)?;
            ensure!(path.is_file(), "not a regular file: {}", path.display());
        }
        for parent in path.ancestors().skip(1) {
            if parent.exists() {
                ensure!(parent.is_dir(), "not a directory: {}", parent.display());
            }
        }
    }
    for (path, content) in changes {
        fs_safe::write(&path, &content).with_context(|| {
            format!(
                "setup failed at {}; earlier changes may exist; inspect .skillvolution.bak backups",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn resolve_project_key(explicit: Option<&str>, project: &Path) -> Result<String> {
    let key = match explicit {
        Some(key) => key.to_owned(),
        None => default_project_key(project).with_context(|| {
            format!(
                "cannot derive a project key from {}; pass --project-key",
                project.display()
            )
        })?,
    };
    crate::vault::validate_id(&key).context("invalid --project-key")?;
    Ok(key)
}

/// Lowercases the directory name, collapses runs of non `[a-z0-9]` bytes into a single `-`,
/// trims leading/trailing `-`, then truncates to 64 bytes and re-trims.
fn default_project_key(project: &Path) -> Option<String> {
    let name = project.file_name()?.to_str()?;
    let mut key = String::new();
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            key.push(lower);
        } else if !key.ends_with('-') {
            key.push('-');
        }
    }
    let key = key.trim_matches('-');
    let mut truncated = key.as_bytes();
    if truncated.len() > 64 {
        truncated = &truncated[..64];
    }
    // Every byte is ASCII (a-z, 0-9, or '-'), so byte truncation never splits a char.
    let key = std::str::from_utf8(truncated).unwrap().trim_matches('-');
    (!key.is_empty()).then(|| key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::default_project_key;
    use std::path::Path;

    #[test]
    fn derives_default_keys() {
        assert_eq!(
            default_project_key(Path::new("/tmp/project with spaces")).as_deref(),
            Some("project-with-spaces")
        );
        assert_eq!(
            default_project_key(Path::new("/tmp/__café__")).as_deref(),
            Some("caf")
        );
        assert_eq!(default_project_key(Path::new("/tmp/---")), None);
    }
}
