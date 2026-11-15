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

#[test]
fn sent_timed_quote_cap_must_equal_immutable_calibration_after_rehashing() {
    let mut value = incident_fixture();
    value["attempts"][0]["accounting"]["quote"]["output_cap"] = 513.into();
    assert!(matches!(
        parse_decision_bundle(&seal(value)),
        Err(BundleError::InconsistentLedger)
    ));
}

#[test]
fn sent_timed_quote_input_must_be_covered_even_with_consistent_usage() {
    let mut value = incident_fixture();
    let attempt = &mut value["attempts"][0];
    attempt["accounting"]["quote"]["input_tokens"] = 1025.into();
    attempt["accounting"]["charge"] = 1026.into();
    attempt["delivery"]["usage"] = 1026.into();
    assert!(matches!(
        parse_decision_bundle(&seal(value)),
        Err(BundleError::InconsistentLedger)
    ));
}

#[test]
fn unsent_timed_refusal_keeps_uncovered_reservation_facts() {
    for state in ["reserved", "denied", "cancelled"] {
        let mut value = incident_fixture();
        let attempt = &mut value["attempts"][0];
        attempt["recovery"]["state"] = state.into();
        attempt["dispatch_request"] = Value::Null;
        attempt["delivery"] = json!({"state":"unresolved"});
        attempt["accounting"] = json!({"unit":"unknown","quote":{"input_tokens":1025,"output_cap":513},"granted":state != "denied","budget_reference":1,"charge":0,"hold_bound":if state == "reserved" {1538} else {0}});
        let report = validate_recorded_bundle(&parsed(value));
        assert_eq!(report.consistency, ReplayStatus::Matched, "{state}");
        assert_eq!(report.reconstruction, ReplayStatus::Matched, "{state}");
        assert_eq!(report.validation, ReplayStatus::Incomplete, "{state}");
        assert_eq!(
            report.accounting[0].held,
            if state == "reserved" { 1538 } else { 0 }
        );
    }
}

#[test]
fn recovery_composite_key_is_unique_across_distinct_attempt_ids() {
    let mut value = incident_fixture();
    let mut duplicate = value["attempts"][0].clone();
    duplicate["id"] = 2.into();
    value["attempts"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        parse_decision_bundle(&seal(value)),
        Err(BundleError::InconsistentLedger)
    ));
}

#[test]
fn included_recovery_phases_share_immutable_timing_and_contract_metadata() {
    for (field, key, replacement) in [
        ("timing_configuration", "deadline_ms", json!(1002)),
        ("immutable_runtime", "threads", json!(2)),
        ("immutable_manifest", "output_cap", json!(513)),
    ] {
        let mut value = retry_fixture();
        let envelope: ByteEnvelope =
            serde_json::from_value(value["attempts"][1][field].clone()).unwrap();
        let mut document: Value = serde_json::from_slice(&envelope.decode().unwrap()).unwrap();
        document[key] = replacement;
        value["attempts"][1][field] = bytes(&serde_json::to_vec(&document).unwrap());
        assert!(
            matches!(
                parse_decision_bundle(&seal(value)),
                Err(BundleError::InconsistentLedger)
            ),
            "{field}"
        );
    }
}

#[test]
fn included_recovery_phases_cannot_name_different_known_budgets() {
    let mut value = retry_fixture();
    value["attempts"][1]["accounting"]["budget_reference"] = 2.into();
    assert!(matches!(
        parse_decision_bundle(&seal(value)),
        Err(BundleError::InconsistentLedger)
    ));
}

#[test]
fn planned_recovery_phase_preserves_absent_per_attempt_budget_and_quote() {
    let mut value = retry_fixture();
    let planned = &mut value["attempts"][1];
    planned["recovery"]["state"] = "planned".into();
    planned["recovery"]["request"] = Value::Null;
    planned["dispatch_request"] = Value::Null;
    planned["delivery"] = json!({"state":"unresolved"});
    planned["accounting"] = json!({"unit":"unknown","quote":null,"granted":null,"budget_reference":null,"charge":0,"hold_bound":0});
    for reverse in [false, true] {
        let mut ordered = value.clone();
        if reverse {
            ordered["attempts"].as_array_mut().unwrap().reverse();
        }
        let bundle = parsed(ordered);
        let planned = bundle.attempts.iter().find(|a| a.id == 2).unwrap();
        assert_eq!(planned.accounting.quote, None);
        assert_eq!(planned.accounting.budget_reference, None);
        assert_eq!(
            validate_recorded_bundle(&bundle).consistency,
            ReplayStatus::Matched
        );
    }
}

// Store export authority tests: removing either the diagnostic authorization or
// current admission gate must expose the canaries and fail these tests.
fn export_request(attempt: i64, purpose: BundlePurpose) -> BundleRequest {
    BundleRequest {
        actor: "a".into(),
        scope: "s".into(),
        attempt,
        purpose,
    }
}
struct AllowExport;
impl DiagnosticAuthorizer for AllowExport {
    fn authorize(&self, _: &DiagnosticReadRequest) -> Result<bool, BundleError> {
        Ok(true)
    }
}
fn complete(export: BundleExport) -> DecisionBundleV1 {
    match export {
        BundleExport::Complete(bundle) => bundle,
        other => panic!("expected complete: {other:?}"),
    }
}
#[test]
fn export_default_denial_missing_withheld_and_revoked_are_payload_free() {
    let mut store = kairotensor::Store::in_memory().unwrap();
    let evidence = store
        .observe_and_admit_many("a", "s", &[("SOURCE_CANARY", b"PAYLOAD_CANARY")])
        .unwrap()[0];
    let id = store.begin_attempt("a", "s", "main", &[evidence]).unwrap();
    store.record_dispatch(id, b"REQUEST_CANARY").unwrap();
    store
        .reject_invalid(
            id,
            b"RESPONSE_CANARY",
            3,
            "VALIDATOR_CANARY",
            "REASON_CANARY",
        )
        .unwrap();
    let request = export_request(id, BundlePurpose::DiagnosticReplay);
    let denied = store
        .export_decision_bundle(&request, &DenyDiagnostics)
        .unwrap();
    assert_eq!(
        denied,
        BundleExport::Redacted(RedactedBundleV1 {
            requested_attempt: id,
            reason: RedactionReason::Unavailable
        })
    );
    let authorized = complete(
        store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
    );
    assert_eq!(
        authorized.evidence[0].source.decode().unwrap(),
        b"SOURCE_CANARY"
    );
    assert_eq!(
        authorized.attempts[0]
            .dispatch_request
            .as_ref()
            .unwrap()
            .decode()
            .unwrap(),
        b"REQUEST_CANARY"
    );
    store.set_admitted("a", "s", evidence, false).unwrap();
    assert_eq!(
        store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
        denied
    );
    let missing = export_request(999, BundlePurpose::DiagnosticReplay);
    assert_eq!(
        store
            .export_decision_bundle(&missing, &AllowExport)
            .unwrap(),
        BundleExport::Redacted(RedactedBundleV1 {
            requested_attempt: 999,
            reason: RedactionReason::Unavailable
        })
    );
    let mut wrong = request.clone();
    wrong.actor = "other".into();
    assert_eq!(
        store.export_decision_bundle(&wrong, &AllowExport).unwrap(),
        denied
    );
    let redacted = serde_json::to_string(&denied).unwrap();
    for private in ["CANARY", "sha256", "source", "inputs", "validator", "head"] {
        assert!(!redacted.contains(private));
    }
}
#[test]
fn export_current_gate_preserves_filtered_positions_and_legacy_unknowns() {
    let mut store = kairotensor::Store::in_memory().unwrap();
    let evidence = store
        .observe_and_admit_many("a", "s", &[("raw", b"\xff\0")])
        .unwrap()[0];
    let (id, _) = store
        .begin_from_candidates("a", "s", "main", &[999, evidence, evidence])
        .unwrap();
    store.settle(id, b"\xfe\0", 7).unwrap();
    let request = export_request(id, BundlePurpose::CurrentRead);
    let b = complete(
        store
            .export_decision_bundle(&request, &DenyDiagnostics)
            .unwrap(),
    );
    assert_eq!(
        b.attempts[0]
            .candidates
            .iter()
            .map(|c| (c.position, c.evidence_id))
            .collect::<Vec<_>>(),
        [(0, 999), (1, evidence), (2, evidence)]
    );
    assert_eq!(
        b.attempts[0]
            .inputs
            .iter()
            .map(|c| (c.position, c.evidence_id))
            .collect::<Vec<_>>(),
        [(0, evidence), (1, evidence)]
    );
    assert_eq!(b.evidence.len(), 1);
    assert_eq!(b.evidence[0].payload.decode().unwrap(), [255, 0]);
    assert_eq!(b.attempts[0].validator, None);
    assert_eq!(b.attempts[0].accounting.unit, AccountingUnit::Unknown);
    assert_eq!(parse_decision_bundle(&b.to_json().unwrap()).unwrap(), b);
    let next = store.begin_attempt("a", "s", "main", &[]).unwrap();
    store.settle(next, b"new", 1).unwrap();
    assert!(matches!(
        store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
        BundleExport::Redacted(_)
    ));
    let historical = complete(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::DiagnosticReplay),
                &AllowExport,
            )
            .unwrap(),
    );
    assert_eq!(historical.snapshot.head, Some(next));
    // Parent is lineage only: selecting next must not copy the old payload.
    let next_bundle = complete(
        store
            .export_decision_bundle(
                &export_request(next, BundlePurpose::CurrentRead),
                &DenyDiagnostics,
            )
            .unwrap(),
    );
    assert_eq!(next_bundle.attempts.len(), 1);
    assert!(next_bundle.evidence.is_empty());
}

#[test]
fn export_support_ready_still_denies_withdrawn_captured_input() {
    let path = export_database();
    let mut store = kairotensor::Store::open(&path).unwrap();
    let ids = store
        .observe_and_admit_many("a", "s", &[("one", b"one"), ("two", b"two")])
        .unwrap();
    let id = store
        .publish_support("a", "s", "main", &[vec![ids[0]], vec![ids[1]], vec![999]])
        .unwrap();
    let before = complete(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::CurrentRead),
                &DenyDiagnostics,
            )
            .unwrap(),
    );
    assert_eq!(
        before.attempts[0].support.as_ref().unwrap().clauses.len(),
        3
    );
    assert_eq!(
        before.attempts[0].accounting.unit,
        AccountingUnit::LogicalSupport
    );
    assert!(
        before.evidence.is_empty(),
        "support registration captures formula, not evidence bytes"
    );
    // Adversarial persisted-state fixture, not produced by register_support:
    // retain an additional captured input to isolate the export admission gate.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO attempt_inputs(attempt_id,position,evidence_id) VALUES(?1,0,?2)",
            rusqlite::params![id, ids[0]],
        )
        .unwrap();
    store.set_admitted("a", "s", ids[0], false).unwrap();
    assert!(matches!(
        store.read_current("a", "s", "main").unwrap(),
        kairotensor::Current::Ready(_)
    ));
    for purpose in [BundlePurpose::CurrentRead, BundlePurpose::DiagnosticReplay] {
        assert!(matches!(
            store
                .export_decision_bundle(&export_request(id, purpose), &AllowExport)
                .unwrap(),
            BundleExport::Redacted(_)
        ));
    }
}
#[test]
fn export_ambiguous_hold_cancelled_and_planned_are_per_attempt() {
    let mut store = kairotensor::Store::in_memory().unwrap();
    let budget = store.create_token_budget(1000).unwrap();
    let foreign = store
        .begin_attempt("private", "scope", "main", &[])
        .unwrap();
    store
        .reserve_dispatch(budget, foreign, b"FOREIGN_REQUEST", 37, 50)
        .unwrap();
    store.settle(foreign, b"FOREIGN_OUTPUT", 40).unwrap();
    let id = store.begin_attempt("a", "s", "main", &[]).unwrap();
    store
        .reserve_dispatch(budget, id, b"\xff request", 3, 20)
        .unwrap();
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::DiagnosticReplay),
                &AllowExport,
            )
            .unwrap(),
    );
    assert_eq!(b.attempts.len(), 1);
    assert_eq!(b.attempts[0].accounting.hold_bound, 23);
    assert_eq!(b.attempts[0].accounting.charge, 0);
    assert_eq!(b.attempts[0].accounting.budget_reference, Some(budget));
    let wire = String::from_utf8(b.to_json().unwrap()).unwrap();
    for name in [
        "token_limit",
        "spent_tokens",
        "reserved_tokens",
        "FOREIGN",
        "private",
    ] {
        assert!(!wire.contains(name));
    }
    let recovery = store
        .create_recovery("r", budget, "a", "s", "other")
        .unwrap();
    let phase = match store.begin_recovery_attempt(recovery, &[]).unwrap() {
        kairotensor::RecoveryStart::New { attempt, .. } => attempt,
        _ => panic!(),
    };
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(phase, BundlePurpose::DiagnosticReplay),
                &AllowExport,
            )
            .unwrap(),
    );
    assert_eq!(b.attempts[0].recovery.as_ref().unwrap().state, "planned");
    assert_eq!(b.attempts[0].accounting.quote, None);
    store
        .reserve_recovery_attempt(phase, b"prepared", 4, 10)
        .unwrap();
    store.cancel_recovery_attempt(phase).unwrap();
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(phase, BundlePurpose::DiagnosticReplay),
                &AllowExport,
            )
            .unwrap(),
    );
    assert_eq!(b.attempts[0].accounting.hold_bound, 0);
    assert_eq!(
        b.attempts[0]
            .accounting
            .quote
            .as_ref()
            .unwrap()
            .input_tokens,
        4
    );
    assert_eq!(b.attempts[0].dispatch_request, None);
    assert_eq!(store.token_budget_state(budget).unwrap().reserved, 23);
}
#[test]
fn export_oversized_source_payload_request_and_reason_redact_generically() {
    for which in 0..4 {
        let mut store = kairotensor::Store::in_memory().unwrap();
        let large = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let id = store
            .observe(
                if which == 0 { &large } else { "source" },
                if which == 1 {
                    large.as_bytes()
                } else {
                    b"input"
                },
            )
            .unwrap();
        store.set_admitted("a", "s", id, true).unwrap();
        let a = store.begin_attempt("a", "s", "main", &[id]).unwrap();
        store
            .record_dispatch(
                a,
                if which == 2 {
                    large.as_bytes()
                } else {
                    b"request"
                },
            )
            .unwrap();
        store
            .reject_invalid(
                a,
                b"output",
                1,
                "v",
                if which == 3 { &large } else { "reason" },
            )
            .unwrap();
        assert_eq!(
            store
                .export_decision_bundle(
                    &export_request(a, BundlePurpose::DiagnosticReplay),
                    &AllowExport
                )
                .unwrap(),
            BundleExport::Redacted(RedactedBundleV1 {
                requested_attempt: a,
                reason: RedactionReason::Unavailable
            })
        );
    }
}
#[test]
fn export_inherited_closure_is_exact_and_current_requires_every_output() {
    use std::cell::RefCell;
    struct Inspect(RefCell<Option<DiagnosticReadRequest>>);
    impl DiagnosticAuthorizer for Inspect {
        fn authorize(&self, r: &DiagnosticReadRequest) -> Result<bool, BundleError> {
            *self.0.borrow_mut() = Some(r.clone());
            Ok(true)
        }
    }
    let mut store = kairotensor::Store::in_memory().unwrap();
    let evidence = store
        .observe_and_admit_many("a", "s", &[("source", b"data")])
        .unwrap()[0];
    let first = store
        .begin_attempt("a", "s", "main", &[evidence, evidence])
        .unwrap();
    store.settle(first, b"first", 1).unwrap();
    let (next, _) = store.begin_resume("a", "s", "main", &[]).unwrap();
    store.settle(next, b"next", 2).unwrap();
    let inspect = Inspect(RefCell::new(None));
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(next, BundlePurpose::DiagnosticReplay),
                &inspect,
            )
            .unwrap(),
    );
    assert_eq!(
        b.attempts.iter().map(|a| a.id).collect::<Vec<_>>(),
        [next, first]
    );
    let selection = inspect.0.borrow();
    let selection = selection.as_ref().unwrap();
    assert_eq!(selection.root_attempt, next);
    assert_eq!(selection.purpose, BundlePurpose::DiagnosticReplay);
    assert_eq!(
        selection
            .attempts
            .iter()
            .map(|a| a.attempt)
            .collect::<Vec<_>>(),
        [next, first]
    );
    assert_eq!(
        selection.attempts[1].inputs,
        [
            SelectedInput {
                position: 0,
                occurrence_id: evidence
            },
            SelectedInput {
                position: 1,
                occurrence_id: evidence
            }
        ]
    );
    assert_eq!(selection.attempts[1].head, Some(next));
    assert_eq!(selection.attempts[1].revision, 2);
    assert_eq!(selection.attempts[1].captured_revision, 0);
    assert!(matches!(
        store
            .export_decision_bundle(
                &export_request(next, BundlePurpose::CurrentRead),
                &AllowExport
            )
            .unwrap(),
        BundleExport::Redacted(_)
    ));
}

fn export_database() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "kairotensor-decision-export-{}-{}.sqlite",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[test]
fn export_cross_scope_closure_requires_exact_authority_and_all_consuming_admissions() {
    use std::cell::Cell;
    struct Check {
        called: Cell<bool>,
        root: i64,
        origin: i64,
        evidence: i64,
    }
    impl DiagnosticAuthorizer for Check {
        fn authorize(&self, r: &DiagnosticReadRequest) -> Result<bool, BundleError> {
            self.called.set(true);
            assert_eq!(r.actor, "a");
            assert_eq!(r.scope, "s");
            assert_eq!(
                r.attempts
                    .iter()
                    .map(|a| (a.attempt, a.actor.as_str(), a.scope.as_str()))
                    .collect::<Vec<_>>(),
                [(self.root, "a", "s"), (self.origin, "foreign", "other")]
            );
            for a in &r.attempts {
                assert_eq!(
                    a.inputs,
                    [SelectedInput {
                        position: 0,
                        occurrence_id: self.evidence
                    }]
                );
            }
            Ok(true)
        }
    }
    let path = export_database();
    let mut store = kairotensor::Store::open(&path).unwrap();
    let evidence = store
        .observe_and_admit_many("a", "s", &[("shared", b"payload")])
        .unwrap()[0];
    store
        .set_admitted("foreign", "other", evidence, true)
        .unwrap();
    let (origin, _) = store
        .begin_model_attempt("foreign", "other", "main", &[evidence], "contract")
        .unwrap();
    store.resolve_model_attempt(origin, b"request").unwrap();
    store.settle(origin, b"response", 4).unwrap();
    let (root, _) = store
        .begin_model_attempt("a", "s", "main", &[evidence], "contract")
        .unwrap();
    // Construct a legal imported historical cross-scope reuse edge. The public
    // model-reuse producer is narrower; the exporter must not assume that policy.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "INSERT INTO model_reuses(attempt_id,origin_attempt) VALUES(?1,?2)",
        rusqlite::params![root, origin],
    )
    .unwrap();
    db.execute(
        "INSERT INTO deliveries(attempt_id,disposition,output,usage) VALUES(?1,'accepted',?2,0)",
        rusqlite::params![root, b"response".as_slice()],
    )
    .unwrap();
    db.execute(
        "UPDATE branches SET head=?1,revision=1 WHERE actor='a' AND scope='s'",
        [root],
    )
    .unwrap();
    let auth = Check {
        called: Cell::new(false),
        root,
        origin,
        evidence,
    };
    assert!(matches!(
        store
            .export_decision_bundle(&export_request(root, BundlePurpose::CurrentRead), &auth)
            .unwrap(),
        BundleExport::Redacted(_)
    ));
    assert!(!auth.called.get());
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(root, BundlePurpose::DiagnosticReplay),
                &auth,
            )
            .unwrap(),
    );
    assert!(auth.called.get());
    assert_eq!(b.evidence.len(), 1);
    assert_eq!(
        b.evidence[0].authorized_scopes,
        [
            AuthorizedScope {
                actor: "a".into(),
                scope: "s".into()
            },
            AuthorizedScope {
                actor: "foreign".into(),
                scope: "other".into()
            }
        ]
    );
    store
        .set_admitted("foreign", "other", evidence, false)
        .unwrap();
    assert!(matches!(
        store
            .export_decision_bundle(
                &export_request(root, BundlePurpose::DiagnosticReplay),
                &auth
            )
            .unwrap(),
        BundleExport::Redacted(_)
    ));
}

#[allow(dead_code)]
#[path = "../examples/incident_store_bridge/adapter.rs"]
mod export_bridge;
#[test]
fn export_bridge_uses_actual_fixture_manifest_retry_and_immutable_bytes() {
    use export_bridge::*;
    let path = export_database();
    let mut f = Fixture::new(kairotensor::Store::open(&path).unwrap()).unwrap();
    let initial = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    f.store
        .reserve_recovery_attempt(
            initial.attempt,
            &initial.request,
            initial.preparation.input_tokens,
            initial.preparation.output_cap,
        )
        .unwrap();
    let run = f
        .store
        .run_timed_recovery(
            initial.attempt,
            &initial.preparation,
            || tick(200),
            |request| Ok::<_, String>(initial.measure(request, b"bad")),
        )
        .unwrap();
    assert!(matches!(
        run,
        kairotensor::RecoveryRun::Settled {
            disposition: kairotensor::Disposition::InvalidOutput,
            ..
        }
    ));
    let retry = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract).unwrap();
    // A planned retry has no quote/request in the ledger until reservation.
    let request = BundleRequest {
        actor: ACTOR.into(),
        scope: SCOPE.into(),
        attempt: retry.attempt,
        purpose: BundlePurpose::DiagnosticReplay,
    };
    let planned = complete(
        f.store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
    );
    assert_eq!(planned.attempts.len(), 2);
    assert_eq!(
        planned.attempts[0]
            .recovery
            .as_ref()
            .unwrap()
            .predecessor_id,
        Some(initial.attempt)
    );
    assert_eq!(
        planned.attempts[0]
            .immutable_manifest
            .as_ref()
            .unwrap()
            .sha256,
        initial.manifest_sha256
    );
    f.store
        .reserve_recovery_attempt(
            retry.attempt,
            &retry.request,
            retry.preparation.input_tokens,
            retry.preparation.output_cap,
        )
        .unwrap();
    let b = complete(
        f.store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
    );
    assert_eq!(
        b.attempts[0]
            .recovery
            .as_ref()
            .unwrap()
            .request
            .as_ref()
            .unwrap()
            .decode()
            .unwrap(),
        retry.request
    );
    assert!(b.attempts[0].dispatch_request.is_none());
    assert_eq!(
        b.attempts[1]
            .dispatch_request
            .as_ref()
            .unwrap()
            .decode()
            .unwrap(),
        initial.request
    );
    assert_eq!(b.attempts[0].accounting.unit, AccountingUnit::FixtureBytes);
    let db = rusqlite::Connection::open(&path).unwrap();
    let raw: (String, String) = db
        .query_row(
            "SELECT configuration,runtime_json FROM recovery_timing_bindings WHERE recovery_id=?1",
            [f.recovery],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    for a in &b.attempts {
        assert_eq!(
            a.timing_configuration.as_ref().unwrap().decode().unwrap(),
            raw.0.as_bytes()
        );
        assert_eq!(
            a.immutable_runtime.as_ref().unwrap().decode().unwrap(),
            raw.1.as_bytes()
        );
        assert_eq!(
            a.immutable_manifest.as_ref().unwrap().sha256,
            retry.manifest_sha256
        );
        assert_eq!(a.inputs[0].role, InputRole::Context);
    }
    assert_eq!(
        b.attempts[0].validator.as_ref().unwrap().provenance,
        ValidatorProvenance::DeclaredContract
    );
    assert_eq!(
        b.attempts[1].validator.as_ref().unwrap().provenance,
        ValidatorProvenance::RecordedRejection
    );
    let replay = validate_recorded_bundle(&b);
    assert_eq!(replay.consistency, ReplayStatus::Matched);
    assert_eq!(replay.reconstruction, ReplayStatus::Matched);
    // Data mutation leaves the captured immutable fingerprint unchanged: never
    // attach a recognized manifest/roles/fixture units from a near match.
    db.execute(
        "UPDATE evidence SET payload=?1 WHERE id=?2",
        rusqlite::params![b"changed".as_slice(), f.ids[1]],
    )
    .unwrap();
    let changed = complete(
        f.store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
    );
    for a in changed.attempts {
        assert!(a.immutable_manifest.is_none());
        assert_eq!(a.accounting.unit, AccountingUnit::Unknown);
        assert!(a.inputs.iter().all(|i| i.role == InputRole::Unknown));
    }
}

#[test]
fn export_missing_retry_predecessor_redacts_instead_of_inventing_complete_history() {
    let path = export_database();
    let mut store = kairotensor::Store::open(&path).unwrap();
    let budget = store.create_token_budget(100).unwrap();
    let run = store
        .create_recovery("r", budget, "a", "s", "main")
        .unwrap();
    let id = match store.begin_recovery_attempt(run, &[]).unwrap() {
        kairotensor::RecoveryStart::New { attempt, .. } => attempt,
        _ => panic!(),
    };
    // Adversarial persisted state: ordinal 1 exists without its required phase 0.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE recovery_attempts SET ordinal=1 WHERE attempt_id=?1",
            [id],
        )
        .unwrap();
    assert!(matches!(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::DiagnosticReplay),
                &AllowExport
            )
            .unwrap(),
        BundleExport::Redacted(_)
    ));
}
#[test]
fn export_large_text_source_is_enveloped_as_bytes_not_json_text() {
    let mut store = kairotensor::Store::in_memory().unwrap();
    let source = "s".repeat(3 * 1024 * 1024);
    let e = store.observe(&source, b"input").unwrap();
    store.set_admitted("a", "s", e, true).unwrap();
    let id = store.begin_attempt("a", "s", "main", &[e]).unwrap();
    store.settle(id, b"output", 0).unwrap();
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::CurrentRead),
                &DenyDiagnostics,
            )
            .unwrap(),
    );
    assert_eq!(b.evidence[0].source.decode().unwrap(), source.as_bytes());
}
#[test]
fn export_missing_payload_dependency_cycle_and_closure_limit_redact() {
    for scenario in 0..3 {
        let path = export_database();
        let mut store = kairotensor::Store::open(&path).unwrap();
        let e = store
            .observe_and_admit_many("a", "s", &[("secret", b"payload")])
            .unwrap()[0];
        let first = store.begin_attempt("a", "s", "main", &[e]).unwrap();
        store.settle(first, b"output", 0).unwrap();
        let (mut root, _) = store.begin_resume("a", "s", "main", &[]).unwrap();
        store.settle(root, b"output", 0).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        match scenario {
            0 => {
                db.execute("DELETE FROM deliveries WHERE attempt_id=?1", [first])
                    .unwrap();
            }
            1 => {
                db.execute(
                    "UPDATE attempts SET parent=id,inherited_from=id WHERE id=?1",
                    [root],
                )
                .unwrap();
            }
            2 => {
                for _ in 0..127 {
                    root = store.begin_resume("a", "s", "main", &[]).unwrap().0;
                    store.settle(root, b"output", 0).unwrap();
                }
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            store
                .export_decision_bundle(
                    &export_request(root, BundlePurpose::DiagnosticReplay),
                    &AllowExport
                )
                .unwrap(),
            BundleExport::Redacted(_)
        ));
    }
}
#[test]
fn export_aggregate_envelopes_are_bounded_and_sql_corruption_is_an_error() {
    let mut store = kairotensor::Store::in_memory().unwrap();
    let payload = vec![0; 3 * 1024 * 1024];
    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = store.observe("source", &payload).unwrap();
        store.set_admitted("a", "s", id, true).unwrap();
        ids.push(id);
    }
    let id = store.begin_attempt("a", "s", "main", &ids).unwrap();
    assert!(matches!(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::DiagnosticReplay),
                &AllowExport
            )
            .unwrap(),
        BundleExport::Redacted(_)
    ));
    let path = export_database();
    let mut store = kairotensor::Store::open(&path).unwrap();
    let id = store.begin_attempt("a", "s", "main", &[]).unwrap();
    store.settle(id, b"output", 0).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE deliveries SET output='CORRUPTION_CANARY' WHERE attempt_id=?1",
            [id],
        )
        .unwrap();
    assert!(
        matches!(
            store
                .export_decision_bundle(
                    &export_request(id, BundlePurpose::DiagnosticReplay),
                    &DenyDiagnostics
                )
                .unwrap(),
            BundleExport::Redacted(_)
        ),
        "denial must not load corrupt payload bytes"
    );
    let error = store
        .export_decision_bundle(
            &export_request(id, BundlePurpose::DiagnosticReplay),
            &AllowExport,
        )
        .unwrap_err();
    assert!(matches!(error, BundleError::Store(_)));
    assert!(!error.to_string().contains("CANARY"));
}
#[test]
fn export_report_binding_replays_exactly_without_invented_units() {
    let mut store = kairotensor::Store::in_memory().unwrap();
    let e = store
        .observe_and_admit_many(
            "a",
            "s",
            &[("report", br#"{"release":"r1","passed":2,"total":2}"#)],
        )
        .unwrap()[0];
    let id = store.begin_report_verdict("a", "s", "main", &[e]).unwrap();
    store.commit_report_verdict(id).unwrap();
    let b = complete(
        store
            .export_decision_bundle(
                &export_request(id, BundlePurpose::CurrentRead),
                &DenyDiagnostics,
            )
            .unwrap(),
    );
    let report = validate_recorded_bundle(&b);
    assert_eq!(report.reconstruction, ReplayStatus::Matched);
    assert_eq!(report.validation, ReplayStatus::Matched);
    assert_eq!(
        b.attempts[0].accounting.unit,
        AccountingUnit::LogicalSupport
    );
    assert_eq!(
        b.attempts[0].validator.as_ref().unwrap().provenance,
        ValidatorProvenance::DeterministicBinding
    );
}
#[test]
fn export_retains_noncanonical_timing_and_runtime_bytes() {
    use export_bridge::*;
    let path = export_database();
    let mut f = Fixture::new(kairotensor::Store::open(&path).unwrap()).unwrap();
    let second = f
        .store
        .create_recovery("raw timing", f.budget, ACTOR, SCOPE, "other")
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    // Explicit adversarial persisted fixture: valid JSON with retained whitespace,
    // inserted before attempts exist, respecting the immutable timing triggers.
    db.execute("INSERT INTO recovery_timing_bindings SELECT ?1, '  ' || configuration || char(10),clock_domain,start_ms,deadline_ms, ' ' || runtime_json || char(10),calibration_id,max_duration_ms,safety_margin_ms,max_input_tokens,output_cap,measured_at_ms,expires_at_ms,last_observed_ms FROM recovery_timing_bindings WHERE recovery_id=?2",rusqlite::params![second,f.recovery]).unwrap();
    let id = match f.store.begin_recovery_attempt(second, &f.ids).unwrap() {
        kairotensor::RecoveryStart::New { attempt, .. } => attempt,
        _ => panic!(),
    };
    let request = BundleRequest {
        actor: ACTOR.into(),
        scope: SCOPE.into(),
        attempt: id,
        purpose: BundlePurpose::DiagnosticReplay,
    };
    let b = complete(
        f.store
            .export_decision_bundle(&request, &AllowExport)
            .unwrap(),
    );
    let raw: (String, String) = db
        .query_row(
            "SELECT configuration,runtime_json FROM recovery_timing_bindings WHERE recovery_id=?1",
            [second],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        b.attempts[0]
            .timing_configuration
            .as_ref()
            .unwrap()
            .decode()
            .unwrap(),
        raw.0.as_bytes()
    );
    assert_eq!(
        b.attempts[0]
            .immutable_runtime
            .as_ref()
            .unwrap()
            .decode()
            .unwrap(),
        raw.1.as_bytes()
    );
    assert!(raw.0.starts_with("  "));
    assert!(raw.1.ends_with('\n'));
    assert!(b.attempts[0].immutable_manifest.is_some());
}

#[test]
fn export_manifest_expansion_is_bounded_before_replicating_case_metadata() {
    use export_bridge::*;
    let mut f = Fixture::new(kairotensor::Store::in_memory().unwrap()).unwrap();
    let (mut context, _) = pinned_case("cal-01").unwrap();
    context.case_id = "c".repeat(250_000);
    context.evidence_ids = (0..20).map(|n| format!("e{n}")).collect();
    let raw = context.to_json().unwrap();
    let mut ids = f
        .store
        .observe_and_admit_many(ACTOR, SCOPE, &[("context", raw.as_slice())])
        .unwrap();
    for _ in 0..20 {
        ids.extend(
            f.store
                .observe_and_admit_many(ACTOR, SCOPE, &[("report", b"small")])
                .unwrap(),
        );
    }
    let id = match f.store.begin_recovery_attempt(f.recovery, &ids).unwrap() {
        kairotensor::RecoveryStart::New { attempt, .. } => attempt,
        _ => panic!(),
    };
    let request = BundleRequest {
        actor: ACTOR.into(),
        scope: SCOPE.into(),
        attempt: id,
        purpose: BundlePurpose::DiagnosticReplay,
    };
    assert!(matches!(f.store.export_decision_bundle(&request,&AllowExport).unwrap(),BundleExport::Redacted(_)),"a bounded context must not expand into an unbounded candidate manifest before the fingerprint comparison");
}
