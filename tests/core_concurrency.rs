#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::Vault;
use std::sync::{Arc, Barrier};
use support::Draft;

#[test]
fn simultaneous_connections_allocate_unique_persistent_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    Vault::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut vault = Vault::open(&path).unwrap();
                barrier.wait();
                Draft::new("shared").propose(&mut vault).version
            })
        })
        .collect();
    let mut versions: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    versions.sort();
    assert_eq!(versions, (1..=8).collect::<Vec<_>>());
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.drafts().unwrap().len(), 8);
}

#[test]
fn competing_publishers_have_exactly_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    for _ in 0..2 {
        Draft::new("shared").propose(&mut vault);
    }
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (1..=2)
        .map(|version| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut vault = Vault::open(&path).unwrap();
                barrier.wait();
                vault.publish("shared", version).is_ok()
            })
        })
        .collect();
    let winners = threads
        .into_iter()
        .filter_map(|t| t.join().unwrap().then_some(()))
        .count();
    assert_eq!(winners, 1);
    assert!(vault.get("shared", None, None).is_ok());
}

#[test]
fn a_writer_waits_for_an_existing_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    let mut vault = Vault::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        send.send(()).unwrap();
        Draft::new("shared").propose(&mut vault)
    });
    receive.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    conn.execute_batch("COMMIT").unwrap();
    assert_eq!(thread.join().unwrap().version, 1);
}
