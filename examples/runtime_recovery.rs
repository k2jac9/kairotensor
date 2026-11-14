//! Offline admission mechanics with explicitly synthetic timing/profile claims.
//! No model runs; this is neither a measured calibration nor an efficacy result.
use kairotensor::{
    Disposition, LinuxBootClock, MeasuredRecoveryReply, RecoveryCalibration, RecoveryClock,
    RecoveryPreparation, RecoveryReadiness, RecoveryRun, RecoveryRuntime, RecoveryStart,
    RecoveryTiming, ReplyValidation, Store, TimingRefusal,
};
use serde_json::json;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = Store::in_memory()?;
    let budget = store.create_token_budget(30)?;
    let timing = RecoveryTiming {
        clock_domain: "synthetic-session/1".into(),
        start_ms: 100,
        deadline_ms: 1000,
        safety_margin_ms: 20,
        runtime: RecoveryRuntime {
            model: "fixture-only".into(),
            model_revision: "fixture/1".into(),
            tokenizer_revision: "fixture/1".into(),
            runtime_version: "no-inference".into(),
            dtype: "fixture".into(),
            decoding: "fixture".into(),
            cpu_fingerprint: "a".repeat(64),
            threads: 1,
            prompt_fingerprint: "b".repeat(64),
            readiness: RecoveryReadiness::Warm,
        },
        calibration: RecoveryCalibration {
            id: "synthetic-not-measured/1".into(),
            max_duration_ms: 100,
            sample_count: 2,
            max_input_tokens: 179,
            output_cap: 8,
            measured_at_ms: 0,
            expires_at_ms: 2000,
        },
    };
    let recovery =
        store.create_timed_recovery("demo", budget, "local", "demo", "verdict", &timing)?;
    let RecoveryStart::New { attempt, .. } = store.begin_recovery_attempt(recovery, &[])? else {
        return Err("expected new attempt".into());
    };
    let prepared = RecoveryPreparation {
        runtime: Some(timing.runtime.clone()),
        calibration_id: Some(timing.calibration.id.clone()),
        request: b"synthetic request".to_vec(),
        input_tokens: 4,
        output_cap: 8,
    };
    assert!(store.reserve_recovery_attempt(attempt, &prepared.request, 4, 8)?);
    let mut backend_calls = 0;
    let first = store.run_timed_recovery(
        attempt,
        &prepared,
        || {
            Ok(RecoveryClock {
                domain: timing.clock_domain.clone(),
                now_ms: 200,
            })
        },
        |request| {
            assert_eq!(request, prepared.request);
            backend_calls += 1;
            // Explicit synthetic measured-response analogue, no transport/model.
            Ok::<_, String>(MeasuredRecoveryReply {
                output: b"fixture invalid".to_vec(),
                input_tokens: 4,
                output_tokens: 3,
                validation: ReplyValidation::Invalid {
                    validator: "fixture/1".into(),
                    reason: "fixture rejection".into(),
                },
            })
        },
    )?;
    assert!(matches!(
        first,
        RecoveryRun::Settled {
            disposition: Disposition::InvalidOutput,
            ..
        }
    ));
    let RecoveryStart::New { attempt: retry, .. } = store.begin_recovery_attempt(recovery, &[])?
    else {
        return Err("expected retry".into());
    };
    assert!(store.reserve_recovery_attempt(retry, &prepared.request, 4, 8)?);
    let mut samples = 0;
    let skipped = store.run_timed_recovery(
        retry,
        &prepared,
        || {
            samples += 1;
            Ok(RecoveryClock {
                domain: timing.clock_domain.clone(),
                now_ms: if samples == 1 { 300 } else { 900 },
            })
        },
        |_| -> Result<MeasuredRecoveryReply, String> { panic!("must not call after commit delay") },
    )?;
    assert!(matches!(
        skipped,
        RecoveryRun::ClaimedButNotInvoked {
            reason: TimingRefusal::InsufficientTime,
            ..
        }
    ));
    assert!(!store.cancel_recovery_attempt(retry)?);
    let ledger = store.token_budget_state(budget)?;
    assert_eq!(ledger.spent, 7);
    assert_eq!(ledger.reserved, 12);
    println!(
        "{}",
        json!({"mode":"synthetic_runtime_boundary_fixture","new_model_calls":0,
        "fixture_backend_invocations":backend_calls,"synthetic_charged_tokens":ledger.spent,
        "postcommit_skip":"insufficient_time","claimed_hold_retained":ledger.reserved,
        "linux_boottime_available":LinuxBootClock::sample().is_ok(),
        "completion_guaranteed":false,"exactly_once_remote_transport_guaranteed":false})
    );
    Ok(())
}
