//! Offline fixture: Store admission and byte accounting, never model efficacy.
use kairotensor::incident_protocol::{
    render_incident_request, validate_incident_output, IncidentContextV1, IncidentPhase,
    IncidentProtocolError, IncidentReport,
};
use kairotensor::{
    MeasuredRecoveryReply, RecoveryCalibration, RecoveryClock, RecoveryPreparation,
    RecoveryReadiness, RecoveryRun, RecoveryRuntime, RecoveryStart, RecoveryTiming,
    ReplyValidation, Store,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub(super) const ACTOR: &str = "fixture-actor";
pub(super) const SCOPE: &str = "incident-fixture";
pub(super) const FEEDBACK: &str = "incident-routing-protocol/1:invalid_output";
const VERSION: &str = "incident-routing/1";
const VALIDATOR: &str = "incident-routing-protocol/1";
const FREEZE_SHA256: &str = "f71732c193ff889f24709a11855b59820a657c21c2b43ec721cddfbff1ede2c8";
const FREEZE: &[u8] = include_bytes!("../../workloads/incident-routing-v1/freeze.json");
const CASES: &[u8] = include_bytes!("../../workloads/incident-routing-v1/cases.json");
const PROMPT: &[u8] = include_bytes!("../../workloads/incident-routing-v1/prompt.json");
const CONTRACT: &[u8] = include_bytes!("../../workloads/incident-routing-v1/contract.json");
const MAX_INPUTS: usize = 1024;

pub(super) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug)]
pub(super) enum CancellationFailure {
    Refused,
    Store(rusqlite::Error),
}
#[derive(Debug)]
pub(super) enum BridgeError {
    UnavailableEvidence,
    ChangedContract,
    ExistingRecovery,
    TerminalRecovery,
    InvalidManifest,
    Protocol(IncidentProtocolError),
    Store(rusqlite::Error),
    // Technical-owner clarification: keep both failures without disguising SQL errors.
    CancellationFailed {
        original: Box<BridgeError>,
        cancellation: CancellationFailure,
    },
}
impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(error) => write!(f, "{error}"),
            Self::Store(error) => write!(f, "incident store: {error}"),
            Self::CancellationFailed {
                original,
                cancellation,
            } => {
                write!(f, "{original}; cancellation: ")?;
                match cancellation {
                    CancellationFailure::Refused => f.write_str("refused; retain hold"),
                    CancellationFailure::Store(error) => write!(f, "{error}"),
                }
            }
            Self::UnavailableEvidence => f.write_str("incident evidence unavailable"),
            Self::ChangedContract => f.write_str("incident contract changed"),
            Self::ExistingRecovery => f.write_str("incident recovery already exists"),
            Self::TerminalRecovery => f.write_str("incident recovery finished"),
            Self::InvalidManifest => f.write_str("invalid incident manifest"),
        }
    }
}
impl std::error::Error for BridgeError {}
impl From<rusqlite::Error> for BridgeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}
impl From<IncidentProtocolError> for BridgeError {
    fn from(error: IncidentProtocolError) -> Self {
        Self::Protocol(error)
    }
}
type Result<T> = std::result::Result<T, BridgeError>;

fn decode(raw: &[u8]) -> Result<Value> {
    serde_json::from_slice(raw).map_err(|_| BridgeError::InvalidManifest)
}
fn text(value: &Value) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or(BridgeError::InvalidManifest)
}
fn strings(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .ok_or(BridgeError::InvalidManifest)?
        .iter()
        .map(text)
        .collect()
}

/// Trusted input loader: hashes are checked before parsing. Labels are never loaded.
pub(super) fn pinned_case(case_id: &str) -> Result<(IncidentContextV1, Vec<IncidentReport>)> {
    if sha256(FREEZE) != FREEZE_SHA256 {
        return Err(BridgeError::InvalidManifest);
    }
    let freeze = decode(FREEZE)?;
    for (name, raw) in [
        ("cases.json", CASES),
        ("prompt.json", PROMPT),
        ("contract.json", CONTRACT),
    ] {
        if freeze["sha256"][name].as_str() != Some(sha256(raw).as_str()) {
            return Err(BridgeError::InvalidManifest);
        }
    }
    let cases = decode(CASES)?;
    let prompt = decode(PROMPT)?;
    let contract = decode(CONTRACT)?;
    for document in [&cases, &prompt, &contract] {
        if document["version"] != VERSION || document["schema_version"] != 1 {
            return Err(BridgeError::InvalidManifest);
        }
    }
    let case = cases["cases"]
        .as_array()
        .ok_or(BridgeError::InvalidManifest)?
        .iter()
        .find(|case| case["case_id"].as_str() == Some(case_id))
        .ok_or(BridgeError::InvalidManifest)?;
    let reports: Vec<_> = case["evidence"]
        .as_array()
        .ok_or(BridgeError::InvalidManifest)?
        .iter()
        .map(|report| {
            Ok(IncidentReport {
                external_id: text(&report["id"])?,
                text: text(&report["text"])?,
            })
        })
        .collect::<Result<_>>()?;
    let context = IncidentContextV1 {
        schema_version: 1,
        version: "incident-store-context/1".into(),
        workload_version: VERSION.into(),
        freeze_sha256: FREEZE_SHA256.into(),
        prompt_sha256: sha256(PROMPT),
        validator_version: VALIDATOR.into(),
        case_id: case_id.into(),
        question: text(&case["question"])?,
        owner_ids: strings(&case["owner_ids"])?,
        evidence_ids: reports.iter().map(|r| r.external_id.clone()).collect(),
        system_prompt: text(&prompt["system"])?,
    };
    context.to_json()?;
    Ok((context, reports))
}

#[derive(Clone, Debug)]
pub(super) struct SourceMapping {
    pub(super) workload_version: String,
    pub(super) case_id: String,
    pub(super) external_id: String,
    pub(super) occurrence: i64,
}

/// Private fields prevent a caller from replacing the required set or cached bytes.
pub(super) struct IncidentContract {
    context: IncidentContextV1,
    context_id: i64,
    context_sha256: String,
    mappings: Vec<SourceMapping>,
    report_sha256: Vec<String>,
    runtime: RecoveryRuntime,
    output_cap: i64,
    manifest_sha256: String,
}
impl IncidentContract {
    pub(super) fn new(
        case_id: &str,
        context_id: i64,
        context_bytes: &[u8],
        mappings: Vec<SourceMapping>,
        mut runtime: RecoveryRuntime,
        output_cap: i64,
    ) -> Result<Self> {
        let (context, reports) = pinned_case(case_id)?;
        if mappings.len() + 1 > MAX_INPUTS
            || mappings.len() != reports.len()
            || output_cap <= 0
            || context_id <= 0
        {
            return Err(BridgeError::InvalidManifest);
        }
        // Public codec bounds the caller bytes before parsing; byte identity is exact.
        if IncidentContextV1::from_json(context_bytes)? != context
            || context_bytes != context.to_json()?
        {
            return Err(BridgeError::ChangedContract);
        }
        let mut seen = HashSet::from([context_id]);
        for (mapping, report) in mappings.iter().zip(&reports) {
            if mapping.workload_version != VERSION
                || mapping.case_id != case_id
                || mapping.external_id != report.external_id
                || mapping.occurrence <= 0
                || !seen.insert(mapping.occurrence)
            {
                return Err(BridgeError::InvalidManifest);
            }
        }
        // The supplied fingerprint is never authority; the exact manifest derives it.
        runtime.prompt_fingerprint.clear();
        let mut result = Self {
            context,
            context_id,
            context_sha256: sha256(context_bytes),
            mappings,
            report_sha256: reports.iter().map(|r| sha256(r.text.as_bytes())).collect(),
            runtime,
            output_cap,
            manifest_sha256: String::new(),
        };
        result.manifest_sha256 =
            sha256(&result.manifest(&result.context_sha256, &result.report_sha256)?);
        result.runtime.prompt_fingerprint = result.manifest_sha256.clone();
        Ok(result)
    }

    fn ids(&self) -> Vec<i64> {
        std::iter::once(self.context_id)
            .chain(self.mappings.iter().map(|m| m.occurrence))
            .collect()
    }

    fn manifest(&self, context_digest: &str, report_digests: &[String]) -> Result<Vec<u8>> {
        if self.mappings.len() + 1 > MAX_INPUTS || report_digests.len() != self.mappings.len() {
            return Err(BridgeError::InvalidManifest);
        }
        let mut runtime =
            serde_json::to_value(&self.runtime).map_err(|_| BridgeError::InvalidManifest)?;
        runtime
            .as_object_mut()
            .ok_or(BridgeError::InvalidManifest)?
            .remove("prompt_fingerprint");
        let mappings: Vec<_> = self.mappings.iter().zip(report_digests).map(|(m, digest)| json!({
            "workload_version":m.workload_version, "case_id":m.case_id,
            "external_id":m.external_id, "occurrence_id":m.occurrence, "payload_sha256":digest,
        })).collect();
        serde_json::to_vec(&json!({
            "version":"incident-store-manifest/1", "schema_version":1,
            "context":{"occurrence_id":self.context_id,"sha256":context_digest},
            "sources":mappings, "workload_version":VERSION,
            "freeze_sha256":self.context.freeze_sha256,"prompt_sha256":self.context.prompt_sha256,
            "validator_version":VALIDATOR, "initial_renderer":"incident-request/1",
            "retry_renderer":"incident-request-retry/1", "retry_feedback":FEEDBACK,
            "output_cap":self.output_cap, "runtime":runtime,
        }))
        .map_err(|_| BridgeError::InvalidManifest)
    }

    #[cfg(test)]
    pub(super) fn mappings_for_test(&self) -> Vec<SourceMapping> {
        self.mappings.clone()
    }
}

#[derive(Debug)]
pub(super) struct IncidentPreparation {
    pub(super) attempt: i64,
    pub(super) request: Vec<u8>,
    pub(super) captured_ids: Vec<i64>,
    pub(super) manifest_sha256: String,
    pub(super) preparation: RecoveryPreparation,
    // Only context decoded from the captured occurrence reaches reply validation.
    context: IncidentContextV1,
}
impl IncidentPreparation {
    pub(super) fn measure(&self, request: &[u8], output: &[u8]) -> MeasuredRecoveryReply {
        let validation = match validate_incident_output(output, &self.context) {
            Ok(_) => ReplyValidation::Valid,
            Err(_) => ReplyValidation::Invalid {
                validator: VALIDATOR.into(),
                reason: FEEDBACK.into(),
            },
        };
        MeasuredRecoveryReply {
            output: output.to_vec(),
            input_tokens: request.len() as i64,
            output_tokens: output.len() as i64,
            validation,
        }
    }
}

/// No reservation or backend invocation occurs here. New captures alone can prepare.
pub(super) fn prepare_incident_attempt(
    store: &mut Store,
    recovery: i64,
    candidates: &[i64],
    contract: &IncidentContract,
) -> Result<IncidentPreparation> {
    if candidates.len() > MAX_INPUTS || candidates != contract.ids() {
        return Err(BridgeError::InvalidManifest);
    }
    let timing = store
        .recovery_timing(recovery)?
        .ok_or(BridgeError::ChangedContract)?;
    let manifest_hash =
        sha256(&contract.manifest(&contract.context_sha256, &contract.report_sha256)?);
    if manifest_hash != contract.manifest_sha256
        || timing.runtime != contract.runtime
        || timing.runtime.prompt_fingerprint != manifest_hash
        || timing.calibration.output_cap != contract.output_cap
    {
        return Err(BridgeError::ChangedContract);
    }
    let start = store
        .begin_recovery_attempt(recovery, candidates)
        .map_err(|error| {
            // Match just the documented complete-capture refusal, never arbitrary SQL failures.
            match &error {
                rusqlite::Error::SqliteFailure(code, Some(message))
                    if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT
                        && message == "candidate evidence is incomplete or unavailable" =>
                {
                    BridgeError::UnavailableEvidence
                }
                _ => BridgeError::Store(error),
            }
        })?;
    match start {
        RecoveryStart::Existing(_) => Err(BridgeError::ExistingRecovery),
        RecoveryStart::Finished => Err(BridgeError::TerminalRecovery),
        RecoveryStart::New {
            attempt,
            inputs,
            retry_reason,
        } => {
            let result =
                prepare_captured(attempt, &inputs, retry_reason.is_some(), contract, &timing);
            result.map_err(|error| cancel_new(store, attempt, error))
        }
    }
}

pub(super) fn cancel_new(store: &mut Store, attempt: i64, original: BridgeError) -> BridgeError {
    match store.cancel_recovery_attempt(attempt) {
        Ok(true) => original,
        Ok(false) => BridgeError::CancellationFailed {
            original: Box::new(original),
            cancellation: CancellationFailure::Refused,
        },
        Err(error) => BridgeError::CancellationFailed {
            original: Box::new(original),
            cancellation: CancellationFailure::Store(error),
        },
    }
}

fn prepare_captured(
    attempt: i64,
    inputs: &[(i64, Vec<u8>)],
    retry: bool,
    contract: &IncidentContract,
    timing: &RecoveryTiming,
) -> Result<IncidentPreparation> {
    if inputs.len() != contract.mappings.len() + 1
        || inputs.iter().map(|(id, _)| *id).ne(contract.ids())
    {
        return Err(BridgeError::InvalidManifest);
    }
    let context = IncidentContextV1::from_json(&inputs[0].1)?;
    let context_digest = sha256(&inputs[0].1);
    let report_digests: Vec<_> = inputs[1..].iter().map(|(_, bytes)| sha256(bytes)).collect();
    let manifest_hash = sha256(&contract.manifest(&context_digest, &report_digests)?);
    if context != contract.context
        || context_digest != contract.context_sha256
        || report_digests != contract.report_sha256
        || manifest_hash != contract.manifest_sha256
        || manifest_hash != timing.runtime.prompt_fingerprint
    {
        return Err(BridgeError::ChangedContract);
    }
    let reports: Vec<_> = inputs[1..]
        .iter()
        .zip(&contract.mappings)
        .map(|((_, bytes), mapping)| {
            Ok(IncidentReport {
                external_id: mapping.external_id.clone(),
                text: std::str::from_utf8(bytes)
                    .map_err(|_| BridgeError::Protocol(IncidentProtocolError::InvalidEncoding))?
                    .into(),
            })
        })
        .collect::<Result<_>>()?;
    let request = render_incident_request(
        &context,
        &reports,
        if retry {
            IncidentPhase::ProtocolRetry
        } else {
            IncidentPhase::Initial
        },
    )?;
    let preparation = RecoveryPreparation {
        runtime: Some(timing.runtime.clone()),
        calibration_id: Some(timing.calibration.id.clone()),
        input_tokens: request.len() as i64,
        output_cap: contract.output_cap,
        request: request.clone(),
    };
    Ok(IncidentPreparation {
        attempt,
        request,
        captured_ids: inputs.iter().map(|(id, _)| *id).collect(),
        manifest_sha256: manifest_hash,
        preparation,
        context,
    })
}

pub(super) fn fixture_runtime() -> RecoveryRuntime {
    RecoveryRuntime {
        model: "canned-no-model".into(),
        model_revision: "fixture/1".into(),
        tokenizer_revision: "utf8-bytes/1".into(),
        runtime_version: "incident-canned/1".into(),
        dtype: "bytes".into(),
        decoding: "canned-no-inference".into(),
        cpu_fingerprint: sha256(b"synthetic-cpu-not-measured"),
        threads: 1,
        prompt_fingerprint: String::new(),
        readiness: RecoveryReadiness::Warm,
    }
}
pub(super) fn tick(now_ms: i64) -> rusqlite::Result<RecoveryClock> {
    Ok(RecoveryClock {
        domain: "synthetic-incident-clock/1".into(),
        now_ms,
    })
}

pub(super) struct Fixture {
    pub(super) store: Store,
    pub(super) contract: IncidentContract,
    pub(super) ids: Vec<i64>,
    pub(super) recovery: i64,
    pub(super) budget: i64,
}
impl Fixture {
    pub(super) fn new(store: Store) -> Result<Self> {
        Self::for_case(store, "cal-01")
    }

    pub(super) fn for_case(mut store: Store, case_id: &str) -> Result<Self> {
        let (context, reports) = pinned_case(case_id)?;
        let raw = context.to_json()?;
        let observations: Vec<_> = std::iter::once(("incident-context", raw.as_slice()))
            .chain(
                reports
                    .iter()
                    .map(|r| (r.external_id.as_str(), r.text.as_bytes())),
            )
            .collect();
        let ids = store.observe_and_admit_many(ACTOR, SCOPE, &observations)?;
        let mappings = reports
            .iter()
            .zip(&ids[1..])
            .map(|(r, id)| SourceMapping {
                workload_version: VERSION.into(),
                case_id: context.case_id.clone(),
                external_id: r.external_id.clone(),
                occurrence: *id,
            })
            .collect();
        let contract = IncidentContract::new(
            &context.case_id,
            ids[0],
            &raw,
            mappings,
            fixture_runtime(),
            512,
        )?;
        let timing = RecoveryTiming {
            clock_domain: "synthetic-incident-clock/1".into(),
            start_ms: 100,
            deadline_ms: 1000,
            safety_margin_ms: 20,
            runtime: contract.runtime.clone(),
            calibration: RecoveryCalibration {
                id: "synthetic-not-measured/1".into(),
                max_duration_ms: 100,
                sample_count: 2,
                max_input_tokens: 1024 * 1024,
                output_cap: 512,
                measured_at_ms: 0,
                expires_at_ms: 2000,
            },
        };
        let budget = store.create_token_budget(100_000)?;
        let recovery = store.create_timed_recovery(
            "incident-demo/1",
            budget,
            ACTOR,
            SCOPE,
            "verdict",
            &timing,
        )?;
        Ok(Self {
            store,
            contract,
            ids,
            recovery,
            budget,
        })
    }
}

pub(super) fn demo() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut f = Fixture::new(Store::in_memory()?)?;
    let p = prepare_incident_attempt(&mut f.store, f.recovery, &f.ids, &f.contract)?;
    assert!(f.store.reserve_recovery_attempt(
        p.attempt,
        &p.request,
        p.preparation.input_tokens,
        p.preparation.output_cap
    )?);
    let mut calls = 0;
    let result = f.store.run_timed_recovery(p.attempt, &p.preparation, || tick(200), |request| {
        calls += 1;
        // Deliberately format-valid only: this is not a claim of semantic correctness.
        Ok::<_, String>(p.measure(request, br#"{"decision":"abstain","owner":null,"reason":"undetermined_cause","evidence_ids":["e01"]}"#))
    })?;
    assert!(matches!(result, RecoveryRun::Settled { .. }));
    let ledger = f.store.token_budget_state(f.budget)?;
    println!(
        "{}",
        json!({"mode":"synthetic_store_incident_fixture","new_model_calls":0,
        "fixture_backend_invocations":calls,"accounting_unit":"UTF-8 bytes",
        "spent":ledger.spent,"reserved":ledger.reserved,"captured_ids":p.captured_ids,
        "manifest_sha256":p.manifest_sha256,"semantic_correctness_claim":false,
        "calibration_measured":false})
    );
    Ok(())
}
