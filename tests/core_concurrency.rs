#[path = "support/mod.rs"]
mod support;

use skillvolution::vault::Vault;
use std::sync::{Arc, Barrier};
use support::Draft;

#[test]
fn simultaneous_connections_publish_distinct_skills_without_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    Vault::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut vault = Vault::open(&path).unwrap();
                barrier.wait();
                Draft::new(&format!("skill-{i}"))
                    .propose(&mut vault)
                    .version
            })
        })
        .collect();
    let versions: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(versions, vec![1; 8]);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.search("", None, 20, 0).unwrap().total, 8);
}

#[test]
fn competing_publishers_have_exactly_one_winner() {
    // Proposals publish immediately against an expected base, so concurrent
    // proposals for the same new id cannot all succeed.
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
                vault.propose(&Draft::new("shared").proposal()).is_ok()
            })
        })
        .collect();
    let winners = threads
        .into_iter()
        .filter_map(|t| t.join().unwrap().then_some(()))
        .count();
    assert_eq!(winners, 1);
    let vault = Vault::open(&path).unwrap();
    assert_eq!(vault.get("shared", None, None).unwrap().version, 1);
}

#[test]
fn opening_a_fresh_database_concurrently_never_fails_with_busy() {
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skills.db");
        let barrier = Arc::new(Barrier::new(10));
        let threads: Vec<_> = (0..10)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    Vault::open(&path)
                })
            })
            .collect();
        for thread in threads {
            thread
                .join()
                .unwrap()
                .expect("opening a brand-new database concurrently should never fail");
        }
    }
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
