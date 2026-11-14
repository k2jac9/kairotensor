//! Offline adapter/runtime contract fixture. Never starts a live model worker.
#[allow(dead_code)]
#[path = "corrected_handoff/adapter.rs"]
mod adapter;
#[cfg(test)]
#[path = "timed_recovery_runner/tests.rs"]
mod tests;
use kairotensor::{
    Disposition, MeasuredRecoveryReply, RecoveryCalibration, RecoveryClock, RecoveryPreparation,
    RecoveryReadiness, RecoveryRun, RecoveryRuntime, RecoveryStart, RecoveryTiming,
    ReplyValidation, Store, TimingRefusal,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const ACTOR: &str = "offline-timed-runner";
const SCOPE: &str = "report-fixture";
const BRANCH: &str = "verdict";
const VALIDATOR: &str = "numeric-report-verdict/1";
const REPORT: &[u8] = br#"{"release":"fixture-1","passed":7,"total":8}"#;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Happy,
    Missing,
    Stale,
    QueueDelay,
    CommitDelay,
    Uncertain,
    Malformed,
    SettlementFailure,
    MalformedSettlementFailure,
    AuditFailure,
    CompletionClockUnavailable,
}
impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Happy => "happy",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::QueueDelay => "queue-delay",
            Self::CommitDelay => "commit-delay",
            Self::Uncertain => "uncertain",
            Self::Malformed => "malformed",
            Self::SettlementFailure => "settlement-failure",
            Self::MalformedSettlementFailure => "malformed-settlement-failure",
            Self::AuditFailure => "audit-failure",
            Self::CompletionClockUnavailable => "completion-clock-unavailable",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        [
            Self::Happy,
            Self::Missing,
            Self::Stale,
            Self::QueueDelay,
            Self::CommitDelay,
            Self::Uncertain,
            Self::Malformed,
            Self::SettlementFailure,
            Self::MalformedSettlementFailure,
            Self::AuditFailure,
            Self::CompletionClockUnavailable,
        ]
        .into_iter()
        .find(|v| v.name() == s)
        .ok_or_else(|| "unknown scenario".into())
    }
}
struct Options {
    out: PathBuf,
    scenario: Scenario,
}
fn parse_args(args: &[String]) -> Result<Options> {
    let mut out = None;
    let mut scenario = None;
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        i += 1;
        if key == "--live" {
            return Err(
                "live inference is disabled; synthetic calibration cannot authorize model work"
                    .into(),
            );
        }
        if key != "--out" && key != "--scenario" {
            return Err("unknown option".into());
        }
        let value = args.get(i).ok_or("missing option value")?;
        i += 1;
        if value.is_empty() || value.starts_with("--") {
            return Err("invalid option value".into());
        }
        match key.as_str() {
            "--out" if out.is_none() => out = Some(PathBuf::from(value)),
            "--scenario" if scenario.is_none() => scenario = Some(Scenario::parse(value)?),
            _ => return Err("duplicate option".into()),
        }
    }
    Ok(Options {
        out: out.ok_or("--out DIR is required")?,
        scenario: scenario.unwrap_or(Scenario::Happy),
    })
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Report {
    release: String,
    passed: u64,
    total: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verdict {
    release: String,
    tests: String,
    status: String,
}
fn report_from(bytes: &[u8]) -> Result<Report> {
    let r: Report = serde_json::from_slice(bytes)?;
    if r.release.is_empty() || r.total == 0 || r.passed > r.total {
        return Err("incomplete report".into());
    }
    Ok(r)
}
fn valid_answer(bytes: &[u8], report: &Report) -> bool {
    let Ok(v) = serde_json::from_slice::<Verdict>(bytes) else {
        return false;
    };
    v.release == report.release
        && v.tests == format!("{}/{}", report.passed, report.total)
        && v.status
            == if report.passed == report.total {
                "pass"
            } else {
                "fail"
            }
}
fn timing() -> RecoveryTiming {
    RecoveryTiming {
        clock_domain: "synthetic-offline-clock/v1".into(),
        start_ms: 100,
        deadline_ms: 1000,
        safety_margin_ms: 20,
        runtime: RecoveryRuntime {
            model: "fixture-only-no-model".into(),
            model_revision: "fixture/1".into(),
            tokenizer_revision: "utf8-byte-accounting/1".into(),
            runtime_version: "Adapter::dispatch fixture/1".into(),
            dtype: "not-applicable-fixture".into(),
            decoding: "deterministic-contract-fixture".into(),
            cpu_fingerprint: "a".repeat(64),
            prompt_fingerprint: "b".repeat(64),
            threads: 1,
            readiness: RecoveryReadiness::Warm,
        },
        calibration: RecoveryCalibration {
            id: "synthetic-not-measured/1".into(),
            max_duration_ms: 100,
            sample_count: 2,
            max_input_tokens: 4096,
            output_cap: 96,
            measured_at_ms: 0,
            expires_at_ms: 2000,
        },
    }
}
fn write_new(path: &Path, value: &Value) -> Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer(&mut f, value)?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    Ok(())
}
struct Journal {
    file: File,
    start: Instant,
    errors: Vec<String>,
    fail_event: Option<&'static str>,
    next_sequence: u64,
}
impl Journal {
    fn create(path: &Path, fail_received: bool) -> Result<Self> {
        Ok(Self {
            file: OpenOptions::new().write(true).create_new(true).open(path)?,
            start: Instant::now(),
            errors: vec![],
            fail_event: if fail_received {
                Some("received")
            } else {
                None
            },
            next_sequence: 0,
        })
    }
    fn capture(&mut self, mut value: Value) -> Result<Value> {
        value["observed_elapsed_ns"] = json!(u64::try_from(self.start.elapsed().as_nanos())?);
        value["sequence"] = json!(self.next_sequence);
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or("journal sequence overflow")?;
        Ok(value)
    }
    fn record(&mut self, value: Value) -> Result<()> {
        let value = self.capture(value)?;
        self.append(value)
    }
    fn append(&mut self, value: Value) -> Result<()> {
        let write = (|| -> Result<()> {
            if self.fail_event.is_some() && value["event"].as_str() == self.fail_event {
                self.fail_event = None;
                return Err("injected journal write failure".into());
            }
            serde_json::to_writer(&mut self.file, &value)?;
            self.file.write_all(b"\n")?;
            self.file.sync_data()?;
            Ok(())
        })();
        if let Err(e) = &write {
            self.errors.push(e.to_string());
        }
        write
    }
}
#[derive(Clone)]
struct Plan {
    attempt: i64,
    budget: i64,
    report: Report,
    preparation: RecoveryPreparation,
}
fn refusal_name(reason: TimingRefusal) -> String {
    let s = format!("{reason:?}");
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}
fn measured_record(reply: &MeasuredRecoveryReply) -> Value {
    let validation = match &reply.validation {
        ReplyValidation::Valid => json!({"kind":"valid"}),
        ReplyValidation::Invalid { validator, reason } => {
            json!({"kind":"invalid","validator":validator,"reason":reason})
        }
    };
    json!({"output_bytes":reply.output,"input_units":reply.input_tokens,"output_units":reply.output_tokens,"validation":validation})
}
fn run_plan(
    store: &mut Store,
    plan: &Plan,
    scenario: Scenario,
    journal: &RefCell<Journal>,
) -> Result<Value> {
    // Frozen reservation is independently read back before entering the runtime.
    if let Some(q) = store.recovery_reservation(plan.attempt)? {
        if q.request != plan.preparation.request
            || q.input_tokens != plan.preparation.input_tokens
            || q.output_cap != plan.preparation.output_cap
        {
            return Err("persisted quote differs from preparation".into());
        }
    }
    let mut adapter = adapter::Adapter::new(false)?;
    let clocks = Cell::new(0);
    let deferred_clocks = RefCell::new(Vec::new());
    let invocations = Cell::new(0);
    let known = RefCell::new(None::<Value>);
    let on_time = Cell::new(None::<bool>);
    let reconciliation = RefCell::new(None::<Value>);
    let result=store.run_timed_recovery(plan.attempt,&plan.preparation,||{
  let call=clocks.get()+1;clocks.set(call);
  let now=if scenario==Scenario::QueueDelay||(scenario==Scenario::CommitDelay&&call==2){900}else{200};
  // Sampling must not perform blocking journal I/O under the Store writer lock.
  let event=journal.borrow_mut().capture(json!({"event":"clock_sample","phase":if call==1{"claim_under_writer_lock"}else{"postcommit_pre_backend"},"synthetic_tick_ms":now})).map_err(|_|rusqlite::Error::InvalidQuery)?;
  deferred_clocks.borrow_mut().push(event);
  Ok(RecoveryClock{domain:timing().clock_domain,now_ms:now})
 },|request|->std::result::Result<MeasuredRecoveryReply,String>{
  // Capture in memory only; the durable sent claim already fences crashes.
  let entered=journal.borrow_mut().capture(json!({"event":"adapter_entered","attempt":plan.attempt})).map_err(|e|e.to_string())?;
  invocations.set(invocations.get()+1);
  let returned=adapter.dispatch(request);
  let returned_elapsed_ns=u64::try_from(journal.borrow().start.elapsed().as_nanos()).map_err(|e|e.to_string())?;
  let _=journal.borrow_mut().append(entered);
  let reply=returned.map_err(|e|e.to_string())?;
  if scenario==Scenario::Uncertain{journal.borrow_mut().record(json!({"event":"injected_unknown_after_adapter_invocation"})).map_err(|e|e.to_string())?;return Err("injected uncertain callback result; no usable receipt".into());}
  // Adapter fixture has no tokenizer counts. Explicit byte units, not estimates
  // of Qwen tokens; never claim this exercises prepared live-worker handles.
  if reply.input_tokens.is_some()||reply.output_tokens.is_some(){return Err("unexpected live measurements in offline fixture".into());}
  let response=if matches!(scenario,Scenario::Malformed|Scenario::MalformedSettlementFailure){"{bad json".to_owned()}else{reply.text};
  let input=i64::try_from(request.len()).map_err(|e|e.to_string())?;
  let output=i64::try_from(response.len()).map_err(|e|e.to_string())?;
  let receipt=if scenario==Scenario::CompletionClockUnavailable{None}else{Some(250)};
  on_time.set(receipt.map(|now|now<=timing().deadline_ms));
  let observed=json!({"event":"received","adapter_returned_elapsed_ns":returned_elapsed_ns,"response_utf8":response,"input_units":input,"output_units":output,"synthetic_received_tick_ms":receipt,"adapter_generation_us":reply.generation_us,"adapter_roundtrip_us":reply.roundtrip_us,"fault_injected":matches!(scenario,Scenario::Malformed|Scenario::MalformedSettlementFailure)});
  *known.borrow_mut()=Some(observed.clone());
  // Once a measured reply is known, journal/timing failures cannot erase usage.
  // Summary provides a second durable reconciliation record if this append fails.
  let _=journal.borrow_mut().record(observed);
  let validation=if valid_answer(response.as_bytes(),&plan.report){ReplyValidation::Valid}else{ReplyValidation::Invalid{validator:VALIDATOR.into(),reason:"output does not match the complete captured report".into()}};
  let measured=MeasuredRecoveryReply{output:response.into_bytes(),input_tokens:input,output_tokens:output,validation};
  let full=measured_record(&measured);
  *reconciliation.borrow_mut()=Some(full.clone());
  let _=journal.borrow_mut().record(json!({"event":"validated","measured_reply":full}));
  Ok(measured)
 })?;
    // Physical JSONL order differs from observed order for buffered clock events;
    // sequence and observed_elapsed_ns preserve their original observation point.
    for event in deferred_clocks.into_inner() {
        let _ = journal.borrow_mut().append(event);
    }
    let outcome = match result {
        RecoveryRun::NotReserved => "not_reserved".to_owned(),
        RecoveryRun::Refused { reason, .. } => format!("refused_{}", refusal_name(reason)),
        RecoveryRun::ClaimedButNotInvoked { reason, .. } => {
            format!("claimed_not_invoked_{}", refusal_name(reason))
        }
        RecoveryRun::BackendFailed { .. } => "backend_failed".into(),
        RecoveryRun::SettlementFailed { reply, .. } => {
            let full = measured_record(&reply);
            *reconciliation.borrow_mut() = Some(full.clone());
            let _ = journal
                .borrow_mut()
                .record(json!({"event":"settlement_failed","measured_reply":full}));
            "settlement_failed".into()
        }
        RecoveryRun::Settled { disposition, .. } => match disposition {
            Disposition::Accepted => "settled_accepted",
            Disposition::InvalidOutput => "settled_invalid_output",
            _ => "settled_stale",
        }
        .into(),
    };
    let ledger = store.token_budget_state(plan.budget)?;
    let _=journal.borrow_mut().record(json!({"event":"finished","outcome":outcome,"charged":ledger.spent,"reserved":ledger.reserved}));
    Ok(
        json!({"schema_version":1,"mode":"offline_timed_adapter_fixture","new_model_calls":0,"collection_complete":true,"audit_complete":journal.borrow().errors.is_empty(),"audit_errors":journal.borrow().errors,"accounting_reconciled":ledger.reserved==0,"accounting":"utf8_byte_fixture_units","attempt":plan.attempt,"outcome":outcome,"adapter_invocations":invocations.get(),"charged":ledger.spent,"reserved":ledger.reserved,"known_reply":*known.borrow(),"reconciliation_reply":*reconciliation.borrow(),"synthetic_receipt_on_time":on_time.get(),"correctness_implies_timeliness":false}),
    )
}
fn collect(out: &Path, scenario: Scenario) -> Result<Value> {
    // Exclusive directory ownership is established before any Store/backend work.
    fs::create_dir(out)?;
    write_new(
        &out.join("manifest.json"),
        &json!({"schema_version":1,"mode":"offline_timed_adapter_fixture","new_model_calls":0,"initial_status":"incomplete_until_summary","scenario":scenario.name(),"accounting":"utf8_byte_fixture_units","adapter":"fixture/v1","validator":VALIDATOR,"timing":timing(),"latency_is_measured_model_calibration":false,"automatic_resume":false,"journal_observation_order":"sequence","clock_events_flushed":"after_runtime_returns","adapter_entry_flushed":"after_adapter_returns"}),
    )?;
    let journal = RefCell::new(Journal::create(
        &out.join("events.jsonl"),
        scenario == Scenario::AuditFailure,
    )?);
    let mut store = Store::open(out.join("store.sqlite"))?;
    let budget = store.create_token_budget(10000)?;
    let evidence = store.observe("fixed-contract-fixture/1", REPORT)?;
    store.set_admitted(ACTOR, SCOPE, evidence, true)?;
    let recovery =
        store.create_timed_recovery("offline-fixture", budget, ACTOR, SCOPE, BRANCH, &timing())?;
    journal
        .borrow_mut()
        .record(json!({"event":"created","budget":budget,"recovery":recovery}))?;
    let candidates = if scenario == Scenario::Missing {
        vec![evidence, i64::MAX]
    } else {
        vec![evidence]
    };
    let (attempt, inputs) = match store.begin_recovery_attempt(recovery, &candidates) {
        Ok(RecoveryStart::New {
            attempt, inputs, ..
        }) => (attempt, inputs),
        Err(_) if scenario == Scenario::Missing => {
            journal.borrow_mut().record(json!({"event":"capture_refused","reason":"complete eligible evidence unavailable"}))?;
            let s = json!({"schema_version":1,"mode":"offline_timed_adapter_fixture","new_model_calls":0,"collection_complete":true,"audit_complete":true,"accounting_reconciled":true,"outcome":"capture_refused","adapter_invocations":0,"charged":0,"reserved":0});
            write_new(&out.join("summary.json"), &s)?;
            return Ok(s);
        }
        Err(e) => return Err(e.into()),
        _ => return Err("expected fresh attempt".into()),
    };
    if inputs.len() != 1 {
        return Err("exactly one complete captured report required".into());
    }
    let report = report_from(&inputs[0].1)?;
    journal.borrow_mut().record(json!({"event":"captured","attempt":attempt,"evidence_id":inputs[0].0,"payload_utf8":std::str::from_utf8(&inputs[0].1)?}))?;
    let adapter = adapter::Adapter::new(false)?;
    let messages = json!([{"role":"system","content":"Return only JSON string fields release, tests, status from the supplied CI report."},{"role":"user","content":serde_json::to_string(&json!({"ci_report":{"release":report.release,"test_summary":format!("{}/{}",report.passed,report.total),"failing_cases":report.total-report.passed}}))?}]);
    let request = adapter.freeze(messages)?;
    let input = i64::try_from(request.len())?;
    let preparation = RecoveryPreparation {
        runtime: Some(timing().runtime),
        calibration_id: Some(timing().calibration.id),
        request,
        input_tokens: input,
        output_cap: 96,
    };
    journal.borrow_mut().record(json!({"event":"prepared","attempt":attempt,"request_utf8":std::str::from_utf8(&preparation.request)?,"input_units":input,"output_cap":96}))?;
    if !store.reserve_recovery_attempt(attempt, &preparation.request, input, 96)? {
        return Err("fixture unexpectedly exceeds budget".into());
    }
    if scenario == Scenario::Stale {
        store.set_admitted(ACTOR, SCOPE, evidence, false)?;
    }
    if matches!(
        scenario,
        Scenario::SettlementFailure | Scenario::MalformedSettlementFailure
    ) {
        rusqlite::Connection::open(out.join("store.sqlite"))?.execute_batch("CREATE TRIGGER injected_delivery_failure BEFORE INSERT ON deliveries BEGIN SELECT RAISE(ABORT,'injected settlement failure'); END;")?;
    }
    let plan = Plan {
        attempt,
        budget,
        report,
        preparation,
    };
    let summary = run_plan(&mut store, &plan, scenario, &journal)?;
    write_new(&out.join("summary.json"), &summary)?;
    Ok(summary)
}
fn main() -> Result<()> {
    let options = parse_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    println!("{}", collect(&options.out, options.scenario)?);
    Ok(())
}
