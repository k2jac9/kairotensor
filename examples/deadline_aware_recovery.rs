//! Offline admission mechanics with explicitly synthetic timing/profile claims.
//! No model runs; this is neither a measured calibration nor an efficacy result.
use kairotensor::{
    Disposition, RecoveryCalibration, RecoveryClock, RecoveryPreparation, RecoveryReadiness,
    RecoveryRuntime, RecoveryStart, RecoveryTiming, Store, TimedRecoveryClaim, TimingRefusal,
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
    assert!(store.claim_recovery_dispatch(attempt).is_err());
    let claim = store.claim_timed_recovery_dispatch(attempt, &prepared, || {
        Ok(RecoveryClock {
            domain: timing.clock_domain.clone(),
            now_ms: 200,
        })
    })?;
    let TimedRecoveryClaim::Dispatched {
        decision_id,
        observed_ms,
        estimated_finish_ms,
        ..
    } = claim
    else {
        return Err("expected checked claim".into());
    };
    // No transport is called. This explicit fixture supplies a measured-result
    // analogue to exercise settlement; it is not a real model usage measurement.
    assert_eq!(
        store.reject_invalid(
            attempt,
            b"fixture invalid",
            7,
            "fixture-validator/1",
            "fixture reason"
        )?,
        Disposition::InvalidOutput
    );
    let RecoveryStart::New { attempt: retry, .. } = store.begin_recovery_attempt(recovery, &[])?
    else {
        return Err("expected retry".into());
    };
    assert!(store.reserve_recovery_attempt(retry, &prepared.request, 4, 8)?);
    let refused = store.claim_timed_recovery_dispatch(retry, &prepared, || {
        Ok(RecoveryClock {
            domain: timing.clock_domain.clone(),
            now_ms: 880,
        })
    })?;
    assert!(matches!(
        refused,
        TimedRecoveryClaim::Refused {
            reason: TimingRefusal::InsufficientTime,
            ..
        }
    ));
    let before_cancel = store.token_budget_state(budget)?;
    assert_eq!(before_cancel.reserved, 12);
    assert!(store.cancel_recovery_attempt(retry)?);
    let after_cancel = store.token_budget_state(budget)?;
    println!(
        "{}",
        json!({"mode":"synthetic_deadline_admission_fixture","new_model_calls":0,
        "first_claim":{"decision_id":decision_id,"observed_ms":observed_ms,"estimated_finish_ms":estimated_finish_ms},
        "retry":"insufficient_time","original_deadline_ms":timing.deadline_ms,
        "unsent_hold_before_explicit_cancel":before_cancel.reserved,"reserved_after_cancel":after_cancel.reserved,
        "synthetic_charged_tokens":after_cancel.spent,"completion_guaranteed":false})
    );
    Ok(())
}
