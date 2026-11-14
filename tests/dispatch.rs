use kairotensor::{Disposition, Store};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-dispatch-{}-{stamp}-{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn frozen_request_and_ordered_occurrence_ids_survive_reopen_and_revocation() -> Result<()> {
    let file = Database::new();
    let mut worker = Store::open(&file.0)?;
    let ids = worker.observe_and_admit_many("alice", "run", &[("one", b"1"), ("two", b"2")])?;
    let order = [ids[1], ids[0], ids[1]];
    let attempt = worker.begin_attempt("alice", "run", "plan", &order)?;
    let request = b"\xffmodel\0non-text payload\r\n";
    worker.record_dispatch(attempt, request)?;
    assert!(worker.record_dispatch(attempt, b"different bytes").is_err());
    drop(worker);

    let mut resumed = Store::open(&file.0)?;
    let mut editor = Store::open(&file.0)?;
    resumed.record_dispatch(attempt, request)?;
    editor.set_admitted("alice", "run", ids[0], false)?;
    assert!(resumed.record_dispatch(attempt, request).is_err());
    assert_eq!(resumed.settle(attempt, b"late", 7)?, Disposition::StaleView);
    assert_eq!(resumed.settle(attempt, b"late", 7)?, Disposition::StaleView);
    assert_eq!(resumed.charged_usage()?, 7);
    assert!(resumed.record_dispatch(attempt, request).is_err());

    // Trusted offline audit. Current reads continue to enforce the actor view.
    let audit = Connection::open(&file.0)?;
    let stored: Vec<u8> = audit.query_row(
        "SELECT request FROM dispatches WHERE attempt_id = ?1", [attempt], |r| r.get(0),
    )?;
    assert_eq!(stored.as_slice(), request);
    let mut query = audit.prepare(
        "SELECT evidence_id FROM attempt_inputs WHERE attempt_id = ?1 ORDER BY position",
    )?;
    let inputs = query.query_map([attempt], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>>>()?;
    assert_eq!(inputs.as_slice(), &order[..]);
    assert_eq!(resumed.read_evidence("alice", "run", ids[0])?, None);
    Ok(())
}

#[test]
fn stale_or_terminal_attempts_cannot_capture_requests() -> Result<()> {
    let mut store = Store::in_memory()?;
    let id = store.observe("test", b"one")?;
    store.set_admitted("a", "r", id, true)?;
    let never_dispatched = store.begin_attempt("a", "r", "plan", &[id])?;
    let dispatched = store.begin_attempt("a", "r", "plan", &[])?;
    store.record_dispatch(dispatched, b"")?;
    let winner = store.begin_attempt("a", "r", "plan", &[id])?;
    assert_eq!(store.settle(winner, b"done", 4)?, Disposition::Accepted);
    assert!(store.record_dispatch(never_dispatched, b"new").is_err());
    assert!(store.record_dispatch(dispatched, b"").is_err());
    assert!(store.record_dispatch(winner, b"post-hoc capture").is_err());
    assert!(store.record_dispatch(-1, b"unknown attempt").is_err());
    let other = store.begin_attempt("a", "r", "independent", &[id])?;
    store.set_admitted("a", "r", id, false)?;
    store.set_admitted("a", "r", id, true)?;
    assert!(store.record_dispatch(other, b"same evidence, newer view").is_err());
    Ok(())
}

#[test]
fn migration_from_v2_enables_dispatch_on_old_pending_attempts() -> Result<()> {
    let file = Database::new();
    let old = Connection::open(&file.0)?;
    old.execute_batch(include_str!("../src/schema.sql"))?;
    old.execute_batch(include_str!("../src/attempts.sql"))?;
    old.execute_batch(include_str!("../src/support.sql"))?;
    old.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    old.pragma_update(None, "user_version", 2)?;
    old.execute_batch(
        "INSERT INTO branches (id, actor, scope, name) VALUES (1, 'a', 'r', 'plan');
         INSERT INTO attempts (id, branch_id, branch_revision, view_epoch) VALUES (1, 1, 0, 0);",
    )?;
    drop(old);
    let mut store = Store::open(&file.0)?;
    store.record_dispatch(1, b"request")?;
    assert_eq!(store.settle(1, b"result", 1)?, Disposition::Accepted);
    drop(store);
    let audit = Connection::open(&file.0)?;
    let version: i32 = audit.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 7);
    let request: Vec<u8> = audit.query_row(
        "SELECT request FROM dispatches WHERE attempt_id = 1", [], |row| row.get(0),
    )?;
    assert_eq!(request.as_slice(), b"request");
    Ok(())
}
