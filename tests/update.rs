//! `skillvolution update` refusals that need no network: everything here fails before
//! a release is queried.

#[path = "support/mod.rs"]
mod support;

use std::path::Path;
use std::process::Output;

/// `update ARGS` in a fresh sandbox home whose receipt directory is `receipts`.
fn update(home: &Path, receipts: &Path, args: &[&str]) -> Output {
    support::isolated(home)
        .env("AXOUPDATER_CONFIG_PATH", receipts)
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        .env_remove("GITHUB_TOKEN")
        .arg("--db")
        .arg(home.join("skills.db"))
        .arg("update")
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_copy_without_an_install_receipt_is_not_updated() {
    let home = tempfile::tempdir().unwrap();
    let receipts = tempfile::tempdir().unwrap();
    let output = update(home.path(), receipts.path(), &["--check"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("only updates a copy installed with install.sh"),
        "{err}"
    );
    assert!(err.contains("no install receipt"), "{err}");
    assert!(!home.path().join("skills.db").exists());
}

#[test]
fn a_receipt_for_another_install_is_not_used() {
    let home = tempfile::tempdir().unwrap();
    let receipts = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let receipt = serde_json::json!({
        "binaries": ["skillvolution"],
        "binary_aliases": {},
        "cdylibs": [],
        "cstaticlibs": [],
        "install_layout": "flat",
        "install_prefix": elsewhere.path().to_str().unwrap(),
        "modify_path": false,
        "provider": {"source": "cargo-dist", "version": "0.33.0"},
        "source": {
            "app_name": "skillvolution",
            "name": "Skillvolution",
            "owner": "KikeKnox",
            "release_type": "github"
        },
        "version": "0.7.0"
    });
    std::fs::write(
        receipts.path().join("skillvolution-receipt.json"),
        receipt.to_string(),
    )
    .unwrap();
    let output = update(home.path(), receipts.path(), &[]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("is not the copy the installer put in place"),
        "{err}"
    );
    assert!(!home.path().join("skills.db").exists());
}

#[test]
fn an_older_version_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let receipts = tempfile::tempdir().unwrap();
    let output = update(home.path(), receipts.path(), &["--version", "v0.0.1"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("refusing to downgrade"), "{err}");
    assert!(err.contains("SKILLVOLUTION_VERSION=0.0.1"), "{err}");
}

#[test]
fn the_running_version_is_already_installed() {
    let home = tempfile::tempdir().unwrap();
    let receipts = tempfile::tempdir().unwrap();
    let current = env!("CARGO_PKG_VERSION");
    let output = update(home.path(), receipts.path(), &["--version", current]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("is already installed"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_malformed_version_is_rejected() {
    let home = tempfile::tempdir().unwrap();
    let receipts = tempfile::tempdir().unwrap();
    let output = update(home.path(), receipts.path(), &["--version", "latest"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("is not a version like 0.8.0"));
}
