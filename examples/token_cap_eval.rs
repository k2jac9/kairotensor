//! Run without arguments for a zero-inference archived token-budget boundary check.
//! Use --live --family NAME --trace FILE for actual independent capped generations.
#[allow(dead_code)]
#[path = "corrected_handoff/prepared_adapter.rs"]
mod adapter;
#[path = "token_cap_eval/cases.rs"]
mod cases;

use adapter::Adapter;
use cases::Case;
use kairotensor::{Disposition, Store};
use serde_json::{json, Value};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ACTOR: &str = "token-cap-pilot";
const SCOPE: &str = "ci-report";
const LIMIT: i64 = 1024;
const DEADLINE_MS: u64 = 60_000;
const VALIDATOR: &str = "cap-pilot-verdict/1";
const FIRST: &str = include_str!("../docs/evaluations/scoped-reuse-first-qwen-2026-11-13.json");
const REVISED: &str = include_str!("../docs/evaluations/scoped-reuse-qwen-2026-11-13.json");

fn archived_event(raw: &str) -> Result<Value> {
    let trace: Value = serde_json::from_str(raw)?;
    trace["events"].as_array().ok_or("missing archived events")?.iter()
        .find(|row| row["phase"] == "report_replaced" && row["task"] == "ci-judgment")
        .cloned().ok_or_else(|| "missing archived judgment".into())
}

fn number(row: &Value, key: &str) -> Result<i64> {
    row[key].as_i64().filter(|n| *n >= 0)
        .ok_or_else(|| format!("missing nonnegative integer: {key}").into())
}

// The offline path uses recorded counts. The live path independently tokenizes
// the exact archived envelope and discards the held tensor without inference.
fn quote_archive(worker: Option<&mut Adapter>, bytes: &[u8], expected: i64, cap: i64) -> Result<i64> {
    if let Some(worker) = worker {
        let prepared = worker.prepare(bytes)?;
        let actual = i64::try_from(prepared.input_tokens)?;
        let actual_cap = i64::try_from(prepared.max_new_tokens)?;
        worker.discard(prepared)?;
        if actual != expected || actual_cap != cap {
            return Err("live preflight differs from archived counts or configured cap".into());
        }
        return Ok(actual);
    }
    Ok(expected)
}

fn boundary(mut worker: Option<&mut Adapter>) -> Result<Value> {
    let old = archived_event(FIRST)?;
    let revised = archived_event(REVISED)?;
    let out = |row: &Value| -> Result<Vec<u8>> {
        Ok(row["output"].as_str().ok_or("missing archived answer")?.as_bytes().to_vec())
    };
    let mut quotes = Vec::new();
    for row in [&old, &revised] {
        let raw = row["request_utf8"].as_str().ok_or("missing archived request")?;
        let envelope: Value = serde_json::from_str(raw)?;
        let cap = number(&envelope["generation"], "max_new_tokens")?;
        let input = quote_archive(worker.as_deref_mut(), raw.as_bytes(), number(row, "input_tokens")?, cap)?;
        quotes.push((raw.as_bytes(), input, cap, input + number(row, "output_tokens")?));
    }
    let (original_request, original_input, original_cap, spent_before_retry) = quotes[0];
    let (retry_request, retry_input, retry_cap, observed_retry_usage) = quotes[1];
    let mut reduced: Value = serde_json::from_slice(retry_request)?;
    reduced["generation"]["max_new_tokens"] = json!(retry_cap - 1);
    let reduced = serde_json::to_vec(&reduced)?;
    let reduced_input = quote_archive(worker.as_deref_mut(), &reduced, retry_input, retry_cap - 1)?;
    let mut results = Vec::new();
    for limit in [403, 404] {
        let mut store = Store::in_memory()?;
        let budget = store.create_token_budget(limit)?;
        let original = store.begin_attempt(ACTOR, SCOPE, "historical-rejection", &[])?;
        if !store.reserve_dispatch(budget, original, original_request, original_input, original_cap)? {
            return Err("could not reserve the archived original".into());
        }
        store.reject_invalid(original, &out(&old)?, spent_before_retry, "strict-verdict-validator/1",
                             "test verdict disagrees with captured evidence")?;
        let retry = store.begin_attempt(ACTOR, SCOPE, "historical-retry", &[])?;
        let allowed = store.reserve_dispatch(budget, retry, retry_request, retry_input, retry_cap)?;
        if allowed {
            if store.settle(retry, &out(&revised)?, observed_retry_usage)? != Disposition::Accepted {
                return Err("archived retry failed to settle".into());
            }
        }
        let state = store.token_budget_state(budget)?;
        if allowed != (limit == 404) || state.spent != if allowed { 338 } else { 127 } || state.reserved != 0 {
            return Err("archived budget boundary failed".into());
        }
        let mut row = json!({
            "limit":limit, "spent_before_retry":spent_before_retry, "input_tokens":retry_input,
            "output_cap":retry_cap, "requested_reservation_tokens":retry_input + retry_cap,
            "allowed":allowed, "replay_spent":state.spent, "replay_outstanding":state.reserved,
            "unused_reservation_tokens":if allowed {retry_input + retry_cap - observed_retry_usage} else {0}
        });
        if !allowed {
            // A different request requires its own reservation. No archived output is
            // reattributed to this cap: without a measured generation its hold stays open.
            let attempt = store.begin_attempt(ACTOR, SCOPE, "lower-cap-counterfactual", &[])?;
            let fits = store.reserve_dispatch(budget, attempt, &reduced, reduced_input, retry_cap - 1)?;
            let after = store.token_budget_state(budget)?;
            if !fits || after.spent != 127 || after.reserved != 276 { return Err("cap 95 failed to fit".into()); }
            row["different_cap_counterfactual"] = json!({
                "cap":retry_cap-1,"reservation_granted":fits,"outstanding":after.reserved,
                "measured_completion":false,"new_model_calls":0
            });
        }
        results.push(row);
    }
    Ok(json!({"mode":"archived_replay","count_source":if worker.is_some() {"live_verified"} else {"archived"},
              "new_model_calls":0,"budgets":results}))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!("kairotensor-cap-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn trial(case: Case, cap: u64, order: usize, store: &mut Store, worker: &mut Adapter, row: &mut Value) -> Result<()> {
    let report = serde_json::to_vec(&case.report())?;
    let ids = store.observe_and_admit_many(ACTOR, SCOPE, &[(case.id, report.as_slice())])?;
    let budget = store.create_token_budget(LIMIT)?;
    *row = json!({"case_id":case.id,"family":case.family,"split":case.split,"trial_order":order,
        "cap":cap,"budget_limit":LIMIT,"budget_id":budget,"deadline_ms":DEADLINE_MS,"report":case.report(),
        "dispatched":false,"valid":false,"correct":false,"success":false,"on_time":false,
        "input_tokens":0,"output_tokens":0,"charged_tokens":0,"reserved_tokens":0,
        "hit_cap":false,"ended_eos":false,"disposition":"pending","reason":null});
    // Start before assembling and tokenizing the final request; stop only after
    // receiving its committed settlement. DB creation and model loading are outside.
    let start = Instant::now();
    let (attempt, captured) = store.begin_from_candidates(ACTOR, SCOPE, "verdict", &ids)?;
    if captured.len()!=1 || captured[0].0 != ids[0] || captured[0].1 != report {
        return Err("captured report differs from fixture".into());
    }
    row["attempt"] = json!(attempt);
    let request = worker.freeze_with_cap(case.messages(), cap)?;
    row["request_utf8"] = json!(std::str::from_utf8(&request)?);
    let prepared = worker.prepare(&request)?;
    let input = i64::try_from(prepared.input_tokens)?;
    let cap = i64::try_from(cap)?;
    if i64::try_from(prepared.max_new_tokens)? != cap { return Err("preflight quoted a different cap".into()); }
    row["preflight_input_tokens"] = json!(input);
    row["requested_reservation_tokens"] = json!(input + cap);
    let within_deadline = start.elapsed() <= Duration::from_millis(DEADLINE_MS);
    if !within_deadline || !store.reserve_dispatch(budget, attempt, &request, input, cap)? {
        worker.discard(prepared)?;
        row["disposition"] = if within_deadline { json!("budget_denied") } else { json!("deadline_expired") };
        row["reason"] = row["disposition"].clone();
    } else {
        row["dispatched"] = json!(true);
        row["reserved_tokens"] = json!(input + cap);
        // A send can fail after inference started. No usage is known until the
        // measured response arrives. The store retains the outstanding hold.
        row["input_tokens"] = Value::Null;
        row["output_tokens"] = Value::Null;
        row["charged_tokens"] = Value::Null;
        let response = worker.generate_prepared(prepared)?;
        let reply = response.reply;
        let input_observed = reply.input_tokens.ok_or("missing live input token count")?;
        let output = i64::try_from(reply.output_tokens.ok_or("missing live output token count")?)?;
        if i64::try_from(input_observed)? != input || output > cap {
            return Err("worker violated reservation".into());
        }
        let score = case.score(&reply.text);
        let usage = input + output;
        row["input_tokens"] = json!(input_observed);
        row["output_tokens"] = json!(output);
        row["output"] = json!(reply.text);
        row["valid"] = json!(score.valid);
        row["correct"] = json!(score.correct);
        row["reason"] = json!(score.reason);
        row["generation_us"] = json!(reply.generation_us);
        row["roundtrip_us"] = json!(reply.roundtrip_us);
        row["hit_cap"] = json!(response.hit_cap);
        row["ended_eos"] = json!(response.ended_eos);
        if score.valid && score.correct {
            if store.settle(attempt, reply.text.as_bytes(), usage)? != Disposition::Accepted {
                return Err("accepted verdict unexpectedly stale".into());
            }
            row["disposition"] = json!("accepted");
        } else {
            let reason = score.reason.as_deref().ok_or("invalid score has no reason")?;
            if store.reject_invalid(attempt, reply.text.as_bytes(), usage, VALIDATOR, reason)? != Disposition::InvalidOutput {
                return Err("invalid verdict escaped rejection".into());
            }
            row["disposition"] = json!("invalid_output");
        }
        row["charged_tokens"] = json!(usage);
    }
    let elapsed = start.elapsed().as_micros();
    row["receipt_us"] = json!(u64::try_from(elapsed)?);
    row["on_time"] = json!(elapsed <= u128::from(DEADLINE_MS) * 1_000);
    row["success"] = json!(row["dispatched"] == true && row["valid"] == true && row["correct"] == true && row["on_time"] == true);
    let state = store.token_budget_state(budget)?;
    if state.reserved != 0 || state.spent != number(row, "charged_tokens")? || state.spent > state.limit {
        return Err("ledger did not reconcile measured usage within the budget".into());
    }
    row["outstanding_after"] = json!(state.reserved);
    row["spent_after"] = json!(state.spent);
    Ok(())
}

fn main() -> Result<()> {
    let mut live = false;
    let mut family = None;
    let mut path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--live" if !live => live = true,
            "--family" if family.is_none() => family = Some(args.next().ok_or("--family requires NAME")?),
            "--trace" if path.is_none() => path = Some(PathBuf::from(args.next().ok_or("--trace requires FILE")?)),
            _ => return Err("usage: token_cap_eval [--live --family NAME] [--trace FILE]".into()),
        }
    }
    if live != family.is_some() { return Err("live mode requires --live and --family together".into()); }
    let mut trace = json!({"mode":if live {"live"} else {"archived_replay"},"family":family,
        "model":"Qwen/Qwen2.5-1.5B-Instruct","revision":"989aa7980e4cf806f80c7fef2b1adb7bc71aa306",
        "deadline_ms":DEADLINE_MS,"budget_limit":LIMIT,"samples":[],"error":null});
    let result: Result<()> = (|| {
        if let Some(name) = family.as_deref() {
            let cases = cases::family(name).ok_or("unknown report family")?;
            let tmp = Scratch::new()?;
            let mut model = Adapter::new(true)?;
            trace["boundary"] = boundary(Some(&mut model))?;
            for case in cases {
                for (order, cap) in case.cap_order().into_iter().enumerate() {
                    let mut store = Store::open(tmp.0.join(format!("{}-{cap}.sqlite",case.id)))?;
                    let mut row = Value::Null;
                    let measured = trial(case, cap, order, &mut store, &mut model, &mut row);
                    if let Err(error) = &measured {
                        row["operational_error"] = json!(error.to_string());
                        if let Some(id) = row["budget_id"].as_i64() {
                            if let Ok(state) = store.token_budget_state(id) {
                                row["outstanding_after"] = json!(state.reserved);
                                row["spent_after"] = json!(state.spent);
                            }
                        }
                    }
                    trace["samples"].as_array_mut().unwrap().push(row);
                    measured?;
                }
            }
        } else {
            trace["boundary"] = boundary(None)?;
        }
        Ok(())
    })();
    trace["error"] = json!(result.as_ref().err().map(ToString::to_string));
    if let Some(path) = path { fs::write(path,serde_json::to_vec_pretty(&trace)?)?; }
    println!("{}", json!({"mode":trace["mode"],"family":trace["family"],"boundary":trace["boundary"],
        "sample_count":trace["samples"].as_array().unwrap().len(),"error":trace["error"]}));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_reserves_the_cap_and_releases_only_measured_usage() -> Result<()> {
        let replay = boundary(None)?;
        assert_eq!(replay["count_source"], "archived");
        assert_eq!(replay["new_model_calls"], 0);
        assert_eq!(replay["budgets"][0]["allowed"], false);
        assert_eq!(replay["budgets"][1]["unused_reservation_tokens"], 66);
        Ok(())
    }
}
