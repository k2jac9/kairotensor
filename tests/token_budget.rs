use kairotensor::{Current, Disposition, Store, TokenBudgetState};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

fn attempt(store: &mut Store, branch: &str) -> Result<i64> {
    store.begin_attempt("planner", "release", branch, &[])
}

#[test]
fn exact_fit_and_one_token_short_preserve_the_original_failed_charge() -> Result<()> {
    for (limit, retry_granted) in [(403, false), (404, true)] {
        let mut store = Store::in_memory()?;
        let budget = store.create_token_budget(limit)?;
        let failed = attempt(&mut store, "judge")?;
        assert!(store.reserve_dispatch(budget, failed, b"archived", 93, 96)?);
        assert_eq!(store.token_budget_state(budget)?,
                   TokenBudgetState { limit, spent: 0, reserved: 189 });
        store.reject_invalid(failed, b"pass", 127, "check/1", "incorrect verdict")?;
        let retry = attempt(&mut store, "judge")?;
        assert_eq!(store.reserve_dispatch(budget, retry, b"retry", 181, 96)?, retry_granted);
        assert!(store.record_dispatch(retry, b"retry").is_err());
        assert!(store.reserve_dispatch(budget, retry, b"retry", 181, 96).is_err());
        if retry_granted {
            assert_eq!(store.token_budget_state(budget)?.reserved, 277);
            assert_eq!(store.settle(retry, b"fail", 211)?, Disposition::Accepted);
            assert_eq!(store.settle(retry, b"fail", 211)?, Disposition::Accepted);
            assert_eq!(store.token_budget_state(budget)?,
                       TokenBudgetState { limit, spent: 338, reserved: 0 });
        } else {
            assert!(store.settle(retry, b"bypass", 211).is_err());
            assert!(store.reject_invalid(retry, b"bypass", 211, "a", "b").is_err());
            assert_eq!(store.read_current("planner", "release", "judge")?, Current::Empty);
            assert_eq!(store.token_budget_state(budget)?,
                       TokenBudgetState { limit, spent: 127, reserved: 0 });
            let adjusted = attempt(&mut store, "judge")?;
            assert!(store.reserve_dispatch(budget, adjusted, b"cap=95", 181, 95)?);
        }
    }
    Ok(())
}

#[test]
fn only_credible_usage_releases_capacity_for_accept_reject_and_stale_deliveries() -> Result<()> {
    let mut store = Store::in_memory()?;
    let budget = store.create_token_budget(20)?;
    let branch_stale = attempt(&mut store, "test")?;
    assert!(store.reserve_dispatch(budget, branch_stale, b"payload", 7, 3)?);
    for value in [6, 11] {
        assert!(store.settle(branch_stale, b"result", value).is_err());
        assert!(store.reject_invalid(branch_stale, b"result", value, "check/1", "bad").is_err());
    }
    assert_eq!(store.token_budget_state(budget)?,
               TokenBudgetState { limit: 20, spent: 0, reserved: 10 });
    let winner = attempt(&mut store, "test")?;
    store.settle(winner, b"unbudgeted work", 22)?;
    assert_eq!(store.settle(branch_stale, b"late", 8)?, Disposition::StaleBranch);
    let view_stale = attempt(&mut store, "other")?;
    assert!(store.reserve_dispatch(budget, view_stale, b"request", 4, 8)?);
    store.observe_and_admit_many("planner", "release", &[("new", b"dependency")])?;
    assert_eq!(store.reject_invalid(view_stale, b"bad", 6, "check/1", "malformed")?, Disposition::InvalidOutput);
    assert_eq!(store.reject_invalid(view_stale, b"bad", 6, "check/1", "malformed")?, Disposition::InvalidOutput);
    assert_eq!(store.token_budget_state(budget)?,
               TokenBudgetState { limit: 20, spent: 14, reserved: 0 });
    assert_eq!(store.charged_usage()?, 36);
    Ok(())
}

#[test]
fn invalid_limits_overflow_and_already_frozen_requests_cannot_reserve() -> Result<()> {
    let mut store = Store::in_memory()?;
    assert!(store.create_token_budget(-1).is_err());
    let budget = store.create_token_budget(i64::MAX)?;
    let ordinary = attempt(&mut store, "first")?;
    for (input, cap) in [(-1, 2), (5, 0), (i64::MAX, 1)] {
        assert!(store.reserve_dispatch(budget, ordinary, b"invalid", input, cap).is_err());
    }
    assert!(store.reserve_dispatch(0, ordinary, b"unknown budget", 2, 3).is_err());
    store.record_dispatch(ordinary, b"already frozen")?;
    assert!(store.reserve_dispatch(budget, ordinary, b"already frozen", 2, 3).is_err());
    let closed = store.begin_model_attempt("planner", "release", "cached", &[], "model/1")?.0;
    assert!(store.reserve_dispatch(budget, closed, b"closed request", 1, 1).is_err());
    let full = attempt(&mut store, "full")?;
    assert!(store.reserve_dispatch(budget, full, b"max", i64::MAX - 1, 1)?);
    let another = attempt(&mut store, "another")?;
    assert!(!store.reserve_dispatch(budget, another, b"full budget", 0, 1)?);
    assert_eq!(store.token_budget_state(budget)?.reserved, i64::MAX);
    Ok(())
}

struct Database(PathBuf);
impl Database {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!("kairotensor-budget-{label}-{}-{stamp}.sqlite", std::process::id())))
    }
}
impl Drop for Database {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

#[test]
fn concurrent_reservations_serialize_and_unknown_usage_survives_restart() -> Result<()> {
    let file = Database::new("race");
    let mut setup = Store::open(&file.0)?;
    let budget = setup.create_token_budget(12)?;
    let jobs = [(Store::open(&file.0)?, attempt(&mut setup, "a")?),
                (Store::open(&file.0)?, attempt(&mut setup, "b")?)];
    let barrier = Arc::new(Barrier::new(jobs.len()));
    let handles: Vec<_> = jobs.into_iter().map(|(mut store, id)| {
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || -> Result<(i64, bool)> {
            barrier.wait();
            Ok((id, store.reserve_dispatch(budget, id, b"bounded", 5, 7)?))
        })
    }).collect();
    let decisions = handles.into_iter().map(|h| h.join().unwrap()).collect::<Result<Vec<_>>>()?;
    assert_eq!(decisions.iter().filter(|(_, granted)| *granted).count(), 1);
    let winner = decisions.iter().find(|(_, granted)| *granted).unwrap().0;
    drop(setup);
    let mut restarted = Store::open(&file.0)?;
    let before = TokenBudgetState { limit: 12, spent: 0, reserved: 12 };
    assert_eq!(restarted.token_budget_state(budget)?, before);
    assert!(restarted.reserve_dispatch(budget, winner, b"bounded", 5, 7).is_err());
    let next = attempt(&mut restarted, "next")?;
    assert!(!restarted.reserve_dispatch(budget, next, b"cannot overspend", 0, 1)?);
    // A failure late in publication cannot release the reservation.
    let audit = Connection::open(&file.0)?;
    audit.execute_batch("CREATE TRIGGER block_head BEFORE UPDATE OF head ON branches
                         BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;")?;
    assert!(restarted.settle(winner, b"real", 8).is_err());
    assert_eq!(restarted.token_budget_state(budget)?, before);
    audit.execute_batch("DROP TRIGGER block_head")?;
    assert_eq!(restarted.settle(winner, b"real", 8)?, Disposition::Accepted);
    assert_eq!(restarted.token_budget_state(budget)?,
               TokenBudgetState { limit: 12, spent: 8, reserved: 0 });
    let late = attempt(&mut restarted, "late")?;
    assert!(restarted.reserve_dispatch(budget, late, b"after reconciliation", 2, 2)?);
    Ok(())
}

#[test]
fn v5_migration_never_assigns_prior_or_pending_work_to_a_new_budget() -> Result<()> {
    let file = Database::new("migration");
    let legacy = Connection::open(&file.0)?;
    for script in [
        include_str!("../src/schema.sql"), include_str!("../src/attempts.sql"), include_str!("../src/support.sql"),
        include_str!("../src/dispatch.sql"), include_str!("../src/model_reuse.sql"), include_str!("../src/validation.sql"),
    ] { legacy.execute_batch(script)?; }
    legacy.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    legacy.pragma_update(None, "user_version", 5)?;
    legacy.execute_batch(
        "INSERT INTO branches(id,actor,scope,name) VALUES(1,'planner','release','legacy');
         INSERT INTO attempts(id,branch_id,branch_revision,view_epoch) VALUES(1,1,0,0),(2,1,0,0);
         INSERT INTO dispatches VALUES(1,X'01'),(2,X'02');
         INSERT INTO deliveries VALUES(1,'invalid_output',X'00',7,'validator/1','bad','current');",
    )?;
    drop(legacy);
    let mut store = Store::open(&file.0)?;
    let budget = store.create_token_budget(10)?;
    assert_eq!(store.charged_usage()?, 7);
    assert!(store.reserve_dispatch(budget, 2, &[2], 3, 4).is_err());
    assert_eq!(store.settle(2, b"legacy result", 3)?, Disposition::Accepted);
    assert_eq!(store.charged_usage()?, 10);
    assert_eq!(store.token_budget_state(budget)?,
               TokenBudgetState { limit: 10, spent: 0, reserved: 0 });
    Ok(())
}
