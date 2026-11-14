use kairotensor::{Current, Disposition, Store, TokenBudgetState};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

const ACTOR: &str = "reviewer";
const SCOPE: &str = "release";
const REPORT: &[u8] = br#"{"release":"r1","passed":11,"total":12}"#;
const VERDICT: &[u8] = br#"{"release":"r1","tests":"11/12","status":"fail"}"#;
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-verdict-{}-{stamp}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn admit(store: &mut Store) -> Result<i64> {
    let id = store.observe("ci-report", REPORT)?;
    store.set_admitted(ACTOR, SCOPE, id, true)?;
    Ok(id)
}
fn counts(db: &Database) -> Result<(i64, i64, i64, i64)> {
    Connection::open(&db.0)?.query_row("SELECT (SELECT COUNT(*) FROM attempts), (SELECT COUNT(*) FROM deliveries), (SELECT COUNT(*) FROM dispatches), (SELECT COUNT(*) FROM token_reservations)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
}

#[test]
fn deterministic_publication_is_bound_zero_cost_and_idempotent_across_restart() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let report = admit(&mut store)?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
    assert_eq!(store.read_current(ACTOR, SCOPE, "plan")?, Current::Empty);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(
        store.read_current(ACTOR, SCOPE, "plan")?,
        Current::Ready(VERDICT.to_vec())
    );
    assert_eq!(store.charged_usage()?, 0);
    assert_eq!(counts(&file)?, (1, 1, 0, 0));
    let db = Connection::open(&file.0)?;
    let row: (String,Vec<u8>,i64) = db.query_row("SELECT v.computation,v.output,b.revision FROM deterministic_verdicts v JOIN attempts a ON a.id=v.attempt_id JOIN branches b ON b.id=a.branch_id", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    assert_eq!(row, ("report-verdict/v1".into(), VERDICT.to_vec(), 1));
    Ok(())
}

#[test]
fn incomplete_candidates_never_silently_select_the_one_visible_report() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let visible = admit(&mut store)?;
    let hidden = store.observe("private", REPORT)?;
    store.set_admitted("other", SCOPE, hidden, true)?;
    let missing = hidden + 1000;
    let hidden_error = store
        .begin_report_verdict(ACTOR, SCOPE, "plan", &[visible, hidden])
        .unwrap_err()
        .to_string();
    let missing_error = store
        .begin_report_verdict(ACTOR, SCOPE, "plan", &[visible, missing])
        .unwrap_err()
        .to_string();
    assert_eq!(hidden_error, missing_error);
    assert_eq!(
        store
            .begin_report_verdict(ACTOR, SCOPE, "plan", &[hidden])
            .unwrap_err()
            .to_string(),
        store
            .begin_report_verdict(ACTOR, SCOPE, "plan", &[missing])
            .unwrap_err()
            .to_string()
    );
    for ids in [vec![], vec![hidden], vec![missing], vec![visible, visible]] {
        assert!(store
            .begin_report_verdict(ACTOR, SCOPE, "plan", &ids)
            .is_err());
    }
    assert!(store
        .begin_report_verdict("other", SCOPE, "plan", &[visible])
        .is_err());
    assert!(store
        .begin_report_verdict(ACTOR, "other", "plan", &[visible])
        .is_err());
    let conflicting = store.observe("conflict", br#"{"release":"r1","passed":12,"total":12}"#)?;
    store.set_admitted(ACTOR, SCOPE, conflicting, true)?;
    assert!(store
        .begin_report_verdict(ACTOR, SCOPE, "plan", &[visible, conflicting])
        .is_err());
    assert_eq!(counts(&file)?, (0, 0, 0, 0));
    let db = Connection::open(&file.0)?;
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM branches", [], |r| r.get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn strict_report_parser_rejects_duplicates_types_extras_and_invalid_counts_atomically() -> Result<()>
{
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    for payload in [
        br#"{"release":"r","passed":true,"total":1}"#.as_slice(),
        br#"{"release":"r","passed":1.0,"total":1}"#,
        br#"{"release":"r","passed":1e0,"total":1}"#,
        br#"{"release":"r","passed":-1,"total":1}"#,
        br#"{"release":"r","passed":2,"total":1}"#,
        br#"{"release":"r","passed":0,"total":0}"#,
        br#"{"release":"r","passed":0,"total":18446744073709551616}"#,
        br#"{"release":"r","passed":0,"passed":1,"total":1}"#,
        br#"{"release":"r","passed":1,"total":1,"extra":true}"#,
        br#"{"release":"r","passed":1}"#,
        br#"{"release":"","passed":1,"total":1}"#,
        br#"{"release":"\ud800","passed":1,"total":1}"#,
        b"not JSON",
        b"\xff",
        b"[]",
    ] {
        let id = store.observe("bad", payload)?;
        store.set_admitted(ACTOR, SCOPE, id, true)?;
        assert!(store
            .begin_report_verdict(ACTOR, SCOPE, "plan", &[id])
            .is_err());
    }
    assert_eq!(counts(&file)?, (0, 0, 0, 0));
    Ok(())
}

#[test]
fn unicode_and_u64_bounds_preserve_exact_meaning() -> Result<()> {
    let mut store = Store::in_memory()?;
    let raw = br#"{"release":"\u4f60\u597d\ud83d\ude80\n\"","passed":18446744073709551615,"total":18446744073709551615}"#;
    let evidence = store.observe("max", raw)?;
    store.set_admitted(ACTOR, SCOPE, evidence, true)?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[evidence])?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    let Current::Ready(output) = store.read_current(ACTOR, SCOPE, "plan")? else {
        panic!("missing verdict")
    };
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"release":"你好🚀\n\"","tests":"18446744073709551615/18446744073709551615","status":"pass"})
    );
    Ok(())
}

#[test]
fn view_changes_between_capture_and_commit_fail_closed_even_after_regrant() -> Result<()> {
    for regrant in [false, true] {
        let file = Database::new();
        let mut store = Store::open(&file.0)?;
        let report = admit(&mut store)?;
        let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
        let mut editor = Store::open(&file.0)?;
        editor.set_admitted(ACTOR, SCOPE, report, false)?;
        if regrant {
            editor.set_admitted(ACTOR, SCOPE, report, true)?;
        }
        assert_eq!(store.commit_report_verdict(id)?, Disposition::StaleView);
        drop(store);
        let mut store = Store::open(&file.0)?;
        assert_eq!(store.commit_report_verdict(id)?, Disposition::StaleView);
        assert_eq!(store.read_current(ACTOR, SCOPE, "plan")?, Current::Empty);
        assert_eq!(counts(&file)?, (1, 1, 0, 0));
        assert_eq!(store.charged_usage()?, 0);
    }
    Ok(())
}

#[test]
fn historical_accepted_receipt_does_not_restore_revoked_current_eligibility() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let report = admit(&mut store)?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    store.set_admitted(ACTOR, SCOPE, report, false)?;
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(
        store.read_current(ACTOR, SCOPE, "plan")?,
        Current::NeedsReview
    );
    assert!(store.begin_resume(ACTOR, SCOPE, "plan", &[]).is_err());
    assert!(store
        .begin_report_verdict(ACTOR, SCOPE, "plan", &[report])
        .is_err());
    assert_eq!(counts(&file)?, (1, 1, 0, 0));
    Ok(())
}

#[test]
fn competing_workers_publish_one_head_and_duplicates_do_not_publish_twice() -> Result<()> {
    for same_id in [false, true] {
        let file = Database::new();
        let mut store = Store::open(&file.0)?;
        let report = admit(&mut store)?;
        let first = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
        let second = if same_id {
            first
        } else {
            store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?
        };
        let barrier = Arc::new(Barrier::new(2));
        let jobs = [
            (Store::open(&file.0)?, first),
            (Store::open(&file.0)?, second),
        ];
        let workers: Vec<_> = jobs
            .into_iter()
            .map(|(mut db, id)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    db.commit_report_verdict(id)
                })
            })
            .collect();
        let results = workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(
            results
                .iter()
                .filter(|&&r| r == Disposition::Accepted)
                .count(),
            if same_id { 2 } else { 1 }
        );
        if !same_id {
            assert!(results.contains(&Disposition::StaleBranch));
        }
        assert_eq!(
            store.read_current(ACTOR, SCOPE, "plan")?,
            Current::Ready(VERDICT.to_vec())
        );
        let db = Connection::open(&file.0)?;
        assert_eq!(
            db.query_row("SELECT revision FROM branches", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(store.charged_usage()?, 0);
    }
    Ok(())
}

#[test]
fn deterministic_attempt_cannot_dispatch_reserve_or_accept_caller_supplied_output() -> Result<()> {
    let mut store = Store::in_memory()?;
    let report = admit(&mut store)?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
    let budget = store.create_token_budget(100)?;
    assert!(store.settle(id, VERDICT, 0).is_err());
    assert!(store.settle(id, b"forged", 0).is_err());
    assert!(store.record_dispatch(id, b"request").is_err());
    assert!(store
        .reserve_dispatch(budget, id, b"request", 1, 10)
        .is_err());
    assert!(store
        .reject_invalid(id, b"bad", 1, "validator", "reason")
        .is_err());
    assert!(store.resolve_model_attempt(id, b"request").is_err());
    assert!(store
        .reserve_recovery_attempt(id, b"request", 1, 10)
        .is_err());
    assert!(store.claim_recovery_dispatch(id).is_err());
    assert!(store.cancel_recovery_attempt(id).is_err());
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(
        store.token_budget_state(budget)?,
        TokenBudgetState {
            limit: 100,
            spent: 0,
            reserved: 0
        }
    );
    let ordinary = store.begin_attempt(ACTOR, SCOPE, "ordinary", &[report])?;
    assert!(store.commit_report_verdict(ordinary).is_err());
    Ok(())
}

#[test]
fn uncertain_model_holds_and_prior_usage_remain_untouched() -> Result<()> {
    let mut store = Store::in_memory()?;
    let report = admit(&mut store)?;
    let budget = store.create_token_budget(50)?;
    let measured = store.begin_attempt(ACTOR, SCOPE, "prior", &[])?;
    assert!(store.reserve_dispatch(budget, measured, b"previous", 4, 8)?);
    assert_eq!(
        store.settle(measured, b"previous result", 7)?,
        Disposition::Accepted
    );
    let pending = store.begin_attempt(ACTOR, SCOPE, "uncertain", &[])?;
    assert!(store.reserve_dispatch(budget, pending, b"possibly sent", 4, 8)?);
    assert!(store.commit_report_verdict(pending).is_err());
    let recovery = store.create_recovery("uncertain-recovery", budget, ACTOR, SCOPE, "recovery")?;
    let kairotensor::RecoveryStart::New { attempt: sent, .. } =
        store.begin_recovery_attempt(recovery, &[])?
    else {
        panic!("new recovery")
    };
    assert!(store.reserve_recovery_attempt(sent, b"claimed", 4, 8)?);
    assert_eq!(
        store.claim_recovery_dispatch(sent)?,
        Some(b"claimed".to_vec())
    );
    assert!(store.commit_report_verdict(sent).is_err());
    let id = store.begin_report_verdict(ACTOR, SCOPE, "deterministic", &[report])?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(
        store.token_budget_state(budget)?,
        TokenBudgetState {
            limit: 50,
            spent: 7,
            reserved: 24
        }
    );
    assert_eq!(store.claim_recovery_dispatch(sent)?, None);
    assert!(!store.cancel_recovery_attempt(sent)?);
    assert_eq!(store.charged_usage()?, 7);
    Ok(())
}

#[test]
fn publication_failure_rolls_back_delivery_and_is_retryable_after_reopen() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let report = admit(&mut store)?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
    let db = Connection::open(&file.0)?;
    db.execute_batch("CREATE TRIGGER block_head BEFORE UPDATE OF head ON branches BEGIN SELECT RAISE(ABORT,'injected failure'); END;")?;
    assert!(store.commit_report_verdict(id).is_err());
    assert_eq!(counts(&file)?, (1, 0, 0, 0));
    assert_eq!(store.read_current(ACTOR, SCOPE, "plan")?, Current::Empty);
    db.execute_batch("DROP TRIGGER block_head")?;
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(counts(&file)?, (1, 1, 0, 0));
    Ok(())
}

#[test]
fn v7_migration_preserves_accepted_usage_and_unknown_reservations() -> Result<()> {
    let file = Database::new();
    let legacy = Connection::open(&file.0)?;
    for script in [
        include_str!("../src/schema.sql"),
        include_str!("../src/attempts.sql"),
        include_str!("../src/support.sql"),
        include_str!("../src/dispatch.sql"),
        include_str!("../src/model_reuse.sql"),
        include_str!("../src/validation.sql"),
        include_str!("../src/token_budget.sql"),
        include_str!("../src/recovery.sql"),
    ] {
        legacy.execute_batch(script)?;
    }
    legacy.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    legacy.pragma_update(None, "user_version", 7)?;
    legacy.execute_batch(
        "INSERT INTO branches(id,actor,scope,name) VALUES(1,'reviewer','release','legacy');
        INSERT INTO attempts(id,branch_id,branch_revision,view_epoch) VALUES(1,1,0,0);
        UPDATE branches SET head=1,revision=1 WHERE id=1;
        INSERT INTO attempts(id,branch_id,parent,branch_revision,view_epoch) VALUES(2,1,1,1,0);
        INSERT INTO dispatches VALUES(1,X'01'),(2,X'02');
        INSERT INTO token_budgets VALUES(1,30);
        INSERT INTO token_reservations VALUES(1,1,4,8,1),(2,1,4,8,1);
        INSERT INTO deliveries(attempt_id,disposition,output,usage) VALUES(1,'accepted',X'41',7);",
    )?;
    drop(legacy);
    let mut store = Store::open(&file.0)?;
    assert_eq!(
        store.token_budget_state(1)?,
        TokenBudgetState {
            limit: 30,
            spent: 7,
            reserved: 12
        }
    );
    assert_eq!(store.settle(1, b"A", 7)?, Disposition::Accepted);
    assert!(store.commit_report_verdict(2).is_err());
    let evidence = admit(&mut store)?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "new", &[evidence])?;
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    assert_eq!(
        store.token_budget_state(1)?,
        TokenBudgetState {
            limit: 30,
            spent: 7,
            reserved: 12
        }
    );
    // Admission changed the old attempt's view, but its measured late usage is
    // still reconciled once; deterministic work has not adopted/released it.
    assert_eq!(store.settle(2, b"late", 9)?, Disposition::StaleView);
    assert_eq!(store.settle(2, b"late", 9)?, Disposition::StaleView);
    assert_eq!(
        store.token_budget_state(1)?,
        TokenBudgetState {
            limit: 30,
            spent: 16,
            reserved: 0
        }
    );
    let db = Connection::open(&file.0)?;
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?,
        9
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))?,
        0
    );
    Ok(())
}

#[test]
fn typed_sql_guards_and_binding_failure_preserve_atomicity() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let report = admit(&mut store)?;
    let db = Connection::open(&file.0)?;
    db.execute_batch("CREATE TRIGGER block_binding BEFORE INSERT ON deterministic_verdicts BEGIN SELECT RAISE(ABORT,'injected binding failure'); END;")?;
    assert!(store
        .begin_report_verdict(ACTOR, SCOPE, "plan", &[report])
        .is_err());
    assert_eq!(counts(&file)?, (0, 0, 0, 0));
    db.execute_batch("DROP TRIGGER block_binding")?;
    let id = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[report])?;
    let budget = store.create_token_budget(10)?;
    let recovery = store.create_recovery("typed-guard", budget, ACTOR, SCOPE, "other")?;
    assert!(db
        .execute(
            "INSERT INTO model_attempts(attempt_id,contract) VALUES(?1,'closed/v1')",
            [id]
        )
        .is_err());
    assert!(db.execute("INSERT INTO recovery_attempts(recovery_id,ordinal,attempt_id,state) VALUES(?1,0,?2,'planned')", rusqlite::params![recovery,id]).is_err());
    assert!(db
        .execute("INSERT INTO dispatches VALUES(?1,X'01')", [id])
        .is_err());
    assert!(db
        .execute(
            "INSERT INTO token_reservations VALUES(?1,?2,1,1,1)",
            rusqlite::params![id, budget]
        )
        .is_err());
    assert!(db.execute("INSERT INTO deliveries(attempt_id,disposition,output,usage) VALUES(?1,'accepted',X'01',0)",[id]).is_err());
    assert!(db.execute("INSERT INTO deliveries(attempt_id,disposition,output,usage) VALUES(?1,'accepted',?2,1)",rusqlite::params![id,VERDICT]).is_err());
    assert!(db
        .execute(
            "UPDATE deterministic_verdicts SET output=X'01' WHERE attempt_id=?1",
            [id]
        )
        .is_err());
    assert!(db
        .execute(
            "DELETE FROM deterministic_verdicts WHERE attempt_id=?1",
            [id]
        )
        .is_err());
    assert_eq!(counts(&file)?, (1, 0, 0, 0));
    assert_eq!(store.commit_report_verdict(id)?, Disposition::Accepted);
    let legacy = store.begin_attempt(ACTOR, SCOPE, "legacy", &[])?;
    assert!(store.reserve_dispatch(budget, legacy, b"uncertain", 1, 1)?);
    assert!(db.execute("INSERT INTO deterministic_verdicts(attempt_id,computation,output) VALUES(?1,'report-verdict/v1',?2)", rusqlite::params![legacy,VERDICT]).is_err());
    assert_eq!(
        store.token_budget_state(budget)?,
        TokenBudgetState {
            limit: 10,
            spent: 0,
            reserved: 2
        }
    );
    Ok(())
}

#[test]
fn distinct_identical_reports_are_not_coalesced_and_legacy_subset_capture_is_unchanged(
) -> Result<()> {
    let mut store = Store::in_memory()?;
    let first = admit(&mut store)?;
    let second = admit(&mut store)?;
    assert!(store
        .begin_report_verdict(ACTOR, SCOPE, "plan", &[first, second])
        .is_err());
    let hidden = store.observe("hidden", REPORT)?;
    let (_, selected) = store.begin_from_candidates(ACTOR, SCOPE, "legacy", &[first, hidden])?;
    assert_eq!(selected, vec![(first, REPORT.to_vec())]);
    Ok(())
}

#[test]
fn integer_lexemes_and_string_bytes_have_an_explicit_strict_rust_contract() -> Result<()> {
    let mut store = Store::in_memory()?;
    for bytes in [
        br#"{"release":"r","passed":-0,"total":1}"#.as_slice(),
        br#"{"release":"r","passed":0,"\u0070assed":1,"total":1}"#,
        br#"{"release":"\udfff","passed":0,"total":1}"#,
    ] {
        let id = store.observe("bad", bytes)?;
        store.set_admitted(ACTOR, SCOPE, id, true)?;
        assert!(store
            .begin_report_verdict(ACTOR, SCOPE, "plan", &[id])
            .is_err());
    }
    // u64 parsing is exact above i64::MAX, with no float intermediate. The
    // strict serde u64 route refuses the -0 representation (unlike Python JSON).
    let id=store.observe("large",br#"{"release":" \u0000e\u0301","passed":9223372036854775809,"total":18446744073709551615}"#)?;
    store.set_admitted(ACTOR, SCOPE, id, true)?;
    let attempt = store.begin_report_verdict(ACTOR, SCOPE, "plan", &[id])?;
    assert_eq!(store.commit_report_verdict(attempt)?, Disposition::Accepted);
    let Current::Ready(bytes) = store.read_current(ACTOR, SCOPE, "plan")? else {
        panic!("no verdict")
    };
    let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        actual,
        serde_json::json!({"release":" \0e\u{301}","tests":"9223372036854775809/18446744073709551615","status":"fail"})
    );
    Ok(())
}
