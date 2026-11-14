use kairotensor::{
    Disposition, MeasuredRecoveryReply, RecoveryCalibration, RecoveryClock, RecoveryPreparation,
    RecoveryReadiness, RecoveryRun, RecoveryRuntime, RecoveryStart, RecoveryTiming,
    ReplyValidation, Store, TimingRefusal,
};
use rusqlite::Result;
use std::cell::Cell;

fn setup() -> Result<(Store, i64, i64, RecoveryPreparation)> {
    setup_store(Store::in_memory()?)
}
fn setup_store(s: Store) -> Result<(Store, i64, i64, RecoveryPreparation)> {
    setup_window(s, 1000, 2000)
}
fn setup_window(
    mut s: Store,
    deadline: i64,
    expires: i64,
) -> Result<(Store, i64, i64, RecoveryPreparation)> {
    let budget = s.create_token_budget(100)?;
    let runtime = RecoveryRuntime {
        model: "fixture".into(),
        model_revision: "1".into(),
        tokenizer_revision: "1".into(),
        runtime_version: "1".into(),
        dtype: "f32".into(),
        decoding: "greedy".into(),
        cpu_fingerprint: "a".repeat(64),
        prompt_fingerprint: "b".repeat(64),
        threads: 1,
        readiness: RecoveryReadiness::Warm,
    };
    let timing = RecoveryTiming {
        clock_domain: "fixture-boot".into(),
        start_ms: 100,
        deadline_ms: deadline,
        safety_margin_ms: 20,
        runtime: runtime.clone(),
        calibration: RecoveryCalibration {
            id: "fixture-calibration".into(),
            max_duration_ms: 100,
            sample_count: 2,
            max_input_tokens: 4,
            output_cap: 8,
            measured_at_ms: 0,
            expires_at_ms: expires,
        },
    };
    let run = s.create_timed_recovery("run", budget, "actor", "scope", "branch", &timing)?;
    let attempt = match s.begin_recovery_attempt(run, &[])? {
        RecoveryStart::New { attempt, .. } => attempt,
        _ => panic!(),
    };
    let p = RecoveryPreparation {
        runtime: Some(runtime),
        calibration_id: Some("fixture-calibration".into()),
        request: b"exact request".to_vec(),
        input_tokens: 4,
        output_cap: 8,
    };
    assert!(s.reserve_recovery_attempt(attempt, &p.request, 4, 8)?);
    Ok((s, budget, attempt, p))
}
fn tick(now_ms: i64) -> Result<RecoveryClock> {
    Ok(RecoveryClock {
        domain: "fixture-boot".into(),
        now_ms,
    })
}
fn reply() -> MeasuredRecoveryReply {
    MeasuredRecoveryReply {
        output: b"answer".to_vec(),
        input_tokens: 4,
        output_tokens: 3,
        validation: ReplyValidation::Valid,
    }
}
#[test]
fn one_claim_one_backend_measured_settlement() -> Result<()> {
    let (mut s, b, a, p) = setup()?;
    let calls = Cell::new(0);
    let result = s.run_timed_recovery(
        a,
        &p,
        || tick(200),
        |request| {
            assert_eq!(request, b"exact request");
            calls.set(calls.get() + 1);
            Ok::<_, String>(reply())
        },
    )?;
    assert!(matches!(
        result,
        RecoveryRun::Settled {
            disposition: Disposition::Accepted,
            ..
        }
    ));
    assert_eq!(calls.get(), 1);
    assert_eq!(s.token_budget_state(b)?.spent, 7);
    assert!(matches!(
        s.run_timed_recovery(
            a,
            &p,
            || panic!("no second clock"),
            |_| panic!("no second backend")
        )?,
        RecoveryRun::<String>::NotReserved
    ));
    assert_eq!(s.charged_usage()?, 7);
    Ok(())
}
#[test]
fn final_expiry_keeps_claimed_hold_and_never_invokes() -> Result<()> {
    let (mut s, b, a, p) = setup()?;
    let count = Cell::new(0);
    let result = s.run_timed_recovery(
        a,
        &p,
        || {
            count.set(count.get() + 1);
            tick(if count.get() == 1 { 200 } else { 1000 })
        },
        |_| panic!("expired backend"),
    )?;
    assert!(matches!(
        result,
        RecoveryRun::<String>::ClaimedButNotInvoked {
            reason: TimingRefusal::DeadlineExpired,
            ..
        }
    ));
    assert!(!s.cancel_recovery_attempt(a)?);
    let ledger = s.token_budget_state(b)?;
    assert_eq!(ledger.reserved, 12);
    assert_eq!(ledger.spent, 0);
    Ok(())
}
#[test]
fn backend_failure_is_uncertain_and_cannot_resend() -> Result<()> {
    let (mut s, b, a, p) = setup()?;
    assert!(matches!(
        s.run_timed_recovery(
            a,
            &p,
            || tick(200),
            |_| Err::<MeasuredRecoveryReply, _>("transport unknown")
        )?,
        RecoveryRun::BackendFailed {
            error: "transport unknown",
            ..
        }
    ));
    assert_eq!(s.token_budget_state(b)?.reserved, 12);
    assert_eq!(s.charged_usage()?, 0);
    assert!(matches!(
        s.run_timed_recovery(a, &p, || panic!(), |_| panic!())?,
        RecoveryRun::<String>::NotReserved
    ));
    Ok(())
}
#[test]
fn invalid_reply_charges_and_preserves_validator() -> Result<()> {
    let (mut s, b, a, p) = setup()?;
    let mut r = reply();
    r.validation = ReplyValidation::Invalid {
        validator: "fixture/1".into(),
        reason: "bad output".into(),
    };
    assert!(matches!(
        s.run_timed_recovery(a, &p, || tick(200), |_| Ok::<_, String>(r))?,
        RecoveryRun::Settled {
            disposition: Disposition::InvalidOutput,
            ..
        }
    ));
    assert_eq!(s.token_budget_state(b)?.spent, 7);
    assert_eq!(s.token_budget_state(b)?.reserved, 0);
    assert_eq!(
        s.reject_invalid(a, b"answer", 7, "fixture/1", "bad output")?,
        Disposition::InvalidOutput
    );
    assert_eq!(s.charged_usage()?, 7);
    Ok(())
}
#[test]
fn invalid_accounting_returns_measurement_without_releasing_hold() -> Result<()> {
    for (input, output) in [(-1, 3), (4, -1), (4, 9), (5, 3), (i64::MAX, 1)] {
        let (mut s, b, a, p) = setup()?;
        let mut r = reply();
        r.input_tokens = input;
        r.output_tokens = output;
        let result = s.run_timed_recovery(a, &p, || tick(200), |_| Ok::<_, String>(r.clone()))?;
        match result {
            RecoveryRun::SettlementFailed { reply, .. } => assert_eq!(reply, r),
            _ => panic!("must preserve rejected measurement"),
        };
        assert_eq!(s.token_budget_state(b)?.reserved, 12);
        assert_eq!(s.charged_usage()?, 0);
    }
    Ok(())
}
#[test]
fn final_clock_uncertainty_never_sends_or_releases() -> Result<()> {
    for expected in [
        TimingRefusal::ClockUnavailable,
        TimingRefusal::ClockDomainMismatch,
        TimingRefusal::ClockRegressed,
    ] {
        let (mut s, b, a, p) = setup()?;
        let n = Cell::new(0);
        let result = s.run_timed_recovery(
            a,
            &p,
            || {
                n.set(n.get() + 1);
                if n.get() == 1 {
                    return tick(200);
                }
                match expected {
                    TimingRefusal::ClockUnavailable => Err(rusqlite::Error::InvalidQuery),
                    TimingRefusal::ClockDomainMismatch => Ok(RecoveryClock {
                        domain: "different-boot".into(),
                        now_ms: 201,
                    }),
                    _ => tick(199),
                }
            },
            |_| panic!("must not send"),
        )?;
        assert!(
            matches!(result,RecoveryRun::<String>::ClaimedButNotInvoked{reason,..} if reason==expected)
        );
        assert_eq!(s.token_budget_state(b)?.reserved, 12);
        assert_eq!(s.charged_usage()?, 0);
    }
    Ok(())
}
#[test]
fn panic_after_claim_retains_hold() -> Result<()> {
    for panic_clock in [false, true] {
        let (mut s, b, a, p) = setup()?;
        let n = Cell::new(0);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            s.run_timed_recovery(
                a,
                &p,
                || {
                    n.set(n.get() + 1);
                    if panic_clock && n.get() == 2 {
                        panic!("clock panic")
                    };
                    tick(200)
                },
                |_| -> std::result::Result<MeasuredRecoveryReply, String> {
                    panic!("backend panic")
                },
            )
        }));
        assert!(caught.is_err());
        assert_eq!(s.token_budget_state(b)?.reserved, 12);
        assert!(!s.cancel_recovery_attempt(a)?);
        assert_eq!(s.charged_usage()?, 0);
    }
    Ok(())
}
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
                "recovery-runtime-{}-{}-{}.sqlite",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[test]
fn settlement_failure_preserves_reply_for_late_reconciliation_after_restart() -> Result<()> {
    let db = Db::new();
    let (mut s, b, a, p) = setup_store(Store::open(&db.0)?)?;
    let sql = rusqlite::Connection::open(&db.0)?;
    sql.execute_batch("CREATE TRIGGER fail_delivery BEFORE INSERT ON deliveries BEGIN SELECT RAISE(ABORT,'injected failure'); END;")?;
    let result = s.run_timed_recovery(a, &p, || tick(200), |_| Ok::<_, String>(reply()))?;
    let measured = match result {
        RecoveryRun::SettlementFailed { reply, .. } => reply,
        _ => panic!("expected rollback"),
    };
    assert_eq!(s.token_budget_state(b)?.reserved, 12);
    assert_eq!(s.charged_usage()?, 0);
    drop(s);
    sql.execute_batch("DROP TRIGGER fail_delivery;")?;
    let mut s = Store::open(&db.0)?;
    // A real clock may now be long past the deadline; measured receipt still charges.
    let late = tick(5000)?;
    assert!(late.now_ms > 1000);
    assert_eq!(s.settle(a, &measured.output, 7)?, Disposition::Accepted);
    assert_eq!(s.settle(a, &measured.output, 7)?, Disposition::Accepted);
    assert_eq!(s.token_budget_state(b)?.spent, 7);
    assert_eq!(s.token_budget_state(b)?.reserved, 0);
    assert!(matches!(
        s.run_timed_recovery(a, &p, || panic!(), |_| panic!())?,
        RecoveryRun::<String>::NotReserved
    ));
    Ok(())
}
#[test]
fn competing_workers_invoke_only_one_backend() -> Result<()> {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    let db = Db::new();
    let (s, b, a, p) = setup_store(Store::open(&db.0)?)?;
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let calls = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let path = db.0.clone();
            let p = p.clone();
            let gate = gate.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                let mut s = Store::open(path).unwrap();
                gate.wait();
                s.run_timed_recovery(
                    a,
                    &p,
                    || tick(200),
                    |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, String>(reply())
                    },
                )
                .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, RecoveryRun::Settled { .. }))
            .count(),
        1
    );
    assert_eq!(Store::open(&db.0)?.token_budget_state(b)?.spent, 7);
    Ok(())
}
#[test]
fn linux_clock_child() {
    let Some(path) = std::env::var_os("KAIROTENSOR_CLOCK_SMOKE_PATH") else {
        return;
    };
    let value = match kairotensor::LinuxBootClock::sample() {
        Ok(c) => serde_json::json!({"domain":c.domain,"now":c.now_ms}),
        Err(e) => serde_json::json!({"unavailable":e.to_string()}),
    };
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}
#[test]
fn linux_clock_process_restart_smoke() {
    let first = match kairotensor::LinuxBootClock::sample() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Linux boot clock unavailable (fail closed): {e}");
            return;
        }
    };
    let path = Db::new();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "linux_clock_child"])
        .env("KAIROTENSOR_CLOCK_SMOKE_PATH", &path.0)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path.0).unwrap()).unwrap();
    assert!(
        value.get("unavailable").is_none(),
        "child clock became unavailable"
    );
    // Do not print host/boot identifiers even when this equality fails.
    assert!(
        value["domain"].as_str() == Some(first.domain.as_str()),
        "restart clock domain differs"
    );
    assert!(value["now"].as_i64().unwrap() >= first.now_ms);
}
#[test]
fn postcommit_delay_must_still_fit_calibrated_allowance() -> Result<()> {
    let (mut s, b, a, p) = setup()?;
    let n = Cell::new(0);
    let result = s.run_timed_recovery(
        a,
        &p,
        || {
            n.set(n.get() + 1);
            tick(if n.get() == 1 { 200 } else { 880 })
        },
        |_| panic!("insufficient remaining time"),
    )?;
    assert!(matches!(
        result,
        RecoveryRun::<String>::ClaimedButNotInvoked {
            reason: TimingRefusal::InsufficientTime,
            ..
        }
    ));
    assert_eq!(s.token_budget_state(b)?.reserved, 12);
    Ok(())
}
#[test]
fn final_calibration_expiry_preserves_claimed_hold() -> Result<()> {
    let (mut s, b, a, p) = setup_window(Store::in_memory()?, 1000, 250)?;
    let n = Cell::new(0);
    let r = s.run_timed_recovery(
        a,
        &p,
        || {
            n.set(n.get() + 1);
            tick(if n.get() == 1 { 200 } else { 250 })
        },
        |_| panic!("expired calibration"),
    )?;
    assert!(matches!(
        r,
        RecoveryRun::<String>::ClaimedButNotInvoked {
            reason: TimingRefusal::CalibrationExpired,
            ..
        }
    ));
    assert_eq!(s.token_budget_state(b)?.reserved, 12);
    Ok(())
}
#[test]
fn concrete_linux_wrapper_never_falls_back_to_fixture_domain() -> Result<()> {
    let (mut s, b, a, p) = setup()?;
    let r = s.run_timed_recovery_linux(a, &p, |_| panic!("fixture domain cannot match OS"))?;
    assert!(matches!(
        r,
        RecoveryRun::<String>::Refused {
            reason: TimingRefusal::ClockUnavailable | TimingRefusal::ClockDomainMismatch,
            ..
        }
    ));
    assert_eq!(s.token_budget_state(b)?.reserved, 12);
    assert!(s.cancel_recovery_attempt(a)?);
    Ok(())
}
