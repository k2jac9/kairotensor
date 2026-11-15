use kairotensor::decision_bundle::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn digest(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn bytes(raw: &[u8]) -> Value {
    json!({"hex":raw.iter().map(|b|format!("{b:02x}")).collect::<String>(),"byte_len":raw.len(),"sha256":digest(raw)})
}
fn seal(mut value: Value) -> Vec<u8> {
    value["integrity"].as_object_mut().unwrap().remove("sha256");
    let hash = digest(&serde_json::to_vec(&value).unwrap());
    value["integrity"]["sha256"] = hash.into();
    serde_json::to_vec(&value).unwrap()
}
fn fixture() -> Value {
    json!({
        "schema_version":1,"root_attempt":1,"actor":"a","scope":"s","export_purpose":"diagnostic",
        "snapshot":{"root_branch":1,"head":1,"revision":1,"current_epoch":0},
        "attempts":[{"id":1,"branch":1,"actor":"a","scope":"s","captured_epoch":0,"captured_revision":0,
        "parent":null,"inherited_from":null,"candidates":[{"position":0,"evidence_id":1}],
        "inputs":[{"position":0,"evidence_id":1,"role":"unknown"}],"dispatch_request":null,
        "delivery":{"state":"recorded","raw":bytes(b"\xff\0"),"disposition":"accepted","usage":0,"rejection":null},
        "validator":null,"immutable_runtime":null,"immutable_manifest":null,"timing_configuration":null,
        "accounting":{"unit":"unknown","quote":null,"granted":null,"budget_reference":null,"charge":0,"hold_bound":0},
        "reuse_origin":null,"model_contract":null,"deterministic":null,"support":null,"recovery":null}],
        "evidence":[{"occurrence_id":1,"source":bytes(b"\xfe"),"payload":bytes(b"\xff\0"),"authorized_scopes":[{"actor":"a","scope":"s"}]}],
        "integrity":{"version":"decision-bundle-integrity/1","sha256":""}
    })
}
fn parsed(v: Value) -> DecisionBundleV1 {
    parse_decision_bundle(&seal(v)).unwrap()
}
fn report_fixture() -> Value {
    let mut v = fixture();
    v["evidence"][0]["payload"] = bytes(br#"{"release":"r1","passed":2,"total":2}"#);
    let a = &mut v["attempts"][0];
    a["inputs"][0]["role"] = "report".into();
    a["delivery"]["raw"] = bytes(br#"{"release":"r1","tests":"2/2","status":"pass"}"#);
    a["deterministic"] = json!({"version":"report-verdict/v1","output":a["delivery"]["raw"]});
    a["validator"] = json!({"version":"report-verdict/v1","provenance":"deterministic_binding"});
    a["accounting"]["unit"] = "logical_support".into();
    v
}
#[test]
fn missing_unknown_duplicate_and_wrong_schema_refuse() {
    assert!(matches!(
        parse_decision_bundle(b"{}"),
        Err(BundleError::InvalidSchema)
    ));
    for change in [json!(2), json!("1"), json!(-1)] {
        let mut v = fixture();
        v["schema_version"] = change;
        assert!(parse_decision_bundle(&seal(v)).is_err());
    }
    let mut v = fixture();
    v["extra"] = true.into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let raw = String::from_utf8(seal(fixture())).unwrap().replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    assert!(parse_decision_bundle(raw.as_bytes()).is_err());
    let mut v = fixture();
    v["attempts"][0].as_object_mut().unwrap().remove("parent");
    assert!(parse_decision_bundle(&seal(v)).is_err());
}
#[test]
fn non_utf8_bytes_round_trip_without_reencoding_and_legacy_unknowns() {
    let b = parsed(fixture());
    assert_eq!(b.evidence[0].payload.decode().unwrap(), [255, 0]);
    let r = validate_recorded_bundle(&b);
    assert_eq!(r.consistency, ReplayStatus::Matched);
    assert_eq!(r.validation, ReplayStatus::NotReproducible);
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::ValidatorUnavailable));
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::HistoricalTimingUnavailable));
    assert_eq!(r.recorded_disposition, Some(RecordedDisposition::Accepted));
    assert_eq!(r.source_bundle_sha256, Some(digest(&seal(fixture()))));
    assert_eq!(parse_decision_bundle(&b.to_json().unwrap()).unwrap(), b);
}
#[test]
fn altered_raw_or_outer_digest_refuses() {
    let mut v = fixture();
    v["evidence"][0]["payload"]["hex"] = "fe00".into();
    assert!(matches!(
        parse_decision_bundle(&seal(v)),
        Err(BundleError::InconsistentLedger)
    ));
    let mut v: Value = serde_json::from_slice(&seal(fixture())).unwrap();
    v["actor"] = "other".into();
    assert!(matches!(
        parse_decision_bundle(&serde_json::to_vec(&v).unwrap()),
        Err(BundleError::InconsistentLedger)
    ));
    let mut v = fixture();
    v["evidence"][0]["payload"]["hex"] = "FF00".into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
}
#[test]
fn occurrence_identity_and_repeated_occurrences_at_distinct_positions_survive() {
    let mut v = fixture();
    let mut e = v["evidence"][0].clone();
    e["occurrence_id"] = 2.into();
    v["evidence"].as_array_mut().unwrap().push(e);
    v["attempts"][0]["inputs"] = json!([{"position":0,"evidence_id":1,"role":"unknown"},{"position":1,"evidence_id":2,"role":"unknown"},{"position":2,"evidence_id":1,"role":"unknown"}]);
    v["attempts"][0]["candidates"] = json!([{"position":0,"evidence_id":1},{"position":1,"evidence_id":2},{"position":2,"evidence_id":1}]);
    let b = parsed(v.clone());
    assert_eq!(
        b.attempts[0]
            .inputs
            .iter()
            .map(|i| i.evidence_id)
            .collect::<Vec<_>>(),
        [1, 2, 1]
    );
    v["attempts"][0]["inputs"][2]["position"] = 1.into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let mut v = fixture();
    let e = v["evidence"][0].clone();
    v["evidence"].as_array_mut().unwrap().push(e);
    assert!(parse_decision_bundle(&seal(v)).is_err());
}
#[test]
fn json_and_constructed_values_are_bounded() {
    assert!(matches!(
        parse_decision_bundle(&vec![b' '; 16 * 1024 * 1024 + 1]),
        Err(BundleError::LimitExceeded)
    ));
    let nested = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    assert!(matches!(
        parse_decision_bundle(nested.as_bytes()),
        Err(BundleError::LimitExceeded)
    ));
    assert!(matches!(
        parse_decision_bundle(&[255]),
        Err(BundleError::InvalidEncoding)
    ));
    let mut b = parsed(fixture());
    b.evidence[0].payload.byte_len = 4 * 1024 * 1024 + 1;
    assert_eq!(
        validate_recorded_bundle(&b).consistency,
        ReplayStatus::Mismatch
    );
    let mut b = parsed(fixture());
    b.actor = "x".repeat(16 * 1024 * 1024 + 1);
    assert!(matches!(b.to_json(), Err(BundleError::LimitExceeded)));
    assert_eq!(validate_recorded_bundle(&b).source_bundle_sha256, None);
}
#[test]
fn accounting_rejects_inconsistent_usage_quote_and_holds() {
    for (usage, charge, hold, granted) in [
        (3, 2, 0, true),
        (1, 1, 0, true),
        (9, 9, 0, true),
        (3, 3, 1, true),
        (3, 3, 0, false),
    ] {
        let mut v = fixture();
        let a = &mut v["attempts"][0];
        a["delivery"]["usage"] = usage.into();
        a["accounting"] = json!({"unit":"unknown","quote":{"input_tokens":2,"output_cap":4},"granted":granted,"budget_reference":1,"charge":charge,"hold_bound":hold});
        assert!(parse_decision_bundle(&seal(v)).is_err());
    }
    let mut v = fixture();
    v["attempts"][0]["accounting"]["quote"] = json!({"input_tokens":i64::MAX,"output_cap":1});
    assert!(parse_decision_bundle(&seal(v)).is_err());
}
#[test]
fn unresolved_delivery_retains_hold_and_unknown_usage() {
    let mut v = fixture();
    let a = &mut v["attempts"][0];
    a["delivery"] = json!({"state":"unresolved"});
    a["accounting"] = json!({"unit":"unknown","quote":{"input_tokens":2,"output_cap":4},"granted":true,"budget_reference":1,"charge":0,"hold_bound":6});
    let b = parsed(v);
    let r = validate_recorded_bundle(&b);
    assert_eq!(r.consistency, ReplayStatus::Matched);
    assert_eq!(r.validation, ReplayStatus::Incomplete);
    assert_eq!(r.accounting[0].held, 6);
    assert_eq!(r.accounting[0].charged, 0);
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::MissingDelivery));
}
#[test]
fn deterministic_replay_and_changed_validator_preserve_original_disposition() {
    let b = parsed(report_fixture());
    let r = validate_recorded_bundle(&b);
    assert_eq!(r.reconstruction, ReplayStatus::Matched);
    assert_eq!(r.validation, ReplayStatus::Matched);
    let changed = reevaluate_bundle(&b, "report-verdict/v2");
    assert_eq!(changed.validation, ReplayStatus::NotReproducible);
    assert_eq!(
        changed.reevaluated_validator.as_deref(),
        Some("report-verdict/v2")
    );
    assert_eq!(changed.source_bundle_sha256, r.source_bundle_sha256);
    assert_eq!(changed.recorded_disposition, r.recorded_disposition);
    let mut v = report_fixture();
    v["attempts"][0]["deterministic"]["output"] = bytes(b"wrong");
    v["attempts"][0]["delivery"]["raw"] = bytes(b"wrong");
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.consistency, ReplayStatus::Matched);
    assert_eq!(r.reconstruction, ReplayStatus::Mismatch);
}
#[test]
fn missing_data_dependencies_are_incomplete_but_unconsumed_parent_is_only_lineage() {
    let mut v = fixture();
    v["attempts"][0]["parent"] = 2.into();
    let b = parsed(v.clone());
    assert_eq!(
        validate_recorded_bundle(&b).consistency,
        ReplayStatus::Matched
    );
    v["attempts"][0]["inherited_from"] = 2.into();
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::Incomplete);
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::MissingDependency));
    let mut v = fixture();
    v["attempts"][0]["reuse_origin"] = 2.into();
    v["attempts"][0]["model_contract"] = "legacy".into();
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::Incomplete);
    let mut v = report_fixture();
    v["evidence"] = json!([]);
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.validation, ReplayStatus::Incomplete);
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::MissingInput));
}

#[test]
fn legacy_filtered_candidates_and_exact_inputs_are_independent_positions() {
    let mut v = fixture();
    v["attempts"][0]["candidates"] =
        json!([{"position":0,"evidence_id":99},{"position":1,"evidence_id":1}]);
    assert!(parse_decision_bundle(&seal(v)).is_ok());
    let mut v = fixture();
    v["attempts"][0]["candidates"] = json!([]);
    assert!(parse_decision_bundle(&seal(v)).is_ok());
}
#[test]
fn cancelled_unsent_recovery_releases_granted_hold() {
    let mut v = fixture();
    let a = &mut v["attempts"][0];
    a["delivery"] = json!({"state":"unresolved"});
    a["recovery"] = json!({"recovery_id":1,"ordinal":0,"state":"cancelled","predecessor_id":null,"request":bytes(b"request")});
    a["accounting"] = json!({"unit":"unknown","quote":{"input_tokens":2,"output_cap":4},"granted":true,"budget_reference":1,"charge":0,"hold_bound":0});
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.accounting[0].held, 0);
}
#[test]
fn shared_occurrence_requires_each_consuming_attempt_scope() {
    let mut v = fixture();
    let mut dependency = v["attempts"][0].clone();
    dependency["id"] = 2.into();
    dependency["branch"] = 2.into();
    dependency["scope"] = "foreign".into();
    v["attempts"][0]["parent"] = 2.into();
    v["attempts"][0]["inherited_from"] = 2.into();
    v["attempts"].as_array_mut().unwrap().push(dependency);
    v["evidence"][0]["authorized_scopes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"actor":"a","scope":"foreign"}));
    assert!(parse_decision_bundle(&seal(v.clone())).is_ok());
    v["evidence"][0]["authorized_scopes"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let mut v = fixture();
    v["attempts"][0]["actor"] = "wrong".into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let mut v = fixture();
    v["evidence"][0]["authorized_scopes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"actor":"a","scope":"s"}));
    assert!(parse_decision_bundle(&seal(v)).is_err());
}
#[test]
fn descriptive_roles_and_validator_provenance_cannot_be_invented() {
    let mut v = fixture();
    v["attempts"][0]["inputs"][0]["role"] = "report".into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let mut v = fixture();
    v["attempts"][0]["validator"] =
        json!({"version":"report-verdict/v1","provenance":"deterministic_binding"});
    assert!(parse_decision_bundle(&seal(v)).is_err());
}

fn incident_fixture() -> Value {
    let context = json!({"schema_version":1,"version":"incident-store-context/1","workload_version":"incident-routing/1","freeze_sha256":"f".repeat(64),"prompt_sha256":"e".repeat(64),"validator_version":"incident-routing-protocol/1","case_id":"c1","question":"Where?","owner_ids":["network"],"evidence_ids":["r1"],"system_prompt":"Route."});
    let context_raw = serde_json::to_vec(&context).unwrap();
    let report_raw = b"network lost";
    let runtime = json!({"cpu_fingerprint":"c".repeat(64),"decoding":"canned-no-inference","dtype":"bytes","model":"canned-no-model","model_revision":"fixture/1","readiness":"warm","runtime_version":"incident-canned/1","threads":1,"tokenizer_revision":"utf8-bytes/1"});
    let manifest = json!({"context":{"occurrence_id":1,"sha256":digest(&context_raw)},"freeze_sha256":context["freeze_sha256"],"initial_renderer":"incident-request/1","output_cap":512,"prompt_sha256":context["prompt_sha256"],"retry_feedback":"incident-routing-protocol/1:invalid_output","retry_renderer":"incident-request-retry/1","runtime":runtime,"schema_version":1,"sources":[{"case_id":"c1","external_id":"r1","occurrence_id":2,"payload_sha256":digest(report_raw),"workload_version":"incident-routing/1"}],"validator_version":"incident-routing-protocol/1","version":"incident-store-manifest/1","workload_version":"incident-routing/1"});
    let manifest_raw = serde_json::to_vec(&manifest).unwrap();
    let mut runtime = runtime;
    runtime["prompt_fingerprint"] = digest(&manifest_raw).into();
    let timing = json!({"clock_domain":"synthetic","start_ms":0,"deadline_ms":1000,"runtime":runtime,"calibration":{"id":"fixture","max_duration_ms":5,"sample_count":1,"max_input_tokens":1024,"output_cap":512,"measured_at_ms":0,"expires_at_ms":1001},"safety_margin_ms":1});
    // Independent literal oracle: no call to the Rust incident renderer.
    let request=br#"{"messages":[{"content":"Route.","role":"system"},{"content":"{\"evidence\":[{\"id\":\"r1\",\"text\":\"network lost\"}],\"owner_ids\":[\"network\"],\"question\":\"Where?\"}","role":"user"}]}"#;
    let mut v = fixture();
    v["evidence"][0]["payload"] = bytes(&context_raw);
    let mut e = v["evidence"][0].clone();
    e["occurrence_id"] = 2.into();
    e["payload"] = bytes(report_raw);
    v["evidence"].as_array_mut().unwrap().push(e);
    let a = &mut v["attempts"][0];
    a["inputs"] = json!([{"position":0,"evidence_id":1,"role":"context"},{"position":1,"evidence_id":2,"role":"report"}]);
    a["candidates"] = json!([{"position":0,"evidence_id":1},{"position":1,"evidence_id":2}]);
    a["dispatch_request"] = bytes(request);
    a["delivery"]["raw"]=bytes(br#"{"decision":"route","owner":"network","reason":"diagnosed_component","evidence_ids":["r1"]}"#);
    a["validator"] =
        json!({"version":"incident-routing-protocol/1","provenance":"declared_contract"});
    a["immutable_runtime"] = bytes(&serde_json::to_vec(&runtime).unwrap());
    a["immutable_manifest"] = bytes(&manifest_raw);
    a["timing_configuration"] = bytes(&serde_json::to_vec(&timing).unwrap());
    a["recovery"] = json!({"recovery_id":1,"ordinal":0,"state":"sent","predecessor_id":null,"request":bytes(request)});
    let usage = request.len() as i64 + a["delivery"]["raw"]["byte_len"].as_i64().unwrap();
    a["delivery"]["usage"] = usage.into();
    a["accounting"] = json!({"unit":"unknown","quote":{"input_tokens":request.len(),"output_cap":512},"granted":true,"budget_reference":1,"charge":usage,"hold_bound":0});
    v
}
fn retry_fixture() -> Value {
    let mut v = incident_fixture();
    let mut next = v["attempts"][0].clone();
    let retry=br#"{"messages":[{"content":"Route.","role":"system"},{"content":"{\"evidence\":[{\"id\":\"r1\",\"text\":\"network lost\"}],\"owner_ids\":[\"network\"],\"question\":\"Where?\",\"validation_error\":\"incident-routing-protocol/1:invalid_output\"}","role":"user"}]}"#;
    next["id"] = 2.into();
    next["dispatch_request"] = bytes(retry);
    next["recovery"]["ordinal"] = 1.into();
    next["recovery"]["predecessor_id"] = 1.into();
    next["recovery"]["request"] = bytes(retry);
    let usage = retry.len() as i64 + next["delivery"]["raw"]["byte_len"].as_i64().unwrap();
    next["delivery"]["usage"] = usage.into();
    next["accounting"]["charge"] = usage.into();
    next["accounting"]["quote"]["input_tokens"] = retry.len().into();
    let first_usage = v["attempts"][0]["accounting"]["quote"]["input_tokens"]
        .as_i64()
        .unwrap()
        + 7;
    v["attempts"][0]["delivery"]["usage"] = first_usage.into();
    v["attempts"][0]["accounting"]["charge"] = first_usage.into();
    v["attempts"][0]["delivery"]["raw"] = bytes(b"invalid");
    v["attempts"][0]["delivery"]["disposition"] = "invalid_output".into();
    v["attempts"][0]["delivery"]["rejection"] = json!({"validator":"incident-routing-protocol/1","reason":"free text must never enter retry request","applicability":"current"});
    v["attempts"].as_array_mut().unwrap().push(next);
    v["root_attempt"] = 2.into();
    v["snapshot"]["head"] = 2.into();
    v
}
#[test]
fn incident_manifest_initial_and_retry_match_exact_oracles() {
    for v in [incident_fixture(), retry_fixture()] {
        let r = validate_recorded_bundle(&parsed(v));
        assert_eq!(r.consistency, ReplayStatus::Matched);
        assert_eq!(r.reconstruction, ReplayStatus::Matched);
        assert_eq!(r.validation, ReplayStatus::Matched);
        assert_eq!(
            r.validator_provenance,
            Some(ValidatorProvenance::DeclaredContract)
        );
    }
    let r = validate_recorded_bundle(&parsed(retry_fixture()));
    assert_eq!(
        r.attempts[0].validator_provenance,
        Some(ValidatorProvenance::RecordedRejection)
    );
    assert_eq!(
        r.attempts[0].recorded_disposition,
        Some(RecordedDisposition::InvalidOutput)
    );
}
#[test]
fn incident_changed_request_order_runtime_and_unknown_phase_do_not_fallback() {
    let mut v = incident_fixture();
    v["attempts"][0]["dispatch_request"] = bytes(b"wrong");
    v["attempts"][0]["recovery"]["request"] = bytes(b"wrong");
    assert_eq!(
        validate_recorded_bundle(&parsed(v)).reconstruction,
        ReplayStatus::Mismatch
    );
    let mut v = incident_fixture();
    v["attempts"][0]["immutable_runtime"] = bytes(b"{}");
    assert_eq!(
        validate_recorded_bundle(&parsed(v)).reconstruction,
        ReplayStatus::Mismatch
    );
    let mut v = incident_fixture();
    v["attempts"][0]["recovery"]["ordinal"] = 2.into();
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::NotReproducible);
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::RendererUnavailable));
    let mut v = incident_fixture();
    v["attempts"][0]["candidates"][0]["evidence_id"] = 2.into();
    v["attempts"][0]["candidates"][1]["evidence_id"] = 1.into();
    assert_eq!(
        validate_recorded_bundle(&parsed(v)).reconstruction,
        ReplayStatus::Mismatch
    );
    let mut v = retry_fixture();
    v["attempts"].as_array_mut().unwrap().remove(0);
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::Incomplete);
}
#[test]
fn protocol_rejection_matches_but_new_evaluation_reports_invalid() {
    let mut v = retry_fixture();
    v["attempts"].as_array_mut().unwrap().pop();
    v["root_attempt"] = 1.into();
    v["snapshot"]["head"] = 1.into();
    let b = parsed(v);
    assert_eq!(
        validate_recorded_bundle(&b).validation,
        ReplayStatus::Matched
    );
    let r = reevaluate_bundle(&b, "incident-routing-protocol/1");
    assert_eq!(r.validation, ReplayStatus::Mismatch);
    assert_eq!(
        r.recorded_disposition,
        Some(RecordedDisposition::InvalidOutput)
    );
}
#[test]
fn typed_schema_rejects_nested_duplicates_and_record_count_overflow() {
    let raw = String::from_utf8(seal(fixture())).unwrap();
    for (find, replace) in [
        ("\"usage\":0", "\"usage\":0,\"usage\":0"),
        (
            "\"state\":\"recorded\"",
            "\"state\":\"recorded\",\"state\":\"recorded\"",
        ),
        ("\"position\":0", "\"position\":0,\"position\":0"),
    ] {
        assert!(parse_decision_bundle(raw.replacen(find, replace, 1).as_bytes()).is_err());
    }
    let mut b = parsed(fixture());
    let a = b.attempts[0].clone();
    b.attempts = vec![a; 129];
    assert!(matches!(b.to_json(), Err(BundleError::LimitExceeded)));
    assert!(ByteEnvelope::from_bytes(&vec![0; 4 * 1024 * 1024 + 1]).is_err());
}

#[test]
fn dependency_cycles_are_incomplete_and_support_rule_owner_is_not_an_output_dependency() {
    let mut v = fixture();
    v["attempts"][0]["parent"] = 2.into();
    v["attempts"][0]["inherited_from"] = 2.into();
    let mut d = v["attempts"][0].clone();
    d["id"] = 2.into();
    d["parent"] = 1.into();
    d["inherited_from"] = 1.into();
    v["attempts"].as_array_mut().unwrap().push(d);
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::Incomplete);
    assert!(r
        .missing_requirements
        .contains(&MissingRequirement::MissingDependency));
    let mut v = fixture();
    v["attempts"][0]["support"] = json!({"version":"support/v1","rule_id":999,"verdict":true,"clauses":[{"clause_id":1,"premises":[{"evidence_id":1,"actor":"a","scope":"s"}]}]});
    v["attempts"][0]["inputs"][0]["role"] = "evidence".into();
    let r = validate_recorded_bundle(&parsed(v));
    assert!(!r
        .missing_requirements
        .contains(&MissingRequirement::MissingDependency));
}
#[test]
fn fixture_units_require_recognized_runtime_and_deterministic_units_require_binding() {
    let mut v = fixture();
    v["attempts"][0]["accounting"]["unit"] = "logical_support".into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let mut v = incident_fixture();
    v["attempts"][0]["accounting"]["unit"] = "fixture_bytes".into();
    assert!(parse_decision_bundle(&seal(v.clone())).is_ok());
    v["attempts"][0]["immutable_runtime"] = bytes(b"{}");
    assert!(parse_decision_bundle(&seal(v)).is_err());
    let mut v = fixture();
    v["attempts"][0]["accounting"]["unit"] = "model_tokens".into();
    assert!(parse_decision_bundle(&seal(v)).is_err());
}

#[test]
fn invalid_deterministic_input_is_a_validator_mismatch_not_an_unavailable_validator() {
    let mut v = report_fixture();
    v["evidence"][0]["payload"] = bytes(b"invalid report");
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::Mismatch);
    assert_eq!(r.validation, ReplayStatus::Mismatch);
    assert!(!r
        .missing_requirements
        .contains(&MissingRequirement::ValidatorUnavailable));
}
#[test]
fn reserved_unsent_incident_reconstructs_recorded_preparation_without_claiming_delivery() {
    let mut v = incident_fixture();
    let a = &mut v["attempts"][0];
    a["dispatch_request"] = Value::Null;
    a["delivery"] = json!({"state":"unresolved"});
    a["recovery"]["state"] = "reserved".into();
    a["accounting"] = json!({"unit":"unknown","quote":{"input_tokens":2,"output_cap":512},"granted":true,"budget_reference":1,"charge":0,"hold_bound":514});
    let r = validate_recorded_bundle(&parsed(v));
    assert_eq!(r.reconstruction, ReplayStatus::Matched);
    assert_eq!(r.validation, ReplayStatus::Incomplete);
    assert_eq!(r.accounting[0].held, 514);
}

#[test]
fn recovery_state_and_reservation_cannot_contradict_each_other() {
    for state in ["denied", "reserved", "planned", "cancelled"] {
        let mut v = incident_fixture();
        v["attempts"][0]["recovery"]["state"] = state.into();
        assert!(parse_decision_bundle(&seal(v)).is_err());
    }
    let mut v = incident_fixture();
    let a = &mut v["attempts"][0];
    a["accounting"]["quote"] = Value::Null;
    a["accounting"]["granted"] = Value::Null;
    a["accounting"]["budget_reference"] = Value::Null;
    assert!(parse_decision_bundle(&seal(v)).is_err());
}

#[test]
fn reuse_delivery_must_equal_its_explicit_dispatched_origin() {
    let mut v = fixture();
    let mut origin = v["attempts"][0].clone();
    origin["dispatch_request"] = bytes(b"request");
    origin["model_contract"] = "legacy".into();
    v["attempts"][0]["id"] = 2.into();
    v["attempts"][0]["reuse_origin"] = 1.into();
    v["attempts"][0]["model_contract"] = "legacy".into();
    v["root_attempt"] = 2.into();
    v["snapshot"]["head"] = 2.into();
    v["attempts"].as_array_mut().unwrap().push(origin);
    assert!(parse_decision_bundle(&seal(v.clone())).is_ok());
    v["attempts"][0]["delivery"]["raw"] = bytes(b"invented");
    assert!(parse_decision_bundle(&seal(v)).is_err());
}

// Frozen Python json/hashlib oracle, shared verbatim with the v1 format document.
const GOLDEN: &str = r#"{"actor":"é","attempts":[{"accounting":{"budget_reference":null,"charge":0,"granted":null,"hold_bound":0,"quote":null,"unit":"unknown"},"actor":"é","branch":1,"candidates":[],"captured_epoch":0,"captured_revision":0,"delivery":{"state":"unresolved"},"deterministic":null,"dispatch_request":null,"id":1,"immutable_manifest":null,"immutable_runtime":null,"inherited_from":null,"inputs":[],"model_contract":null,"parent":null,"recovery":null,"reuse_origin":null,"scope":"s","support":null,"timing_configuration":null,"validator":null}],"evidence":[],"export_purpose":"golden","integrity":{"sha256":"765fbaa97312a42a4e5da7022d6350efd2fb4811767bf763f327f6ddf0832093","version":"decision-bundle-integrity/1"},"root_attempt":1,"schema_version":1,"scope":"s","snapshot":{"current_epoch":0,"head":null,"revision":0,"root_branch":1}}"#;
#[test]
fn canonical_golden_bytes_digest_and_transport_whitespace_are_distinct() {
    let bundle = parse_decision_bundle(GOLDEN.as_bytes()).unwrap();
    assert_eq!(bundle.to_json().unwrap(), GOLDEN.as_bytes());
    let report = validate_recorded_bundle(&bundle);
    assert_eq!(
        report.source_bundle_sha256.as_deref(),
        Some("569b219c44295bb8e1a42358b1bbbd66c153ff379694dd55ef5decf229dbe471")
    );
    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_str::<Value>(GOLDEN).unwrap()).unwrap();
    let again = parse_decision_bundle(&pretty).unwrap();
    assert_eq!(
        validate_recorded_bundle(&again).source_bundle_sha256,
        report.source_bundle_sha256
    );
    assert_ne!(digest(&pretty), report.source_bundle_sha256.unwrap());
    let mut changed = bundle.clone();
    changed.export_purpose = "changed".into();
    changed.seal().unwrap();
    assert_ne!(changed.integrity.sha256, bundle.integrity.sha256);
    assert!(parse_decision_bundle(&changed.to_json().unwrap()).is_ok());
}

#[test]
fn unknown_manifest_or_renderer_version_never_falls_back() {
    for field in ["version", "initial_renderer", "retry_renderer"] {
        let mut bundle = parsed(incident_fixture());
        let raw = bundle.attempts[0]
            .immutable_manifest
            .as_ref()
            .unwrap()
            .decode()
            .unwrap();
        let mut manifest: Value = serde_json::from_slice(&raw).unwrap();
        manifest[field] = "unknown/2".into();
        bundle.attempts[0].immutable_manifest =
            Some(ByteEnvelope::from_bytes(&serde_json::to_vec(&manifest).unwrap()).unwrap());
        if field == "version" {
            for input in &mut bundle.attempts[0].inputs {
                input.role = InputRole::Unknown;
            }
        }
        bundle.seal().unwrap();
        let report = validate_recorded_bundle(&bundle);
        assert_eq!(report.reconstruction, ReplayStatus::NotReproducible);
        assert!(report
            .missing_requirements
            .contains(&MissingRequirement::RendererUnavailable));
    }
}
