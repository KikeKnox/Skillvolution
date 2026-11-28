//! `skillvolution update`: replaces the installed binary with a newer release through the
//! cargo-dist installer (via `axoupdater`, which verifies the archive), after backing up
//! the vault, then has the new binary refresh what `setup` wrote (`doctor --fix`).

use crate::vault::Vault;
use anyhow::{Context, Result, anyhow, bail};
use axoupdater::{AxoUpdater, UpdateRequest, Version};
use std::path::{Path, PathBuf};
use std::process::Command;

const APP: &str = "skillvolution";

/// What `update` was asked to do.
pub struct UpdateArgs {
    /// A release to install instead of the latest (`0.8.0` or `v0.8.0`).
    pub version: Option<String>,
    /// Only report whether an update is available.
    pub check: bool,
    /// Skip the `doctor --fix` refresh after installing.
    pub no_setup: bool,
}

/// Runs the update. `db` is the vault to back up; `db_flag` is the `--db` the user passed,
/// forwarded to the new binary. Returns whether everything succeeded (the refresh's exit
/// status included).
pub fn run(db: &Path, db_flag: Option<&Path>, args: UpdateArgs) -> Result<bool> {
    let current = current_version();
    let requested = args.version.as_deref().map(parse_version).transpose()?;
    if let Some(requested) = &requested {
        check_not_downgrade(&current, requested)?;
        if *requested == current {
            println!("skillvolution v{current} is already installed.");
            return Ok(true);
        }
    }

    // Captured before the installer replaces the file: afterwards, on Linux, the running
    // process's own path can read as "(deleted)".
    let exe = std::env::current_exe().context("locate the running skillvolution binary")?;
    let mut updater = AxoUpdater::new_for(APP);
    updater.load_receipt().map_err(|error| {
        anyhow!(
            "{error}\n{}",
            not_installer_hint("no install receipt was found")
        )
    })?;
    if !updater.check_receipt_is_for_this_executable()? {
        bail!(
            "{}",
            not_installer_hint(&format!(
                "{} is not the copy the installer put in place",
                exe.display()
            ))
        );
    }
    updater.set_current_version(current.clone())?;
    if let Some(requested) = &requested {
        updater.configure_version_specifier(UpdateRequest::SpecificTag(format!("v{requested}")));
    }
    if let Some(token) = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()) {
        updater.set_github_token(&token);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start the async runtime")?;
    let target = runtime
        .block_on(updater.query_new_version())?
        .cloned()
        .context("no release was found")?;
    if requested.is_none() && target <= current {
        println!("skillvolution v{current} is up to date.");
        return Ok(true);
    }
    if args.check {
        println!("Update available: v{current} -> v{target}. Run `skillvolution update`.");
        return Ok(true);
    }

    if db.exists() {
        let backup = backup_path(db, &target);
        Vault::open(db)
            .with_context(|| format!("open {}", db.display()))?
            .backup(&backup)
            .context("back up the vault before updating")?;
        println!("Backed up the vault to {}.", backup.display());
    }

    println!("Installing skillvolution v{target}...");
    let Some(result) = runtime.block_on(updater.run())? else {
        println!("skillvolution v{current} is up to date.");
        return Ok(true);
    };
    println!("Installed skillvolution v{}.", result.new_version);

    if args.no_setup {
        println!(
            "Run `skillvolution doctor --fix` (or `skillvolution setup`) to refresh the \
             skills, hooks, and plugin your clients use."
        );
        return Ok(true);
    }
    println!("Refreshing the configured clients (`skillvolution doctor --fix`)...");
    let mut refresh = Command::new(&exe);
    if let Some(db) = db_flag {
        refresh.arg("--db").arg(db);
    }
    let status = refresh
        .args(["doctor", "--fix"])
        .status()
        .with_context(|| format!("run {} doctor --fix", exe.display()))?;
    println!(
        "Project-level installs are not refreshed: rerun `skillvolution setup --project DIR` \
         for each one."
    );
    Ok(status.success())
}

/// The running binary's version.
fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("the crate version is valid semver")
}

/// `0.8.0` or `v0.8.0`.
fn parse_version(text: &str) -> Result<Version> {
    let trimmed = text.trim();
    let bare = trimmed.strip_prefix('v').unwrap_or(trimmed);
    bare.parse::<Version>()
        .map_err(|_| anyhow!("{text:?} is not a version like 0.8.0"))
}

/// A vault migrated by the running version may not open with an older one, so `update`
/// never goes back; the installer can, on purpose.
fn check_not_downgrade(current: &Version, requested: &Version) -> Result<()> {
    if requested < current {
        bail!(
            "refusing to downgrade from v{current} to v{requested}: a vault this version \
             migrated may not open with an older one. To install it anyway, rerun the \
             installer with SKILLVOLUTION_VERSION={requested}."
        );
    }
    Ok(())
}

/// `<db>.before-v<version>.bak`, or the first free `.bak.N` after it.
fn backup_path(db: &Path, target: &Version) -> PathBuf {
    let base = format!("{}.before-v{target}.bak", db.display());
    let mut candidate = PathBuf::from(&base);
    let mut n = 1;
    while candidate.exists() {
        candidate = PathBuf::from(format!("{base}.{n}"));
        n += 1;
    }
    candidate
}

fn not_installer_hint(reason: &str) -> String {
    format!(
        "`skillvolution update` only updates a copy installed with install.sh / install.ps1 \
         ({reason}). Update this one the way you installed it (for example \
         `cargo install --locked --path .`), or rerun the installer."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_with_or_without_the_v() {
        assert_eq!(
            parse_version("v0.8.0").unwrap(),
            parse_version("0.8.0").unwrap()
        );
        assert!(parse_version("latest").is_err());
        assert!(parse_version("0.8").is_err());
    }

    #[test]
    fn only_downgrades_are_refused() {
        let v = |s| parse_version(s).unwrap();
        assert!(check_not_downgrade(&v("0.7.0"), &v("0.6.0")).is_err());
        assert!(check_not_downgrade(&v("0.7.0"), &v("0.7.0")).is_ok());
        assert!(check_not_downgrade(&v("0.7.0"), &v("0.8.0")).is_ok());
    }

    #[test]
    fn backups_never_overwrite_an_earlier_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("skills.db");
        let target = parse_version("0.8.0").unwrap();
        let first = backup_path(&db, &target);
        assert_eq!(first, dir.path().join("skills.db.before-v0.8.0.bak"));
        std::fs::write(&first, "").unwrap();
        let second = backup_path(&db, &target);
        assert_eq!(second, dir.path().join("skills.db.before-v0.8.0.bak.1"));
    }
}
