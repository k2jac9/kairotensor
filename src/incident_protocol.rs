//! Pure incident v1 codec. Format validation never grants admission, send
//! permission, or scientific correctness. No Store or backend is consulted.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 64;
const RETRY_FEEDBACK: &str = "incident-routing-protocol/1:invalid_output";

/// Stable, payload-free protocol errors. Rejected bytes are never retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidentProtocolError {
    TooLarge,
    InvalidEncoding,
    InvalidJson,
    UnknownVersion,
    InvalidContext,
    InvalidOutput,
}
impl fmt::Display for IncidentProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "incident protocol: size limit exceeded",
            Self::InvalidEncoding => "incident protocol: invalid UTF-8",
            Self::InvalidJson => "incident protocol: invalid JSON",
            Self::UnknownVersion => "incident protocol: unknown version",
            Self::InvalidContext => "incident protocol: invalid context",
            Self::InvalidOutput => "incident protocol: invalid output",
        })
    }
}
impl std::error::Error for IncidentProtocolError {}
use IncidentProtocolError as Error;

/// Supplied context metadata is not evidence of a trusted workload or authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentContextV1 {
    pub schema_version: u32,
    pub version: String,
    pub workload_version: String,
    pub freeze_sha256: String,
    pub prompt_sha256: String,
    pub validator_version: String,
    pub case_id: String,
    pub question: String,
    pub owner_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub system_prompt: String,
}

// Keep deserialization/serialization behind the validated public byte codec.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextWire {
    schema_version: u32,
    version: String,
    workload_version: String,
    freeze_sha256: String,
    prompt_sha256: String,
    validator_version: String,
    case_id: String,
    question: String,
    owner_ids: Vec<String>,
    evidence_ids: Vec<String>,
    system_prompt: String,
}
impl IncidentContextV1 {
    /// Decode bounded UTF-8 JSON with exact fields and unique object keys.
    pub fn from_json(raw: &[u8]) -> Result<Self, Error> {
        let w: ContextWire = decode(raw, Error::InvalidContext)?;
        let context = Self {
            schema_version: w.schema_version,
            version: w.version,
            workload_version: w.workload_version,
            freeze_sha256: w.freeze_sha256,
            prompt_sha256: w.prompt_sha256,
            validator_version: w.validator_version,
            case_id: w.case_id,
            question: w.question,
            owner_ids: w.owner_ids,
            evidence_ids: w.evidence_ids,
            system_prompt: w.system_prompt,
        };
        context.validate()?;
        Ok(context)
    }

    /// Encode sorted-key compact UTF-8 JSON, checking direct constructions too.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        self.encode()
    }

    fn encode(&self) -> Result<Vec<u8>, Error> {
        canonical(&serde_json::json!({
            "schema_version":self.schema_version, "version":self.version,
            "workload_version":self.workload_version, "freeze_sha256":self.freeze_sha256,
            "prompt_sha256":self.prompt_sha256, "validator_version":self.validator_version,
            "case_id":self.case_id, "question":self.question, "owner_ids":self.owner_ids,
            "evidence_ids":self.evidence_ids, "system_prompt":self.system_prompt,
        }))
    }

    fn validate(&self) -> Result<(), Error> {
        if self.schema_version != 1 || self.version != "incident-store-context/1" {
            return Err(Error::UnknownVersion);
        }
        if ![
            &self.workload_version,
            &self.freeze_sha256,
            &self.prompt_sha256,
            &self.validator_version,
            &self.case_id,
            &self.question,
            &self.system_prompt,
        ]
        .into_iter()
        .all(|s| nonempty(s))
            || !unique_nonempty(&self.owner_ids)
            || !unique_nonempty(&self.evidence_ids)
        {
            return Err(Error::InvalidContext);
        }
        // Bounds apply to the whole context, even fields not included in a request.
        self.encode()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentReport {
    pub external_id: String,
    pub text: String,
}

/// Each phase has fixed rendering behavior; no prior output or free-form feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidentPhase {
    Initial,
    ProtocolRetry,
}

/// Valid format and supplied IDs only, independent of the reports' meaning.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentOutput {
    pub decision: String,
    // Option ordinarily permits missing keys; frozen v1 requires an explicit key.
    #[serde(deserialize_with = "required_owner")]
    pub owner: Option<String>,
    pub reason: String,
    pub evidence_ids: Vec<String>,
}
fn required_owner<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// Render `incident-request/1` or `incident-request-retry/1` with Python v1 bytes.
pub fn render_incident_request(
    context: &IncidentContextV1,
    reports: &[IncidentReport],
    phase: IncidentPhase,
) -> Result<Vec<u8>, Error> {
    context.validate()?;
    if reports.len() != context.evidence_ids.len()
        || reports
            .iter()
            .zip(&context.evidence_ids)
            .any(|(r, id)| &r.external_id != id || !nonempty(&r.text))
    {
        return Err(Error::InvalidContext);
    }
    let evidence: Vec<_> = reports
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.external_id, "text": r.text,
            })
        })
        .collect();
    let mut payload = serde_json::json!({
        "question": context.question, "owner_ids": context.owner_ids, "evidence": evidence,
    });
    if phase == IncidentPhase::ProtocolRetry {
        payload["validation_error"] = RETRY_FEEDBACK.into();
    }
    let user = String::from_utf8(canonical(&payload)?).map_err(|_| Error::InvalidEncoding)?;
    canonical(&serde_json::json!({"messages": [
        {"role": "system", "content": context.system_prompt},
        {"role": "user", "content": user},
    ]}))
}

/// Validate frozen v1 structure and supplied IDs, never labels or semantics.
pub fn validate_incident_output(
    raw: &[u8],
    context: &IncidentContextV1,
) -> Result<IncidentOutput, Error> {
    context.validate()?;
    let output: IncidentOutput = decode(raw, Error::InvalidOutput)?;
    let valid_decision = match output.decision.as_str() {
        "route" => {
            output
                .owner
                .as_ref()
                .is_some_and(|owner| context.owner_ids.contains(owner))
                && output.reason == "diagnosed_component"
        }
        "abstain" => {
            output.owner.is_none()
                && matches!(
                    output.reason.as_str(),
                    "undetermined_cause" | "conflicting_diagnoses" | "owner_not_unique"
                )
        }
        _ => false,
    };
    if !valid_decision
        || !unique_nonempty(&output.evidence_ids)
        || output
            .evidence_ids
            .iter()
            .any(|id| !context.evidence_ids.contains(id))
    {
        return Err(Error::InvalidOutput);
    }
    Ok(output)
}

fn nonempty(s: &str) -> bool {
    // Python str.strip additionally treats the four ASCII information
    // separators as whitespace; Rust char::is_whitespace does not.
    s.chars()
        .any(|c| !c.is_whitespace() && !('\u{1c}'..='\u{1f}').contains(&c))
}
fn unique_nonempty(ids: &[String]) -> bool {
    let mut seen = HashSet::new();
    !ids.is_empty() && ids.iter().all(|s| nonempty(s) && seen.insert(s))
}
fn canonical(value: &serde_json::Value) -> Result<Vec<u8>, Error> {
    // serde_json's default Map is a BTreeMap: sorted keys, compact separators,
    // literal UTF-8, and Python-compatible string escaping.
    let raw = serde_json::to_vec(value).map_err(|_| Error::InvalidJson)?;
    if raw.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(raw)
}
fn decode<'a, T: Deserialize<'a>>(raw: &'a [u8], invalid: Error) -> Result<T, Error> {
    if raw.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let source = std::str::from_utf8(raw).map_err(|_| Error::InvalidEncoding)?;
    check_depth(source)?;
    serde_json::from_str(source).map_err(|e| match e.classify() {
        serde_json::error::Category::Data => invalid,
        _ => Error::InvalidJson,
    })
}
fn check_depth(source: &str) -> Result<(), Error> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for byte in source.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err(Error::InvalidJson);
                    }
                }
                b']' | b'}' => {
                    depth = depth.checked_sub(1).ok_or(Error::InvalidJson)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
