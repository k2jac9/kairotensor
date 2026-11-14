use kairotensor::{
    Disposition, RecoveryCalibration, RecoveryClock, RecoveryPreparation, RecoveryReadiness,
    RecoveryRuntime, RecoveryStart, RecoveryTiming, Store, TimedRecoveryClaim, TimingRefusal,
    TokenBudgetState,
};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-timing-{}-{stamp}-{}.db",
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
fn timing() -> RecoveryTiming {
    RecoveryTiming {
        clock_domain: "boot/session-A".into(),
        start_ms: 100,
        deadline_ms: 1000,
        safety_margin_ms: 20,
        runtime: RecoveryRuntime {
            model: "model".into(),
            model_revision: "revision".into(),
            tokenizer_revision: "tokenizer".into(),
            runtime_version: "runtime/1".into(),
            dtype: "bfloat16".into(),
            decoding: "greedy".into(),
            cpu_fingerprint: "a".repeat(64),
            threads: 2,
            prompt_fingerprint: "b".repeat(64),
            readiness: RecoveryReadiness::Warm,
        },
        calibration: RecoveryCalibration {
            id: "calibration/1".into(),
            max_duration_ms: 100,
            sample_count: 2,
            max_input_tokens: 179,
            output_cap: 8,
            measured_at_ms: 50,
            expires_at_ms: 2000,
        },
    }
}
fn prep(t: &RecoveryTiming) -> RecoveryPreparation {
    RecoveryPreparation {
        runtime: Some(t.runtime.clone()),
        calibration_id: Some(t.calibration.id.clone()),
        request: b"frozen\0request\xff".to_vec(),
        input_tokens: 4,
        output_cap: 8,
    }
}
fn clock(now_ms: i64) -> Result<RecoveryClock> {
    Ok(RecoveryClock {
        domain: "boot/session-A".into(),
        now_ms,
    })
}
fn begin(s: &mut Store, r: i64, candidates: &[i64]) -> Result<i64> {
    match s.begin_recovery_attempt(r, candidates)? {
        RecoveryStart::New { attempt, .. } => Ok(attempt),
        other => panic!("expected new, got {other:?}"),
    }
}
fn setup(
    s: &mut Store,
    limit: i64,
) -> Result<(i64, i64, i64, RecoveryTiming, RecoveryPreparation)> {
    let t = timing();
    let p = prep(&t);
    let b = s.create_token_budget(limit)?;
    let r = s.create_timed_recovery("operation", b, "actor", "scope", "branch", &t)?;
    let a = begin(s, r, &[])?;
    assert!(s.reserve_recovery_attempt(a, &p.request, p.input_tokens, p.output_cap)?);
    Ok((b, r, a, t, p))
}
fn refused(result: TimedRecoveryClaim, expected: TimingRefusal) {
    assert!(matches!(result,TimedRecoveryClaim::Refused{reason,..} if reason==expected));
}
fn counts(file: &Database) -> Result<(i64, i64)> {
    Connection::open(&file.0)?.query_row(
        "SELECT (SELECT COUNT(*) FROM dispatches),(SELECT COUNT(*) FROM deliveries)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

#[test]
fn timed_binding_is_exact_idempotent_durable_and_cannot_adopt_legacy_work() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (b, r, _, t, _) = setup(&mut s, 100)?;
    assert_eq!(
        s.create_timed_recovery("operation", b, "actor", "scope", "branch", &t)?,
        r
    );
    for changed in [
        RecoveryTiming {
            deadline_ms: 1001,
            ..t.clone()
        },
        RecoveryTiming {
            safety_margin_ms: 21,
            ..t.clone()
        },
        RecoveryTiming {
            clock_domain: "other".into(),
            ..t.clone()
        },
    ] {
        assert!(s
            .create_timed_recovery("operation", b, "actor", "scope", "branch", &changed)
            .is_err());
    }
    assert!(s
        .create_recovery("operation", b, "actor", "scope", "branch")
        .is_err());
    let legacy = s.create_recovery("legacy", b, "actor", "scope", "legacy")?;
    let a = begin(&mut s, legacy, &[])?;
    assert!(s
        .create_timed_recovery("legacy", b, "actor", "scope", "legacy", &t)
        .is_err());
    assert!(s.reserve_recovery_attempt(a, b"legacy", 1, 1)?);
    assert_eq!(s.claim_recovery_dispatch(a)?, Some(b"legacy".to_vec()));
    drop(s);
    let s = Store::open(&file.0)?;
    assert_eq!(s.recovery_timing(r)?, Some(t));
    assert_eq!(s.recovery_timing(legacy)?, None);
    Ok(())
}

#[test]
fn exact_quote_and_existing_hold_are_checked_without_double_counting() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (b, _, a, _, p) = setup(&mut s, 12)?;
    assert!(s.claim_recovery_dispatch(a).is_err());
    let result = s.claim_timed_recovery_dispatch(a, &p, || clock(200))?;
    assert!(
        matches!(result,TimedRecoveryClaim::Dispatched{request,observed_ms:200,estimated_finish_ms:320,..} if request==p.request)
    );
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || panic!("clock must not run twice"))?,
        TimedRecoveryClaim::NotReserved
    ));
    assert!(!s.cancel_recovery_attempt(a)?);
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 12,
            spent: 0,
            reserved: 12
        }
    );
    assert_eq!(counts(&file)?, (1, 0));
    Ok(())
}

#[test]
fn refusal_keeps_unsent_hold_and_records_reason_then_explicit_cancel_releases_it() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (b, _, a, _, p) = setup(&mut s, 20)?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(880))?,
        TimingRefusal::InsufficientTime,
    );
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 20,
            spent: 0,
            reserved: 12
        }
    );
    assert_eq!(counts(&file)?, (0, 0));
    let db = Connection::open(&file.0)?;
    let row: (String, i64, i64) = db.query_row(
        "SELECT reason,observed_now_ms,estimated_finish_ms FROM recovery_admission_decisions",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(row, ("insufficient_time".into(), 880, 1000));
    assert!(s.cancel_recovery_attempt(a)?);
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 20,
            spent: 0,
            reserved: 0
        }
    );
    assert_eq!(s.charged_usage()?, 0);
    Ok(())
}

#[test]
fn runtime_calibration_readiness_and_coverage_never_fall_back() -> Result<()> {
    let mut s = Store::in_memory()?;
    let (_, _, a, t, p) = setup(&mut s, 1000)?;
    let mut unknown = p.clone();
    unknown.runtime = None;
    refused(
        s.claim_timed_recovery_dispatch(a, &unknown, || clock(200))?,
        TimingRefusal::UnknownRuntime,
    );
    let mut cold = p.clone();
    cold.runtime.as_mut().unwrap().readiness = RecoveryReadiness::Cold;
    refused(
        s.claim_timed_recovery_dispatch(a, &cold, || clock(200))?,
        TimingRefusal::RuntimeMismatch,
    );
    let mut other = p.clone();
    other.runtime.as_mut().unwrap().cpu_fingerprint = "c".repeat(64);
    refused(
        s.claim_timed_recovery_dispatch(a, &other, || clock(200))?,
        TimingRefusal::RuntimeMismatch,
    );
    let mut unknown = p.clone();
    unknown.calibration_id = None;
    refused(
        s.claim_timed_recovery_dispatch(a, &unknown, || clock(200))?,
        TimingRefusal::UnknownCalibration,
    );
    let mut wrong = p.clone();
    wrong.calibration_id = Some("different".into());
    refused(
        s.claim_timed_recovery_dispatch(a, &wrong, || clock(200))?,
        TimingRefusal::CalibrationMismatch,
    );
    let mut wrong = p.clone();
    wrong.request = b"different bytes".to_vec();
    refused(
        s.claim_timed_recovery_dispatch(a, &wrong, || clock(200))?,
        TimingRefusal::QuoteMismatch,
    );
    let mut wrong = p.clone();
    wrong.input_tokens += 1;
    refused(
        s.claim_timed_recovery_dispatch(a, &wrong, || clock(200))?,
        TimingRefusal::QuoteMismatch,
    );
    let b = s.create_token_budget(1000)?;
    let r = s.create_timed_recovery("uncovered", b, "actor", "scope", "uncovered", &t)?;
    let uncovered = begin(&mut s, r, &[])?;
    let mut big = p.clone();
    big.input_tokens = 267;
    assert!(s.reserve_recovery_attempt(uncovered, &big.request, 267, 8)?);
    refused(
        s.claim_timed_recovery_dispatch(uncovered, &big, || clock(200))?,
        TimingRefusal::UncoveredQuote,
    );
    Ok(())
}

#[test]
fn cold_restart_clock_domain_and_same_domain_regression_fail_closed() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (b, _, a, _, p) = setup(&mut s, 20)?;
    let mut wrong = p.clone();
    wrong.runtime = None;
    refused(
        s.claim_timed_recovery_dispatch(a, &wrong, || clock(500))?,
        TimingRefusal::UnknownRuntime,
    );
    drop(s);
    let mut s = Store::open(&file.0)?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || {
            Ok(RecoveryClock {
                domain: "new-boot".into(),
                now_ms: 600,
            })
        })?,
        TimingRefusal::ClockDomainMismatch,
    );
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(499))?,
        TimingRefusal::ClockRegressed,
    );
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || Err(rusqlite::Error::InvalidQuery))?,
        TimingRefusal::ClockUnavailable,
    );
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 20,
            spent: 0,
            reserved: 12
        }
    );
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(600))?,
        TimedRecoveryClaim::Dispatched { .. }
    ));
    Ok(())
}

#[test]
fn callback_samples_after_queue_delay_while_writer_lock_is_held() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (_, _, a, _, p) = setup(&mut s, 20)?;
    let locker = Connection::open(&file.0)?;
    locker.execute_batch("BEGIN IMMEDIATE")?;
    let called = Arc::new(AtomicBool::new(false));
    let tick = Arc::new(AtomicI64::new(200));
    let started = Arc::new(Barrier::new(2));
    let c = called.clone();
    let time = tick.clone();
    let barrier = started.clone();
    let path = file.0.clone();
    let worker = std::thread::spawn(move || {
        barrier.wait();
        s.claim_timed_recovery_dispatch(a, &p, || {
            c.store(true, Ordering::SeqCst);
            let competing = Connection::open(path)?;
            competing.busy_timeout(std::time::Duration::ZERO)?;
            assert!(competing.execute_batch("BEGIN IMMEDIATE").is_err());
            clock(time.load(Ordering::SeqCst))
        })
    });
    started.wait();
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(!called.load(Ordering::SeqCst));
    tick.store(880, Ordering::SeqCst);
    locker.execute_batch("COMMIT")?;
    refused(worker.join().unwrap()?, TimingRefusal::InsufficientTime);
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(counts(&file)?, (0, 0));
    Ok(())
}

#[test]
fn bound_capture_is_complete_and_eligibility_is_rechecked_at_claim() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let t = timing();
    let p = prep(&t);
    let b = s.create_token_budget(30)?;
    let r = s.create_timed_recovery("evidence", b, "actor", "scope", "branch", &t)?;
    let e = s.observe("report", b"evidence")?;
    s.set_admitted("actor", "scope", e, true)?;
    let hidden = s.observe("hidden", b"private")?;
    assert!(s.begin_recovery_attempt(r, &[e, hidden]).is_err());
    assert!(s.begin_recovery_attempt(r, &[e, 999]).is_err());
    let a = begin(&mut s, r, &[e])?;
    assert!(s.reserve_recovery_attempt(a, &p.request, 4, 8)?);
    let mut editor = Store::open(&file.0)?;
    editor.set_admitted("actor", "scope", e, false)?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimingRefusal::StaleEvidence,
    );
    assert!(s.cancel_recovery_attempt(a)?);
    assert_eq!(counts(&file)?, (0, 0));
    Ok(())
}

#[test]
fn expiry_and_absolute_deadline_survive_retry_and_late_deliveries_charge_once() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (b, r, a, t, p) = setup(&mut s, 30)?;
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimedRecoveryClaim::Dispatched { .. }
    ));
    drop(s);
    let mut s = Store::open(&file.0)?;
    assert!(!s.cancel_recovery_attempt(a)?);
    assert_eq!(
        s.reject_invalid(a, b"bad", 7, "validator", "reason")?,
        Disposition::InvalidOutput
    );
    assert_eq!(
        s.reject_invalid(a, b"bad", 7, "validator", "reason")?,
        Disposition::InvalidOutput
    );
    let retry = begin(&mut s, r, &[])?;
    assert!(s.reserve_recovery_attempt(retry, &p.request, 4, 8)?);
    assert_eq!(s.recovery_timing(r)?, Some(t));
    refused(
        s.claim_timed_recovery_dispatch(retry, &p, || clock(1000))?,
        TimingRefusal::DeadlineExpired,
    );
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 30,
            spent: 7,
            reserved: 12
        }
    );
    assert!(s.cancel_recovery_attempt(retry)?);
    assert_eq!(s.charged_usage()?, 7);
    // A different already-sent operation can return after any wall deadline;
    // measured usage still settles once and never turns admission into timeliness.
    let t = timing();
    let r = s.create_timed_recovery("late-valid", b, "actor", "scope", "other", &t)?;
    let a = begin(&mut s, r, &[])?;
    assert!(s.reserve_recovery_attempt(a, &p.request, 4, 8)?);
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimedRecoveryClaim::Dispatched { .. }
    ));
    assert_eq!(s.settle(a, b"late correct", 8)?, Disposition::Accepted);
    assert_eq!(s.settle(a, b"late correct", 8)?, Disposition::Accepted);
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 30,
            spent: 15,
            reserved: 0
        }
    );
    Ok(())
}

#[test]
fn calibration_expiry_invalid_clock_and_integer_overflow_never_send() -> Result<()> {
    let mut s = Store::in_memory()?;
    let mut t = timing();
    t.calibration.expires_at_ms = 500;
    let p = prep(&t);
    let b = s.create_token_budget(100)?;
    let r = s.create_timed_recovery("expiry", b, "a", "s", "b", &t)?;
    let a = begin(&mut s, r, &[])?;
    s.reserve_recovery_attempt(a, &p.request, 4, 8)?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(500))?,
        TimingRefusal::CalibrationExpired,
    );
    let mut overflow = timing();
    overflow.start_ms = i64::MAX - 200;
    overflow.deadline_ms = i64::MAX;
    overflow.calibration.measured_at_ms = i64::MAX - 300;
    overflow.calibration.expires_at_ms = i64::MAX;
    let r = s.create_timed_recovery("overflow", b, "a", "s", "c", &overflow)?;
    let a = begin(&mut s, r, &[])?;
    s.reserve_recovery_attempt(a, &p.request, 4, 8)?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(i64::MAX - 50))?,
        TimingRefusal::InvalidTiming,
    );
    Ok(())
}

#[test]
fn concurrent_claims_and_cancel_serialize_without_double_permission_or_hold_release() -> Result<()>
{
    for cancel in [false, true] {
        let file = Database::new();
        let mut s = Store::open(&file.0)?;
        let (b, _, a, _, p) = setup(&mut s, 20)?;
        let barrier = Arc::new(Barrier::new(2));
        let mut first = Store::open(&file.0)?;
        let mut second = Store::open(&file.0)?;
        let start = barrier.clone();
        let q = p.clone();
        let sender = std::thread::spawn(move || {
            start.wait();
            first.claim_timed_recovery_dispatch(a, &q, || clock(200))
        });
        let other = std::thread::spawn(move || {
            barrier.wait();
            if cancel {
                second.cancel_recovery_attempt(a).map(|c| (c, None))
            } else {
                second
                    .claim_timed_recovery_dispatch(a, &p, || clock(200))
                    .map(|r| (false, Some(r)))
            }
        });
        let one = sender.join().unwrap()?;
        let (cancelled, two) = other.join().unwrap()?;
        let grants = usize::from(matches!(one, TimedRecoveryClaim::Dispatched { .. }))
            + usize::from(matches!(two, Some(TimedRecoveryClaim::Dispatched { .. })));
        assert_eq!(grants, if cancelled { 0 } else { 1 });
        assert_eq!(
            s.token_budget_state(b)?,
            TokenBudgetState {
                limit: 20,
                spent: 0,
                reserved: if cancelled { 0 } else { 12 }
            }
        );
        assert_eq!(counts(&file)?, (grants as i64, 0));
    }
    Ok(())
}

#[test]
fn invalid_configuration_and_every_changed_binding_refuse_without_mutation() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let b = s.create_token_budget(100)?;
    let original = timing();
    let mut invalids = Vec::new();
    let mut t = original.clone();
    t.clock_domain = "unknown".into();
    invalids.push(t);
    let mut t = original.clone();
    t.runtime.cpu_fingerprint = "not-a-fingerprint".into();
    invalids.push(t);
    let mut t = original.clone();
    t.runtime.threads = 0;
    invalids.push(t);
    let mut t = original.clone();
    t.runtime.dtype = "".into();
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.id = " ".into();
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.sample_count = 0;
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.max_duration_ms = 0;
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.max_input_tokens = -1;
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.output_cap = 0;
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.measured_at_ms = 101;
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.expires_at_ms = 100;
    invalids.push(t);
    let mut t = original.clone();
    t.safety_margin_ms = -1;
    invalids.push(t);
    let mut t = original.clone();
    t.start_ms = -1;
    invalids.push(t);
    let mut t = original.clone();
    t.deadline_ms = 100;
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.max_duration_ms = i64::MAX;
    invalids.push(t);
    let mut t = original.clone();
    t.clock_domain = "none".into();
    invalids.push(t);
    let mut t = original.clone();
    t.runtime.model = "none".into();
    invalids.push(t);
    let mut t = original.clone();
    t.calibration.id = "none".into();
    invalids.push(t);
    for t in invalids {
        assert!(s
            .create_timed_recovery("invalid", b, "a", "s", "b", &t)
            .is_err());
    }
    let db = Connection::open(&file.0)?;
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM recovery_runs", [], |r| r
            .get::<_, i64>(0))?,
        0
    );
    let r = s.create_timed_recovery("bound", b, "a", "s", "b", &original)?;
    let mut changes = Vec::new();
    let mut t = original.clone();
    t.start_ms = 101;
    changes.push(t);
    let mut t = original.clone();
    t.runtime.readiness = RecoveryReadiness::Cold;
    changes.push(t);
    let mut t = original.clone();
    t.runtime.dtype = "float32".into();
    changes.push(t);
    let mut t = original.clone();
    t.runtime.prompt_fingerprint = "c".repeat(64);
    changes.push(t);
    let mut t = original.clone();
    t.calibration.id = "other-calibration".into();
    changes.push(t);
    let mut t = original.clone();
    t.calibration.max_duration_ms = 101;
    changes.push(t);
    let mut t = original.clone();
    t.calibration.sample_count = 3;
    changes.push(t);
    let mut t = original.clone();
    t.calibration.max_input_tokens = 180;
    changes.push(t);
    let mut t = original.clone();
    t.calibration.output_cap = 7;
    changes.push(t);
    let mut t = original.clone();
    t.calibration.measured_at_ms = 49;
    changes.push(t);
    let mut t = original.clone();
    t.calibration.expires_at_ms = 2001;
    changes.push(t);
    for t in changes {
        assert!(s
            .create_timed_recovery("bound", b, "a", "s", "b", &t)
            .is_err());
    }
    for (actor, scope, branch) in [
        ("other", "s", "b"),
        ("a", "other", "b"),
        ("a", "s", "other"),
    ] {
        assert!(s
            .create_timed_recovery("bound", b, actor, scope, branch, &original)
            .is_err());
    }
    assert_eq!(s.recovery_timing(r)?, Some(original));
    Ok(())
}

#[test]
fn calibration_validity_is_at_admission_and_quote_cap_coverage_is_exact() -> Result<()> {
    let mut s = Store::in_memory()?;
    let mut t = timing();
    t.calibration.expires_at_ms = 500;
    let p = prep(&t);
    let b = s.create_token_budget(100)?;
    let r = s.create_timed_recovery("cross-ttl", b, "a", "s", "b", &t)?;
    let a = begin(&mut s, r, &[])?;
    s.reserve_recovery_attempt(a, &p.request, 4, 8)?;
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(450))?,
        TimedRecoveryClaim::Dispatched {
            estimated_finish_ms: 570,
            ..
        }
    ));
    let r = s.create_timed_recovery("different-cap", b, "a", "s", "c", &t)?;
    let a = begin(&mut s, r, &[])?;
    s.reserve_recovery_attempt(a, &p.request, 4, 7)?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimingRefusal::QuoteMismatch,
    );
    let mut p = p;
    p.output_cap = 7;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimingRefusal::UncoveredQuote,
    );
    Ok(())
}

#[test]
fn unrelated_uncertain_holds_and_spend_are_counted_but_not_released() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let t = timing();
    let p = prep(&t);
    let b = s.create_token_budget(30)?;
    let prior = s.begin_attempt("a", "s", "prior", &[])?;
    s.reserve_dispatch(b, prior, b"prior", 2, 6)?;
    s.settle(prior, b"prior result", 6)?;
    let unknown = s.begin_attempt("a", "s", "unknown", &[])?;
    s.reserve_dispatch(b, unknown, b"potentially sent", 4, 8)?;
    let r = s.create_timed_recovery("bound", b, "a", "s", "b", &t)?;
    let a = begin(&mut s, r, &[])?;
    assert!(s.reserve_recovery_attempt(a, &p.request, 4, 8)?);
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 30,
            spent: 6,
            reserved: 24
        }
    );
    // No double counting: exactly exhausted shared budget still permits this
    // already-reserved quote, without adopting the other unknown dispatch.
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimedRecoveryClaim::Dispatched { .. }
    ));
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 30,
            spent: 6,
            reserved: 24
        }
    );
    assert!(!s.cancel_recovery_attempt(a)?);
    assert_eq!(s.settle(a, b"late", 8)?, Disposition::Accepted);
    assert_eq!(
        s.token_budget_state(b)?,
        TokenBudgetState {
            limit: 30,
            spent: 14,
            reserved: 12
        }
    );
    Ok(())
}

#[test]
fn invalid_shared_balance_is_refused_and_sql_cannot_bypass_the_timed_fence() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (b, r, a, _, p) = setup(&mut s, 20)?;
    let db = Connection::open(&file.0)?;
    assert!(db
        .execute(
            "UPDATE recovery_attempts SET state='sent' WHERE attempt_id=?1",
            [a]
        )
        .is_err());
    assert!(db
        .execute(
            "INSERT INTO dispatches(attempt_id,request) VALUES(?1,?2)",
            rusqlite::params![a, p.request]
        )
        .is_err());
    assert!(db
        .execute(
            "UPDATE recovery_timing_bindings SET deadline_ms=100000 WHERE recovery_id=?1",
            [r]
        )
        .is_err());
    assert!(db
        .execute(
            "DELETE FROM recovery_timing_bindings WHERE recovery_id=?1",
            [r]
        )
        .is_err());
    db.execute("UPDATE token_budgets SET token_limit=11 WHERE id=?1", [b])?;
    refused(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimingRefusal::InvalidBudget,
    );
    assert_eq!(counts(&file)?, (0, 0));
    assert!(s.cancel_recovery_attempt(a)?);
    Ok(())
}

#[test]
fn branch_changes_and_revoke_regrant_are_stale_even_with_same_payloads() -> Result<()> {
    for branch_changed in [false, true] {
        let file = Database::new();
        let mut s = Store::open(&file.0)?;
        let e = s.observe("report", b"same")?;
        s.set_admitted("actor", "scope", e, true)?;
        let (b, r, unused, t, p) = setup(&mut s, 50)?;
        assert!(s.cancel_recovery_attempt(unused)?);
        let r = s.create_timed_recovery(&format!("next-{r}"), b, "actor", "scope", "branch", &t)?;
        let a = begin(&mut s, r, &[e])?;
        s.reserve_recovery_attempt(a, &p.request, 4, 8)?;
        if branch_changed {
            let winner = s.begin_attempt("actor", "scope", "branch", &[e])?;
            s.settle(winner, b"winner", 0)?;
        } else {
            s.set_admitted("actor", "scope", e, false)?;
            s.set_admitted("actor", "scope", e, true)?;
        }
        refused(
            s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
            TimingRefusal::StaleEvidence,
        );
        assert!(s.cancel_recovery_attempt(a)?);
    }
    Ok(())
}

#[test]
fn failed_audit_state_or_dispatch_rolls_back_grant_clock_floor_and_hold_atomically() -> Result<()> {
    for target in ["audit", "state", "dispatch"] {
        let file = Database::new();
        let mut s = Store::open(&file.0)?;
        let (b, _, a, _, p) = setup(&mut s, 20)?;
        let db = Connection::open(&file.0)?;
        let event = match target {
            "audit" => "BEFORE INSERT ON recovery_admission_decisions",
            "state" => "BEFORE UPDATE OF state ON recovery_attempts",
            _ => "BEFORE INSERT ON dispatches",
        };
        db.execute_batch(&format!("CREATE TRIGGER injected {event} BEGIN SELECT RAISE(ABORT,'injected timed failure'); END;"))?;
        assert!(s
            .claim_timed_recovery_dispatch(a, &p, || clock(500))
            .is_err());
        assert_eq!(counts(&file)?, (0, 0));
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM recovery_admission_decisions",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT last_observed_ms FROM recovery_timing_bindings",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            100
        );
        assert_eq!(
            s.token_budget_state(b)?,
            TokenBudgetState {
                limit: 20,
                spent: 0,
                reserved: 12
            }
        );
        db.execute_batch("DROP TRIGGER injected")?;
        drop(s);
        let mut s = Store::open(&file.0)?;
        assert!(matches!(
            s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
            TimedRecoveryClaim::Dispatched { .. }
        ));
        assert!(db
            .execute(
                "UPDATE recovery_admission_decisions SET reason='forged'",
                []
            )
            .is_err());
        assert!(db
            .execute("DELETE FROM recovery_admission_decisions", [])
            .is_err());
    }
    Ok(())
}

#[test]
fn panicking_clock_rolls_back_and_same_store_remains_usable() -> Result<()> {
    let file = Database::new();
    let mut s = Store::open(&file.0)?;
    let (_, _, a, _, p) = setup(&mut s, 20)?;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.claim_timed_recovery_dispatch(a, &p, || panic!("clock failure"))
    }));
    assert!(outcome.is_err());
    assert_eq!(counts(&file)?, (0, 0));
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimedRecoveryClaim::Dispatched { .. }
    ));
    Ok(())
}

#[test]
fn schema8_migration_keeps_legacy_sent_denied_cancelled_and_deterministic_records() -> Result<()> {
    let file = Database::new();
    let db = Connection::open(&file.0)?;
    for script in [
        include_str!("../src/schema.sql"),
        include_str!("../src/attempts.sql"),
        include_str!("../src/support.sql"),
        include_str!("../src/dispatch.sql"),
        include_str!("../src/model_reuse.sql"),
        include_str!("../src/validation.sql"),
        include_str!("../src/token_budget.sql"),
        include_str!("../src/recovery.sql"),
        include_str!("../src/report_verdict.sql"),
    ] {
        db.execute_batch(script)?;
    }
    db.pragma_update(None, "application_id", 0x4b54_534d_i32)?;
    db.pragma_update(None, "user_version", 8)?;
    db.execute_batch("INSERT INTO token_budgets VALUES(1,100);
        INSERT INTO branches(id,actor,scope,name) VALUES(1,'a','s','legacy'),(2,'a','s','deterministic');
        INSERT INTO attempts(id,branch_id,branch_revision,view_epoch) VALUES(1,1,0,0),(2,1,0,0),(3,1,0,0),(4,1,0,0),(5,2,0,0);
        INSERT INTO recovery_runs VALUES(1,'sent',1,'a','s','legacy'),(2,'denied',1,'a','s','legacy'),(3,'cancelled',1,'a','s','legacy');
        INSERT INTO recovery_attempts VALUES(1,0,2,'sent',X'02'),(2,0,3,'denied',X'03'),(3,0,4,'cancelled',X'04');
        INSERT INTO dispatches VALUES(1,X'01'),(2,X'02');
        INSERT INTO token_reservations VALUES(1,1,4,8,1),(2,1,4,8,1),(3,1,4,8,0),(4,1,4,8,1);
        INSERT INTO deterministic_verdicts VALUES(5,'report-verdict/v1',X'7B7D');
        INSERT INTO deliveries(attempt_id,disposition,output,usage) VALUES(5,'accepted',X'7B7D',0);
        UPDATE branches SET head=5,revision=1 WHERE id=2;")?;
    drop(db);
    let mut s = Store::open(&file.0)?;
    assert_eq!(
        s.token_budget_state(1)?,
        TokenBudgetState {
            limit: 100,
            spent: 0,
            reserved: 24
        }
    );
    assert_eq!(s.claim_recovery_dispatch(2)?, None);
    assert!(!s.cancel_recovery_attempt(2)?);
    assert_eq!(s.claim_recovery_dispatch(3)?, None);
    assert!(!s.cancel_recovery_attempt(3)?);
    assert_eq!(s.claim_recovery_dispatch(4)?, None);
    assert!(s.cancel_recovery_attempt(4)?);
    assert_eq!(s.commit_report_verdict(5)?, Disposition::Accepted);
    assert_eq!(s.recovery_timing(1)?, None);
    let t = timing();
    let p = prep(&t);
    let r = s.create_timed_recovery("new", 1, "a", "s", "new", &t)?;
    let a = begin(&mut s, r, &[])?;
    s.reserve_recovery_attempt(a, &p.request, 4, 8)?;
    assert!(matches!(
        s.claim_timed_recovery_dispatch(a, &p, || clock(200))?,
        TimedRecoveryClaim::Dispatched { .. }
    ));
    assert_eq!(
        s.token_budget_state(1)?,
        TokenBudgetState {
            limit: 100,
            spent: 0,
            reserved: 36
        }
    );
    assert_eq!(s.settle(1, b"late", 6)?, Disposition::Accepted);
    assert_eq!(s.settle(1, b"late", 6)?, Disposition::Accepted);
    assert_eq!(
        s.token_budget_state(1)?,
        TokenBudgetState {
            limit: 100,
            spent: 6,
            reserved: 24
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
