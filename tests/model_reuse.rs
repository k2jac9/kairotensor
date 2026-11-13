use kairotensor::{Current, Disposition, ModelAction, Store};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ACTOR: &str = "reviewer";
const SCOPE: &str = "release";
const BRANCH: &str = "tests";
const CONTRACT: &str = "test-summary/schema-v1";
const REQUEST: &[u8] = b"model@1\0template@3\ninput";
const ANSWER: &[u8] = b"12 passed";

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let salt = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-model-reuse-{}-{salt}-{}.sqlite",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

impl Drop for Database {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

fn begin(store: &mut Store, candidates: &[i64]) -> Result<i64> {
    Ok(store.begin_model_attempt(ACTOR, SCOPE, BRANCH, candidates, CONTRACT)?.0)
}

fn execute(store: &mut Store, attempt: i64, request: &[u8], answer: &[u8], usage: i64) -> Result<()> {
    assert!(matches!(store.resolve_model_attempt(attempt, request)?, ModelAction::Dispatch));
    // The durable dispatch is already recorded, including if the client loses this reply.
    assert!(store.resolve_model_attempt(attempt, request).is_err());
    assert_eq!(store.settle(attempt, answer, usage)?, Disposition::Accepted);
    Ok(())
}

fn expect_hit(store: &mut Store, attempt: i64, origin: i64) -> Result<()> {
    match store.resolve_model_attempt(attempt, REQUEST)? {
        ModelAction::Dispatch => panic!("expected the original output to be reused"),
        ModelAction::Reused { origin_attempt, output } => {
            assert_eq!(origin_attempt, origin);
            assert_eq!(output.as_slice(), ANSWER);
        }
    }
    Ok(())
}

#[test]
fn unrelated_edits_reuse_the_real_origin_across_reopen_and_charge_only_the_execution() -> Result<()> {
    let file = Database::new();
    let mut worker = Store::open(&file.0)?;
    let report = worker.observe_and_admit_many(ACTOR, SCOPE, &[("tests", b"ok")])?[0];
    let origin = begin(&mut worker, &[report, report])?;
    execute(&mut worker, origin, REQUEST, ANSWER, 17)?;

    let requirement = worker.observe_and_admit_many(ACTOR, SCOPE, &[("policy", b"canary")])?[0];
    assert_eq!(worker.read_current(ACTOR, SCOPE, BRANCH)?, Current::NeedsReview);
    let first = begin(&mut worker, &[report, report])?;
    expect_hit(&mut worker, first, origin)?;
    assert_eq!(worker.read_current(ACTOR, SCOPE, BRANCH)?, Current::Ready(ANSWER.to_vec()));
    drop(worker);

    let mut worker = Store::open(&file.0)?;
    worker.set_admitted(ACTOR, SCOPE, requirement, false)?;
    let second = begin(&mut worker, &[report, report])?;
    expect_hit(&mut worker, second, origin)?;
    assert_eq!(worker.charged_usage()?, 17);

    let audit = Connection::open(&file.0)?;
    for (id, parent) in [(first, origin), (second, first)] {
        let row: (i64, i64, i64, i64, Vec<u8>) = audit.query_row(
            "SELECT a.parent, r.origin_attempt, d.usage,
                    EXISTS (SELECT 1 FROM dispatches WHERE attempt_id = a.id), d.output
             FROM model_reuses r JOIN attempts a ON a.id = r.attempt_id
             JOIN deliveries d ON d.attempt_id = a.id AND d.disposition = 'accepted'
             WHERE a.id = ?1",
            [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        assert_eq!(row, (parent, origin, 0, 0, ANSWER.to_vec()));
    }
    let head: i64 = audit.query_row("SELECT head FROM branches WHERE name = ?1", [BRANCH], |r| r.get(0))?;
    assert_eq!(head, second);
    assert!(worker.record_dispatch(second, REQUEST).is_err());
    worker.set_admitted(ACTOR, SCOPE, report, false)?;
    assert!(worker.resolve_model_attempt(second, REQUEST).is_err());
    assert_eq!(worker.read_current(ACTOR, SCOPE, BRANCH)?, Current::NeedsReview);
    Ok(())
}

#[test]
fn request_contract_and_ordered_candidate_identity_each_constrain_reuse() -> Result<()> {
    let scenarios: &[(&[i64], &str, &[u8])] = &[
        (&[1, 2, 1, 9], CONTRACT, b"model@2\0template@3\ninput"),
        (&[1, 2, 1, 9], "test-summary/schema-v2", REQUEST),
        (&[2, 1, 1, 9], CONTRACT, REQUEST),
        (&[1, 2, 9], CONTRACT, REQUEST),
        (&[1, 2, 1, 10], CONTRACT, REQUEST),
    ];
    for &(candidates, contract, request) in scenarios {
        let mut store = Store::in_memory()?;
        store.observe_and_admit_many(ACTOR, SCOPE, &[("a", b"same"), ("b", b"same")])?;
        let origin = begin(&mut store, &[1, 2, 1, 9])?;
        execute(&mut store, origin, REQUEST, ANSWER, 5)?;
        let attempt = store.begin_model_attempt(ACTOR, SCOPE, BRANCH, candidates, contract)?.0;
        execute(&mut store, attempt, request, b"another execution", 5)?;
        assert_eq!(store.charged_usage()?, 10);
    }
    Ok(())
}

#[test]
fn absent_candidates_and_membership_reversal_force_a_fresh_dispatch() -> Result<()> {
    let mut store = Store::in_memory()?;
    let (empty, initial) = store.begin_model_attempt(ACTOR, SCOPE, BRANCH, &[1], CONTRACT)?;
    assert!(initial.is_empty());
    execute(&mut store, empty, REQUEST, ANSWER, 3)?;
    let id = store.observe("future report", b"new")?;
    let unrelated = store.observe_and_admit_many(ACTOR, SCOPE, &[("elsewhere", b"ok")])?[0];
    store.set_admitted(ACTOR, SCOPE, unrelated, false)?;
    // Observing without admitting an occurrence leaves this empty selection valid.
    let unchanged = begin(&mut store, &[id])?;
    expect_hit(&mut store, unchanged, empty)?;

    store.set_admitted(ACTOR, SCOPE, id, true)?;
    store.set_admitted(ACTOR, SCOPE, id, false)?;
    let still_empty = begin(&mut store, &[id])?;
    execute(&mut store, still_empty, REQUEST, ANSWER, 3)?;
    store.set_admitted(ACTOR, SCOPE, id, true)?;
    let visible = begin(&mut store, &[id])?;
    execute(&mut store, visible, REQUEST, ANSWER, 3)?;
    store.set_admitted(ACTOR, SCOPE, id, false)?;
    store.set_admitted(ACTOR, SCOPE, id, true)?;
    let visible_again = begin(&mut store, &[id])?;
    execute(&mut store, visible_again, REQUEST, ANSWER, 3)?;
    assert_eq!(store.charged_usage()?, 12);
    Ok(())
}

#[test]
fn only_opted_in_heads_in_the_same_actor_scope_and_branch_can_supply_a_sample() -> Result<()> {
    let mut store = Store::in_memory()?;
    let id = store.observe("tests", b"ok")?;
    for (actor, scope) in [(ACTOR, SCOPE), ("other", SCOPE), (ACTOR, "other")] {
        store.set_admitted(actor, scope, id, true)?;
    }
    let origin = begin(&mut store, &[id])?;
    assert!(store.record_dispatch(origin, REQUEST).is_err());
    assert!(store.settle(origin, ANSWER, 2).is_err());
    assert_eq!(store.read_current(ACTOR, SCOPE, BRANCH)?, Current::Empty);
    assert_eq!(store.charged_usage()?, 0);
    execute(&mut store, origin, REQUEST, ANSWER, 2)?;
    for (actor, scope, branch) in [("other", SCOPE, BRANCH), (ACTOR, "other", BRANCH), (ACTOR, SCOPE, "other")] {
        let attempt = store.begin_model_attempt(actor, scope, branch, &[id], CONTRACT)?.0;
        execute(&mut store, attempt, REQUEST, ANSWER, 2)?;
    }
    assert!(store.begin_model_attempt(ACTOR, SCOPE, BRANCH, &[id], "").is_err());
    let (ordinary, _) = store.begin_from_candidates(ACTOR, SCOPE, BRANCH, &[id])?;
    assert!(store.resolve_model_attempt(ordinary, REQUEST).is_err());
    store.record_dispatch(ordinary, REQUEST)?;
    assert_eq!(store.settle(ordinary, ANSWER, 2)?, Disposition::Accepted);
    let after_opaque = begin(&mut store, &[id])?;
    execute(&mut store, after_opaque, REQUEST, ANSWER, 2)?;
    assert_eq!(store.charged_usage()?, 12);
    Ok(())
}

#[test]
fn stale_view_or_head_cannot_resolve_even_when_request_and_sources_match() -> Result<()> {
    let file = Database::new();
    let mut left = Store::open(&file.0)?;
    let mut right = Store::open(&file.0)?;
    let ids = left.observe_and_admit_many(ACTOR, SCOPE, &[("tests", b"ok"), ("policy", b"staging")])?;
    let origin = begin(&mut left, &ids[..1])?;
    execute(&mut left, origin, REQUEST, ANSWER, 2)?;
    let old_view = begin(&mut left, &ids[..1])?;
    right.set_admitted(ACTOR, SCOPE, ids[1], false)?;
    assert!(left.resolve_model_attempt(old_view, REQUEST).is_err());
    let stale_parent = begin(&mut left, &ids[..1])?;
    let published = begin(&mut right, &ids[..1])?;
    expect_hit(&mut right, published, origin)?;
    assert!(left.resolve_model_attempt(stale_parent, REQUEST).is_err());
    assert_eq!(left.charged_usage()?, 2);
    Ok(())
}

#[test]
fn a_late_write_error_leaves_no_reuse_receipt_delivery_or_head_movement() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let origin = begin(&mut store, &[])?;
    execute(&mut store, origin, REQUEST, ANSWER, 7)?;
    let next = begin(&mut store, &[])?;
    let audit = Connection::open(&file.0)?;
    audit.execute_batch(
        "CREATE TRIGGER fail_publish BEFORE UPDATE OF head ON branches
         BEGIN SELECT RAISE(ABORT, 'injected model publication failure'); END;",
    )?;
    assert!(store.resolve_model_attempt(next, REQUEST).is_err());
    let state: (i64, i64, i64, i64) = audit.query_row(
        "SELECT (SELECT head FROM branches WHERE name = ?1),
                (SELECT COUNT(*) FROM model_reuses WHERE attempt_id = ?2),
                (SELECT COUNT(*) FROM deliveries WHERE attempt_id = ?2),
                (SELECT COUNT(*) FROM dispatches WHERE attempt_id = ?2)",
        rusqlite::params![BRANCH, next], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    assert_eq!(state, (origin, 0, 0, 0));
    audit.execute_batch("DROP TRIGGER fail_publish")?;
    expect_hit(&mut store, next, origin)?;
    assert_eq!(store.charged_usage()?, 7);
    Ok(())
}

#[test]
fn migrating_v3_keeps_historical_dispatches_outside_opt_in_reuse() -> Result<()> {
    let file = Database::new();
    let old = Connection::open(&file.0)?;
    old.execute_batch(include_str!("../src/schema.sql"))?;
    old.execute_batch(include_str!("../src/attempts.sql"))?;
    old.execute_batch(include_str!("../src/support.sql"))?;
    old.execute_batch(include_str!("../src/dispatch.sql"))?;
    old.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    old.pragma_update(None, "user_version", 3)?;
    old.execute_batch(
        "INSERT INTO branches(id, actor,scope,name) VALUES (1,'reviewer','release','tests');
         INSERT INTO attempts(id,branch_id,branch_revision,view_epoch) VALUES (1,1,0,0);
         INSERT INTO deliveries VALUES (1,'accepted',X'31',5);
         UPDATE branches SET head=1,revision=1 WHERE id=1;",
    )?;
    old.execute("INSERT INTO dispatches VALUES (1,?1)", [REQUEST])?;
    drop(old);
    let mut store = Store::open(&file.0)?;
    assert_eq!(store.read_current(ACTOR, SCOPE, BRANCH)?, Current::Ready(b"1".to_vec()));
    let next = begin(&mut store, &[])?;
    execute(&mut store, next, REQUEST, ANSWER, 3)?;
    assert_eq!(store.charged_usage()?, 8);
    Ok(())
}
