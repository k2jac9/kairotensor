use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
struct Output(PathBuf);
impl Output {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
                "timed-runner-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                N.fetch_add(1, Ordering::Relaxed)
            )))
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn events(path: &Path) -> Vec<Value> {
    fs::read_to_string(path.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
fn sql(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        path.join("store.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}
#[test]
fn cli_rejects_live_unknown_duplicate_before_output() {
    let output = Output::new();
    let path = output.0.to_str().unwrap();
    for args in [
        vec!["--out", path, "--live"],
        vec!["--live", "--out", path],
        vec!["--out", path, "--out", path],
        vec!["--out", path, "--bogus"],
        vec!["--out", path, "--scenario", "unknown"],
    ] {
        assert!(parse_args(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
        assert!(!output.0.exists());
    }
    assert!(parse_args(&["--out".into(), path.into()]).is_ok());
}
#[test]
fn adapter_wrapper_end_to_end_exact_bytes_and_ledger() {
    let output = Output::new();
    let summary = collect(&output.0, Scenario::Happy).unwrap();
    assert_eq!(summary["outcome"], "settled_accepted");
    assert_eq!(summary["new_model_calls"], 0);
    assert_eq!(summary["adapter_invocations"], 1);
    assert_eq!(summary["audit_complete"], true);
    assert_eq!(summary["reserved"], 0);
    let ev = events(&output.0);
    let prepared = ev.iter().find(|v| v["event"] == "prepared").unwrap();
    let received = ev.iter().find(|v| v["event"] == "received").unwrap();
    let request = prepared["request_utf8"].as_str().unwrap();
    let raw = received["response_utf8"].as_str().unwrap();
    assert!(
        received["adapter_returned_elapsed_ns"].as_u64().unwrap()
            <= received["observed_elapsed_ns"].as_u64().unwrap()
    );
    assert_eq!(prepared["input_units"], request.len());
    assert_eq!(received["output_units"], raw.len());
    assert_eq!(summary["charged"], request.len() + raw.len());
    let db = sql(&output.0);
    let persisted: Vec<u8> = db
        .query_row("SELECT request FROM dispatches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(persisted, request.as_bytes());
    let usage: i64 = db
        .query_row("SELECT usage FROM deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(usage, (request.len() + raw.len()) as i64);
    assert!(
        ev.iter().position(|v| v["event"] == "received").unwrap()
            < ev.iter().position(|v| v["event"] == "finished").unwrap()
    );
    assert!(collect(&output.0, Scenario::Happy).is_err());
    assert_eq!(events(&output.0), ev);
}
#[test]
fn missing_stale_and_delay_do_not_invoke_adapter() {
    for (scenario, outcome, held) in [
        (Scenario::Missing, "capture_refused", false),
        (Scenario::Stale, "refused_stale_evidence", true),
        (Scenario::QueueDelay, "refused_insufficient_time", true),
        (
            Scenario::CommitDelay,
            "claimed_not_invoked_insufficient_time",
            true,
        ),
    ] {
        let output = Output::new();
        let summary = collect(&output.0, scenario).unwrap();
        assert_eq!(summary["outcome"], outcome);
        assert_eq!(summary["adapter_invocations"], 0);
        assert_eq!(summary["charged"], 0);
        assert_eq!(summary["reserved"].as_i64().unwrap() > 0, held);
        let db = sql(&output.0);
        assert_eq!(
            db.query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
#[test]
fn uncertain_backend_keeps_hold_and_malformed_is_charged() {
    for (scenario, outcome, has_delivery) in [
        (Scenario::Uncertain, "backend_failed", false),
        (Scenario::Malformed, "settled_invalid_output", true),
    ] {
        let output = Output::new();
        let summary = collect(&output.0, scenario).unwrap();
        assert_eq!(summary["outcome"], outcome);
        assert_eq!(summary["adapter_invocations"], 1);
        assert_eq!(summary["reserved"].as_i64().unwrap() == 0, has_delivery);
        assert_eq!(summary["charged"].as_i64().unwrap() > 0, has_delivery);
    }
}
#[test]
fn known_measurement_survives_audit_and_completion_clock_faults() {
    for scenario in [Scenario::AuditFailure, Scenario::CompletionClockUnavailable] {
        let output = Output::new();
        let s = collect(&output.0, scenario).unwrap();
        assert_eq!(s["outcome"], "settled_accepted");
        assert!(s["charged"].as_i64().unwrap() > 0);
        assert_eq!(s["reserved"], 0);
        if scenario == Scenario::AuditFailure {
            assert_eq!(s["audit_complete"], false);
            assert!(s["known_reply"]["response_utf8"].is_string());
        } else {
            assert_eq!(s["synthetic_receipt_on_time"], Value::Null);
        }
    }
}
#[test]
fn settlement_failure_preserves_response_and_uncertain_hold() {
    let output = Output::new();
    let s = collect(&output.0, Scenario::SettlementFailure).unwrap();
    assert_eq!(s["outcome"], "settlement_failed");
    assert_eq!(s["charged"], 0);
    assert!(s["reserved"].as_i64().unwrap() > 0);
    assert!(s["known_reply"]["response_utf8"].is_string());
    assert!(events(&output.0).iter().any(|v| v["event"] == "received"));
}
fn plan_from_audit(path: &Path) -> Plan {
    let ev = events(path);
    let created = ev.iter().find(|v| v["event"] == "created").unwrap();
    let captured = ev.iter().find(|v| v["event"] == "captured").unwrap();
    let prepared = ev.iter().find(|v| v["event"] == "prepared").unwrap();
    Plan {
        attempt: prepared["attempt"].as_i64().unwrap(),
        budget: created["budget"].as_i64().unwrap(),
        report: report_from(captured["payload_utf8"].as_str().unwrap().as_bytes()).unwrap(),
        preparation: RecoveryPreparation {
            runtime: Some(timing().runtime),
            calibration_id: Some(timing().calibration.id),
            request: prepared["request_utf8"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
            input_tokens: prepared["input_units"].as_i64().unwrap(),
            output_cap: prepared["output_cap"].as_i64().unwrap(),
        },
    }
}
#[test]
fn restart_and_duplicate_run_cannot_repeat_adapter_invocation() {
    for scenario in [
        Scenario::Happy,
        Scenario::Uncertain,
        Scenario::CommitDelay,
        Scenario::SettlementFailure,
    ] {
        let out = Output::new();
        let first = collect(&out.0, scenario).unwrap();
        let plan = plan_from_audit(&out.0);
        let mut reopened = Store::open(out.0.join("store.sqlite")).unwrap();
        let journal =
            RefCell::new(Journal::create(&out.0.join("restart-events.jsonl"), false).unwrap());
        let second = run_plan(&mut reopened, &plan, Scenario::Happy, &journal).unwrap();
        assert_eq!(second["outcome"], "not_reserved");
        assert_eq!(second["adapter_invocations"], 0);
        assert_eq!(second["charged"], first["charged"]);
        assert_eq!(second["reserved"], first["reserved"]);
        assert!(!reopened.cancel_recovery_attempt(plan.attempt).unwrap());
    }
}
#[test]
fn measured_failed_settlement_reconciles_once_after_restart() {
    let out = Output::new();
    let first = collect(&out.0, Scenario::SettlementFailure).unwrap();
    let plan = plan_from_audit(&out.0);
    let conn = rusqlite::Connection::open(out.0.join("store.sqlite")).unwrap();
    conn.execute_batch("DROP TRIGGER injected_delivery_failure")
        .unwrap();
    drop(conn);
    let mut store = Store::open(out.0.join("store.sqlite")).unwrap();
    let reply = &first["known_reply"];
    let usage = reply["input_units"].as_i64().unwrap() + reply["output_units"].as_i64().unwrap();
    let bytes = reply["response_utf8"].as_str().unwrap().as_bytes();
    assert_eq!(
        store.settle(plan.attempt, bytes, usage).unwrap(),
        Disposition::Accepted
    );
    assert_eq!(
        store.settle(plan.attempt, bytes, usage).unwrap(),
        Disposition::Accepted
    );
    let ledger = store.token_budget_state(plan.budget).unwrap();
    assert_eq!(ledger.spent, usage);
    assert_eq!(ledger.reserved, 0);
}
#[test]
fn staged_restart_uses_exact_quote_and_original_deadline() {
    let out = Output::new();
    collect(&out.0, Scenario::QueueDelay).unwrap();
    let plan = plan_from_audit(&out.0);
    let mut store = Store::open(out.0.join("store.sqlite")).unwrap();
    let journal =
        RefCell::new(Journal::create(&out.0.join("restart-events.jsonl"), false).unwrap());
    let later = run_plan(&mut store, &plan, Scenario::QueueDelay, &journal).unwrap();
    assert_eq!(later["outcome"], "refused_insufficient_time");
    assert_eq!(later["adapter_invocations"], 0);
    let mut altered = plan.clone();
    altered.preparation.request.push(b' ');
    assert!(run_plan(&mut store, &altered, Scenario::Happy, &journal).is_err());
    assert_eq!(store.charged_usage().unwrap(), 0);
}
#[test]
fn writer_queue_delays_production_clock_callback() {
    let out = Output::new();
    collect(&out.0, Scenario::QueueDelay).unwrap();
    let plan = plan_from_audit(&out.0);
    let mut store = Store::open(out.0.join("store.sqlite")).unwrap();
    let conn = rusqlite::Connection::open(out.0.join("store.sqlite")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let journal_path = out.0.join("queue-events.jsonl");
    let journal = RefCell::new(Journal::create(&journal_path, false).unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(()).unwrap();
        run_plan(&mut store, &plan, Scenario::QueueDelay, &journal).unwrap()
    });
    rx.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(
        fs::read_to_string(&journal_path).unwrap().is_empty(),
        "clock callback must not run before writer lock"
    );
    conn.execute_batch("COMMIT").unwrap();
    let summary = worker.join().unwrap();
    assert_eq!(summary["outcome"], "refused_insufficient_time");
    assert_eq!(summary["adapter_invocations"], 0);
}
#[test]
fn validator_and_report_parser_do_not_relax_contract() {
    let report = report_from(REPORT).unwrap();
    assert!(valid_answer(
        br#"{"release":"fixture-1","tests":"7/8","status":"fail"}"#,
        &report
    ));
    for output in [
        r#"{"release":"fixture-1","tests":"7/8","status":"pass"}"#,
        r#"{"release":"fixture-1","tests":"7/8","status":"fail","extra":0}"#,
        r#"{"release":"fixture-1","tests":"7/8","status":"fail","status":"fail"}"#,
        r#"{"release":"fixture-1","tests":"7/8","status":"fail"} trailing"#,
        r#"{"release":"fixture-2","tests":"7/8","status":"fail"}"#,
    ] {
        assert!(!valid_answer(output.as_bytes(), &report));
    }
    for bytes in [
        r#"{"release":"x","passed":true,"total":1}"#,
        r#"{"release":"x","passed":1.0,"total":1}"#,
        r#"{"release":"x","passed":2,"total":1}"#,
        r#"{"release":"","passed":1,"total":1}"#,
        r#"{"release":"x","passed":0,"total":0}"#,
        r#"{"release":"x","passed":1,"passed":1,"total":1}"#,
    ] {
        assert!(report_from(bytes.as_bytes()).is_err());
    }
}
#[test]
fn clock_audit_is_buffered_until_after_runtime_with_original_sequence() {
    let out = Output::new();
    collect(&out.0, Scenario::Happy).unwrap();
    let ev = events(&out.0);
    let received = ev.iter().position(|v| v["event"] == "received").unwrap();
    let clock = ev
        .iter()
        .position(|v| v["event"] == "clock_sample")
        .unwrap();
    assert!(
        clock > received,
        "clock sampling must buffer, not fsync under lock or at pre-I/O check"
    );
    assert!(ev[clock]["sequence"].as_u64().unwrap() < ev[received]["sequence"].as_u64().unwrap());
    assert!(
        ev[clock]["observed_elapsed_ns"].as_u64().unwrap()
            <= ev[received]["observed_elapsed_ns"].as_u64().unwrap()
    );
}
#[test]
fn failed_settlement_summary_preserves_full_validation_decision() {
    let out = Output::new();
    let s = collect(&out.0, Scenario::SettlementFailure).unwrap();
    assert_eq!(s["reconciliation_reply"]["validation"]["kind"], "valid");
    assert_eq!(
        s["reconciliation_reply"]["output_bytes"],
        serde_json::json!(s["known_reply"]["response_utf8"]
            .as_str()
            .unwrap()
            .as_bytes())
    );
}
#[test]
fn malformed_failed_settlement_can_reconcile_exact_invalid_decision() {
    let out = Output::new();
    let s = collect(&out.0, Scenario::MalformedSettlementFailure).unwrap();
    assert_eq!(s["outcome"], "settlement_failed");
    let full = &s["reconciliation_reply"];
    assert_eq!(full["validation"]["kind"], "invalid");
    assert_eq!(full["validation"]["validator"], VALIDATOR);
    assert_eq!(
        full["validation"]["reason"],
        "output does not match the complete captured report"
    );
    let bytes: Vec<u8> = serde_json::from_value(full["output_bytes"].clone()).unwrap();
    let usage = full["input_units"].as_i64().unwrap() + full["output_units"].as_i64().unwrap();
    let a = s["attempt"].as_i64().unwrap();
    rusqlite::Connection::open(out.0.join("store.sqlite"))
        .unwrap()
        .execute_batch("DROP TRIGGER injected_delivery_failure")
        .unwrap();
    let mut store = Store::open(out.0.join("store.sqlite")).unwrap();
    for _ in 0..2 {
        assert_eq!(
            store
                .reject_invalid(
                    a,
                    &bytes,
                    usage,
                    full["validation"]["validator"].as_str().unwrap(),
                    full["validation"]["reason"].as_str().unwrap()
                )
                .unwrap(),
            Disposition::InvalidOutput
        );
    }
    assert_eq!(store.charged_usage().unwrap(), usage);
}
#[test]
fn adapter_entry_is_buffered_until_adapter_returns_even_if_append_fails() {
    let out = Output::new();
    fs::create_dir(&out.0).unwrap();
    let mut store = Store::open(out.0.join("store.sqlite")).unwrap();
    let budget = store.create_token_budget(10000).unwrap();
    let e = store.observe("fixture", REPORT).unwrap();
    store.set_admitted(ACTOR, SCOPE, e, true).unwrap();
    let r = store
        .create_timed_recovery("fixture", budget, ACTOR, SCOPE, BRANCH, &timing())
        .unwrap();
    let RecoveryStart::New { attempt, .. } = store.begin_recovery_attempt(r, &[e]).unwrap() else {
        panic!()
    };
    let adapter = adapter::Adapter::new(false).unwrap();
    let request=adapter.freeze(serde_json::json!([{"role":"user","content":"{\"ci_report\":{\"release\":\"fixture-1\",\"test_summary\":\"7/8\",\"failing_cases\":1}}"}])).unwrap();
    let input = request.len() as i64;
    store
        .reserve_recovery_attempt(attempt, &request, input, 96)
        .unwrap();
    let plan = Plan {
        attempt,
        budget,
        report: report_from(REPORT).unwrap(),
        preparation: RecoveryPreparation {
            runtime: Some(timing().runtime),
            calibration_id: Some(timing().calibration.id),
            request,
            input_tokens: input,
            output_cap: 96,
        },
    };
    let journal = RefCell::new(Journal::create(&out.0.join("events.jsonl"), false).unwrap());
    journal.borrow_mut().fail_event = Some("adapter_entered");
    let result = run_plan(&mut store, &plan, Scenario::Happy, &journal).unwrap();
    assert_eq!(result["adapter_invocations"], 1);
    assert_eq!(result["outcome"], "settled_accepted");
    assert_eq!(result["audit_complete"], false);
    assert_eq!(result["reserved"], 0);
}
