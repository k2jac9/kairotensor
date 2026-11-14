//! Bounded recovery: deterministic byte-token fixture by default, --live for Qwen.
#[allow(dead_code)]
#[path = "corrected_handoff/adapter.rs"]
mod adapter;
use kairotensor::{Disposition, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ACTOR: &str = "recovery";
const SCOPE: &str = "report";
const BRANCH: &str = "verdict";
const VALIDATOR: &str = "recovery-verdict/1";
const REPORT: &[u8] = br#"{"release":"R42","passed":11,"total":12}"#;
const GOOD: &str = r#"{"release":"R42","tests":"11/12","status":"fail"}"#;
const BAD: &str = r#"{"release":"R42","tests":"11/12","status":"pass"}"#;

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Serialize)]
struct Outcome {
    reason: String,
    dispatches: usize,
    success: bool,
    spent: i64,
    reserved: i64,
}

trait Backend {
    type Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)>;
    fn generate(&mut self, prepared: Self::Prepared) -> Result<(String, i64)>;
    fn discard(&mut self, prepared: Self::Prepared) -> Result<()>;
}

fn recover<B: Backend>(
    store: &mut Store,
    budget: i64,
    candidates: &[i64],
    model: &mut B,
    cap: i64,
    deadline: Duration,
    elapsed: impl Fn() -> Duration,
) -> Result<Outcome> {
    if cap <= 0 {
        return Err("output cap must be positive".into());
    }
    let mut dispatches = 0;
    let mut validation_error = None;
    let mut reason = "retry_exhausted";
    let mut success = false;
    // An unresolved earlier send is not permission to retry after restart.
    if store.token_budget_state(budget)?.reserved != 0 {
        return finish(store, budget, "unknown_usage", dispatches, false);
    }
    for _ in 0..2 {
        if elapsed() >= deadline {
            reason = "deadline_expired";
            break;
        }
        // Recapture on every try; never inherit a rejected answer as evidence.
        let (attempt, captured) = store.begin_from_candidates(ACTOR, SCOPE, BRANCH, candidates)?;
        let report = if captured.len() == 1 {
            serde_json::from_slice::<Report>(&captured[0].1).ok()
        } else {
            None
        };
        let Some(report) =
            report.filter(|r| !r.release.is_empty() && r.total > 0 && r.passed <= r.total)
        else {
            reason = "insufficient_evidence";
            break;
        };
        let source: Value = serde_json::from_slice(&captured[0].1)?;
        let mut payload = json!({"ci_report":source});
        if let Some(error) = &validation_error {
            payload["validation_error"] = json!(error);
        }
        let messages = json!([
            {"role":"system","content":"Read ci_report. Return only JSON string fields release, tests, status. Copy release. Format tests as passed/total. status is pass only when passed equals total; otherwise fail."},
            {"role":"user","content":serde_json::to_string(&payload)?}
        ]);
        let (request, input, prepared) = model.prepare(messages, cap)?;
        // Preflight can consume the remaining time. Do not reserve or send then.
        if elapsed() >= deadline {
            model.discard(prepared)?;
            reason = "deadline_expired";
            break;
        }
        let reserved = store.reserve_dispatch(budget, attempt, &request, input, cap);
        match reserved {
            Ok(true) => {}
            Ok(false) => {
                model.discard(prepared)?;
                reason = "budget_denied";
                break;
            }
            Err(error) => {
                model.discard(prepared)?;
                return Err(error.into());
            }
        }
        // SQLite can wait for another writer. A reservation is not permission to
        // start inference after the deadline. There is no cancellation API yet;
        // conservatively retain this known-unsent hold and expose it in the result.
        if elapsed() >= deadline {
            model.discard(prepared)?;
            reason = "deadline_expired_reserved";
            break;
        }
        dispatches += 1;
        let (output, usage) = match model.generate(prepared) {
            Ok(reply) => reply,
            Err(_) => {
                reason = "unknown_usage";
                break;
            }
        };
        let valid = serde_json::from_str::<Verdict>(&output)
            .ok()
            .is_some_and(|v| {
                v.release == report.release
                    && v.tests == format!("{}/{}", report.passed, report.total)
                    && v.status
                        == if report.passed == report.total {
                            "pass"
                        } else {
                            "fail"
                        }
            });
        if valid {
            let disposition = store.settle(attempt, output.as_bytes(), usage)?;
            if disposition != Disposition::Accepted {
                reason = "stale_result";
                break;
            }
            success = elapsed() <= deadline;
            reason = if success { "accepted" } else { "late_result" };
            break;
        }
        let error = "Return complete JSON matching the captured release, passed/total count, and pass/fail status.";
        store.reject_invalid(attempt, output.as_bytes(), usage, VALIDATOR, error)?;
        validation_error = Some(error);
        // A second invalid answer falls out of the bounded loop, never a third call.
    }
    finish(store, budget, reason, dispatches, success)
}

fn finish(
    store: &Store,
    budget: i64,
    reason: &str,
    dispatches: usize,
    success: bool,
) -> Result<Outcome> {
    let state = store.token_budget_state(budget)?;
    Ok(Outcome {
        reason: reason.into(),
        dispatches,
        success,
        spent: state.spent,
        reserved: state.reserved,
    })
}

struct Fixture {
    answers: std::collections::VecDeque<String>,
    requests: Vec<Vec<u8>>,
    lost: bool,
    discards: usize,
}
impl Fixture {
    fn new(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|s| s.to_string()).collect(),
            requests: vec![],
            lost: false,
            discards: 0,
        }
    }
}
impl Backend for Fixture {
    type Prepared = (Vec<u8>, i64);
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        let request = serde_json::to_vec(
            &json!({"adapter":"byte-token-fixture/1","messages":messages,"cap":cap}),
        )?;
        Ok((request.clone(), request.len() as i64, (request, cap)))
    }
    fn generate(&mut self, (request, cap): Self::Prepared) -> Result<(String, i64)> {
        let input = request.len() as i64;
        self.requests.push(request);
        if self.lost {
            return Err("simulated lost response".into());
        }
        let answer = self.answers.pop_front().ok_or("fixture exhausted")?;
        // The fixture's vocabulary is UTF-8 bytes, deliberately not Qwen tokens.
        let bytes = &answer.as_bytes()[..answer.len().min(cap as usize)];
        let text = String::from_utf8_lossy(bytes).into_owned();
        Ok((text, input + bytes.len() as i64))
    }
    fn discard(&mut self, _: Self::Prepared) -> Result<()> {
        self.discards += 1;
        Ok(())
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() {
        execute(
            Fixture::new(&[BAD, GOOD]),
            "deterministic_byte_token_fixture",
        )
    } else if args == ["--live"] {
        execute(adapter::Adapter::new(true)?, "live_qwen")
    } else {
        Err("usage: budgeted_recovery [--live]".into())
    }
}

impl Backend for adapter::Adapter {
    type Prepared = adapter::Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        let request = self.freeze_with_cap(messages, u64::try_from(cap)?)?;
        let quote = adapter::Adapter::prepare(self, &request)?;
        if quote.max_new_tokens != cap as u64 {
            self.discard(quote)?;
            return Err("worker returned a different output cap".into());
        }
        Ok((request, i64::try_from(quote.input_tokens)?, quote))
    }
    fn generate(&mut self, prepared: Self::Prepared) -> Result<(String, i64)> {
        let reply = self.generate_prepared(prepared)?.reply;
        let input = reply.input_tokens.ok_or("missing input count")?;
        let output = reply.output_tokens.ok_or("missing output count")?;
        let usage = i64::try_from(input.checked_add(output).ok_or("usage overflow")?)?;
        Ok((reply.text, usage))
    }
    fn discard(&mut self, prepared: Self::Prepared) -> Result<()> {
        adapter::Adapter::discard(self, prepared)
    }
}

fn execute(mut model: impl Backend, mode: &str) -> Result<()> {
    let mut store = Store::in_memory()?;
    let evidence = store.observe_and_admit_many(ACTOR, SCOPE, &[("ci", REPORT)])?;
    let budget = store.create_token_budget(4096)?;
    let start = Instant::now();
    let result = recover(
        &mut store,
        budget,
        &evidence,
        &mut model,
        96,
        Duration::from_secs(60),
        || start.elapsed(),
    )?;
    println!(
        "{}",
        json!({"mode":mode,"new_model_calls":if mode == "live_qwen" {result.dispatches} else {0},"outcome":result})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kairotensor::Current;
    fn setup(limit: i64) -> Result<(Store, i64, Vec<i64>)> {
        let mut store = Store::in_memory()?;
        let ids = store.observe_and_admit_many(ACTOR, SCOPE, &[("ci", REPORT)])?;
        let budget = store.create_token_budget(limit)?;
        Ok((store, budget, ids))
    }
    fn run(store: &mut Store, budget: i64, ids: &[i64], model: &mut Fixture) -> Result<Outcome> {
        recover(
            store,
            budget,
            ids,
            model,
            96,
            Duration::from_secs(60),
            || Duration::ZERO,
        )
    }
    #[test]
    fn invalid_answer_retries_with_error_and_charges_both_calls() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&[BAD, GOOD]);
        let result = run(&mut store, budget, &ids, &mut model)?;
        assert!(result.success);
        assert_eq!(result.dispatches, 2);
        assert_eq!(result.reserved, 0);
        assert_eq!(
            result.spent,
            model.requests.iter().map(|r| r.len() as i64).sum::<i64>() + 98
        );
        let retry: Value = serde_json::from_slice(&model.requests[1])?;
        assert!(retry["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("validation_error"));
        assert_eq!(
            store.read_current(ACTOR, SCOPE, BRANCH)?,
            Current::Ready(GOOD.as_bytes().to_vec())
        );
        Ok(())
    }
    #[test]
    fn second_invalid_answer_is_terminal() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&[BAD, BAD, GOOD]);
        let result = run(&mut store, budget, &ids, &mut model)?;
        assert_eq!(result.reason, "retry_exhausted");
        assert!(!result.success);
        assert_eq!(model.requests.len(), 2);
        assert_eq!(store.read_current(ACTOR, SCOPE, BRANCH)?, Current::Empty);
        Ok(())
    }
    #[test]
    fn missing_evidence_and_expired_deadline_do_not_dispatch() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&[GOOD]);
        assert_eq!(
            run(&mut store, budget, &[], &mut model)?.reason,
            "insufficient_evidence"
        );
        let result = recover(
            &mut store,
            budget,
            &ids,
            &mut model,
            96,
            Duration::ZERO,
            || Duration::ZERO,
        )?;
        assert_eq!(result.reason, "deadline_expired");
        assert_eq!(result.spent, 0);
        assert_eq!(model.requests.len(), 0);
        Ok(())
    }
    #[test]
    fn budget_denial_does_not_send_and_unknown_usage_keeps_the_hold() -> Result<()> {
        let (mut store, budget, ids) = setup(0)?;
        let mut model = Fixture::new(&[GOOD]);
        assert_eq!(
            run(&mut store, budget, &ids, &mut model)?.reason,
            "budget_denied"
        );
        assert!(model.requests.is_empty());
        let budget = store.create_token_budget(4096)?;
        model.lost = true;
        let result = run(&mut store, budget, &ids, &mut model)?;
        assert_eq!(result.reason, "unknown_usage");
        assert_eq!(result.dispatches, 1);
        assert_eq!(result.spent, 0);
        assert!(result.reserved > 0);
        let again = run(&mut store, budget, &ids, &mut model)?;
        assert_eq!(again.reason, "unknown_usage");
        assert_eq!(model.requests.len(), 1);
        Ok(())
    }

    #[test]
    fn retry_budget_uses_the_larger_feedback_prompt() -> Result<()> {
        // First call costs 405. A stale original quote would reserve 452 and
        // incorrectly fit at 900; the actual feedback quote needs 571.
        let (mut store, budget, ids) = setup(900)?;
        let mut model = Fixture::new(&[BAD, GOOD]);
        let result = run(&mut store, budget, &ids, &mut model)?;
        assert_eq!(result.reason, "budget_denied");
        assert_eq!(result.dispatches, 1);
        assert_eq!(result.spent, model.requests[0].len() as i64 + 49);
        assert_eq!(result.reserved, 0);
        assert_eq!(model.discards, 1);
        Ok(())
    }

    #[test]
    fn late_valid_result_is_charged_but_not_a_success() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&[GOOD]);
        let times = std::cell::RefCell::new([0, 0, 0, 61].into_iter());
        let result = recover(
            &mut store,
            budget,
            &ids,
            &mut model,
            96,
            Duration::from_secs(60),
            || Duration::from_secs(times.borrow_mut().next().unwrap()),
        )?;
        assert_eq!(result.reason, "late_result");
        assert!(!result.success);
        assert!(result.spent > 0);
        assert_eq!(result.reserved, 0);
        assert_eq!(model.requests.len(), 1);
        Ok(())
    }

    #[test]
    fn preflight_expiry_discards_without_reserving() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&[GOOD]);
        let times = std::cell::RefCell::new([0, 60].into_iter());
        let result = recover(
            &mut store,
            budget,
            &ids,
            &mut model,
            96,
            Duration::from_secs(60),
            || Duration::from_secs(times.borrow_mut().next().unwrap()),
        )?;
        assert_eq!(result.reason, "deadline_expired");
        assert_eq!(result.spent, 0);
        assert_eq!(result.reserved, 0);
        assert!(model.requests.is_empty());
        assert_eq!(model.discards, 1);
        Ok(())
    }

    #[test]
    fn deadline_during_reservation_does_not_start_generation() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&[GOOD]);
        let times = std::cell::RefCell::new([0, 0, 60].into_iter());
        let result = recover(
            &mut store,
            budget,
            &ids,
            &mut model,
            96,
            Duration::from_secs(60),
            || Duration::from_secs(times.borrow_mut().next().unwrap()),
        )?;
        assert_eq!(result.reason, "deadline_expired_reserved");
        assert!(model.requests.is_empty());
        assert_eq!(model.discards, 1);
        assert_eq!(result.spent, 0);
        assert!(result.reserved > 0);
        Ok(())
    }

    #[test]
    fn truncated_multibyte_fixture_response_has_known_usage() -> Result<()> {
        let (mut store, budget, ids) = setup(4096)?;
        let mut model = Fixture::new(&["é", "é"]);
        let result = recover(
            &mut store,
            budget,
            &ids,
            &mut model,
            1,
            Duration::from_secs(60),
            || Duration::ZERO,
        )?;
        assert_eq!(result.reason, "retry_exhausted");
        assert_eq!(result.dispatches, 2);
        assert_eq!(result.reserved, 0);
        assert_eq!(
            result.spent,
            model.requests.iter().map(|r| r.len() as i64).sum::<i64>() + 2
        );
        Ok(())
    }

    struct ChangeDuringGeneration {
        inner: Fixture,
        editor: Store,
        retire: i64,
        replacement: i64,
    }
    impl Backend for ChangeDuringGeneration {
        type Prepared = <Fixture as Backend>::Prepared;
        fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
            self.inner.prepare(messages, cap)
        }
        fn generate(&mut self, prepared: Self::Prepared) -> Result<(String, i64)> {
            let reply = self.inner.generate(prepared)?;
            self.editor.set_admitted(ACTOR, SCOPE, self.retire, false)?;
            self.editor
                .set_admitted(ACTOR, SCOPE, self.replacement, true)?;
            Ok(reply)
        }
        fn discard(&mut self, prepared: Self::Prepared) -> Result<()> {
            self.inner.discard(prepared)
        }
    }

    #[test]
    fn correction_recaptures_new_evidence_without_reusing_the_old_report() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "recovery-test-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let mut store = Store::open(&path)?;
        let old = store.observe_and_admit_many(ACTOR, SCOPE, &[("old", REPORT)])?[0];
        let new = store.observe("new", br#"{"release":"R43","passed":7,"total":7}"#)?;
        let budget = store.create_token_budget(4096)?;
        let mut model = ChangeDuringGeneration {
            inner: Fixture::new(&[BAD, r#"{"release":"R43","tests":"7/7","status":"pass"}"#]),
            editor: Store::open(&path)?,
            retire: old,
            replacement: new,
        };
        let result = recover(
            &mut store,
            budget,
            &[old, new],
            &mut model,
            96,
            Duration::from_secs(60),
            || Duration::ZERO,
        )?;
        assert!(result.success);
        assert_eq!(result.dispatches, 2);
        let retry: Value = serde_json::from_slice(&model.inner.requests[1])?;
        let body: Value = serde_json::from_str(retry["messages"][1]["content"].as_str().unwrap())?;
        assert_eq!(body["ci_report"]["release"], "R43");
        assert_eq!(body["ci_report"]["passed"], 7);
        drop(model);
        drop(store);
        std::fs::remove_file(path)?;
        Ok(())
    }
}
