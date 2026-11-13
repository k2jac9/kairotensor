use kairotensor::{Current, Disposition, ModelAction, Store};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

const VALIDATOR: &str = "release-judgment/v1";
const REASON: &str = "status disagrees with report";
const BAD: &[u8] = b"bad\0judgment\xff";

struct Database(PathBuf);

impl Database {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-rejection-{label}-{}-{stamp}.sqlite", std::process::id()
        )))
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn dispatch(store: &mut Store, branch: &str) -> Result<i64> {
    let id = store.begin_attempt("a", "release", branch, &[])?;
    store.record_dispatch(id, b"frozen request")?;
    Ok(id)
}

fn reject(store: &mut Store, id: i64) -> Result<Disposition> {
    store.reject_invalid(id, BAD, 127, VALIDATOR, REASON)
}

#[test]
fn rejection_is_durable_and_exactly_one_terminal_delivery() -> Result<()> {
    let file = Database::new("restart");
    let mut store = Store::open(&file.0)?;
    let winner = dispatch(&mut store, "plan")?;
    assert_eq!(store.settle(winner, b"previous", 4)?, Disposition::Accepted);
    let (id, _) = store.begin_model_attempt("a", "release", "plan", &[], "closed/v1")?;
    assert!(matches!(store.resolve_model_attempt(id, b"judge-v1")?, ModelAction::Dispatch));
    assert_eq!(reject(&mut store, id)?, Disposition::InvalidOutput);
    assert_eq!(store.read_current("a", "release", "plan")?, Current::Ready(b"previous".to_vec()));
    assert!(store.resolve_model_attempt(id, b"judge-v1").is_err());
    assert!(store.record_dispatch(id, b"judge-v1").is_err());
    let audit = Connection::open(&file.0)?;
    let row: (Vec<u8>, i64, String, String, String, String) = audit.query_row(
        "SELECT output,usage,validator,reason,applicability,disposition FROM deliveries WHERE attempt_id=?1",
        [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    assert_eq!(row, (BAD.to_vec(), 127, VALIDATOR.into(), REASON.into(), "current".into(), "invalid_output".into()));

    store.observe_and_admit_many("a", "release", &[("new", b"policy")])?;
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(reject(&mut store, id)?, Disposition::InvalidOutput);
    for (output, cost, validator, reason) in [
        (&b"other"[..], 127, VALIDATOR, REASON), (BAD, 128, VALIDATOR, REASON),
        (BAD, 127, "release-judgment/v2", REASON), (BAD, 127, VALIDATOR, "other reason"),
    ] {
        assert!(store.reject_invalid(id, output, cost, validator, reason).is_err());
    }
    assert!(store.settle(id, BAD, 127).is_err());
    assert!(store.reject_invalid(winner, b"previous", 4, VALIDATOR, REASON).is_err());
    let next = store.begin_model_attempt("a", "release", "plan", &[], "closed/v1")?.0;
    assert!(matches!(store.resolve_model_attempt(next, b"judge-v1")?, ModelAction::Dispatch));
    assert_eq!(store.charged_usage()?, 131);
    let state: (i64, i64, String) = audit.query_row(
        "SELECT b.head,b.revision,d.applicability FROM branches b, deliveries d WHERE b.name='plan' AND d.attempt_id=?1",
        [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(state, (winner, 1, "current".into()));
    Ok(())
}

#[test]
fn rejected_stale_replies_retain_their_first_applicability() -> Result<()> {
    let file = Database::new("stale");
    let mut store = Store::open(&file.0)?;
    let losing = dispatch(&mut store, "busy")?;
    let winning = dispatch(&mut store, "busy")?;
    store.settle(winning, b"winner", 2)?;
    assert_eq!(reject(&mut store, losing)?, Disposition::InvalidOutput);
    let outdated = dispatch(&mut store, "separate")?;
    store.observe_and_admit_many("a", "release", &[("changed", b"data")])?;
    assert_eq!(reject(&mut store, outdated)?, Disposition::InvalidOutput);
    assert_eq!(reject(&mut store, losing)?, Disposition::InvalidOutput);
    assert_eq!(store.read_current("a", "release", "separate")?, Current::Empty);
    let audit = Connection::open(&file.0)?;
    for (id, state) in [(losing, "stale_branch"), (outdated, "stale_view")] {
        let actual: (String, String) = audit.query_row(
            "SELECT applicability,reason FROM deliveries WHERE attempt_id=?1",
            [id], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(actual, (state.into(), REASON.into()));
    }
    assert_eq!(store.charged_usage()?, 256);
    Ok(())
}

#[test]
fn invalid_settlement_requires_real_dispatch_usage_and_a_validator_reason() -> Result<()> {
    let mut store = Store::in_memory()?;
    let pending = store.begin_attempt("a", "release", "unbilled", &[])?;
    assert!(reject(&mut store, pending).is_err());
    assert!(reject(&mut store, -7).is_err());
    store.record_dispatch(pending, b"send")?;
    for (usage, validator, reason) in [(-1, VALIDATOR, REASON), (1, " ", REASON), (1, VALIDATOR, "")] {
        assert!(store.reject_invalid(pending, BAD, usage, validator, reason).is_err());
    }
    assert_eq!(store.charged_usage()?, 0);
    let first = store.begin_model_attempt("a", "release", "model", &[], "closed/v1")?.0;
    assert!(reject(&mut store, first).is_err());
    assert!(matches!(store.resolve_model_attempt(first,b"same")?, ModelAction::Dispatch));
    store.settle(first, b"yes", 1)?;
    let second = store.begin_model_attempt("a", "release", "model", &[], "closed/v1")?.0;
    assert!(matches!(store.resolve_model_attempt(second,b"same")?, ModelAction::Reused { .. }));
    assert!(store.reject_invalid(second, b"yes", 0, VALIDATOR, REASON).is_err());
    assert_eq!(store.charged_usage()?, 1);
    Ok(())
}

#[test]
fn concurrent_valid_and_invalid_settlements_cannot_both_win() -> Result<()> {
    let file = Database::new("race");
    let mut observer = Store::open(&file.0)?;
    let id = dispatch(&mut observer, "plan")?;
    let barrier = Arc::new(Barrier::new(2));
    let writers = [Store::open(&file.0)?, Store::open(&file.0)?];
    let workers: Vec<_> = writers.into_iter().enumerate().map(|(i, mut db)| {
        let start = Arc::clone(&barrier);
        std::thread::spawn(move || {
            start.wait();
            if i == 0 { db.settle(id, BAD, 127) } else { reject(&mut db, id) }
        })
    }).collect();
    let wins: Vec<_> = workers.into_iter().filter_map(|w| w.join().unwrap().ok()).collect();
    assert_eq!(wins.len(), 1);
    let expected = match wins[0] {
        Disposition::Accepted => Current::Ready(BAD.to_vec()),
        Disposition::InvalidOutput => Current::Empty,
        other => panic!("unexpected terminal disposition: {other:?}"),
    };
    assert_eq!(observer.read_current("a","release","plan")?, expected);
    assert_eq!(observer.charged_usage()?, 127);
    Ok(())
}

#[test]
fn populated_v4_migration_preserves_old_intents_and_an_open_dispatch() -> Result<()> {
    let file = Database::new("migration");
    let db = Connection::open(&file.0)?;
    for ddl in [
        include_str!("../src/schema.sql"), include_str!("../src/attempts.sql"),
        include_str!("../src/support.sql"), include_str!("../src/dispatch.sql"),
        include_str!("../src/model_reuse.sql"),
    ] { db.execute_batch(ddl)?; }
    db.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    db.pragma_update(None, "user_version", 4)?;
    db.execute_batch(
        "INSERT INTO branches(id,actor,scope,name) VALUES(1,'a','release','plan');
         INSERT INTO attempts(id,branch_id,branch_revision,view_epoch)
             VALUES (1,1,0,0),(2,1,0,0);
         INSERT INTO deliveries VALUES (1,'accepted',X'41',5),(2,'stale_branch',X'42',9);
         UPDATE branches SET head=1,revision=1 WHERE id=1;
         INSERT INTO attempts(id,branch_id,parent,branch_revision,view_epoch) VALUES (3,1,1,1,0);
         INSERT INTO dispatches VALUES (1,X'01'),(2,X'02'),(3,X'03');"
    )?;
    drop(db);
    let mut upgraded = Store::open(&file.0)?;
    assert_eq!(upgraded.settle(1, b"A", 5)?, Disposition::Accepted);
    assert_eq!(upgraded.settle(2, b"B", 9)?, Disposition::StaleBranch);
    assert!(upgraded.reject_invalid(2, b"B", 9, VALIDATOR, REASON).is_err());
    assert_eq!(upgraded.read_current("a","release","plan")?, Current::Ready(b"A".to_vec()));
    assert_eq!(upgraded.charged_usage()?, 14);
    assert_eq!(reject(&mut upgraded, 3)?, Disposition::InvalidOutput);
    let audit = Connection::open(&file.0)?;
    let legacy: i64 = audit.query_row(
        "SELECT COUNT(*) FROM deliveries WHERE attempt_id IN (1,2) AND validator IS NULL
            AND reason IS NULL AND applicability IS NULL", [], |r| r.get(0),
    )?;
    assert_eq!(legacy, 2);
    let broken: i64 = audit.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
    assert_eq!(broken, 0);
    assert_eq!(upgraded.charged_usage()?, 141);
    Ok(())
}
