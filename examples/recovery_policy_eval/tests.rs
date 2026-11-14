use super::*;

#[test]
fn frozen_corpus_is_disjoint_and_reference_labeled() -> Result<()> {
    let corpus = load_corpus()?;
    assert_eq!(corpus.cases.len(), 10);
    assert_eq!(
        corpus
            .cases
            .iter()
            .filter(|c| c.split == "calibration")
            .count(),
        4
    );
    assert_eq!(
        corpus.cases.iter().filter(|c| c.split == "heldout").count(),
        6
    );
    assert_eq!(
        (corpus.budget_tokens, corpus.output_cap, corpus.deadline_ms),
        (4096, 96, 60000)
    );
    Ok(())
}

#[test]
fn independent_validator_rejects_wrong_extra_and_duplicate_fields() -> Result<()> {
    let corpus = load_corpus()?;
    let reference = corpus.cases[0].initial_reference.as_ref();
    assert!(validate_answer(
        r#"{"release":"cal-A","tests":"12/12","status":"pass"}"#,
        reference
    ));
    for answer in [
        r#"{"release":"cal-A","tests":"12/12","status":"fail"}"#,
        r#"{"release":"cal-A","tests":"12/12","status":"pass","extra":1}"#,
        r#"{"release":"cal-A","release":"cal-A","tests":"12/12","status":"pass"}"#,
        "not JSON",
    ] {
        assert!(!validate_answer(answer, reference));
    }
    assert!(!validate_answer(
        r#"{"release":"cal-A","tests":"12/12","status":"pass"}"#,
        None
    ));
    Ok(())
}

fn journal() -> Journal {
    Journal {
        path: None,
        value: json!({"events":[],"completed_cases":[],"actual_backend_calls":0}),
    }
}
fn initial_ids(journal: &Journal) -> (i64, i64) {
    let start = journal.value["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["phase"] == "case_started")
        .unwrap();
    (
        start["budget_id"].as_i64().unwrap(),
        start["recovery_id"].as_i64().unwrap(),
    )
}
#[test]
fn corpus_run_records_exact_paired_trajectory_and_real_ledgers() -> Result<()> {
    let corpus = load_corpus()?;
    let mut store = Store::in_memory()?;
    let mut journal = journal();
    let document = collect(&mut store, &mut Fixture, &mut journal, &corpus, false)?;
    assert_eq!(document["schema_version"], 2);
    assert_eq!(document["mode"], "synthetic_policy_fixture");
    assert_eq!(journal.value["actual_backend_calls"], 18);
    let cases = document["cases"].as_array().unwrap();
    let mut calls = 0;
    let mut spent = 0;
    for case in cases {
        let initial = &case["initial"];
        calls += usize::from(!initial.is_null());
        spent += initial["charged_tokens"].as_i64().unwrap_or(0);
        let affordable = &case["retries"]["affordable_retry"];
        let gated = &case["retries"]["evidence_gated_retry"];
        if affordable.is_null() {
            assert!(gated.is_null());
            assert_eq!(initial["disposition"], "accepted");
        } else {
            assert_eq!(affordable["context"], gated["context"]);
            calls += usize::from(!affordable["attempt"].is_null());
            spent += affordable["attempt"]["charged_tokens"]
                .as_i64()
                .unwrap_or(0);
            assert!(
                affordable["context"]["input_tokens"].as_i64()
                    > case["initial_context"]["input_tokens"].as_i64()
            );
            let request: Value =
                serde_json::from_str(affordable["context"]["request"].as_str().unwrap())?;
            let payload: Value =
                serde_json::from_str(request["messages"][1]["content"].as_str().unwrap())?;
            assert_eq!(payload["validation_error"], FEEDBACK);
            assert!(payload.get("reference").is_none());
            if affordable["context"]["consistent"] == true {
                assert_eq!(affordable, gated);
            } else {
                assert!(gated["attempt"].is_null());
                assert_eq!(affordable["attempt"]["disposition"], "invalid_output");
            }
        }
    }
    assert_eq!(calls, 18);
    let events = journal.value["events"].as_array().unwrap();
    let totals: i64 = events
        .iter()
        .filter(|e| e["phase"] == "case_completed")
        .map(|e| e["ledger"]["spent"].as_i64().unwrap())
        .sum();
    assert_eq!(totals, spent);
    for event in events.iter().filter(|e| e["phase"] == "settled") {
        let id = event["attempt_id"].as_i64().unwrap();
        assert_eq!(store.claim_recovery_dispatch(id)?, None);
        assert_eq!(event["ledger"]["reserved"], 0);
        let end = event["receipt_ns"].as_i64().unwrap() / 1_000_000;
        let admission = event["admission_ns"].as_i64().unwrap() / 1_000_000;
        assert_eq!(event["attempt"]["elapsed_ms"], end - admission);
    }
    Ok(())
}

#[test]
fn first_budget_denial_has_no_fake_response_or_retry_quote() -> Result<()> {
    let mut corpus = load_corpus()?;
    corpus.budget_tokens = 1;
    let mut store = Store::in_memory()?;
    let mut journal = journal();
    let case = collect_case(
        &mut store,
        &mut Fixture,
        &mut journal,
        &corpus,
        &corpus.cases[1],
    )?;
    assert!(case["initial"].is_null());
    assert!(case["retries"]["affordable_retry"].is_null());
    assert_eq!(journal.value["actual_backend_calls"], 0);
    let (budget, _) = initial_ids(&journal);
    assert_eq!(store.token_budget_state(budget)?.spent, 0);
    assert_eq!(store.token_budget_state(budget)?.reserved, 0);
    Ok(())
}

#[test]
fn retry_uses_fresh_feedback_quote_and_can_be_denied() -> Result<()> {
    let mut corpus = load_corpus()?;
    let mut first_store = Store::in_memory()?;
    let first = collect_case(
        &mut first_store,
        &mut Fixture,
        &mut journal(),
        &corpus,
        &corpus.cases[1],
    )?;
    let retry = &first["retries"]["affordable_retry"]["context"];
    corpus.budget_tokens = first["initial"]["charged_tokens"].as_i64().unwrap()
        + retry["input_tokens"].as_i64().unwrap()
        + retry["output_cap"].as_i64().unwrap()
        - 1;
    let mut store = Store::in_memory()?;
    let mut journal = journal();
    let case = collect_case(
        &mut store,
        &mut Fixture,
        &mut journal,
        &corpus,
        &corpus.cases[1],
    )?;
    assert_eq!(case["initial"]["disposition"], "invalid_output");
    assert!(case["retries"]["affordable_retry"]["attempt"].is_null());
    assert_eq!(journal.value["actual_backend_calls"], 1);
    let (budget, _) = initial_ids(&journal);
    assert_eq!(store.token_budget_state(budget)?.reserved, 0);
    assert_eq!(
        store.token_budget_state(budget)?.spent,
        first["initial"]["charged_tokens"].as_i64().unwrap()
    );
    Ok(())
}

struct Fault {
    preparation: bool,
    usage: bool,
}
impl Backend for Fault {
    type Prepared = <Fixture as Backend>::Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        if self.preparation {
            Err("test preparation failure".into())
        } else {
            Fixture.prepare(messages, cap)
        }
    }
    fn generate(&mut self, prepared: Self::Prepared, output: &str) -> Result<Reply> {
        if self.usage {
            let mut reply = Fixture.generate(prepared, output)?;
            reply.input += 1;
            Ok(reply)
        } else {
            Err("test lost response".into())
        }
    }
    fn discard(&mut self, _: Self::Prepared) -> Result<()> {
        Ok(())
    }
}
#[test]
fn preparation_failure_cancels_but_claimed_failure_keeps_hold() -> Result<()> {
    let corpus = load_corpus()?;
    for preparation in [true, false] {
        let mut store = Store::in_memory()?;
        let mut journal = journal();
        let mut backend = Fault {
            preparation,
            usage: false,
        };
        assert!(collect_case(
            &mut store,
            &mut backend,
            &mut journal,
            &corpus,
            &corpus.cases[1]
        )
        .is_err());
        let (budget, recovery) = initial_ids(&journal);
        let state = store.token_budget_state(budget)?;
        assert_eq!(state.spent, 0);
        if preparation {
            assert_eq!(state.reserved, 0);
            assert!(matches!(
                store.begin_recovery_attempt(recovery, &[])?,
                RecoveryStart::Finished
            ));
            assert_eq!(journal.value["actual_backend_calls"], 0);
        } else {
            assert!(state.reserved > 0);
            let RecoveryStart::Existing(attempt) = store.begin_recovery_attempt(recovery, &[])?
            else {
                panic!("claim must remain existing")
            };
            assert_eq!(store.claim_recovery_dispatch(attempt)?, None);
            assert!(!store.cancel_recovery_attempt(attempt)?);
            let last = journal.value["events"].as_array().unwrap().last().unwrap();
            assert_eq!(last["attempt"]["disposition"], "uncertain");
            assert!(last["attempt"]["charged_tokens"].is_null());
            assert_eq!(journal.value["actual_backend_calls"], 1);
        }
    }
    Ok(())
}
#[test]
fn mismatched_usage_is_not_settled_or_scored() -> Result<()> {
    let corpus = load_corpus()?;
    let mut store = Store::in_memory()?;
    let mut journal = journal();
    assert!(collect_case(
        &mut store,
        &mut Fault {
            preparation: false,
            usage: true
        },
        &mut journal,
        &corpus,
        &corpus.cases[0]
    )
    .is_err());
    let (budget, _) = initial_ids(&journal);
    assert_eq!(store.token_budget_state(budget)?.spent, 0);
    assert!(store.token_budget_state(budget)?.reserved > 0);
    assert_eq!(
        journal.value["events"].as_array().unwrap().last().unwrap()["phase"],
        "received"
    );
    Ok(())
}
#[test]
fn expired_preparation_is_measured_and_cancelled_without_call() -> Result<()> {
    let mut corpus = load_corpus()?;
    corpus.deadline_ms = 0;
    let mut store = Store::in_memory()?;
    let mut journal = journal();
    let record = collect_case(
        &mut store,
        &mut Fixture,
        &mut journal,
        &corpus,
        &corpus.cases[1],
    )?;
    assert!(record["initial"].is_null());
    assert_eq!(journal.value["actual_backend_calls"], 0);
    let (budget, recovery) = initial_ids(&journal);
    assert_eq!(store.token_budget_state(budget)?.reserved, 0);
    assert!(matches!(
        store.begin_recovery_attempt(recovery, &[])?,
        RecoveryStart::Finished
    ));
    Ok(())
}

#[test]
fn unadmitted_candidates_are_not_dispatched_despite_reference() -> Result<()> {
    let corpus = load_corpus()?;
    let mut store = Store::in_memory()?;
    let case = &corpus.cases[0];
    let scope = "unadmitted";
    let ids = admit(&mut store, scope, &case.reports)?;
    store.set_admitted(ACTOR, scope, ids[0], false)?;
    let budget = store.create_token_budget(4096)?;
    let recovery = store.create_recovery("unadmitted", budget, ACTOR, scope, BRANCH)?;
    let mut op = Operation {
        start: Instant::now(),
        previous_ns: 0,
        recovery,
        budget,
        scope,
        case_id: &case.case_id,
        deadline_ms: 60000,
        cap: 96,
    };
    let mut journal = journal();
    let cell = collect_attempt(
        &mut store,
        &mut Fixture,
        &mut journal,
        &mut op,
        &ids,
        case.initial_reference.as_ref(),
        &case.fixture_outputs[0],
    )?;
    assert!(!cell.context.admitted);
    assert!(cell.attempt.is_none());
    assert_eq!(journal.value["actual_backend_calls"], 0);
    assert_eq!(store.token_budget_state(budget)?.reserved, 0);
    Ok(())
}

struct LateFixture;
impl Backend for LateFixture {
    type Prepared = <Fixture as Backend>::Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        Fixture.prepare(messages, cap)
    }
    fn generate(&mut self, prepared: Self::Prepared, output: &str) -> Result<Reply> {
        std::thread::sleep(std::time::Duration::from_millis(300));
        Fixture.generate(prepared, output)
    }
    fn discard(&mut self, _: Self::Prepared) -> Result<()> {
        Ok(())
    }
}
#[test]
fn late_valid_receipt_is_measured_charged_and_does_not_unlock_retry() -> Result<()> {
    let mut corpus = load_corpus()?;
    corpus.deadline_ms = 250;
    let mut store = Store::in_memory()?;
    let mut journal = journal();
    let record = collect_case(
        &mut store,
        &mut LateFixture,
        &mut journal,
        &corpus,
        &corpus.cases[0],
    )?;
    assert_eq!(record["initial"]["disposition"], "accepted");
    assert_eq!(record["initial"]["correct"], true);
    assert!(
        record["initial_context"]["elapsed_ms"].as_i64().unwrap()
            + record["initial"]["elapsed_ms"].as_i64().unwrap()
            > corpus.deadline_ms
    );
    assert!(record["retries"]["affordable_retry"].is_null());
    let (budget, _) = initial_ids(&journal);
    assert!(store.token_budget_state(budget)?.spent > 0);
    assert_eq!(store.token_budget_state(budget)?.reserved, 0);
    assert_eq!(journal.value["actual_backend_calls"], 1);
    Ok(())
}

struct RevokeFixture {
    path: PathBuf,
    scope: String,
    evidence: i64,
}
impl Backend for RevokeFixture {
    type Prepared = <Fixture as Backend>::Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        Fixture.prepare(messages, cap)
    }
    fn generate(&mut self, prepared: Self::Prepared, output: &str) -> Result<Reply> {
        let mut other = Store::open(&self.path)?;
        other.set_admitted(ACTOR, &self.scope, self.evidence, false)?;
        Fixture.generate(prepared, output)
    }
    fn discard(&mut self, _: Self::Prepared) -> Result<()> {
        Ok(())
    }
}
fn temporary_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "kairotensor-eval-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
#[test]
fn stale_valid_and_invalid_deliveries_never_unlock_replay_retry() -> Result<()> {
    let corpus = load_corpus()?;
    for (number, output) in [(0, corpus.cases[0].fixture_outputs[0].as_str()), (1, "{}")] {
        let path = temporary_path(&format!("stale-{number}"));
        let mut store = Store::open(&path)?;
        let case = &corpus.cases[0];
        let scope = "stale";
        let ids = admit(&mut store, scope, &case.reports)?;
        let budget = store.create_token_budget(4096)?;
        let recovery = store.create_recovery("stale", budget, ACTOR, scope, BRANCH)?;
        let mut op = Operation {
            start: Instant::now(),
            previous_ns: 0,
            recovery,
            budget,
            scope,
            case_id: &case.case_id,
            deadline_ms: 60000,
            cap: 96,
        };
        let mut journal = journal();
        journal.value["manifest"] = json!({"database":path});
        let mut backend = RevokeFixture {
            path: path.clone(),
            scope: scope.into(),
            evidence: ids[0],
        };
        let cell = collect_attempt(
            &mut store,
            &mut backend,
            &mut journal,
            &mut op,
            &ids,
            case.initial_reference.as_ref(),
            output,
        )?;
        assert_eq!(cell.attempt.as_ref().unwrap().disposition, "stale");
        assert_eq!(cell.attempt.as_ref().unwrap().correct, Some(number == 0));
        let stored =
            &journal.value["events"].as_array().unwrap().last().unwrap()["stored_delivery"];
        assert_eq!(
            stored["disposition"],
            if number == 0 {
                "stale_view"
            } else {
                "invalid_output"
            }
        );
        if number == 1 {
            assert_eq!(stored["applicability"], "stale_view");
        } else {
            assert!(stored["applicability"].is_null());
        }

        assert_eq!(store.token_budget_state(budget)?.reserved, 0);
        assert!(store.token_budget_state(budget)?.spent > 0);
        drop(store);
        fs::remove_file(path)?;
    }
    Ok(())
}
#[test]
fn repeated_trace_destination_is_refused_without_changing_any_artifact() -> Result<()> {
    let path = temporary_path("rerun");
    execute(&path, false)?;
    let paths = [
        path.clone(),
        suffix(&path, ".raw.json"),
        suffix(&path, ".sqlite"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(fs::read)
        .collect::<std::result::Result<_, _>>()?;
    assert!(execute(&path, false).is_err());
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(path)?, bytes);
        fs::remove_file(path)?;
    }
    Ok(())
}

#[test]
fn failed_collection_preserves_raw_error_database_and_withholds_replay() -> Result<()> {
    for preparation in [true, false] {
        let path = temporary_path(if preparation {
            "prepare-failed"
        } else {
            "send-failed"
        });
        assert!(execute_backend(&path, false, || Ok(Fault {
            preparation,
            usage: false
        }))
        .is_err());
        assert!(fs::read(&path)?.is_empty());
        let raw: Value = serde_json::from_slice(&fs::read(suffix(&path, ".raw.json"))?)?;
        assert_eq!(raw["status"], "failed");
        assert_eq!(raw["new_model_calls"], 0);
        assert!(raw["error"].as_str().unwrap().contains(if preparation {
            "preparation failure"
        } else {
            "lost response"
        }));
        let store = Store::open(suffix(&path, ".sqlite"))?;
        let budget = raw["events"][0]["budget_id"].as_i64().unwrap();
        assert_eq!(store.token_budget_state(budget)?.spent, 0);
        assert_eq!(store.token_budget_state(budget)?.reserved > 0, !preparation);
        drop(store);
        for file in [
            &path,
            &suffix(&path, ".raw.json"),
            &suffix(&path, ".sqlite"),
        ] {
            fs::remove_file(file)?;
        }
    }
    Ok(())
}
