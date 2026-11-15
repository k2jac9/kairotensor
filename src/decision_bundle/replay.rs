//! Pure allowlist replay. No Store, adapter, callback, filesystem or process API.
use super::*;
use crate::incident_protocol::{
    render_incident_request, validate_incident_output, IncidentContextV1, IncidentPhase,
    IncidentReport,
};
use crate::{RecoveryRuntime, RecoveryTiming};
use serde::Deserialize;

fn missing(report: &mut ReplayReport, requirement: MissingRequirement) {
    if !report.missing_requirements.contains(&requirement) {
        report.missing_requirements.push(requirement);
    }
}
fn merge(a: ReplayStatus, b: ReplayStatus) -> ReplayStatus {
    use ReplayStatus::*;
    if a == Mismatch || b == Mismatch {
        Mismatch
    } else if a == Incomplete || b == Incomplete {
        Incomplete
    } else if a == NotReproducible || b == NotReproducible {
        NotReproducible
    } else {
        Matched
    }
}
fn disposition(a: &AttemptRecord) -> Option<RecordedDisposition> {
    match a.delivery {
        DeliveryState::Recorded { disposition, .. } => Some(disposition),
        DeliveryState::Unresolved => None,
    }
}
fn provenance(a: &AttemptRecord) -> Option<ValidatorProvenance> {
    if let DeliveryState::Recorded {
        rejection: Some(_), ..
    } = &a.delivery
    {
        Some(ValidatorProvenance::RecordedRejection)
    } else {
        a.validator.as_ref().map(|v| v.provenance)
    }
}
/// Evaluate captured versions only; historical publication and timing are not replayed.
pub fn validate_recorded_bundle(bundle: &DecisionBundleV1) -> ReplayReport {
    replay(bundle, None)
}
/// A separate linked evaluation; the original bundle and disposition stay unchanged.
pub fn reevaluate_bundle(bundle: &DecisionBundleV1, validator: &str) -> ReplayReport {
    replay(bundle, Some(validator))
}
fn replay(b: &DecisionBundleV1, new_validator: Option<&str>) -> ReplayReport {
    let mut result = ReplayReport {
        consistency: ReplayStatus::Mismatch,
        reconstruction: ReplayStatus::NotReproducible,
        validation: ReplayStatus::NotReproducible,
        recorded_disposition: None,
        validator_provenance: None,
        missing_requirements: vec![],
        accounting: vec![],
        source_bundle_sha256: None,
        reevaluated_validator: None,
        attempts: vec![],
    };
    // Even report metadata is cloned only after bounded public-value validation.
    if new_validator.is_some_and(|v| v.len() > MAX_PAYLOAD_BYTES) {
        return result;
    }
    let Ok(value) = check_bundle(b) else {
        return result;
    };
    result.source_bundle_sha256 = canonical(&value).ok().map(|bytes| sha256_hex(&bytes));
    result.reevaluated_validator = new_validator.map(str::to_owned);
    result.consistency = ReplayStatus::Matched;
    result.reconstruction = ReplayStatus::Matched;
    result.validation = ReplayStatus::Matched;
    missing(&mut result, MissingRequirement::HistoricalTimingUnavailable);
    for a in &b.attempts {
        let unit = a.accounting.unit;
        if unit == AccountingUnit::Unknown {
            missing(&mut result, MissingRequirement::UnknownAccountingUnit);
        }
        if let Some(t) = result.accounting.iter_mut().find(|t| t.unit == unit) {
            t.charged += a.accounting.charge;
            t.held += a.accounting.hold_bound;
        } else {
            result.accounting.push(UnitAccounting {
                unit,
                charged: a.accounting.charge,
                held: a.accounting.hold_bound,
            });
        }
        let mut item = AttemptReplay {
            attempt_id: a.id,
            reconstruction: ReplayStatus::NotReproducible,
            validation: ReplayStatus::NotReproducible,
            recorded_disposition: disposition(a),
            validator_provenance: provenance(a),
        };
        let inputs_missing = a
            .inputs
            .iter()
            .any(|i| !b.evidence.iter().any(|e| e.occurrence_id == i.evidence_id));
        let dependencies_missing =
            !dependencies_complete(b, a, &mut Vec::new(), &mut HashSet::new());
        if inputs_missing {
            missing(&mut result, MissingRequirement::MissingInput);
        }
        if dependencies_missing {
            missing(&mut result, MissingRequirement::MissingDependency);
        }
        if inputs_missing || dependencies_missing {
            item.reconstruction = ReplayStatus::Incomplete;
            item.validation = ReplayStatus::Incomplete;
        } else {
            let (reconstruction, validation) = evaluate(b, a, new_validator, &mut result);
            item.reconstruction = reconstruction;
            item.validation = validation;
        }
        if matches!(a.delivery, DeliveryState::Unresolved) {
            missing(&mut result, MissingRequirement::MissingDelivery);
            item.validation = ReplayStatus::Incomplete;
        }
        result.reconstruction = merge(result.reconstruction, item.reconstruction);
        result.validation = merge(result.validation, item.validation);
        if a.id == b.root_attempt {
            result.recorded_disposition = item.recorded_disposition;
            result.validator_provenance = item.validator_provenance;
        }
        result.attempts.push(item);
    }
    result
}
fn dependencies_complete(
    b: &DecisionBundleV1,
    a: &AttemptRecord,
    path: &mut Vec<i64>,
    complete_ids: &mut HashSet<i64>,
) -> bool {
    if complete_ids.contains(&a.id) {
        return true;
    }
    if path.contains(&a.id) {
        return false;
    }
    path.push(a.id);
    let complete = [a.inherited_from, a.reuse_origin]
        .into_iter()
        .flatten()
        .all(|id| {
            b.attempts.iter().find(|d| d.id == id).is_some_and(|d| {
                matches!(d.delivery, DeliveryState::Recorded { .. })
                    && dependencies_complete(b, d, path, complete_ids)
            })
        });
    path.pop();
    if complete {
        complete_ids.insert(a.id);
    }
    complete
}
fn payload(b: &DecisionBundleV1, id: i64) -> Result<Vec<u8>, Error> {
    b.evidence
        .iter()
        .find(|e| e.occurrence_id == id)
        .ok_or(Error::InconsistentLedger)?
        .payload
        .decode()
}
fn evaluate(
    b: &DecisionBundleV1,
    a: &AttemptRecord,
    new_validator: Option<&str>,
    report: &mut ReplayReport,
) -> (ReplayStatus, ReplayStatus) {
    use ReplayStatus::*;
    let mut reconstruction = NotReproducible;
    let mut derived = None;
    let mut incident = None;
    if let Some(d) = &a.deterministic {
        if d.version == "report-verdict/v1" {
            derived = match a.inputs.as_slice() {
                [input]
                    if a.candidates.len() == 1
                        && a.candidates[0].evidence_id == input.evidence_id =>
                {
                    payload(b, input.evidence_id)
                        .ok()
                        .and_then(|raw| crate::report_verdict::derive(&raw).ok())
                }
                _ => None,
            };
            reconstruction = if derived
                .as_ref()
                .is_some_and(|bytes| d.output.decode().ok().as_ref() == Some(bytes))
            {
                Matched
            } else {
                Mismatch
            };
        } else {
            missing(report, MissingRequirement::RendererUnavailable);
        }
    } else if a.immutable_manifest.is_some() {
        match reconstruct_incident(b, a) {
            Ok((context, request)) => {
                reconstruction = match a
                    .dispatch_request
                    .as_ref()
                    .or_else(|| a.recovery.as_ref().and_then(|r| r.request.as_ref()))
                {
                    Some(raw) => {
                        if raw.decode().ok().as_ref() == Some(&request) {
                            Matched
                        } else {
                            Mismatch
                        }
                    }
                    None => {
                        missing(report, MissingRequirement::MissingInput);
                        Incomplete
                    }
                };
                incident = Some(context);
            }
            Err(IncidentFailure::Unavailable) => {
                missing(report, MissingRequirement::RendererUnavailable);
            }
            Err(IncidentFailure::Missing) => {
                missing(report, MissingRequirement::MissingDependency);
                reconstruction = Incomplete;
            }
            Err(IncidentFailure::Mismatch) => {
                reconstruction = Mismatch;
            }
        }
    } else {
        missing(report, MissingRequirement::RendererUnavailable);
    }
    let selected = new_validator.or_else(|| match &a.delivery {
        DeliveryState::Recorded {
            rejection: Some(r), ..
        } => Some(r.validator.as_str()),
        _ => a
            .validator
            .as_ref()
            .map(|v| v.version.as_str())
            .or_else(|| a.deterministic.as_ref().map(|d| d.version.as_str())),
    });
    let DeliveryState::Recorded {
        raw, disposition, ..
    } = &a.delivery
    else {
        return (reconstruction, Incomplete);
    };
    let raw = raw.decode().expect("checked byte envelope");
    let evaluation = match selected {
        Some("report-verdict/v1")
            if a.deterministic
                .as_ref()
                .is_some_and(|d| d.version == "report-verdict/v1") =>
        {
            Some(derived.as_ref().is_some_and(|output| *output == raw))
        }
        Some("incident-routing-protocol/1") => incident
            .as_ref()
            .map(|context| validate_incident_output(&raw, context).is_ok()),
        _ => None,
    };
    let validation = match evaluation {
        Some(valid) => {
            // Changed-validator evaluation describes validity itself. Recorded replay
            // compares format validity with the recorded acceptance/rejection class.
            let expected = if new_validator.is_some() {
                true
            } else {
                *disposition != RecordedDisposition::InvalidOutput
            };
            if valid == expected {
                Matched
            } else {
                Mismatch
            }
        }
        None => {
            missing(report, MissingRequirement::ValidatorUnavailable);
            NotReproducible
        }
    };
    (reconstruction, validation)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    context: ManifestContext,
    freeze_sha256: String,
    initial_renderer: String,
    output_cap: i64,
    prompt_sha256: String,
    retry_feedback: String,
    retry_renderer: String,
    runtime: ManifestRuntime,
    schema_version: u32,
    sources: Vec<ManifestSource>,
    validator_version: String,
    version: String,
    workload_version: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestContext {
    occurrence_id: i64,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestSource {
    case_id: String,
    external_id: String,
    occurrence_id: i64,
    payload_sha256: String,
    workload_version: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestRuntime {
    cpu_fingerprint: String,
    decoding: String,
    dtype: String,
    model: String,
    model_revision: String,
    readiness: crate::RecoveryReadiness,
    runtime_version: String,
    threads: i64,
    tokenizer_revision: String,
}
enum IncidentFailure {
    Unavailable,
    Missing,
    Mismatch,
}
fn reconstruct_incident(
    b: &DecisionBundleV1,
    a: &AttemptRecord,
) -> Result<(IncidentContextV1, Vec<u8>), IncidentFailure> {
    use IncidentFailure::*;
    let envelope = a.immutable_manifest.as_ref().ok_or(Missing)?;
    let manifest_bytes = envelope.decode().map_err(|_| Mismatch)?;
    let manifest: Manifest = decode_strict(&manifest_bytes).map_err(|_| Unavailable)?;
    if manifest.schema_version != 1
        || manifest.version != "incident-store-manifest/1"
        || manifest.workload_version != "incident-routing/1"
        || manifest.validator_version != "incident-routing-protocol/1"
        || manifest.initial_renderer != "incident-request/1"
        || manifest.retry_renderer != "incident-request-retry/1"
        || manifest.retry_feedback != "incident-routing-protocol/1:invalid_output"
    {
        return Err(Unavailable);
    }
    let recovery = a.recovery.as_ref().ok_or(Missing)?;
    if !matches!(
        recovery.state.as_str(),
        "planned" | "reserved" | "sent" | "cancelled" | "denied"
    ) {
        return Err(Unavailable);
    }
    let phase = match recovery.ordinal {
        0 if recovery.predecessor_id.is_none() => IncidentPhase::Initial,
        1 if recovery.predecessor_id.is_some() => IncidentPhase::ProtocolRetry,
        _ => return Err(Unavailable),
    };
    if let Some(id) = recovery.predecessor_id {
        let previous = b.attempts.iter().find(|p| p.id == id).ok_or(Missing)?;
        if !previous
            .recovery
            .as_ref()
            .is_some_and(|r| r.recovery_id == recovery.recovery_id && r.ordinal == 0)
            || disposition(previous) != Some(RecordedDisposition::InvalidOutput)
            || previous.immutable_manifest != a.immutable_manifest
        {
            return Err(Mismatch);
        }
    }
    let runtime_bytes = a
        .immutable_runtime
        .as_ref()
        .ok_or(Missing)?
        .decode()
        .map_err(|_| Mismatch)?;
    let runtime: RecoveryRuntime = decode_strict(&runtime_bytes).map_err(|_| Mismatch)?;
    let timing_bytes = a
        .timing_configuration
        .as_ref()
        .ok_or(Missing)?
        .decode()
        .map_err(|_| Mismatch)?;
    let timing: RecoveryTiming = decode_strict(&timing_bytes).map_err(|_| Mismatch)?;
    if runtime != timing.runtime
        || runtime.prompt_fingerprint != sha256_hex(&manifest_bytes)
        || manifest.output_cap != timing.calibration.output_cap
        || manifest.output_cap <= 0
        || runtime.threads <= 0
    {
        return Err(Mismatch);
    }
    let mut runtime_value = serde_json::to_value(&runtime).map_err(|_| Mismatch)?;
    runtime_value
        .as_object_mut()
        .unwrap()
        .remove("prompt_fingerprint");
    if runtime_value != serde_json::to_value(&manifest.runtime).map_err(|_| Mismatch)? {
        return Err(Mismatch);
    }
    // Exact canonical manifest encoding is part of B's fingerprint binding.
    let manifest_value: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).map_err(|_| Mismatch)?;
    if canonical(&manifest_value).map_err(|_| Mismatch)? != manifest_bytes {
        return Err(Mismatch);
    }
    let Some(first) = a.inputs.first() else {
        return Err(Missing);
    };
    if a.inputs.len() > 1024
        || a.inputs.len() != manifest.sources.len() + 1
        || first.evidence_id != manifest.context.occurrence_id
        || a.candidates.len() != a.inputs.len()
        || a.candidates
            .iter()
            .zip(&a.inputs)
            .any(|(c, i)| c.evidence_id != i.evidence_id || c.position != i.position)
        || a.inputs
            .iter()
            .enumerate()
            .any(|(position, i)| i.position != position as i64)
    {
        return Err(Mismatch);
    }
    let context_bytes = payload(b, first.evidence_id).map_err(|_| Missing)?;
    if sha256_hex(&context_bytes) != manifest.context.sha256 {
        return Err(Mismatch);
    }
    let context = IncidentContextV1::from_json(&context_bytes).map_err(|_| Mismatch)?;
    if context.freeze_sha256 != manifest.freeze_sha256
        || context.prompt_sha256 != manifest.prompt_sha256
        || context.workload_version != manifest.workload_version
        || context.validator_version != manifest.validator_version
        || context.evidence_ids.len() != manifest.sources.len()
    {
        return Err(Mismatch);
    }
    let mut reports = Vec::new();
    let mut ids = HashSet::new();
    ids.insert(first.evidence_id);
    for ((input, source), external) in a.inputs[1..]
        .iter()
        .zip(&manifest.sources)
        .zip(&context.evidence_ids)
    {
        if !ids.insert(input.evidence_id)
            || input.evidence_id != source.occurrence_id
            || source.case_id != context.case_id
            || source.external_id != *external
            || source.workload_version != manifest.workload_version
        {
            return Err(Mismatch);
        }
        let raw = payload(b, input.evidence_id).map_err(|_| Missing)?;
        if sha256_hex(&raw) != source.payload_sha256 {
            return Err(Mismatch);
        }
        let text = String::from_utf8(raw).map_err(|_| Mismatch)?;
        reports.push(IncidentReport {
            external_id: source.external_id.clone(),
            text,
        });
    }
    let request = render_incident_request(&context, &reports, phase).map_err(|_| Mismatch)?;
    Ok((context, request))
}

pub(super) fn recognized_incident_manifest(raw: &[u8]) -> bool {
    decode_strict::<Manifest>(raw).is_ok_and(|m| {
        m.schema_version == 1
            && m.version == "incident-store-manifest/1"
            && m.workload_version == "incident-routing/1"
            && m.validator_version == "incident-routing-protocol/1"
    })
}

pub(super) fn fixture_runtime(a: &AttemptRecord) -> bool {
    let Some(raw) = a.immutable_runtime.as_ref().and_then(|e| e.decode().ok()) else {
        return false;
    };
    let Ok(runtime) = decode_strict::<RecoveryRuntime>(&raw) else {
        return false;
    };
    runtime.model == "canned-no-model"
        && runtime.model_revision == "fixture/1"
        && runtime.tokenizer_revision == "utf8-bytes/1"
        && runtime.runtime_version == "incident-canned/1"
        && runtime.dtype == "bytes"
        && runtime.decoding == "canned-no-inference"
        && a.immutable_manifest
            .as_ref()
            .is_some_and(|m| runtime.prompt_fingerprint == m.sha256)
}
