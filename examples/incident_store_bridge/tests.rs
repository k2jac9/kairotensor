use super::adapter::*;
use kairotensor::{Disposition, RecoveryRun, Store, TimingRefusal};
use kairotensor::{MeasuredRecoveryReply, ReplyValidation};
use rusqlite::Connection;
use serde_json::{json, Value};

// Format-valid, deliberately wrong for cal-01; no labels supply this canned output.
const REPLY: &[u8] =
    br#"{"decision":"abstain","owner":null,"reason":"undetermined_cause","evidence_ids":["e01"]}"#;

fn counts(file: &Database) -> (i64, i64, i64) {
    Connection::open(&file.0).unwrap().query_row(
        "SELECT (SELECT count(*) FROM attempts), (SELECT count(*) FROM token_reservations), (SELECT count(*) FROM dispatches)",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
}

#[test]
fn fixture_flags_cannot_admit_store_evidence() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    f.store.set_admitted(ACTOR, SCOPE, f.ids[1], false).unwrap();
    let caller_flag_current = true;
    assert!(caller_flag_current);
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
        Err(BridgeError::UnavailableEvidence)
    ));
    assert_eq!(f.store.token_budget_state(f.budget).unwrap().reserved, 0);
}

#[test]
fn missing_required_input_prevents_claim() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids[..2], &f.contract),
        Err(BridgeError::InvalidManifest)
    ));
    assert_eq!(f.store.token_budget_state(f.budget).unwrap().reserved, 0);
    assert_eq!(counts(&file), (0, 0, 0));
}

#[test]
fn duplicate_and_cross_case_mapping_refuse() {
    let f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let (context, _) = pinned_case("cal-01").unwrap();
    let raw = context.to_json().unwrap();
    let mut mappings = f.contract.mappings_for_test();
    mappings[1].occurrence = mappings[0].occurrence;
    assert!(
        IncidentContract::new("cal-01", f.ids[0], &raw, mappings, fixture_runtime(), 512).is_err()
    );
    let mut mappings = f.contract.mappings_for_test();
    mappings[0].case_id = "cal-02".into();
    assert!(
        IncidentContract::new("cal-01", f.ids[0], &raw, mappings, fixture_runtime(), 512).is_err()
    );
    let mut mappings = f.contract.mappings_for_test();
    mappings[0].occurrence = f.ids[0];
    assert!(
        IncidentContract::new("cal-01", f.ids[0], &raw, mappings, fixture_runtime(), 512).is_err()
    );
    let mut mappings = f.contract.mappings_for_test();
    mappings.pop();
    assert!(
        IncidentContract::new("cal-01", f.ids[0], &raw, mappings, fixture_runtime(), 512).is_err()
    );
    let mut mappings = f.contract.mappings_for_test();
    mappings.swap(0, 1);
    assert!(
        IncidentContract::new("cal-01", f.ids[0], &raw, mappings, fixture_runtime(), 512).is_err()
    );
    for case in ["", "unknown", "cal-02"] {
        assert!(IncidentContract::new(
            case,
            f.ids[0],
            &raw,
            f.contract.mappings_for_test(),
            fixture_runtime(),
            512
        )
        .is_err());
    }
}

#[test]
fn correction_after_prepare_refuses_send() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    let mut other = Store::open(&file.0).unwrap();
    other.set_admitted(ACTOR, SCOPE, f.ids[1], false).unwrap();
    other
        .observe_and_admit_many(
            ACTOR,
            SCOPE,
            &[(
                "e01",
                b"Correction: the earlier diagnosis is withdrawn; cause remains unknown.",
            )],
        )
        .unwrap();
    let result = f
        .store
        .run_timed_recovery(
            p.attempt,
            &p.preparation,
            || tick(200),
            |_| -> Result<_, String> { panic!("stale request invoked") },
        )
        .unwrap();
    assert!(matches!(
        result,
        RecoveryRun::Refused {
            reason: TimingRefusal::StaleEvidence,
            ..
        }
    ));
    assert_eq!(f.store.charged_usage().unwrap(), 0);
    assert_eq!(counts(&file).2, 0);
}

#[test]
fn unknown_and_unadmitted_have_identical_payload_free_errors() {
    let mut messages = Vec::new();
    for unknown in [false, true] {
        let file = Database::new();
        let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
        f.store.set_admitted(ACTOR, SCOPE, f.ids[1], false).unwrap();
        if unknown {
            let db = Connection::open(&file.0).unwrap();
            db.execute(
                "DELETE FROM admission_events WHERE evidence_id=?1",
                [f.ids[1]],
            )
            .unwrap();
            db.execute("DELETE FROM evidence WHERE id=?1", [f.ids[1]])
                .unwrap();
        }
        let error =
            prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap_err();
        assert!(matches!(error, BridgeError::UnavailableEvidence));
        messages.push(error.to_string());
        assert_eq!(counts(&file), (0, 0, 0));
    }
    assert_eq!(messages[0], messages[1]);
}

#[test]
fn captured_context_and_report_corruption_cancel_before_reservation() {
    for position in [0, 1] {
        let file = Database::new();
        let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
        let db = Connection::open(&file.0).unwrap();
        let bytes = if position == 0 {
            let (mut context, _) = pinned_case("cal-01").unwrap();
            context.question = "CORRUPTED CAPTURE".into();
            context.to_json().unwrap()
        } else {
            b"CORRUPTED REPORT".to_vec()
        };
        // Deliberately corrupt storage, independent of caller flags or caches.
        db.execute(
            "UPDATE evidence SET payload=?1 WHERE id=?2",
            rusqlite::params![bytes, f.ids[position]],
        )
        .unwrap();
        assert!(matches!(
            prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
            Err(BridgeError::ChangedContract)
        ));
        let state: String = db
            .query_row("SELECT state FROM recovery_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "cancelled");
        assert_eq!(counts(&file), (1, 0, 0));
        assert_eq!(f.store.token_budget_state(f.budget).unwrap().reserved, 0);
    }
}

#[test]
fn full_manifest_runtime_cap_and_source_mismatch_prevent_capture() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let (context, _) = pinned_case("cal-01").unwrap();
    let raw = context.to_json().unwrap();
    for change in 0..4 {
        let mut runtime = fixture_runtime();
        let mut mappings = f.contract.mappings_for_test();
        let mut cap = 512;
        match change {
            0 => runtime.model_revision = "other".into(),
            1 => cap = 513,
            2 => mappings[0].occurrence = 9999,
            _ => runtime.decoding = "different".into(),
        }
        let ids: Vec<_> = std::iter::once(f.ids[0])
            .chain(mappings.iter().map(|m| m.occurrence))
            .collect();
        let contract =
            IncidentContract::new("cal-01", f.ids[0], &raw, mappings, runtime, cap).unwrap();
        assert!(matches!(
            prepare_incident_attempt(&mut f.store, f.recovery, &ids, &contract),
            Err(BridgeError::ChangedContract)
        ));
        assert_eq!(counts(&file), (0, 0, 0));
    }
}

#[test]
fn sql_corruption_is_not_unavailable_evidence() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    // This test intentionally injects schema corruption outside the Store API.
    Connection::open(&file.0)
        .unwrap()
        .execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE evidence;")
        .unwrap();
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
        Err(BridgeError::Store(_))
    ));
}

#[test]
fn correction_before_reservation_has_no_hold_or_dispatch() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    Store::open(&file.0)
        .unwrap()
        .set_admitted(ACTOR, SCOPE, f.ids[1], false)
        .unwrap();
    assert!(f
        .store
        .reserve_recovery_attempt(p.attempt, &p.request, p.preparation.input_tokens, 512)
        .is_err());
    assert_eq!(counts(&file), (1, 0, 0));
    assert_eq!(f.store.token_budget_state(f.budget).unwrap().reserved, 0);
}

#[test]
fn retry_cannot_replace_bound_source_occurrences() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    f.store
        .run_timed_recovery(
            p.attempt,
            &p.preparation,
            || tick(200),
            |request| Ok::<_, String>(p.measure(request, b"bad")),
        )
        .unwrap();
    let (context, reports) = pinned_case("cal-01").unwrap();
    let replacement = f
        .store
        .observe_and_admit_many(ACTOR, SCOPE, &[("replacement", reports[0].text.as_bytes())])
        .unwrap()[0];
    let mut mappings = f.contract.mappings_for_test();
    mappings[0].occurrence = replacement;
    let contract = IncidentContract::new(
        "cal-01",
        f.ids[0],
        &context.to_json().unwrap(),
        mappings,
        fixture_runtime(),
        512,
    )
    .unwrap();
    let mut ids = f.ids.clone();
    ids[1] = replacement;
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &ids, &contract),
        Err(BridgeError::ChangedContract)
    ));
    assert_eq!(counts(&file), (1, 1, 1));
    // The bound occurrences can still obtain the one retry, under the original window.
    assert!(prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).is_ok());
}

#[test]
fn denied_reservation_is_terminal_without_dispatch() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let timing = f.store.recovery_timing(f.recovery).unwrap().unwrap();
    let budget = f.store.create_token_budget(0).unwrap();
    let recovery = f
        .store
        .create_timed_recovery("denied", budget, ACTOR, SCOPE, "denied", &timing)
        .unwrap();
    let p = prepare_incident_attempt(&mut f.store, recovery, &f.ids, &f.contract).unwrap();
    assert!(!f
        .store
        .reserve_recovery_attempt(p.attempt, &p.request, p.preparation.input_tokens, 512)
        .unwrap());
    assert!(matches!(
        f.store
            .run_timed_recovery(
                p.attempt,
                &p.preparation,
                || panic!("denied clock"),
                |_| panic!("denied backend")
            )
            .unwrap(),
        RecoveryRun::<String>::NotReserved
    ));
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, recovery, &f.ids, &f.contract),
        Err(BridgeError::TerminalRecovery)
    ));
}

#[test]
fn initial_request_matches_independent_python_renderer_bytes() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    // Python oracle is independent of Rust codec and never opens label bytes.
    let script = r#"import json, pathlib, sys
root=pathlib.Path(sys.argv[1])
cases=json.loads((root/'cases.json').read_text())
prompt=json.loads((root/'prompt.json').read_text())
case=next(c for c in cases['cases'] if c['case_id']=='cal-01')
canonical=lambda v: json.dumps(v,ensure_ascii=False,sort_keys=True,separators=(',',':'),allow_nan=False)
payload={k:case[k] for k in ('question','owner_ids','evidence')}
sys.stdout.buffer.write(canonical({'messages':[{'role':'system','content':prompt['system']},{'role':'user','content':canonical(payload)}]}).encode())
"#;
    let result = std::process::Command::new("python3")
        .args([
            "-c",
            script,
            concat!(env!("CARGO_MANIFEST_DIR"), "/workloads/incident-routing-v1"),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(p.request, result.stdout);
    assert_eq!(p.captured_ids, f.ids);
    assert_eq!(
        p.preparation.runtime.as_ref().unwrap().prompt_fingerprint,
        p.manifest_sha256
    );
}

#[test]
fn retry_is_byte_exact_fixed_feedback_and_keeps_original_deadline() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let initial = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    let timing = f.store.recovery_timing(f.recovery).unwrap();
    reserve(&mut f.store, &initial);
    let result = f
        .store
        .run_timed_recovery(
            initial.attempt,
            &initial.preparation,
            || tick(200),
            |request| {
                let mut reply = initial.measure(request, b"bad");
                reply.validation = ReplyValidation::Invalid {
                    validator: "fixture-validator".into(),
                    reason: "SECRET LABEL FREE TEXT MUST NOT REACH RETRY".into(),
                };
                Ok::<_, String>(reply)
            },
        )
        .unwrap();
    assert!(matches!(
        result,
        RecoveryRun::Settled {
            disposition: Disposition::InvalidOutput,
            ..
        }
    ));
    let retry = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    let mut expected: Value = serde_json::from_slice(&initial.request).unwrap();
    let mut user: Value =
        serde_json::from_str(expected["messages"][1]["content"].as_str().unwrap()).unwrap();
    user["validation_error"] = json!(FEEDBACK);
    expected["messages"][1]["content"] = json!(serde_json::to_string(&user).unwrap());
    assert_eq!(retry.request, serde_json::to_vec(&expected).unwrap());
    assert_eq!(retry.manifest_sha256, initial.manifest_sha256);
    assert_eq!(f.store.recovery_timing(f.recovery).unwrap(), timing);
    reserve(&mut f.store, &retry);
    let result = f
        .store
        .run_timed_recovery(
            retry.attempt,
            &retry.preparation,
            || tick(300),
            |request| Ok::<_, String>(retry.measure(request, b"bad again")),
        )
        .unwrap();
    assert!(matches!(
        result,
        RecoveryRun::Settled {
            disposition: Disposition::InvalidOutput,
            ..
        }
    ));
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
        Err(BridgeError::TerminalRecovery)
    ));
    assert_eq!(
        f.store.token_budget_state(f.budget).unwrap().spent,
        (initial.request.len() + 3 + retry.request.len() + 9) as i64
    );
}

#[test]
fn correction_inside_backend_records_full_usage_and_stale_publication() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    let mut other = Store::open(&file.0).unwrap();
    let mut calls = 0;
    let result = f.store.run_timed_recovery(p.attempt, &p.preparation, || tick(200), |request| {
        calls += 1;
        other.set_admitted(ACTOR, SCOPE, f.ids[1], false).unwrap();
        other.observe_and_admit_many(ACTOR, SCOPE, &[("e01", b"Correction: the earlier diagnosis is withdrawn; cause remains unknown.")]).unwrap();
        Ok::<_, String>(p.measure(request, REPLY))
    }).unwrap();
    assert!(matches!(
        result,
        RecoveryRun::Settled {
            disposition: Disposition::StaleView,
            ..
        }
    ));
    assert_eq!(calls, 1);
    let usage = (p.request.len() + REPLY.len()) as i64;
    assert_eq!(f.store.token_budget_state(f.budget).unwrap().spent, usage);
    assert_eq!(f.store.token_budget_state(f.budget).unwrap().reserved, 0);
    assert_eq!(
        f.store.settle(p.attempt, REPLY, usage).unwrap(),
        Disposition::StaleView
    );
    assert_eq!(f.store.charged_usage().unwrap(), usage);
    assert_eq!(counts(&file).2, 1);
}

#[test]
fn reopen_reserved_then_sent_does_not_reprepare_or_claim_twice() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    drop(f.store);
    f.store = Store::open(&file.0).unwrap();
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
        Err(BridgeError::ExistingRecovery)
    ));
    assert_eq!(
        f.store
            .recovery_reservation(p.attempt)
            .unwrap()
            .unwrap()
            .request,
        p.request
    );
    let mut calls = 0;
    assert!(matches!(
        f.store
            .run_timed_recovery(
                p.attempt,
                &p.preparation,
                || tick(200),
                |_| {
                    calls += 1;
                    Err::<MeasuredRecoveryReply, _>("backend uncertain")
                }
            )
            .unwrap(),
        RecoveryRun::BackendFailed {
            error: "backend uncertain",
            ..
        }
    ));
    drop(f.store);
    f.store = Store::open(&file.0).unwrap();
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
        Err(BridgeError::ExistingRecovery)
    ));
    assert!(matches!(
        f.store
            .run_timed_recovery(
                p.attempt,
                &p.preparation,
                || panic!("repeat clock"),
                |_| panic!("repeat backend")
            )
            .unwrap(),
        RecoveryRun::<String>::NotReserved
    ));
    assert_eq!(calls, 1);
    assert_eq!(
        f.store.token_budget_state(f.budget).unwrap().reserved,
        p.preparation.input_tokens + 512
    );
    assert!(!f.store.cancel_recovery_attempt(p.attempt).unwrap());
}

#[test]
fn post_claim_expiry_retains_hold_without_adapter_invocation() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    let mut samples = 0;
    let result = f
        .store
        .run_timed_recovery(
            p.attempt,
            &p.preparation,
            || {
                samples += 1;
                tick(if samples == 1 { 200 } else { 1000 })
            },
            |_| panic!("expired backend"),
        )
        .unwrap();
    assert!(matches!(
        result,
        RecoveryRun::<String>::ClaimedButNotInvoked {
            reason: TimingRefusal::DeadlineExpired,
            ..
        }
    ));
    assert_eq!(
        f.store.token_budget_state(f.budget).unwrap().reserved,
        p.preparation.input_tokens + 512
    );
    assert_eq!(f.store.charged_usage().unwrap(), 0);
    assert!(!f.store.cancel_recovery_attempt(p.attempt).unwrap());
}

#[test]
fn settlement_error_preserves_measured_reply_and_hold() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    Connection::open(&file.0).unwrap().execute_batch("CREATE TRIGGER reject_delivery BEFORE INSERT ON deliveries BEGIN SELECT RAISE(ABORT, 'fixture settlement failure'); END;").unwrap();
    let result = f
        .store
        .run_timed_recovery(
            p.attempt,
            &p.preparation,
            || tick(200),
            |request| Ok::<_, String>(p.measure(request, REPLY)),
        )
        .unwrap();
    match result {
        RecoveryRun::SettlementFailed { reply, error, .. } => {
            assert_eq!(reply.output, REPLY);
            assert_eq!(reply.input_tokens, p.request.len() as i64);
            assert!(error.to_string().contains("fixture settlement failure"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        f.store.token_budget_state(f.budget).unwrap().reserved,
        p.preparation.input_tokens + 512
    );
    assert_eq!(f.store.charged_usage().unwrap(), 0);
}

#[test]
fn protocol_valid_semantic_error_is_only_format_acceptance_and_duplicate_is_idempotent() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    assert!(matches!(
        f.store
            .run_timed_recovery(
                p.attempt,
                &p.preparation,
                || tick(200),
                |request| Ok::<_, String>(p.measure(request, REPLY))
            )
            .unwrap(),
        RecoveryRun::Settled {
            disposition: Disposition::Accepted,
            ..
        }
    ));
    let usage = (p.request.len() + REPLY.len()) as i64;
    assert_eq!(
        f.store.settle(p.attempt, REPLY, usage).unwrap(),
        Disposition::Accepted
    );
    assert_eq!(f.store.charged_usage().unwrap(), usage);
    assert!(matches!(
        prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract),
        Err(BridgeError::TerminalRecovery)
    ));
}

#[test]
fn cancellation_failure_preserves_original_error_and_sql_failure() {
    let file = Database::new();
    let mut f = Fixture::new(Store::open(&file.0).unwrap()).unwrap();
    let db = Connection::open(&file.0).unwrap();
    db.execute(
        "UPDATE evidence SET payload=?1 WHERE id=?2",
        rusqlite::params![b"corrupted".as_slice(), f.ids[1]],
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER reject_cancel BEFORE UPDATE OF state ON recovery_attempts WHEN NEW.state='cancelled' BEGIN SELECT RAISE(ABORT, 'fixture cancellation failure'); END;").unwrap();
    match prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap_err() {
        BridgeError::CancellationFailed {
            original,
            cancellation: CancellationFailure::Store(error),
        } => {
            assert!(matches!(*original, BridgeError::ChangedContract));
            assert!(error.to_string().contains("fixture cancellation failure"));
        }
        other => panic!("{other:?}"),
    }
    let state: String = db
        .query_row("SELECT state FROM recovery_attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "planned");
    assert_eq!(counts(&file), (1, 0, 0));
}

#[test]
fn cancellation_refusal_retains_claimed_hold_and_original_error() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    reserve(&mut f.store, &p);
    f.store
        .run_timed_recovery(
            p.attempt,
            &p.preparation,
            || tick(200),
            |_| Err::<MeasuredRecoveryReply, _>("uncertain"),
        )
        .unwrap();
    match cancel_new(&mut f.store, p.attempt, BridgeError::ChangedContract) {
        BridgeError::CancellationFailed {
            original,
            cancellation: CancellationFailure::Refused,
        } => assert!(matches!(*original, BridgeError::ChangedContract)),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        f.store.token_budget_state(f.budget).unwrap().reserved,
        p.preparation.input_tokens + 512
    );
}

#[test]
fn labels_never_enter_request_or_feedback() {
    let mut f = Fixture::new(Store::in_memory().unwrap()).unwrap();
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    let text = std::str::from_utf8(&p.request).unwrap();
    for forbidden in [
        "labels",
        "family_id",
        "split",
        "eligibility",
        "current_joint_success",
    ] {
        assert!(!text.contains(forbidden));
    }
    reserve(&mut f.store, &p);
    let result = f
        .store
        .run_timed_recovery(
            p.attempt,
            &p.preparation,
            || tick(200),
            |request| Ok::<_, String>(p.measure(request, b"invalid")),
        )
        .unwrap();
    assert!(matches!(
        result,
        RecoveryRun::Settled {
            disposition: Disposition::InvalidOutput,
            ..
        }
    ));
    let retry = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    assert!(std::str::from_utf8(&retry.request)
        .unwrap()
        .contains(FEEDBACK));
}

fn reserve(store: &mut Store, p: &IncidentPreparation) {
    assert!(store
        .reserve_recovery_attempt(
            p.attempt,
            &p.request,
            p.preparation.input_tokens,
            p.preparation.output_cap
        )
        .unwrap());
}

struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
                "incident-store-{}-{}-{}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
