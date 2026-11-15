use kairotensor::policy_registry::validate_policy_artifact;

#[test]
fn exact_known_artifact_only() {
    let good = br#"{"schema_version":1,"policy_name":"always_stop","purpose":"fixture/recovery-policy","compatibility_contract_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
    assert!(validate_policy_artifact(good).is_ok());
    for bad in [
        String::from_utf8(good.to_vec())
            .unwrap()
            .replace("always_stop", "user_script"),
        String::from_utf8(good.to_vec())
            .unwrap()
            .replace("fixture/recovery-policy", " "),
        String::from_utf8(good.to_vec())
            .unwrap()
            .replace("schema_version\":1", "schema_version\":2"),
        String::from_utf8(good.to_vec())
            .unwrap()
            .replace("aaaaaaaa", "AAAAAAAA"),
        String::from_utf8(good.to_vec())
            .unwrap()
            .replace("{", "{\"extra\":0,"),
        String::from_utf8(good.to_vec())
            .unwrap()
            .replace("{", "{\"purpose\":\"duplicate\","),
        " ".repeat(65537),
    ] {
        assert!(validate_policy_artifact(bad.as_bytes()).is_err(), "{bad}");
    }
}

use kairotensor::policy_registry::*;
use serde_json::Value;
use std::process::Command;
fn fixture() -> PolicyEvaluation {
    let output = Command::new("python3").args(["-c", "import sys,json;sys.path.insert(0,'scripts');from test_policy_evaluation_v2 import fixture;print(json.dumps(fixture()))"]).output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn synthetic_selection_uses_all_cases_and_original_deadline() {
    let e = fixture();
    match evaluate_policy_comparison(&e).unwrap() {
        SelectionDecision::Eligible {
            validated_counters: c,
        } => {
            assert_eq!(
                (
                    c.baseline.cases,
                    c.baseline.success,
                    c.baseline.false_refusal,
                    c.baseline.cost
                ),
                (1, 0, 1, 4)
            );
            assert_eq!(
                (
                    c.candidate.cases,
                    c.candidate.success,
                    c.candidate.false_refusal,
                    c.candidate.cost,
                    c.candidate.deadline_misses
                ),
                (1, 1, 0, 4, 0)
            );
        }
        other => panic!("{other:?}"),
    }
}
#[test]
fn malformed_unknown_incomparable_and_fabricated_records() {
    let e = fixture();
    for key in [
        "actor",
        "scope",
        "purpose",
        "evaluator_version",
        "evaluator_sha256",
        "experiment_sha256",
        "baseline_sha256",
        "candidate_sha256",
    ] {
        let mut v = serde_json::to_value(&e).unwrap();
        v[key] = Value::String(if key.ends_with("sha256") {
            "0".repeat(64)
        } else {
            "other".into()
        });
        let changed: PolicyEvaluation = serde_json::from_value(v).unwrap();
        assert_eq!(
            evaluate_policy_comparison(&changed).unwrap(),
            SelectionDecision::Refused(SelectionRefusal::Incomparable),
            "{key}"
        );
    }
    for key in ["correct", "eligible", "completed_ms", "charged"] {
        let mut v = serde_json::to_value(&e).unwrap();
        v["candidate_rows"][0][key] = Value::Null;
        let changed = serde_json::from_value(v).unwrap();
        assert_eq!(
            evaluate_policy_comparison(&changed).unwrap(),
            SelectionDecision::Refused(SelectionRefusal::Incomplete)
        );
    }
    for key in [
        "answerable",
        "eligible",
        "false_refusal",
        "unsafe_acceptance",
        "deadline_miss",
    ] {
        let mut v = serde_json::to_value(&e).unwrap();
        let b = v["candidate_rows"][0][key].as_bool().unwrap();
        v["candidate_rows"][0][key] = Value::Bool(!b);
        let changed = serde_json::from_value(v).unwrap();
        assert!(evaluate_policy_comparison(&changed).is_err(), "{key}");
    }
    let mut changed = e.clone();
    changed.candidate_rows[0].charged = Some(u64::MAX);
    changed.candidate_rows[0].reserved_bound = 1;
    assert!(evaluate_policy_comparison(&changed).is_err());
    let mut changed = e.clone();
    changed.candidate_rows.clear();
    assert_eq!(
        evaluate_policy_comparison(&changed).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::Incomplete)
    );
    let mut changed = e.clone();
    changed.candidate_rows[0].case_id = "other".into();
    assert_eq!(
        evaluate_policy_comparison(&changed).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::Incomparable)
    );
    let raw = serde_json::to_vec(&e).unwrap();
    assert_eq!(parse_policy_evaluation(&raw).unwrap(), e);
    let bad = String::from_utf8(raw)
        .unwrap()
        .replacen('{', "{\"actor\":\"duplicate\",", 1);
    assert!(parse_policy_evaluation(bad.as_bytes()).is_err());
    let mut v = serde_json::to_value(&e).unwrap();
    v["candidate_rows"][0]
        .as_object_mut()
        .unwrap()
        .remove("charged");
    assert!(parse_policy_evaluation(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut changed = e.clone();
    changed.evaluation_id = "a".repeat(MAX_EVALUATION_BYTES);
    assert!(evaluate_policy_comparison(&changed).is_err());
    let mut changed = e.clone();
    changed.candidate_rows = vec![changed.candidate_rows[0].clone(); MAX_CASES + 1];
    assert!(evaluate_policy_comparison(&changed).is_err());
}
#[test]
fn no_improvement_and_no_cost_increase() {
    let mut e = fixture();
    e.candidate_rows = e.baseline_rows.clone();
    assert_eq!(
        evaluate_policy_comparison(&e).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::NoImprovement)
    );
    let mut e = fixture();
    e.candidate_rows[0].charged = Some(5);
    assert_eq!(
        evaluate_policy_comparison(&e).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::IncreasedCost)
    );
}
fn observed_fixture() -> PolicyEvaluation {
    let output=Command::new("python3").args(["-c","import sys,json;sys.path.insert(0,'scripts');from test_policy_evaluation_v2 import observed_fixture;print(json.dumps(observed_fixture()))"]).output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn observed_records_require_bound_references_and_recomputed_counters() {
    let e = observed_fixture();
    assert_eq!(
        evaluate_policy_comparison(&e).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::NoImprovement)
    );
    for key in [
        "opportunity_id",
        "request_sha256",
        "evidence_manifest_sha256",
        "validator_sha256",
        "scorer_sha256",
        "label_sha256",
    ] {
        let mut v = serde_json::to_value(&e).unwrap();
        v["candidate_rows"][0]["attempts"][0][key] = Value::String("0".repeat(64));
        let changed = serde_json::from_value(v).unwrap();
        assert_eq!(
            evaluate_policy_comparison(&changed).unwrap(),
            SelectionDecision::Refused(SelectionRefusal::Incomparable),
            "{key}"
        );
    }
    let mut changed = e.clone();
    changed.candidate_rows[0].attempts[0].scoring = None;
    assert_eq!(
        evaluate_policy_comparison(&changed).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::Incomplete)
    );
    let mut changed = e.clone();
    changed.candidate_rows[0].charged = Some(3);
    assert!(evaluate_policy_comparison(&changed).is_err());
    let mut changed = e.clone();
    changed.candidate_rows[0].attempts[0]
        .scoring
        .as_mut()
        .unwrap()
        .normalized = b"no".to_vec();
    changed.candidate_rows[0].correct = Some(false);
    changed.candidate_rows[0].unsafe_acceptance = true;
    assert_eq!(
        evaluate_policy_comparison(&changed).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::Incomparable)
    );
    changed.candidate_rows[0].attempts[0].execution_id = "independent".into();
    assert_eq!(
        evaluate_policy_comparison(&changed).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::NoImprovement)
    );
}
#[test]
fn unknown_usage_retains_full_quote_and_never_qualifies() {
    let mut e = observed_fixture();
    let r = &mut e.candidate_rows[0];
    let a = &mut r.attempts[0];
    a.execution_id = "uncertain".into();
    a.disposition = AttemptDisposition::Uncertain;
    a.measured_input = None;
    a.measured_output = None;
    a.retained_bound = 10;
    a.response = None;
    a.scoring = None;
    a.completed_ms = None;
    r.outcome = Outcome::Unknown;
    r.correct = None;
    r.charged = None;
    r.reserved_bound = 10;
    r.completed_ms = None;
    assert_eq!(
        evaluate_policy_comparison(&e).unwrap(),
        SelectionDecision::Refused(SelectionRefusal::Incomplete)
    );
    e.candidate_rows[0].attempts[0].retained_bound = 9;
    assert!(evaluate_policy_comparison(&e).is_err());
}
#[test]
fn baseline_projection_uses_exact_manifest_and_rows() {
    let e = fixture();
    let mut b = e.clone();
    b.evaluation_id = "baseline-only".into();
    assert!(paired_baseline_matches(&b, &e).unwrap());
    b.experiment_manifest.push(b' ');
    assert!(!paired_baseline_matches(&b, &e).unwrap());
    b = e.clone();
    b.baseline_rows[0].charged = Some(3);
    assert!(!paired_baseline_matches(&b, &e).unwrap());
}

#[test]
fn cross_language_golden_bytes_and_decisions() {
    let e = fixture();
    assert_eq!(
        evaluation_sha256(&e).unwrap(),
        "2b0ed900f956cd8fc2dd215e114a665d4799187dd6610209807fe557a1311a88"
    );
    assert_eq!(
        e.experiment_sha256,
        "1373dd5ff2bc8e1a06840f8feb31329b1a6666d434712db2a6fe4a3ef537c265"
    );
    let output=Command::new("python3").args(["-c","import sys,json;sys.path.insert(0,'scripts');from test_policy_evaluation_v2 import golden_cases;print(json.dumps(golden_cases()))"]).output().unwrap();
    assert!(output.status.success());
    let cases: Vec<(String, Value, String)> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(cases.len() >= 40);
    for (name, value, want) in cases {
        let actual = match parse_policy_evaluation(&serde_json::to_vec(&value).unwrap())
            .and_then(|e| evaluate_policy_comparison(&e))
        {
            Ok(SelectionDecision::Eligible { .. }) => "Eligible".into(),
            Ok(SelectionDecision::Refused(reason)) => format!("{reason:?}"),
            Err(_) => "Error".into(),
        };
        assert_eq!(actual, want, "{name}");
    }
}

#[test]
fn candidate_binds_exact_artifact_purpose_and_identity() {
    let e = fixture();
    let m: ComparisonManifest = serde_json::from_slice(&e.experiment_manifest).unwrap();
    let mut c = PolicyCandidate {
        version_id: "v1".into(),
        actor: e.actor,
        scope: e.scope,
        purpose: e.purpose,
        artifact: m.policies[0].artifact.bytes.clone(),
        sha256: m.policies[0].artifact.sha256.clone(),
    };
    assert_eq!(
        validate_policy_candidate(&c).unwrap().policy_name,
        PolicyName::AlwaysStop
    );
    c.purpose = "other".into();
    assert!(validate_policy_candidate(&c).is_err());
    c.purpose = "fixture/recovery-policy".into();
    c.sha256 = "0".repeat(64);
    assert!(validate_policy_candidate(&c).is_err());
    c.sha256 = m.policies[0].artifact.sha256.clone();
    c.actor.clear();
    assert!(validate_policy_candidate(&c).is_err());
}
