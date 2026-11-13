//! cargo run --example scoped_model_reuse -- [--live] [--trace FILE]
//! Synthetic, single-writer fixture. Both model tasks only produce text.
#[path = "corrected_handoff/adapter.rs"]
mod adapter;

use adapter::Adapter;
use kairotensor::{Current, Disposition, ModelAction, Store};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{error::Error, fs, path::PathBuf, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ACTOR: &str = "release-planner";
const SCOPE: &str = "R42";
const REPORT_BRANCH: &str = "ci-judgment";
const PLAN_BRANCH: &str = "release-plan";
const CONTRACT: &str = "ci-report-extractor/2+strict-verdict-validator/1";
const BEFORE: &[u8] = br#"{"version":"policy/v1","release":"R42","target":"staging","approval":"S42-A"}"#;
const AFTER: &[u8] = br#"{"version":"policy/v2","release":"R42","target":"canary","approval":"C42-B"}"#;
const GREEN: &[u8] = br#"{"version":"run/17","release":"R42","test_summary":"12/12","failing_cases":0}"#;
const RED: &[u8] = br#"{"version":"run/18","release":"R42","test_summary":"11/12","failing_cases":1}"#;
// The same balanced examples are shown for every run and release.
const REPORT_PROMPT: &str = r#"Extract the release test verdict. Even ONE failing case makes the entire verdict "fail".
CI example A: {"release":"EX1","test_summary":"7/8","failing_cases":1} -> {"release":"EX1","tests":"7/8","status":"fail"}
CI example B: {"release":"EX2","test_summary":"8/8","failing_cases":0} -> {"release":"EX2","tests":"8/8","status":"pass"}
Read the user's ci_report. Copy release and test_summary. If failing_cases is positive use "fail"; only zero uses "pass". Return only JSON with keys release, tests, status."#;
const PLAN_PROMPT: &str = r#"Create a JSON release plan. Copy release, target and approval from requirement. Copy tests from test_certificate. The certificate decides the action: {"status":"pass"} means {"action":"deploy"}; {"status":"fail"} means {"action":"hold"} even with an approval code. Return exactly five string fields: release, target, approval, tests, action. No explanation."#;

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Verdict { release: String, tests: String, status: String }

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan { release: String, target: String, approval: String, tests: String, action: String }

fn parse_answer<T: DeserializeOwned>(text: &str) -> Result<T> {
    let text = text.trim();
    let body = if let Some(inner) = text.strip_prefix("```json\n") {
        inner.strip_suffix("```").ok_or("unclosed JSON fence")?.trim()
    } else { text };
    Ok(serde_json::from_str(body)?)
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name].as_str().ok_or_else(|| format!("missing source field {name}").into())
}

fn validate_verdict(text: &str, report: &Value) -> Result<Verdict> {
    let actual: Verdict = parse_answer(text)?;
    let failed = report["failing_cases"].as_u64().ok_or("missing problem count")?;
    let status = if failed == 0 { "pass" } else { "fail" };
    if actual.release != field(report, "release")? || actual.tests != field(report, "test_summary")?
        || actual.status != status {
        return Err("test verdict disagrees with captured evidence".into());
    }
    Ok(actual)
}

fn validate_plan(text: &str, requirement: &Value, verdict: &Verdict) -> Result<Plan> {
    let actual: Plan = parse_answer(text)?;
    let action = if verdict.status == "pass" { "deploy" } else { "hold" };
    if actual.release != field(requirement, "release")? || actual.release != verdict.release
        || actual.target != field(requirement, "target")? || actual.approval != field(requirement, "approval")?
        || actual.tests != verdict.tests || actual.action != action {
        return Err("release plan failed the checklist".into());
    }
    Ok(actual)
}

fn request(model: &Adapter, system: &str, evidence: Value) -> Result<Vec<u8>> {
    model.freeze(json!([
        {"role":"system", "content":system},
        {"role":"user", "content":serde_json::to_string(&evidence)?}
    ]))
}

struct Answer<T> {
    value: T,
    raw: String,
    attempt: i64,
    origin: i64,
    request: Vec<u8>,
}

fn finish<T>(
    store: &mut Store, model: &mut Adapter, attempt: i64, request: Vec<u8>,
    mut entry: Value, events: &mut Vec<Value>, check: impl FnOnce(&str) -> Result<T>,
) -> Result<Answer<T>> {
    entry["attempt"] = json!(attempt);
    entry["request_utf8"] = json!(std::str::from_utf8(&request)?);
    entry["disposition"] = json!("pending");
    events.push(entry);
    let row = events.last_mut().expect("just inserted");
    let before = store.charged_usage()?;
    let action = if row["task"] == REPORT_BRANCH {
        store.resolve_model_attempt(attempt, &request)?
    } else {
        store.record_dispatch(attempt, &request)?;
        ModelAction::Dispatch
    };
    let (origin, text, value) = match action {
        ModelAction::Dispatch => {
            row["path"] = json!("dispatch");
            let reply = model.dispatch(&request)?;
            row["output"] = json!(reply.text);
            row["input_tokens"] = json!(reply.input_tokens);
            row["output_tokens"] = json!(reply.output_tokens);
            row["generation_us"] = json!(reply.generation_us);
            row["roundtrip_us"] = json!(reply.roundtrip_us);
            let value = check(&reply.text)?;
            let charge = match (reply.input_tokens, reply.output_tokens) {
                (Some(i), Some(o)) => i64::try_from(i.checked_add(o).ok_or("token count overflow")?)?,
                (None, None) => 1, // One fixture dispatch, not an estimated token count.
                _ => return Err("incomplete adapter usage".into()),
            };
            if store.settle(attempt, reply.text.as_bytes(), charge)? != Disposition::Accepted {
                return Err("model answer became stale before publication".into());
            }
            (attempt, reply.text, value)
        }
        ModelAction::Reused { origin_attempt, output } => {
            row["path"] = json!("reuse");
            let text = String::from_utf8(output)?;
            row["output"] = json!(text);
            row["input_tokens"] = json!(0);
            row["output_tokens"] = json!(0);
            row["generation_us"] = json!(0);
            row["roundtrip_us"] = json!(0);
            let value = check(&text)?;
            if store.charged_usage()? != before { return Err("charged for model reuse".into()); }
            (origin_attempt, text, value)
        }
    };
    row["disposition"] = json!("accepted");
    row["origin_attempt"] = json!(origin);
    row["charge_delta"] = json!(store.charged_usage()? - before);
    Ok(Answer { value, raw: text, attempt, origin, request })
}

struct Route {
    verdict: Answer<Verdict>,
    plan: Answer<Plan>,
    metrics: Value,
}

fn route(
    store: &mut Store, model: &mut Adapter, phase: &str, policy: i64, watchers: &[i64],
    events: &mut Vec<Value>,
) -> Result<Route> {
    let start = Instant::now();
    let offset = events.len();
    let namespace = if phase == "fresh" { "fresh" } else { "original" };
    let (attempt, captured) = store.begin_model_attempt(ACTOR, SCOPE, REPORT_BRANCH, watchers, CONTRACT)?;
    let [(report_id, payload)] = captured.as_slice() else { return Err("expected one admitted report".into()); };
    let report: Value = serde_json::from_slice(payload)?;
    let frozen = request(model, REPORT_PROMPT, json!({"ci_report":report}))?;
    let verdict = finish(store, model, attempt, frozen,
        json!({"phase":phase,"store":namespace,"task":REPORT_BRANCH,"contract":CONTRACT,
               "candidates":watchers,"source_ids":[report_id],"sources":[report["version"]]}),
        events, |text| validate_verdict(text, &report))?;

    // Record both source occurrences for this opaque planner. The report's validated
    // projection supplies model-visible facts; the explicit lineage links the two tasks.
    let ids = [policy, *report_id];
    let (attempt, captured) = store.begin_from_candidates(ACTOR, SCOPE, PLAN_BRANCH, &ids)?;
    if captured.iter().map(|(id,_)| *id).ne(ids)
        || store.read_current(ACTOR, SCOPE, REPORT_BRANCH)? != Current::Ready(verdict.raw.as_bytes().to_vec()) {
        return Err("planner inputs or report judgment are no longer current".into());
    }
    let requirement: Value = serde_json::from_slice(&captured[0].1)?;
    let frozen = request(model, PLAN_PROMPT, json!({"requirement":requirement,
        "test_certificate":verdict.value, "report_version":report["version"]}))?;
    let plan = finish(store, model, attempt, frozen,
        json!({"phase":phase,"store":namespace,"task":PLAN_BRANCH,"source_ids":ids,
               "sources":[requirement["version"], report["version"]],
               "judgment_attempt":verdict.attempt,"judgment_origin":verdict.origin}),
        events, |text| validate_plan(text, &requirement, &verdict.value))?;

    let rows = &events[offset..];
    let sum = |key: &str| -> Option<u64> {
        rows.iter().try_fold(0_u64, |s,r| s.checked_add(r[key].as_u64()?))
    };
    let metrics = json!({"model_calls":rows.iter().filter(|r|r["path"]=="dispatch").count(),
        "reuse_hits":rows.iter().filter(|r|r["path"]=="reuse").count(),
        "input_tokens":sum("input_tokens"),"output_tokens":sum("output_tokens"),
        "model_generation_us":sum("generation_us"),"model_roundtrip_us":sum("roundtrip_us"),
        "route_us":start.elapsed().as_micros()});
    Ok(Route { verdict, plan, metrics })
}

fn scenario(model: &mut Adapter, events: &mut Vec<Value>) -> Result<Value> {
    let mut store = Store::in_memory()?;
    let ids = store.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v1",BEFORE),("run/17",GREEN)])?;
    // A closed catalogue includes this future replacement before it is admitted.
    let replacement = store.observe("run/18", RED)?;
    let watchers = [ids[1], replacement];
    let initial = route(&mut store, model, "initial", ids[0], &watchers, events)?;
    store.set_admitted_and_revise(ACTOR, SCOPE, ids[0], false)?;
    let corrected = store.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v2", AFTER)])?[0];
    let repair = route(&mut store, model, "repair", corrected, &watchers, events)?;
    if repair.verdict.origin != initial.verdict.attempt || repair.metrics["model_calls"] != 1
        || repair.verdict.request != initial.verdict.request {
        return Err("unrelated edit failed to reuse the original report judgment".into());
    }

    let mut fresh_db = Store::in_memory()?;
    let revised = fresh_db.observe_and_admit_many(ACTOR, SCOPE, &[("policy/v2",AFTER),("run/17",GREEN)])?;
    let pending_report = fresh_db.observe("run/18", RED)?;
    let fresh_watchers = [revised[1], pending_report];
    let fresh = route(&mut fresh_db, model, "fresh", revised[0], &fresh_watchers, events)?;
    if fresh.metrics["model_calls"] != 2 || repair.verdict.request != fresh.verdict.request
        || repair.plan.request != fresh.plan.request || repair.plan.value != fresh.plan.value {
        return Err("repair did not match an independently generated clean plan".into());
    }
    let repaired_bytes = std::str::from_utf8(&repair.plan.request)?;
    if ["staging", "S42-A", "policy/v1"].iter().any(|old| repaired_bytes.contains(old)) {
        return Err("retired policy entered the repaired plan".into());
    }

    store.set_admitted_and_revise(ACTOR, SCOPE, ids[1], false)?;
    store.admit_many(ACTOR, SCOPE, &[replacement])?;
    let negative = route(&mut store, model, "report_replaced", corrected, &watchers, events)?;
    if negative.metrics["model_calls"] != 2 || negative.verdict.origin != negative.verdict.attempt
        || negative.plan.value.action != "hold" || negative.plan.value.tests != "11/12" {
        return Err("changed CI evidence failed to produce a fresh judgment and held release".into());
    }

    Ok(json!({"initial":initial.metrics,"repair":repair.metrics,"fresh":fresh.metrics,
        "report_replaced":negative.metrics,"identical_repair_and_fresh_requests":true,
        "repair_target":repair.plan.value.target,"negative_control_action":negative.plan.value.action}))
}

fn main() -> Result<()> {
    let mut live = false;
    let mut trace = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--live" => live = true,
            "--trace" if trace.is_none() => trace = Some(PathBuf::from(args.next().ok_or("--trace needs FILE")?)),
            _ => return Err("usage: scoped_model_reuse [--live] [--trace FILE]".into()),
        }
    }
    let mut events = Vec::new();
    let result = Adapter::new(live).and_then(|mut model| scenario(&mut model,&mut events));
    let report = json!({"mode":if live {"live"} else {"fixture"},"events":events,
        "summary":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string)});
    if let Some(path) = trace { fs::write(path,serde_json::to_vec_pretty(&report)?)?; }
    println!("{}",report["summary"]);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_one_generation_only_for_the_unchanged_report() -> Result<()> {
        let mut events = Vec::new();
        let result = scenario(&mut Adapter::new(false)?, &mut events)?;
        assert_eq!(events.len(), 8);
        assert_eq!(result["repair"]["model_calls"], 1);
        assert_eq!(result["fresh"]["model_calls"], 2);
        assert_eq!(result["repair"]["input_tokens"], Value::Null);
        assert_eq!(events[2]["path"], "reuse");
        assert_eq!(events[2]["charge_delta"], 0);
        assert_eq!(events[4]["path"], "dispatch");
        assert_eq!(events[6]["path"], "dispatch");
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_and_incorrect_judgments() {
        let evidence: Value = serde_json::from_slice(GREEN).unwrap();
        for text in [
            r#"{"release":"R42","tests":"12/12","status":"fail","status":"pass"}"#,
            r#"{"release":"R42","tests":"12/12","status":"pass","confidence":1}"#,
            r#"{"release":"R42","tests":"11/12","status":"pass"}"#,
        ] { assert!(validate_verdict(text,&evidence).is_err()); }
    }
}
