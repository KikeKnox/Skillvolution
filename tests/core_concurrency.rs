#[path = "support/mod.rs"]
mod support;

use skillvolution::{hook, vault::Vault};
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

#[test]
fn parallel_devin_tool_uses_never_lose_the_work_flag() {
    // Devin runs PostToolUse hooks in parallel, one process each. Whichever of
    // a work and a review tool lands last, the session must stay marked as
    // worked; a read-modify-write update lets a review overwrite the work flag.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skills.db");
    Vault::open(&path).unwrap();
    for round in 0..20 {
        let session = format!("s{round}");
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                let barrier = barrier.clone();
                let tool = if i % 2 == 0 {
                    "write"
                } else {
                    "mcp__skillvolution__report_skill_outcome"
                };
                let input =
                    serde_json::json!({"session_id": session, "tool_name": tool}).to_string();
                std::thread::spawn(move || {
                    let vault = Vault::open(&path).unwrap();
                    barrier.wait();
                    hook::devin_tool_use(&vault, &input).unwrap();
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let (worked, _) = Vault::open(&path)
            .unwrap()
            .take_devin_hook_state(&session)
            .unwrap();
        assert!(worked, "round {round} lost the work flag");
    }
}
