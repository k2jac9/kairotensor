// Observe only allocations on this test thread, after oversized caller inputs
// have been constructed. Other parallel tests cannot affect the measurement.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static MEASURE: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}
struct AllocationObserver;
fn observe(size: usize) {
    let _ = MEASURE.try_with(|active| {
        if active.get() {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
        }
    });
}
unsafe impl GlobalAlloc for AllocationObserver {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationObserver = AllocationObserver;

fn largest_allocation_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            MEASURE.with(|active| active.set(false));
        }
    }
    LARGEST.with(|largest| largest.set(0));
    MEASURE.with(|active| active.set(true));
    let stop = Stop;
    let result = f();
    drop(stop);
    (result, LARGEST.with(Cell::get))
}

use kairotensor::incident_protocol::{
    render_incident_request, validate_incident_output, IncidentContextV1, IncidentPhase,
    IncidentProtocolError as Error, IncidentReport,
};

fn context() -> IncidentContextV1 {
    IncidentContextV1 {
        schema_version: 1,
        version: "incident-store-context/1".into(),
        workload_version: "incident-routing/1".into(),
        freeze_sha256: "freeze".into(),
        prompt_sha256: "prompt".into(),
        validator_version: "incident-routing-protocol/1".into(),
        case_id: "case-1".into(),
        question: "Who?".into(),
        owner_ids: vec!["a".into(), "b".into()],
        evidence_ids: vec!["e1".into(), "e2".into()],
        system_prompt: "Read.".into(),
    }
}
fn reports() -> Vec<IncidentReport> {
    vec![
        IncidentReport {
            external_id: "e1".into(),
            text: "Cause is A.".into(),
        },
        IncidentReport {
            external_id: "e2".into(),
            text: "A belongs to a.".into(),
        },
    ]
}
const ROUTE: &[u8] = br#"{"decision":"route","owner":"a","reason":"diagnosed_component","evidence_ids":["e1","e2"]}"#;

#[test]
fn initial_request_is_exact_python_v1_bytes() {
    let expected = br#"{"messages":[{"content":"Read.","role":"system"},{"content":"{\"evidence\":[{\"id\":\"e1\",\"text\":\"Cause is A.\"},{\"id\":\"e2\",\"text\":\"A belongs to a.\"}],\"owner_ids\":[\"a\",\"b\"],\"question\":\"Who?\"}","role":"user"}]}"#;
    assert_eq!(
        render_incident_request(&context(), &reports(), IncidentPhase::Initial).unwrap(),
        expected
    );
}
#[test]
fn retry_adds_only_fixed_versioned_feedback() {
    let initial: serde_json::Value = serde_json::from_slice(
        &render_incident_request(&context(), &reports(), IncidentPhase::Initial).unwrap(),
    )
    .unwrap();
    let mut expected = initial.clone();
    let mut inner: serde_json::Value =
        serde_json::from_str(initial["messages"][1]["content"].as_str().unwrap()).unwrap();
    inner["validation_error"] = "incident-routing-protocol/1:invalid_output".into();
    expected["messages"][1]["content"] = serde_json::to_string(&inner).unwrap().into();
    assert_eq!(
        render_incident_request(&context(), &reports(), IncidentPhase::ProtocolRetry).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}
#[test]
fn unicode_controls_and_escaping_match_python() {
    let mut c = context();
    c.question = "é🦀\u{2028}\"\\\n\t\0\u{8}\u{c}\r".into();
    let request = render_incident_request(&c, &reports(), IncidentPhase::Initial).unwrap();
    let text = String::from_utf8(request).unwrap();
    assert_eq!(
        text,
        r###"{"messages":[{"content":"Read.","role":"system"},{"content":"{\"evidence\":[{\"id\":\"e1\",\"text\":\"Cause is A.\"},{\"id\":\"e2\",\"text\":\"A belongs to a.\"}],\"owner_ids\":[\"a\",\"b\"],\"question\":\"é🦀 \\\"\\\\\\n\\t\\u0000\\b\\f\\r\"}","role":"user"}]}"###
    );
}
#[test]
fn context_roundtrip_is_canonical_and_preserves_order() {
    let c = context();
    let raw = c.to_json().unwrap();
    assert_eq!(IncidentContextV1::from_json(&raw).unwrap(), c);
    assert!(raw.starts_with(br#"{"case_id":"case-1","evidence_ids":["e1","e2"]"#));
}
#[test]
fn contexts_refuse_unknown_duplicate_missing_and_wrong_typed_fields() {
    let raw = String::from_utf8(context().to_json().unwrap()).unwrap();
    for bad in [
        raw.replacen('{', r#"{"unknown":1,"#, 1),
        raw.replacen('{', r#"{"case_id":"other","#, 1),
        raw.replace(r#""schema_version":1"#, r#""schema_version":1.0"#),
        raw.replace(r#""schema_version":1"#, r#""schema_version":true"#),
        raw.replace(r#""case_id":"case-1","#, ""),
    ] {
        assert!(
            IncidentContextV1::from_json(bad.as_bytes()).is_err(),
            "{bad}"
        );
    }
}
#[test]
fn unknown_context_versions_refuse() {
    let mut c = context();
    c.schema_version = 2;
    assert_eq!(c.to_json(), Err(Error::UnknownVersion));
    c.schema_version = 1;
    c.version = "incident-store-context/2".into();
    assert_eq!(
        render_incident_request(&c, &reports(), IncidentPhase::Initial),
        Err(Error::UnknownVersion)
    );
}
#[test]
fn all_context_text_and_id_invariants_apply_to_direct_construction() {
    for field in [
        "workload_version",
        "freeze_sha256",
        "prompt_sha256",
        "validator_version",
        "case_id",
        "question",
        "system_prompt",
    ] {
        let mut value: serde_json::Value =
            serde_json::from_slice(&context().to_json().unwrap()).unwrap();
        value[field] = " \n\t".into();
        assert_eq!(
            IncidentContextV1::from_json(&serde_json::to_vec(&value).unwrap()),
            Err(Error::InvalidContext)
        );
    }
    for ids in [
        vec![],
        vec!["".into()],
        vec![" \n".into()],
        vec!["x".into(), "x".into()],
    ] {
        for owner in [true, false] {
            let mut c = context();
            if owner {
                c.owner_ids = ids.clone()
            } else {
                c.evidence_ids = ids.clone()
            }
            assert_eq!(c.to_json(), Err(Error::InvalidContext));
            assert_eq!(
                render_incident_request(&c, &reports(), IncidentPhase::Initial),
                Err(Error::InvalidContext)
            );
            assert_eq!(
                validate_incident_output(ROUTE, &c),
                Err(Error::InvalidContext)
            );
        }
    }
}
#[test]
fn report_ids_and_order_must_exactly_match_context() {
    let original = reports();
    for bad in [
        vec![],
        vec![original[0].clone()],
        vec![original[1].clone(), original[0].clone()],
        vec![original[0].clone(), original[0].clone()],
    ] {
        assert_eq!(
            render_incident_request(&context(), &bad, IncidentPhase::Initial),
            Err(Error::InvalidContext)
        );
    }
    let mut bad = reports();
    bad[0].text = " \n".into();
    assert_eq!(
        render_incident_request(&context(), &bad, IncidentPhase::Initial),
        Err(Error::InvalidContext)
    );
}
#[test]
fn protocol_accepts_wrong_semantics_and_non_gold_citation_subset() {
    let raw =
        br#"{"decision":"route","owner":"b","reason":"diagnosed_component","evidence_ids":["e2"]}"#;
    let output = validate_incident_output(raw, &context()).unwrap();
    assert_eq!(output.owner.as_deref(), Some("b"));
    assert_eq!(output.evidence_ids, vec!["e2"]);
    assert_eq!(output.decision, "route");
    assert_eq!(output.reason, "diagnosed_component");
}
#[test]
fn all_abstention_reasons_and_citation_order_are_allowed() {
    for reason in [
        "undetermined_cause",
        "conflicting_diagnoses",
        "owner_not_unique",
    ] {
        let raw = format!(
            r#"{{"decision":"abstain","owner":null,"reason":"{reason}","evidence_ids":["e2","e1"]}}"#
        );
        assert!(validate_incident_output(raw.as_bytes(), &context()).is_ok());
    }
}
#[test]
fn invalid_output_contract_refuses() {
    let base = String::from_utf8(ROUTE.to_vec()).unwrap();
    for bad in [
        base.replace("route", "other"),
        base.replace("diagnosed_component", "undetermined_cause"),
        base.replace(r#""owner":"a""#, r#""owner":null"#),
        base.replace(r#""owner":"a","#, ""),
        base.replace(r#""owner":"a""#, r#""owner":"unknown""#),
        base.replace(r#"["e1","e2"]"#, "[]"),
        base.replace(r#"["e1","e2"]"#, r#"["e1","e1"]"#),
        base.replace(r#"["e1","e2"]"#, r#"["unknown"]"#),
        base.replace(r#"["e1","e2"]"#, "[1]"),
        base.replacen('{', r#"{"extra":1,"#, 1),
        base.replacen('{', r#"{"decision":"route","#, 1),
        base.replace("route", "abstain"),
        "[]".into(),
        "null".into(),
    ] {
        assert!(
            validate_incident_output(bad.as_bytes(), &context()).is_err(),
            "{bad}"
        );
    }
}
#[test]
fn invalid_utf8_syntax_nonfinite_and_error_text_are_payload_free() {
    for bad in [
        vec![0xff],
        b"SECRET not JSON".to_vec(),
        br#"{"decision":NaN}"#.to_vec(),
        br#"{"decision":"\ud800"}"#.to_vec(),
    ] {
        let error = validate_incident_output(&bad, &context()).unwrap_err();
        assert!(!error.to_string().contains("SECRET"));
        assert!(!format!("{error:?}").contains("SECRET"));
    }
    assert_eq!(
        validate_incident_output(&[0xff], &context()),
        Err(Error::InvalidEncoding)
    );
}
#[test]
fn container_depth_counts_strings_and_enforces_64_65_boundary() {
    for depth in [64, 65] {
        let raw = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(
            validate_incident_output(raw.as_bytes(), &context()),
            Err(if depth == 64 {
                Error::InvalidOutput
            } else {
                Error::InvalidJson
            })
        );
        assert_eq!(
            IncidentContextV1::from_json(raw.as_bytes()),
            Err(if depth == 64 {
                Error::InvalidContext
            } else {
                Error::InvalidJson
            })
        );
    }
    let mut c = context();
    c.question = "[{}]\\\"".repeat(100);
    assert!(render_incident_request(&c, &reports(), IncidentPhase::Initial).is_ok());
}
#[test]
fn reply_size_limit_includes_whitespace_and_allows_exactly_one_mib() {
    let mut raw = ROUTE.to_vec();
    raw.resize(1024 * 1024, b' ');
    assert!(validate_incident_output(&raw, &context()).is_ok());
    raw.push(b' ');
    assert_eq!(
        validate_incident_output(&raw, &context()),
        Err(Error::TooLarge)
    );
}
#[test]
fn context_and_rendered_request_have_independent_size_limits() {
    let mut raw = context().to_json().unwrap();
    raw.resize(1024 * 1024, b' ');
    assert!(IncidentContextV1::from_json(&raw).is_ok());
    raw.push(b' ');
    assert_eq!(IncidentContextV1::from_json(&raw), Err(Error::TooLarge));
    let mut c = context();
    c.question = "x".repeat(1024 * 1024);
    assert_eq!(c.to_json(), Err(Error::TooLarge));
    assert_eq!(
        render_incident_request(&c, &reports(), IncidentPhase::Initial),
        Err(Error::TooLarge)
    );
    let mut r = reports();
    r[0].text = "x".repeat(1024 * 1024);
    assert_eq!(
        render_incident_request(&context(), &r, IncidentPhase::Initial),
        Err(Error::TooLarge)
    );
}

// These literals were produced independently by the unchanged frozen Python v1
// renderer, not by the Rust renderer or its helpers.
#[test]
fn all_frozen_initial_requests_match_python_byte_digests() {
    use sha2::{Digest, Sha256};
    let prompt: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../workloads/incident-routing-v1/prompt.json"
    ))
    .unwrap();
    let cases: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../workloads/incident-routing-v1/cases.json"
    ))
    .unwrap();
    let expected = [
        (
            "cal-01",
            "7600121b3d4b7c69fdb12b3f7ceb9acaf3060953fd2f82cb68a8413ac6d7affe",
        ),
        (
            "cal-02",
            "59d5b5c677fd93125f3e70a5174ddc3ee6a4c7c4ca0ce32379333eb22d2988c8",
        ),
        (
            "cal-03",
            "847df04fe5b8b1501366fab4199eba7ce7bb2d7b1d3cc7b95521522bac04f35b",
        ),
        (
            "cal-04",
            "fb8853428685997a53e6f0454d8a6c08d53787e2c7e2b7a2e3ef8a987aa57b25",
        ),
        (
            "hold-01",
            "66c456316a9d96738521be91139fb7c740a5545f54bc64e05b47354bb2e9aa38",
        ),
        (
            "hold-02",
            "e0b6960f8864fe981e1bcb3523e22360cddbfd6c0b80a08f9f71fa6262f85b20",
        ),
        (
            "hold-03",
            "1e32fee2527f579552f3a1047580108976728d32782841b522b1403ef65f284a",
        ),
        (
            "hold-04",
            "a7adac34f6cdb484a22abf2eb84f2b4b30b2ac9234a84c4e13312080900449fe",
        ),
        (
            "hold-05",
            "999cedbd55ee757a642e1e21cae9ead0155f6f840150bfc465be1a2cf281386a",
        ),
        (
            "hold-06",
            "47ff0f1778722ec4e1a0be16c7c144f78bc510235eaddb67153a694fa1ca7e48",
        ),
        (
            "hold-07",
            "cdedc2d6338519c0aebe911afe5eef4dabcb7091502b48b99df85c03afb7f45a",
        ),
        (
            "hold-08",
            "bb63c5e11f46c9dede7858160b5e1a7e4af1a1951e5dd6ac91c8a024dd821106",
        ),
    ];
    let rows = cases["cases"].as_array().unwrap();
    assert_eq!(rows.len(), expected.len());
    for (row, (id, digest)) in rows.iter().zip(expected) {
        let mut c = context();
        c.case_id = row["case_id"].as_str().unwrap().into();
        assert_eq!(c.case_id, id);
        c.system_prompt = prompt["system"].as_str().unwrap().into();
        c.question = row["question"].as_str().unwrap().into();
        c.owner_ids = serde_json::from_value(row["owner_ids"].clone()).unwrap();
        let r: Vec<_> = row["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| IncidentReport {
                external_id: e["id"].as_str().unwrap().into(),
                text: e["text"].as_str().unwrap().into(),
            })
            .collect();
        c.evidence_ids = r.iter().map(|e| e.external_id.clone()).collect();
        let request = render_incident_request(&c, &r, IncidentPhase::Initial).unwrap();
        assert_eq!(
            Sha256::digest(&request)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            digest,
            "{id}"
        );
    }
}

#[test]
fn python_strip_control_whitespace_is_empty_text() {
    let mut c = context();
    c.question = "\u{1c}\u{1d}\u{1e}\u{1f}".into();
    assert_eq!(c.to_json(), Err(Error::InvalidContext));
}

// Catch a late TooLarge check that first clones the caller's oversized String,
// grows a serializer without a bound, or builds a large ID-deduplication table.
#[test]
fn direct_oversize_refuses_without_input_sized_intermediate_allocations() {
    const LIMIT: usize = 1024 * 1024;
    let mut c = context();
    c.question = "x".repeat(8 * LIMIT);
    for call in [0, 1, 2] {
        let (result, largest) = largest_allocation_during(|| match call {
            0 => c.to_json().map(|_| ()),
            1 => render_incident_request(&c, &reports(), IncidentPhase::Initial).map(|_| ()),
            _ => validate_incident_output(ROUTE, &c).map(|_| ()),
        });
        assert_eq!(result, Err(Error::TooLarge));
        assert!(
            largest <= LIMIT,
            "context entry {call} allocated {largest} bytes before refusal"
        );
    }
    let mut r = reports();
    r[0].text = "x".repeat(8 * LIMIT);
    let c = context();
    let (result, largest) =
        largest_allocation_during(|| render_incident_request(&c, &r, IncidentPhase::Initial));
    assert_eq!(result, Err(Error::TooLarge));
    assert!(
        largest <= LIMIT,
        "report allocated {largest} bytes before refusal"
    );

    let mut c = context();
    c.question = "\0".repeat(LIMIT / 2);
    let (result, largest) = largest_allocation_during(|| c.to_json());
    assert_eq!(result, Err(Error::TooLarge));
    assert!(
        largest <= LIMIT,
        "escaped context allocated {largest} bytes before refusal"
    );

    let mut r = reports();
    r[0].text = "\0".repeat(LIMIT / 2);
    let c = context();
    let (result, largest) =
        largest_allocation_during(|| render_incident_request(&c, &r, IncidentPhase::Initial));
    assert_eq!(result, Err(Error::TooLarge));
    assert!(
        largest <= LIMIT,
        "escaped report allocated {largest} bytes before refusal"
    );

    let mut c = context();
    c.owner_ids = (0..65536).map(|i| format!("{i:032}")).collect();
    let (result, largest) = largest_allocation_during(|| c.to_json());
    assert_eq!(result, Err(Error::TooLarge));
    assert!(
        largest <= LIMIT,
        "aggregate IDs allocated {largest} bytes before refusal"
    );
}
