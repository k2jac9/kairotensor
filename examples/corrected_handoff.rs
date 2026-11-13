//! Run with `cargo run --example corrected_handoff -- [--live] [--trace FILE]`.
//! All release data is synthetic. The adapter only generates text.
#[path = "corrected_handoff/adapter.rs"]
mod adapter;

use adapter::{Adapter, Reply};
use kairotensor::{Current, Disposition, Store};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{error::Error, fs, path::PathBuf, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ACTOR: &str = "release-planner";
const SCOPE: &str = "R42";
const PLAN: &str = "deployment-plan";
const PROOF: &str = "passing-report";
const REPORT_VERSION: &str = "ci/run-17";
const PLAN_VALIDATOR: &str = "corrected-release-plan-checklist/1";
const VERIFIER: &str = "release-fixture/check-all-12/v1";
const OLD: &[u8] = br#"{"version":"policy/v1","release":"R42","target":"staging","approval":"S42-A"}"#;
const NEW: &[u8] = br#"{"version":"policy/v2","release":"R42","target":"canary","approval":"C42-B"}"#;
const REPORT: &[u8] = b"release=R42\npassed=12\nfailed=0\nrequired=12\n";
const SYSTEM: &str = "You are a deployment planner. Return exactly one JSON object with five string fields: release, target, approval, tests, action. Copy release, target and approval from requirement. Copy tests from test_certificate. For a passing certificate use action deploy. Do not add other keys or explanation.";

// This fixture verifier deliberately accepts one exact report format and result.
// A rule's admission verdict alone cannot establish a test outcome.
struct Certificate {
    report: i64,
    check_attempt: i64,
    support_head: i64,
    fact: Value,
}

impl Certificate {
    fn issue(store: &mut Store, report: i64, checks: &mut u64) -> Result<Self> {
        let (check_attempt, inputs) = store.begin_from_candidates(ACTOR, SCOPE, VERIFIER, &[report])?;
        *checks += 1;
        if inputs.len() != 1 || inputs[0].0 != report || inputs[0].1.as_slice() != REPORT {
            return Err("report failed the R42 all-tests-passing check".into());
        }
        let fact = json!({"release": "R42", "tests": "12/12"});
        let disposition = store.settle(check_attempt, &serde_json::to_vec(&fact)?, 1)?;
        if disposition != Disposition::Accepted {
            return Err("the check became stale".into());
        }
        let support_head = store.publish_support(ACTOR, SCOPE, PROOF, &[vec![report]])?;
        Ok(Self { report, check_attempt, support_head, fact })
    }

    fn check_view(&self, store: &mut Store) -> Result<()> {
        if store.read_current(ACTOR, SCOPE, PROOF)? != Current::Ready(b"supported".to_vec())
            || store.revise_support(ACTOR, SCOPE, PROOF)? != self.support_head
        {
            return Err("certificate lost its original evidence or rule".into());
        }
        Ok(())
    }

    fn lineage(&self) -> Value {
        json!({"report_occurrence": self.report, "report_version": REPORT_VERSION,
               "verifier": VERIFIER, "verification_attempt": self.check_attempt,
               "support_attempt": self.support_head, "fact": self.fact})
    }
}

struct Call {
    attempt: i64,
    request: Vec<u8>,
    reply: Reply,
    row: usize,
    charge: i64,
}

fn call(
    store: &mut Store, model: &mut Adapter, cert: &Certificate, policy_candidates: &[i64],
    phase: &str, checkpoint: Option<i64>, trace: &mut Vec<Value>,
) -> Result<Call> {
    let mut candidates = policy_candidates.to_vec();
    candidates.push(cert.report);
    // Only the pre-withdrawal worker resumes the accepted summary. Its prior
    // acquisition pins immutable payload bytes; begin_resume rechecks their
    // membership and captures inherited bytes plus parent linkage atomically.
    let (attempt, selected, inherited) = if checkpoint.is_some() {
        let mut inputs = Vec::new();
        for &id in &candidates {
            let payload = store.read_evidence(ACTOR, SCOPE, id)?.ok_or("missing resume input")?;
            inputs.push((id, payload));
        }
        let (id, summary) = store.begin_resume(ACTOR, SCOPE, PLAN, &candidates)?;
        (id, inputs, Some(summary))
    } else {
        let (id, inputs) = store.begin_from_candidates(ACTOR, SCOPE, PLAN, &candidates)?;
        (id, inputs, None)
    };
    if selected.len() != 2 || selected[1].0 != cert.report || !policy_candidates.contains(&selected[0].0) {
        return Err("expected exactly one admitted requirement and the certificate's report".into());
    }
    cert.check_view(store)?;
    let requirement: Value = serde_json::from_slice(&selected[0].1)?;
    let user = json!({
        "requirement": requirement,
        "test_certificate": cert.fact,
        "sources": [requirement["version"], REPORT_VERSION],
        "test_verifier": VERIFIER,
    });
    let mut messages = vec![json!({"role":"system", "content":SYSTEM})];
    if let Some(bytes) = &inherited {
        messages.push(json!({"role":"assistant","content":std::str::from_utf8(bytes)?}));
    }
    messages.push(json!({"role":"user","content":serde_json::to_string(&user)?}));
    let request = model.freeze(Value::Array(messages))?;
    let exact = std::str::from_utf8(&request)?;
    if ["repair","fresh"].contains(&phase)
        && ["staging","S42-A","policy/v1","old-plan checkpoint"].iter().any(|s| exact.contains(s))
    {
        return Err("retired content in corrected dispatch".into());
    }
    store.record_dispatch(attempt, &request)?;
    let row = trace.len();
    trace.push(json!({"phase":phase, "store":if phase=="fresh" {"fresh"} else {"original"},
        "attempt":attempt, "source_ids":selected.iter().map(|s|s.0).collect::<Vec<_>>(),
        "source_versions":user["sources"], "certificate":cert.lineage(),
        "checkpoint_attempt":checkpoint, "request_utf8":exact,
        "disposition":"pending"}));
    let reply = model.dispatch(&request)?;
    let entry = &mut trace[row];
    entry["output"] = json!(reply.text);
    entry["input_tokens"] = json!(reply.input_tokens);
    entry["output_tokens"] = json!(reply.output_tokens);
    entry["roundtrip_us"] = json!(reply.roundtrip_us);
    entry["generation_us"] = json!(reply.generation_us);
    // Fixtures use one dispatch as the usage unit. Live model attempts use
    // measured prompt + completion tokens. Verifier charges are kept separate
    // in the trace and delta checks, even though the store uses one integer.
    let charge = match (reply.input_tokens, reply.output_tokens) {
        (Some(i), Some(o)) => i64::try_from(i.checked_add(o).ok_or("usage overflow")?)?,
        (None, None) => 1,
        _ => return Err("incomplete model usage".into()),
    };
    entry["charge"] = json!(charge);
    Ok(Call { attempt, request, reply, row, charge })
}

// A single optional JSON code fence is presentation only. All keys and values
// must match the checklist. Neither a wrong target nor extra prose can pass.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    release: String,
    target: String,
    approval: String,
    tests: String,
    action: String,
}

fn valid_plan(text: &str, policy: &[u8]) -> Result<bool> {
    let text = text.trim();
    let body = match text.strip_prefix("```json\n") {
        Some(rest) => match rest.strip_suffix("```") { Some(inner) => inner.trim(), None => return Ok(false) },
        None => text,
    };
    let req: Value = serde_json::from_slice(policy)?;
    let Ok(plan) = serde_json::from_str::<Plan>(body) else { return Ok(false) };
    Ok(req["release"].as_str() == Some(plan.release.as_str())
        && req["target"].as_str() == Some(plan.target.as_str())
        && req["approval"].as_str() == Some(plan.approval.as_str())
        && plan.tests == "12/12" && plan.action == "deploy")
}

fn settle_plan(
    store: &mut Store, call: &Call, policy: &[u8], trace: &mut [Value],
) -> Result<Disposition> {
    let valid = valid_plan(&call.reply.text, policy)?;
    let row = &mut trace[call.row];
    row["checklist_passed"] = json!(valid);
    row["validator"] = json!(PLAN_VALIDATOR);
    let before = store.charged_usage()?;
    let disposition = if valid {
        store.settle(call.attempt, call.reply.text.as_bytes(), call.charge)?
    } else {
        let reason = "model plan failed the captured release checklist";
        row["validation_reason"] = json!(reason);
        store.reject_invalid(call.attempt, call.reply.text.as_bytes(), call.charge, PLAN_VALIDATOR, reason)?
    };
    row["charge_delta"] = json!(store.charged_usage()? - before);
    row["disposition"] = json!(match disposition {
        Disposition::Accepted => "accepted",
        Disposition::StaleView => "stale_view",
        Disposition::StaleBranch => "stale_branch",
        Disposition::InvalidOutput => "invalid_output",
    });
    Ok(disposition)
}

fn accept(store: &mut Store, call: &Call, policy: &[u8], trace: &mut [Value]) -> Result<()> {
    match settle_plan(store, call, policy, trace)? {
        Disposition::Accepted => Ok(()),
        Disposition::InvalidOutput => Err("model plan failed the captured release checklist".into()),
        _ => Err("model attempt became ineligible".into()),
    }
}

fn route(call: &Call, checks: u64, start: Instant) -> Value {
    json!({"verifier_runs":checks, "model_calls":1,
           "input_tokens":call.reply.input_tokens, "output_tokens":call.reply.output_tokens,
           "model_roundtrip_us":call.reply.roundtrip_us,"elapsed_us":start.elapsed().as_micros() as u64})
}

fn scenario(model: &mut Adapter, trace: &mut Vec<Value>) -> Result<Value> {
    let mut store = Store::in_memory()?;
    let ids = store.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v1", OLD),(REPORT_VERSION, REPORT)])?;
    let mut checks = 0;
    let cert = Certificate::issue(&mut store, ids[1], &mut checks)?;
    let initial = call(&mut store, model, &cert, &ids[..1], "initial", None, trace)?;
    accept(&mut store, &initial, OLD, trace)?;

    // Build a real checkpoint from the accepted output on its exact basis.
    let summary = format!("old-plan checkpoint\n{}", initial.reply.text);
    let (summary_id, initial_bytes) = store.begin_resume(ACTOR, SCOPE, PLAN, &ids)?;
    if initial_bytes != initial.reply.text.as_bytes()
        || store.settle(summary_id, summary.as_bytes(), 0)? != Disposition::Accepted
    { return Err("could not publish the original checkpoint".into()); }
    let late = call(&mut store, model, &cert, &ids[..1], "delayed", Some(summary_id), trace)?;
    if !std::str::from_utf8(&late.request)?.contains("old-plan checkpoint") {
        return Err("original checkpoint missing from pre-correction request".into());
    }

    // Single writer, two evidence transitions; no dispatch between them. The
    // old worker has already produced its bytes. Only delivery is delayed.
    let repair_start = Instant::now();
    let before_repair = checks;
    store.set_admitted_and_revise(ACTOR, SCOPE, ids[0], false)?;
    let new = store.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v2", NEW)])?[0];
    if store.read_current(ACTOR, SCOPE, PLAN)? != Current::NeedsReview
        || store.begin_resume(ACTOR, SCOPE, PLAN, &[]).is_ok()
    { return Err("withdrawn checkpoint was reusable".into()); }
    let before = store.charged_usage()?;
    // Validate delayed bytes against the inputs they were generated from.
    let disposition = settle_plan(&mut store, &late, OLD, trace)?;
    let after_first = store.charged_usage()?;
    if settle_plan(&mut store, &late, OLD, trace)? != disposition
        || store.charged_usage()? != after_first || after_first - before != late.charge
    {
        return Err("late delivery retry failed to reproduce the original settlement".into());
    }
    trace[late.row]["duplicate_charge"] = trace[late.row]["charge_delta"].clone();
    trace[late.row]["charge_delta"] = json!(after_first - before);
    if disposition != Disposition::StaleView {
        return Err("late answer failed its original checklist or escaped stale rejection".into());
    }
    let repaired = call(&mut store, model, &cert, &[ids[0],new], "repair", None, trace)?;
    accept(&mut store, &repaired, NEW, trace)?;
    let repair_metrics = route(&repaired, checks - before_repair, repair_start);

    // The clean oracle starts from just the current raw sources in its own DB.
    let fresh_start = Instant::now();
    let mut fresh_checks = 0;
    let mut fresh = Store::in_memory()?;
    let current = fresh.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v2", NEW),(REPORT_VERSION,REPORT)])?;
    let rebuilt = Certificate::issue(&mut fresh, current[1], &mut fresh_checks)?;
    let control = call(&mut fresh, model, &rebuilt, &current[..1], "fresh", None, trace)?;
    accept(&mut fresh, &control, NEW, trace)?;
    let fresh_metrics = route(&control, fresh_checks, fresh_start);
    if repaired.request != control.request {
        return Err("repaired and fresh model inputs differ".into());
    }
    let total = |key: &str| -> Option<u64> {
        trace.iter().try_fold(0_u64, |sum, row| sum.checked_add(row[key].as_u64()?))
    };
    Ok(json!({"all_phases":{
        "model_calls":trace.len(),"verifier_runs":checks + fresh_checks,
        "input_tokens":total("input_tokens"),"output_tokens":total("output_tokens"),
        "model_roundtrip_us":total("roundtrip_us"),"model_generation_us":total("generation_us")
    },"repair":repair_metrics, "fresh":fresh_metrics, "reused_certificate":cert.lineage(),
        "old_checkpoint":summary_id,"retired_requirement":ids[0],"corrected_requirement":new,
        "delayed_delivery_charge":late.charge,"delayed_delivery_count":2,
        "checklist_passed":true,"identical_repaired_and_fresh_request":true}))
}

fn main() -> Result<()> {
    let mut live = false;
    let mut path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--live" => live = true,
            "--trace" if path.is_none() => path = Some(args.next().ok_or("--trace needs a path")?.into()),
            _ => return Err("usage: corrected_handoff [--live] [--trace FILE]".into()),
        }
    }
    let mut calls = Vec::new();
    let result = Adapter::new(live).and_then(|mut model| scenario(&mut model, &mut calls));
    let output = json!({"mode":if live {"live"} else {"fixture"},"calls":calls,
        "summary":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string)});
    if let Some(path) = path { fs::write(path, serde_json::to_vec_pretty(&output)?)?; }
    println!("{}", serde_json::to_string(&output["summary"])?);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_matches_clean_dispatch_and_keeps_stale_delivery_cost() -> Result<()> {
        let mut calls = Vec::new();
        let result = scenario(&mut Adapter::new(false)?, &mut calls)?;
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[1]["disposition"], "stale_view");
        assert_eq!(calls[2]["certificate"], calls[0]["certificate"]);
        assert_eq!(calls[2]["request_utf8"], calls[3]["request_utf8"]);
        assert_eq!(result["repair"]["verifier_runs"], 0);
        assert_eq!(result["fresh"]["verifier_runs"], 1);
        Ok(())
    }

    #[test]
    fn invalid_plan_is_recorded_and_charged_even_if_delivery_is_late() -> Result<()> {
        for withdraw in [false, true] {
            let mut store = Store::in_memory()?;
            let id = store.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v1", OLD)])?[0];
            let attempt = store.begin_attempt(ACTOR, SCOPE, PLAN, &[id])?;
            store.record_dispatch(attempt, b"request")?;
            let reply = Reply {
                text: r#"{"release":"R42"}"#.into(), input_tokens: Some(5), output_tokens: Some(7),
                generation_us: 0, roundtrip_us: 0,
            };
            let call = Call { attempt, request: b"request".to_vec(), reply, row: 0, charge: 12 };
            if withdraw { store.set_admitted(ACTOR, SCOPE, id, false)?; }
            let mut rows = [json!({"disposition":"pending"})];
            assert!(accept(&mut store, &call, OLD, &mut rows).is_err());
            assert_eq!(rows[0]["disposition"], "invalid_output");
            assert_eq!(rows[0]["charge_delta"], 12);
            assert!(accept(&mut store, &call, OLD, &mut rows).is_err());
            assert_eq!(rows[0]["charge_delta"], 0);
            assert_eq!(store.charged_usage()?, 12);
            assert_eq!(store.read_current(ACTOR,SCOPE,PLAN)?, Current::Empty);
        }
        Ok(())
    }

    #[test]
    fn a_missing_or_changed_report_cannot_supply_a_passing_certificate() -> Result<()> {
        let mut db = Store::in_memory()?;
        let ids = db.observe_and_admit_many(ACTOR,SCOPE,&[(REPORT_VERSION,REPORT),("bad",b"failed=1")])?;
        let mut checks = 0;
        assert!(Certificate::issue(&mut db,ids[1], &mut checks).is_err());
        let cert = Certificate::issue(&mut db,ids[0], &mut checks)?;
        db.set_admitted(ACTOR,SCOPE,ids[0],false)?;
        assert!(cert.check_view(&mut db).is_err());
        let duplicate = r#"{"target":"staging","release":"R42","target":"canary","approval":"C42-B","tests":"12/12","action":"deploy"}"#;
        assert!(!valid_plan(duplicate, NEW)?);
        assert!(!valid_plan(r#"{"target":"canary"}"#, NEW)?);
        Ok(())
    }
}
