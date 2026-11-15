use kairotensor::{
    Current, CurrentExplanation, Disposition, EligibilityProof as Proof,
    EligibilityReason as Reason, EligibilityState as State, ModelAction, Store,
};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "kairotensor-eligibility-{}-{}.sqlite",
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

// No concurrent writer exists during these separate public calls.
fn check(
    store: &mut Store,
    actor: &str,
    scope: &str,
    branch: &str,
    expected: CurrentExplanation,
    current: Current,
) -> Result<()> {
    assert_eq!(store.explain_current(actor, scope, branch)?, expected);
    assert_eq!(store.read_current(actor, scope, branch)?, current);
    Ok(())
}

#[test]
fn explanation_matches_current_gate() -> Result<()> {
    let mut store = Store::in_memory()?;
    let empty = CurrentExplanation {
        state: State::Empty,
        head_id: None,
        branch_revision: None,
        captured_epoch: None,
        current_epoch: 0,
        proof_kind: None,
        reason: Some(Reason::NoHead),
    };
    check(&mut store, "a", "s", "b", empty.clone(), Current::Empty)?;
    store.begin_attempt("a", "s", "b", &[])?;
    check(
        &mut store,
        "a",
        "s",
        "b",
        CurrentExplanation {
            branch_revision: Some(0),
            ..empty.clone()
        },
        Current::Empty,
    )?;
    assert!(store
        .begin_resume("a", "s", "missing", &[])
        .unwrap_err()
        .to_string()
        .contains("no head to resume"));
    assert!(store
        .begin_resume("a", "s", "b", &[])
        .unwrap_err()
        .to_string()
        .contains("no head to resume"));
    let id = store.observe("private source", b"private evidence")?;
    store.set_admitted("a", "s", id, true)?;
    let opaque = store.begin_attempt("a", "s", "b", &[id])?;
    store.settle(opaque, b"opaque", 3)?;
    let ready = CurrentExplanation {
        state: State::Ready,
        head_id: Some(opaque),
        branch_revision: Some(1),
        captured_epoch: Some(1),
        current_epoch: 1,
        proof_kind: Some(Proof::CoarseEpoch),
        reason: None,
    };
    check(
        &mut store,
        "a",
        "s",
        "b",
        ready.clone(),
        Current::Ready(b"opaque".to_vec()),
    )?;
    store.set_admitted("a", "s", id, false)?;
    check(
        &mut store,
        "a",
        "s",
        "b",
        CurrentExplanation {
            state: State::NeedsReview,
            current_epoch: 2,
            reason: Some(Reason::CoarseViewChanged),
            ..ready
        },
        Current::NeedsReview,
    )?;
    for (actor, scope) in [("other", "s"), ("a", "other")] {
        check(&mut store, actor, scope, "b", empty.clone(), Current::Empty)?;
    }

    // Both published verdicts: same-verdict reuse and transitions in both directions.
    for initially_admitted in [false, true] {
        let mut store = Store::in_memory()?;
        let witness = store.observe("witness", b"sensitive")?;
        let unrelated = store.observe("unrelated", b"other")?;
        store.set_admitted("a", "s", witness, initially_admitted)?;
        let head = store.publish_support("a", "s", "support", &[vec![witness]])?;
        let epoch = i64::from(initially_admitted);
        let initial = CurrentExplanation {
            state: State::Ready,
            head_id: Some(head),
            branch_revision: Some(1),
            captured_epoch: Some(epoch),
            current_epoch: epoch,
            proof_kind: Some(Proof::CoarseEpoch),
            reason: None,
        };
        let output = if initially_admitted {
            b"supported".to_vec()
        } else {
            b"unsupported".to_vec()
        };
        check(
            &mut store,
            "a",
            "s",
            "support",
            initial.clone(),
            Current::Ready(output.clone()),
        )?;
        store.set_admitted("a", "s", unrelated, true)?;
        let selective = CurrentExplanation {
            proof_kind: Some(Proof::SupportV1),
            current_epoch: epoch + 1,
            ..initial
        };
        check(
            &mut store,
            "a",
            "s",
            "support",
            selective.clone(),
            Current::Ready(output.clone()),
        )?;
        assert_eq!(store.begin_resume("a", "s", "support", &[])?.1, output);
        store.set_admitted("a", "s", witness, !initially_admitted)?;
        check(
            &mut store,
            "a",
            "s",
            "support",
            CurrentExplanation {
                state: State::NeedsReview,
                current_epoch: epoch + 2,
                reason: Some(Reason::SupportVerdictChanged),
                ..selective
            },
            Current::NeedsReview,
        )?;
        assert!(store
            .begin_resume("a", "s", "support", &[])
            .unwrap_err()
            .to_string()
            .contains("head must be rebuilt before resume"));
    }
    Ok(())
}

#[test]
fn historical_acceptance_does_not_grant_current_access() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let id = store.observe_and_admit_many("a", "s", &[("source", b"evidence")])?[0];
    let head = store.begin_attempt("a", "s", "b", &[id])?;
    let budget = store.create_token_budget(100)?;
    store.reserve_dispatch(budget, head, b"request", 2, 8)?;
    assert_eq!(store.settle(head, b"accepted", 7)?, Disposition::Accepted);
    let pending = store.begin_attempt("a", "s", "uncertain", &[])?;
    store.reserve_dispatch(budget, pending, b"unknown send", 4, 10)?;
    store.set_admitted("a", "s", id, false)?;
    let before_budget = store.token_budget_state(budget)?;
    let audit = Connection::open(&file.0)?;
    let before = snapshot(&audit)?;
    for _ in 0..3 {
        assert_eq!(
            store.explain_current("a", "s", "b")?.state,
            State::NeedsReview
        );
        assert_eq!(store.read_current("a", "s", "b")?, Current::NeedsReview);
        assert!(store
            .begin_resume("a", "s", "b", &[])
            .unwrap_err()
            .to_string()
            .contains("head must be rebuilt before resume"));
        assert_eq!(store.settle(head, b"accepted", 7)?, Disposition::Accepted);
        assert_eq!(
            store.explain_current("missing", "s", "b")?.state,
            State::Empty
        );
    }
    assert_eq!(snapshot(&audit)?, before);
    assert_eq!(store.token_budget_state(budget)?, before_budget);
    assert_eq!(store.charged_usage()?, 7);
    assert_eq!(before_budget.spent, 7);
    assert_eq!(before_budget.reserved, 14);
    Ok(())
}

fn snapshot(db: &Connection) -> Result<Vec<Vec<rusqlite::types::Value>>> {
    let mut snapshot = Vec::new();
    for table in [
        "branches",
        "attempts",
        "attempt_inputs",
        "admission_events",
        "deliveries",
        "dispatches",
        "token_budgets",
        "token_reservations",
    ] {
        let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY 1, 2"))?;
        let columns = statement.column_count();
        snapshot.extend(
            statement
                .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())?
                .collect::<Result<Vec<_>>>()?,
        );
    }
    Ok(snapshot)
}

#[test]
fn model_reuse_requires_explicit_resolution() -> Result<()> {
    let mut store = Store::in_memory()?;
    let id = store.observe_and_admit_many("a", "s", &[("source", b"evidence")])?[0];
    let (origin, _) = store.begin_model_attempt("a", "s", "b", &[id], "closed/v1")?;
    assert_eq!(
        store.resolve_model_attempt(origin, b"request")?,
        ModelAction::Dispatch
    );
    store.settle(origin, b"model output", 9)?;
    store.observe_and_admit_many("a", "s", &[("irrelevant", b"new")])?;
    let stale = store.explain_current("a", "s", "b")?;
    assert_eq!(stale.state, State::NeedsReview);
    assert_eq!(stale.proof_kind, Some(Proof::CoarseEpoch));
    assert_eq!(stale.reason, Some(Reason::CoarseViewChanged));
    assert!(store.begin_resume("a", "s", "b", &[]).is_err());
    let (next, _) = store.begin_model_attempt("a", "s", "b", &[id], "closed/v1")?;
    assert_eq!(store.explain_current("a", "s", "b")?, stale);
    assert_eq!(
        store.resolve_model_attempt(next, b"request")?,
        ModelAction::Reused {
            origin_attempt: origin,
            output: b"model output".to_vec()
        }
    );
    let ready = store.explain_current("a", "s", "b")?;
    assert_eq!(ready.state, State::Ready);
    assert_eq!(ready.head_id, Some(next));
    assert_eq!(ready.branch_revision, Some(2));
    assert_eq!(ready.proof_kind, Some(Proof::CoarseEpoch));
    assert_eq!(
        store.read_current("a", "s", "b")?,
        Current::Ready(b"model output".to_vec())
    );
    assert_eq!(store.charged_usage()?, 9);
    Ok(())
}

#[test]
fn explanation_contains_no_payload_or_validation_text() -> Result<()> {
    let mut store = Store::in_memory()?;
    let evidence =
        store.observe_and_admit_many("a", "s", &[("SECRET_SOURCE", b"SECRET_EVIDENCE")])?[0];
    let head = store.begin_attempt("a", "s", "b", &[evidence])?;
    store.record_dispatch(head, b"SECRET_REQUEST")?;
    store.settle(head, b"SECRET_OUTPUT", 2)?;
    let rejected = store.begin_attempt("a", "s", "b", &[evidence])?;
    store.record_dispatch(rejected, b"SECRET_REJECTED_REQUEST")?;
    store.reject_invalid(
        rejected,
        b"SECRET_REJECTED",
        4,
        "SECRET_VALIDATOR",
        "SECRET_REASON",
    )?;
    let ready = store.explain_current("a", "s", "b")?;
    assert_eq!(ready.state, State::Ready);
    store.set_admitted("a", "s", evidence, false)?;
    let stale = store.explain_current("a", "s", "b")?;
    assert_eq!(stale.state, State::NeedsReview);
    for explanation in [ready, stale] {
        // Fixed typed fields constrain the public surface; Debug must not leak any text.
        let CurrentExplanation {
            state: _,
            head_id: _,
            branch_revision: _,
            captured_epoch: _,
            current_epoch: _,
            proof_kind: _,
            reason: _,
        } = explanation;
        assert!(!format!("{explanation:?}").contains("SECRET"));
    }
    Ok(())
}

#[test]
fn malformed_head_without_accepted_delivery_is_an_error() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let pending = store.begin_attempt("a", "s", "b", &[])?;
    let db = Connection::open(&file.0)?;
    db.execute("UPDATE branches SET head=?1, revision=1", [pending])?;
    for changed_epoch in [false, true] {
        if changed_epoch {
            store.observe_and_admit_many("a", "s", &[("new", b"x")])?;
        }
        assert!(store.explain_current("a", "s", "b").is_err());
        assert!(store.read_current("a", "s", "b").is_err());
        assert!(store.begin_resume("a", "s", "b", &[]).is_err());
    }
    Ok(())
}
