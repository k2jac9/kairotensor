use kairotensor::{CapturedInputs, Current, Disposition, RecoveryStart, Store, TokenBudgetState};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

const ACTOR: &str = "reviewer";
const SCOPE: &str = "release";
const REQUEST: &[u8] = b"frozen\0request\xff";

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-recovery-{}-{stamp}-{}.db",
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
fn begin(store: &mut Store, recovery: i64, candidates: &[i64]) -> Result<(i64, CapturedInputs)> {
    match store.begin_recovery_attempt(recovery, candidates)? {
        RecoveryStart::New {
            attempt, inputs, ..
        } => Ok((attempt, inputs)),
        other => panic!("expected a new attempt, got {other:?}"),
    }
}
fn budget(store: &Store, id: i64, limit: i64, spent: i64, reserved: i64) -> Result<()> {
    assert_eq!(
        store.token_budget_state(id)?,
        TokenBudgetState {
            limit,
            spent,
            reserved
        }
    );
    Ok(())
}
fn audit(file: &Database) -> Result<(i64, i64)> {
    Connection::open(&file.0)?.query_row(
        "SELECT (SELECT COUNT(*) FROM dispatches), (SELECT COUNT(*) FROM deliveries)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}
fn reject(store: &mut Store, attempt: i64) -> Result<Disposition> {
    store.reject_invalid(attempt, b"invalid", 7, "validator/v1", "malformed")
}

#[test]
fn creation_is_idempotent_but_bindings_are_immutable_across_reopen() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(40)?;
    let other = store.create_token_budget(40)?;
    let r = store.create_recovery("job", b, ACTOR, SCOPE, "plan")?;
    assert_eq!(store.create_recovery("job", b, ACTOR, SCOPE, "plan")?, r);
    for (bid, actor, scope, branch) in [
        (other, ACTOR, SCOPE, "plan"),
        (b, "other", SCOPE, "plan"),
        (b, ACTOR, "other", "plan"),
        (b, ACTOR, SCOPE, "other"),
    ] {
        assert!(store
            .create_recovery("job", bid, actor, scope, branch)
            .is_err());
    }
    assert!(store
        .create_recovery("missing-budget", -1, ACTOR, SCOPE, "plan")
        .is_err());
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(store.create_recovery("job", b, ACTOR, SCOPE, "plan")?, r);
    assert_ne!(
        store.create_recovery("other-job", b, ACTOR, SCOPE, "plan")?,
        r
    );
    budget(&store, b, 40, 0, 0)?;
    assert_eq!(audit(&file)?, (0, 0));
    Ok(())
}

#[test]
fn retry_slots_and_fresh_inputs_survive_reopen_and_stop_after_two_invalid_replies() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let a = store.observe("a", b"first")?;
    let hidden = store.observe("hidden", b"secret")?;
    store.set_admitted(ACTOR, SCOPE, a, true)?;
    let b = store.create_token_budget(40)?;
    let r = store.create_recovery("retry", b, ACTOR, SCOPE, "plan")?;
    let (first, inputs) = begin(&mut store, r, &[a, hidden])?;
    assert_eq!(inputs, vec![(a, b"first".to_vec())]);
    assert!(
        matches!(store.begin_recovery_attempt(r, &[])?, RecoveryStart::Existing(id) if id == first)
    );
    assert!(store.reserve_recovery_attempt(first, REQUEST, 3, 7)?);
    assert!(store
        .reserve_recovery_attempt(first, REQUEST, 3, 7)
        .is_err());
    assert!(store.settle(first, b"unsent", 4).is_err());
    assert!(reject(&mut store, first).is_err());
    assert_eq!(
        store.claim_recovery_dispatch(first)?,
        Some(REQUEST.to_vec())
    );
    assert_eq!(store.claim_recovery_dispatch(first)?, None);
    assert_eq!(reject(&mut store, first)?, Disposition::InvalidOutput);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(reject(&mut store, first)?, Disposition::InvalidOutput);
    store.set_admitted(ACTOR, SCOPE, a, false)?;
    store.set_admitted(ACTOR, SCOPE, hidden, true)?;
    let (second, inputs) = begin(&mut store, r, &[a, hidden])?;
    assert_ne!(first, second);
    assert_eq!(inputs, vec![(hidden, b"secret".to_vec())]);
    assert!(store.reserve_recovery_attempt(second, REQUEST, 3, 7)?);
    assert_eq!(
        store.claim_recovery_dispatch(second)?,
        Some(REQUEST.to_vec())
    );
    assert_eq!(reject(&mut store, second)?, Disposition::InvalidOutput);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert!(matches!(
        store.begin_recovery_attempt(r, &[hidden])?,
        RecoveryStart::Finished
    ));
    assert_eq!(reject(&mut store, second)?, Disposition::InvalidOutput);
    budget(&store, b, 40, 14, 0)?;
    assert_eq!(store.charged_usage()?, 14);
    assert_eq!(audit(&file)?, (2, 2));
    Ok(())
}

#[test]
fn uncertain_sent_is_never_released_or_resent_and_late_valid_reply_charges_once() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("uncertain", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
    assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
    drop(store);
    let mut store = Store::open(&file.0)?;
    budget(&store, b, 20, 0, 12)?;
    assert!(matches!(store.begin_recovery_attempt(r, &[])?, RecoveryStart::Existing(i) if i == id));
    assert!(!store.cancel_recovery_attempt(id)?);
    assert_eq!(store.claim_recovery_dispatch(id)?, None);
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8).is_err());
    assert_eq!(store.settle(id, b"valid", 9)?, Disposition::Accepted);
    assert_eq!(store.settle(id, b"valid", 9)?, Disposition::Accepted);
    assert!(store.settle(id, b"changed", 9).is_err());
    assert!(reject(&mut store, id).is_err());
    assert!(matches!(
        store.begin_recovery_attempt(r, &[])?,
        RecoveryStart::Finished
    ));
    assert_eq!(
        store.read_current(ACTOR, SCOPE, "plan")?,
        Current::Ready(b"valid".to_vec())
    );
    budget(&store, b, 20, 9, 0)?;
    assert_eq!(store.charged_usage()?, 9);
    assert_eq!(audit(&file)?, (1, 1));
    Ok(())
}

#[test]
fn planned_and_reserved_cancellation_release_only_known_unsent_work() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(12)?;
    for (key, reserved) in [("planned", false), ("reserved", true)] {
        let r = store.create_recovery(key, b, ACTOR, SCOPE, key)?;
        let (id, _) = begin(&mut store, r, &[])?;
        if reserved {
            assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
        }
        drop(store);
        store = Store::open(&file.0)?;
        assert!(store.cancel_recovery_attempt(id)?);
        assert!(store.cancel_recovery_attempt(id)?);
        assert_eq!(store.claim_recovery_dispatch(id)?, None);
        assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8).is_err());
        assert!(store.settle(id, b"fabricated", 1).is_err());
        assert!(reject(&mut store, id).is_err());
        assert!(matches!(
            store.begin_recovery_attempt(r, &[])?,
            RecoveryStart::Finished
        ));
        budget(&store, b, 12, 0, 0)?;
    }
    assert_eq!(store.charged_usage()?, 0);
    assert_eq!(audit(&file)?, (0, 0));
    Ok(())
}

#[test]
fn stale_claim_keeps_hold_until_cancel_but_stale_sent_reply_is_charged() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(40)?;
    let r = store.create_recovery("unsent", b, ACTOR, SCOPE, "unsent")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 2, 8)?);
    store.observe_and_admit_many(ACTOR, SCOPE, &[("change", b"one")])?;
    assert!(store.claim_recovery_dispatch(id).is_err());
    budget(&store, b, 40, 0, 10)?;
    assert!(store.cancel_recovery_attempt(id)?);
    budget(&store, b, 40, 0, 0)?;
    let r = store.create_recovery("sent", b, ACTOR, SCOPE, "sent")?;
    let (sent, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(sent, REQUEST, 2, 8)?);
    assert_eq!(store.claim_recovery_dispatch(sent)?, Some(REQUEST.to_vec()));
    store.observe_and_admit_many(ACTOR, SCOPE, &[("change", b"two")])?;
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert!(!store.cancel_recovery_attempt(sent)?);
    assert_eq!(store.settle(sent, b"late", 6)?, Disposition::StaleView);
    assert_eq!(store.settle(sent, b"late", 6)?, Disposition::StaleView);
    assert!(matches!(
        store.begin_recovery_attempt(r, &[])?,
        RecoveryStart::Finished
    ));
    budget(&store, b, 40, 6, 0)?;
    assert_eq!(audit(&file)?, (1, 1));
    Ok(())
}

#[test]
fn denied_budget_is_durable_terminal_and_cannot_be_bypassed() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(9)?;
    let r = store.create_recovery("denied", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.record_dispatch(id, REQUEST).is_err());
    assert!(store.reserve_dispatch(b, id, REQUEST, 1, 1).is_err());
    assert!(!store.reserve_recovery_attempt(id, REQUEST, 2, 8)?);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert!(!store.cancel_recovery_attempt(id)?);
    assert_eq!(store.claim_recovery_dispatch(id)?, None);
    assert!(store.reserve_recovery_attempt(id, REQUEST, 1, 1).is_err());
    assert!(store.settle(id, b"fabricated", 0).is_err());
    assert!(matches!(
        store.begin_recovery_attempt(r, &[])?,
        RecoveryStart::Finished
    ));
    budget(&store, b, 9, 0, 0)?;
    assert_eq!(audit(&file)?, (0, 0));
    Ok(())
}

#[test]
fn concurrent_begin_allocates_exactly_one_initial_and_one_retry_slot() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(40)?;
    let r = store.create_recovery("race", b, ACTOR, SCOPE, "plan")?;
    for _ in 0..2 {
        let start = Arc::new(Barrier::new(2));
        let workers: Vec<_> = [Store::open(&file.0)?, Store::open(&file.0)?]
            .into_iter()
            .map(|mut db| {
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    db.begin_recovery_attempt(r, &[])
                })
            })
            .collect();
        let mut new = Vec::new();
        let mut existing = Vec::new();
        for worker in workers {
            match worker.join().unwrap()? {
                RecoveryStart::New { attempt, .. } => new.push(attempt),
                RecoveryStart::Existing(attempt) => existing.push(attempt),
                RecoveryStart::Finished => panic!("slot ended before dispatch"),
            }
        }
        assert_eq!(new.len(), 1);
        assert_eq!(existing, new);
        let id = new[0];
        assert!(store.reserve_recovery_attempt(id, REQUEST, 3, 7)?);
        assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
        assert_eq!(reject(&mut store, id)?, Disposition::InvalidOutput);
    }
    assert!(matches!(
        store.begin_recovery_attempt(r, &[])?,
        RecoveryStart::Finished
    ));
    budget(&store, b, 40, 14, 0)?;
    assert_eq!(audit(&file)?, (2, 2));
    Ok(())
}

#[test]
fn concurrent_claim_and_cancel_cannot_both_win() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("race", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
    let barrier = Arc::new(Barrier::new(2));
    let mut sender = Store::open(&file.0)?;
    let mut canceller = Store::open(&file.0)?;
    let send_start = barrier.clone();
    let send = std::thread::spawn(move || {
        send_start.wait();
        sender.claim_recovery_dispatch(id)
    });
    let cancel = std::thread::spawn(move || {
        barrier.wait();
        canceller.cancel_recovery_attempt(id)
    });
    let sent = send.join().unwrap()?;
    let cancelled = cancel.join().unwrap()?;
    assert_ne!(sent.is_some(), cancelled);
    assert_eq!(store.claim_recovery_dispatch(id)?, None);
    assert_eq!(store.charged_usage()?, 0);
    if cancelled {
        budget(&store, b, 20, 0, 0)?;
        assert_eq!(audit(&file)?, (0, 0));
        assert!(store.settle(id, b"fake", 8).is_err());
    } else {
        assert_eq!(sent, Some(REQUEST.to_vec()));
        budget(&store, b, 20, 0, 12)?;
        assert_eq!(audit(&file)?, (1, 0));
        assert!(!store.cancel_recovery_attempt(id)?);
        assert_eq!(store.settle(id, b"late", 8)?, Disposition::Accepted);
        budget(&store, b, 20, 8, 0)?;
    }
    Ok(())
}

#[test]
fn legacy_dispatches_remain_potentially_sent_and_not_recovery_cancellable() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let id = store.begin_attempt(ACTOR, SCOPE, "legacy", &[])?;
    assert!(store.reserve_dispatch(b, id, REQUEST, 4, 8)?);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert!(!matches!(store.cancel_recovery_attempt(id), Ok(true)));
    budget(&store, b, 20, 0, 12)?;
    assert_eq!(audit(&file)?, (1, 0));
    assert_eq!(store.settle(id, b"legacy late", 9)?, Disposition::Accepted);
    assert_eq!(store.settle(id, b"legacy late", 9)?, Disposition::Accepted);
    budget(&store, b, 20, 9, 0)?;
    assert_eq!(store.charged_usage()?, 9);
    Ok(())
}

#[test]
fn concurrent_claim_returns_frozen_request_to_exactly_one_sender() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("claim-race", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
    let start = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [Store::open(&file.0)?, Store::open(&file.0)?]
        .into_iter()
        .map(|mut db| {
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                db.claim_recovery_dispatch(id)
            })
        })
        .collect();
    let results = workers
        .into_iter()
        .map(|w| w.join().unwrap())
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
    assert_eq!(
        results.into_iter().flatten().collect::<Vec<_>>(),
        vec![REQUEST.to_vec()]
    );
    budget(&store, b, 20, 0, 12)?;
    assert_eq!(audit(&file)?, (1, 0));
    Ok(())
}

#[test]
fn cancelled_or_denied_second_slot_never_reopens_first_invalid_result() -> Result<()> {
    for deny in [false, true] {
        let file = Database::new();
        let mut store = Store::open(&file.0)?;
        let b = store.create_token_budget(10)?;
        let r = store.create_recovery("retry-terminal", b, ACTOR, SCOPE, "plan")?;
        let (first, _) = begin(&mut store, r, &[])?;
        assert!(store.reserve_recovery_attempt(first, REQUEST, 3, 7)?);
        assert_eq!(
            store.claim_recovery_dispatch(first)?,
            Some(REQUEST.to_vec())
        );
        assert_eq!(reject(&mut store, first)?, Disposition::InvalidOutput);
        let (second, _) = begin(&mut store, r, &[])?;
        if deny {
            assert!(!store.reserve_recovery_attempt(second, REQUEST, 3, 7)?);
        } else {
            assert!(store.reserve_recovery_attempt(second, REQUEST, 1, 2)?);
            assert!(store.cancel_recovery_attempt(second)?);
        }
        drop(store);
        let mut store = Store::open(&file.0)?;
        assert!(matches!(
            store.begin_recovery_attempt(r, &[])?,
            RecoveryStart::Finished
        ));
        assert_eq!(reject(&mut store, first)?, Disposition::InvalidOutput);
        assert!(matches!(
            store.begin_recovery_attempt(r, &[])?,
            RecoveryStart::Finished
        ));
        budget(&store, b, 10, 7, 0)?;
        assert_eq!(audit(&file)?, (1, 1));
    }
    Ok(())
}

#[test]
fn unsent_settlement_cannot_publish_a_branch_head_or_charge_usage() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("preclaim", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    for reserved in [false, true] {
        if reserved {
            assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
        }
        assert!(store.settle(id, b"must not publish", 5).is_err());
        assert!(reject(&mut store, id).is_err());
        assert_eq!(store.read_current(ACTOR, SCOPE, "plan")?, Current::Empty);
        assert_eq!(store.charged_usage()?, 0);
        assert_eq!(audit(&file)?, (0, 0));
    }
    assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
    assert_eq!(store.settle(id, b"real", 5)?, Disposition::Accepted);
    budget(&store, b, 20, 5, 0)?;
    Ok(())
}

#[test]
fn late_invalid_reply_after_restart_charges_once_and_unlocks_only_one_retry() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("late-invalid", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
    assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
    drop(store);
    let mut store = Store::open(&file.0)?;
    budget(&store, b, 20, 0, 12)?;
    assert!(!store.cancel_recovery_attempt(id)?);
    assert_eq!(reject(&mut store, id)?, Disposition::InvalidOutput);
    assert_eq!(reject(&mut store, id)?, Disposition::InvalidOutput);
    assert!(store
        .reject_invalid(id, b"invalid", 8, "validator/v1", "malformed")
        .is_err());
    assert!(store
        .reject_invalid(id, b"invalid", 7, "validator/v2", "malformed")
        .is_err());
    assert!(store.settle(id, b"invalid", 7).is_err());
    budget(&store, b, 20, 7, 0)?;
    assert_eq!(store.charged_usage()?, 7);
    assert_eq!(store.read_current(ACTOR, SCOPE, "plan")?, Current::Empty);
    let (retry, _) = begin(&mut store, r, &[])?;
    assert_ne!(id, retry);
    assert!(
        matches!(store.begin_recovery_attempt(r, &[])?, RecoveryStart::Existing(i) if i == retry)
    );
    assert_eq!(audit(&file)?, (1, 1));
    Ok(())
}

#[test]
fn reservation_readback_is_exact_across_restart_and_disappears_after_claim_or_cancel() -> Result<()>
{
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(30)?;
    for claim in [false, true] {
        let key = if claim { "claim" } else { "cancel" };
        let r = store.create_recovery(key, b, ACTOR, SCOPE, key)?;
        let (id, _) = begin(&mut store, r, &[])?;
        assert!(store.recovery_reservation(id)?.is_none());
        assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
        drop(store);
        store = Store::open(&file.0)?;
        let saved = store.recovery_reservation(id)?.expect("reserved quote");
        assert_eq!(saved.request, REQUEST);
        assert_eq!(saved.input_tokens, 4);
        assert_eq!(saved.output_cap, 8);
        assert_eq!(store.recovery_reservation(id)?, Some(saved));
        assert_eq!(audit(&file)?, (0, 0));
        if claim {
            assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
        } else {
            assert!(store.cancel_recovery_attempt(id)?);
        }
        assert!(store.recovery_reservation(id)?.is_none());
        drop(store);
        store = Store::open(&file.0)?;
        assert!(store.recovery_reservation(id)?.is_none());
    }
    budget(&store, b, 30, 0, 12)?;
    assert_eq!(audit(&file)?, (1, 0));
    Ok(())
}

#[test]
fn concurrent_conflicting_reservations_never_overwrite_the_winning_quote() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(30)?;
    let r = store.create_recovery("reserve-race", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [Store::open(&file.0)?, Store::open(&file.0)?]
        .into_iter()
        .enumerate()
        .map(|(index, mut db)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let (request, input, cap): (&[u8], i64, i64) = if index == 0 {
                    (b"alpha\0", 2, 8)
                } else {
                    (b"beta\xff", 4, 6)
                };
                barrier.wait();
                (
                    request.to_vec(),
                    input,
                    cap,
                    db.reserve_recovery_attempt(id, request, input, cap),
                )
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| matches!(r.3, Ok(true))).count(),
        1
    );
    assert_eq!(results.iter().filter(|r| r.3.is_err()).count(), 1);
    let winner = results.iter().find(|r| matches!(r.3, Ok(true))).unwrap();
    drop(store);
    let mut store = Store::open(&file.0)?;
    let saved = store
        .recovery_reservation(id)?
        .expect("winning reservation");
    assert_eq!(
        (&saved.request, saved.input_tokens, saved.output_cap),
        (&winner.0, winner.1, winner.2)
    );
    budget(&store, b, 30, 0, 10)?;
    assert_eq!(store.claim_recovery_dispatch(id)?, Some(winner.0.clone()));
    assert_eq!(audit(&file)?, (1, 0));
    Ok(())
}

#[test]
fn concurrent_cancel_and_reserve_leave_no_unsent_hold_or_dispatch() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("cancel-reserve", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    let barrier = Arc::new(Barrier::new(2));
    let mut reserver = Store::open(&file.0)?;
    let mut canceller = Store::open(&file.0)?;
    let reserve_start = barrier.clone();
    let reserve = std::thread::spawn(move || {
        reserve_start.wait();
        reserver.reserve_recovery_attempt(id, REQUEST, 4, 8)
    });
    let cancel = std::thread::spawn(move || {
        barrier.wait();
        canceller.cancel_recovery_attempt(id)
    });
    let reservation = reserve.join().unwrap();
    assert!(reservation.is_err() || matches!(reservation, Ok(true)));
    assert!(cancel.join().unwrap()?);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert!(store.cancel_recovery_attempt(id)?);
    assert!(store.recovery_reservation(id)?.is_none());
    assert_eq!(store.claim_recovery_dispatch(id)?, None);
    assert!(matches!(
        store.begin_recovery_attempt(r, &[])?,
        RecoveryStart::Finished
    ));
    budget(&store, b, 20, 0, 0)?;
    assert_eq!(store.charged_usage()?, 0);
    assert_eq!(audit(&file)?, (0, 0));
    Ok(())
}

#[test]
fn v6_migration_keeps_legacy_pending_reservation_uncancellable_and_supports_new_recovery(
) -> Result<()> {
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
    ] {
        legacy.execute_batch(script)?;
    }
    legacy.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    legacy.pragma_update(None, "user_version", 6)?;
    legacy.execute_batch(
        "INSERT INTO branches(id,actor,scope,name) VALUES(1,'reviewer','release','legacy');
         INSERT INTO attempts(id,branch_id,branch_revision,view_epoch) VALUES(1,1,0,0);
         INSERT INTO dispatches VALUES(1,X'01FF00');
         INSERT INTO token_budgets(id,token_limit) VALUES(1,30);
         INSERT INTO token_reservations(attempt_id,budget_id,input_tokens,output_cap,granted) VALUES(1,1,4,8,1);"
    )?;
    drop(legacy);
    let mut store = Store::open(&file.0)?;
    budget(&store, 1, 30, 0, 12)?;
    assert!(!matches!(store.cancel_recovery_attempt(1), Ok(true)));
    assert!(store.recovery_reservation(1)?.is_none());
    budget(&store, 1, 30, 0, 12)?;
    let r = store.create_recovery("post-migration", 1, ACTOR, SCOPE, "new")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert_ne!(id, 1);
    assert!(store.reserve_recovery_attempt(id, REQUEST, 2, 8)?);
    budget(&store, 1, 30, 0, 22)?;
    assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
    assert_eq!(store.settle(id, b"new result", 6)?, Disposition::Accepted);
    assert_eq!(store.settle(1, b"legacy result", 9)?, Disposition::Accepted);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(store.settle(1, b"legacy result", 9)?, Disposition::Accepted);
    assert_eq!(
        store.create_recovery("post-migration", 1, ACTOR, SCOPE, "new")?,
        r
    );
    assert!(matches!(
        store.begin_recovery_attempt(r, &[])?,
        RecoveryStart::Finished
    ));
    budget(&store, 1, 30, 15, 0)?;
    assert_eq!(store.charged_usage()?, 15);
    assert_eq!(audit(&file)?, (2, 2));
    let db = Connection::open(&file.0)?;
    let bytes: Vec<u8> = db.query_row(
        "SELECT request FROM dispatches WHERE attempt_id=1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(bytes, vec![1, 255, 0]);
    let broken: i64 = db.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
        row.get(0)
    })?;
    assert_eq!(broken, 0);
    Ok(())
}

#[test]
fn failing_dispatch_insert_rolls_back_claim_and_leaves_unsent_hold_cancellable() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("failed-claim", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
    let db = Connection::open(&file.0)?;
    db.execute_batch(
        "CREATE TRIGGER fail_dispatch_insert BEFORE INSERT ON dispatches
         BEGIN SELECT RAISE(ABORT, 'injected dispatch insert failure'); END;",
    )?;
    let error = store
        .claim_recovery_dispatch(id)
        .expect_err("claim must roll back");
    assert!(error
        .to_string()
        .contains("injected dispatch insert failure"));
    drop(store);
    let mut store = Store::open(&file.0)?;
    let reservation = store
        .recovery_reservation(id)?
        .expect("unsent reservation must survive");
    assert_eq!(reservation.request, REQUEST);
    assert_eq!(reservation.input_tokens, 4);
    assert_eq!(reservation.output_cap, 8);
    assert!(matches!(store.begin_recovery_attempt(r, &[])?, RecoveryStart::Existing(i) if i == id));
    budget(&store, b, 20, 0, 12)?;
    assert_eq!(audit(&file)?, (0, 0));
    assert!(store.settle(id, b"not sent", 5).is_err());
    assert!(store.cancel_recovery_attempt(id)?);
    assert!(store.recovery_reservation(id)?.is_none());
    assert_eq!(store.claim_recovery_dispatch(id)?, None);
    budget(&store, b, 20, 0, 0)?;
    assert_eq!(store.charged_usage()?, 0);
    assert_eq!(audit(&file)?, (0, 0));
    Ok(())
}

#[test]
fn failing_reservation_insert_rolls_back_quote_and_leaves_planned_attempt_retryable() -> Result<()>
{
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let b = store.create_token_budget(20)?;
    let r = store.create_recovery("failed-reserve", b, ACTOR, SCOPE, "plan")?;
    let (id, _) = begin(&mut store, r, &[])?;
    let db = Connection::open(&file.0)?;
    // AFTER INSERT makes the statement fail after SQLite has staged a hold.
    db.execute_batch(
        "CREATE TRIGGER fail_reservation_insert AFTER INSERT ON token_reservations
         BEGIN SELECT RAISE(ABORT, 'injected reservation insert failure'); END;",
    )?;
    let error = store
        .reserve_recovery_attempt(id, b"failed quote", 2, 8)
        .expect_err("reservation must roll back");
    assert!(error
        .to_string()
        .contains("injected reservation insert failure"));
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert!(store.recovery_reservation(id)?.is_none());
    assert_eq!(store.claim_recovery_dispatch(id)?, None);
    assert!(matches!(store.begin_recovery_attempt(r, &[])?, RecoveryStart::Existing(i) if i == id));
    budget(&store, b, 20, 0, 0)?;
    assert_eq!(store.charged_usage()?, 0);
    assert_eq!(audit(&file)?, (0, 0));
    db.execute_batch("DROP TRIGGER fail_reservation_insert")?;
    assert!(store.reserve_recovery_attempt(id, REQUEST, 4, 8)?);
    let reservation = store
        .recovery_reservation(id)?
        .expect("new quote should persist");
    assert_eq!(reservation.request, REQUEST);
    assert_eq!(reservation.input_tokens, 4);
    assert_eq!(reservation.output_cap, 8);
    budget(&store, b, 20, 0, 12)?;
    assert_eq!(store.claim_recovery_dispatch(id)?, Some(REQUEST.to_vec()));
    assert_eq!(store.settle(id, b"real result", 6)?, Disposition::Accepted);
    budget(&store, b, 20, 6, 0)?;
    assert_eq!(audit(&file)?, (1, 1));
    Ok(())
}

#[test]
fn stale_invalid_reply_can_unlock_only_one_fresh_recapture_after_restart() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let old = store.observe("old", b"obsolete report")?;
    store.set_admitted(ACTOR, SCOPE, old, true)?;
    let b = store.create_token_budget(30)?;
    let r = store.create_recovery("stale-invalid-recapture", b, ACTOR, SCOPE, "plan")?;
    let (first, _) = begin(&mut store, r, &[old])?;
    assert!(store.reserve_recovery_attempt(first, REQUEST, 4, 8)?);
    assert_eq!(
        store.claim_recovery_dispatch(first)?,
        Some(REQUEST.to_vec())
    );
    store.set_admitted(ACTOR, SCOPE, old, false)?;
    let fresh = store.observe("new", b"corrected report")?;
    store.set_admitted(ACTOR, SCOPE, fresh, true)?;
    assert_eq!(reject(&mut store, first)?, Disposition::InvalidOutput);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(reject(&mut store, first)?, Disposition::InvalidOutput);
    budget(&store, b, 30, 7, 0)?;
    let db = Connection::open(&file.0)?;
    let applicability: String = db.query_row(
        "SELECT applicability FROM deliveries WHERE attempt_id=?",
        [first],
        |row| row.get(0),
    )?;
    assert_eq!(applicability, "stale_view");
    // Core recovery may recapture after an invalid stale reply. A caller's
    // stricter retry policy can stop instead; invalid bytes are never evidence.
    let (second, inputs) = begin(&mut store, r, &[old, fresh])?;
    assert_eq!(inputs, vec![(fresh, b"corrected report".to_vec())]);
    assert_ne!(first, second);
    assert!(store.reserve_recovery_attempt(second, b"fresh request", 4, 8)?);
    assert_eq!(
        store.claim_recovery_dispatch(second)?,
        Some(b"fresh request".to_vec())
    );
    assert_eq!(reject(&mut store, second)?, Disposition::InvalidOutput);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(reject(&mut store, second)?, Disposition::InvalidOutput);
    assert!(matches!(
        store.begin_recovery_attempt(r, &[fresh])?,
        RecoveryStart::Finished
    ));
    budget(&store, b, 30, 14, 0)?;
    assert_eq!(store.charged_usage()?, 14);
    assert_eq!(audit(&file)?, (2, 2));
    Ok(())
}
