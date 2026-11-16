//! Version 1 wire records. Optional facts are explicit JSON nulls, never defaults.
use serde::{Deserialize, Serialize};

fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBundleV1 {
    pub schema_version: u32,
    pub root_attempt: i64,
    pub actor: String,
    pub scope: String,
    pub export_purpose: String,
    pub snapshot: BundleSnapshot,
    pub attempts: Vec<AttemptRecord>,
    pub evidence: Vec<EvidenceRecord>,
    pub integrity: IntegrityManifest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSnapshot {
    pub root_branch: i64,
    #[serde(deserialize_with = "required")]
    pub head: Option<i64>,
    pub revision: i64,
    pub current_epoch: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityManifest {
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteEnvelope {
    pub hex: String,
    pub byte_len: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedScope {
    pub actor: String,
    pub scope: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    pub occurrence_id: i64,
    pub source: ByteEnvelope,
    pub payload: ByteEnvelope,
    pub authorized_scopes: Vec<AuthorizedScope>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRecord {
    pub position: i64,
    pub evidence_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRecord {
    pub position: i64,
    pub evidence_id: i64,
    pub role: InputRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub actor: String,
    pub scope: String,
    pub id: i64,
    pub branch: i64,
    pub captured_epoch: i64,
    pub captured_revision: i64,
    #[serde(deserialize_with = "required")]
    pub parent: Option<i64>,
    #[serde(deserialize_with = "required")]
    pub inherited_from: Option<i64>,
    pub candidates: Vec<CandidateRecord>,
    pub inputs: Vec<InputRecord>,
    #[serde(deserialize_with = "required")]
    pub dispatch_request: Option<ByteEnvelope>,
    pub delivery: DeliveryState,
    #[serde(deserialize_with = "required")]
    pub validator: Option<ValidatorBinding>,
    #[serde(deserialize_with = "required")]
    pub immutable_runtime: Option<ByteEnvelope>,
    #[serde(deserialize_with = "required")]
    pub immutable_manifest: Option<ByteEnvelope>,
    #[serde(deserialize_with = "required")]
    pub timing_configuration: Option<ByteEnvelope>,
    pub accounting: AttemptAccounting,
    #[serde(deserialize_with = "required")]
    pub reuse_origin: Option<i64>,
    #[serde(deserialize_with = "required")]
    pub model_contract: Option<String>,
    #[serde(deserialize_with = "required")]
    pub deterministic: Option<DeterministicContract>,
    #[serde(deserialize_with = "required")]
    pub support: Option<SupportContract>,
    #[serde(deserialize_with = "required")]
    pub recovery: Option<RecoveryRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorBinding {
    pub version: String,
    pub provenance: ValidatorProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectionMetadata {
    pub validator: String,
    pub reason: String,
    pub applicability: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptAccounting {
    pub unit: AccountingUnit,
    #[serde(deserialize_with = "required")]
    pub quote: Option<ReservationQuote>,
    #[serde(deserialize_with = "required")]
    pub granted: Option<bool>,
    #[serde(deserialize_with = "required")]
    pub budget_reference: Option<i64>,
    pub charge: i64,
    pub hold_bound: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationQuote {
    pub input_tokens: i64,
    pub output_cap: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeterministicContract {
    pub version: String,
    pub output: ByteEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportContract {
    pub version: String,
    pub rule_id: i64,
    pub verdict: bool,
    pub clauses: Vec<SupportClause>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportClause {
    pub clause_id: i64,
    pub premises: Vec<SupportPremise>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportPremise {
    pub evidence_id: i64,
    pub actor: String,
    pub scope: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    #[serde(deserialize_with = "required")]
    pub request: Option<ByteEnvelope>,
    pub recovery_id: i64,
    pub ordinal: i64,
    pub state: String,
    #[serde(deserialize_with = "required")]
    pub predecessor_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    Context,
    Policy,
    Report,
    Evidence,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedDisposition {
    Accepted,
    StaleView,
    StaleBranch,
    InvalidOutput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorProvenance {
    DeclaredContract,
    RecordedRejection,
    DeterministicBinding,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingUnit {
    LogicalSupport,
    FixtureBytes,
    ModelTokens,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayStatus {
    Matched,
    Mismatch,
    Incomplete,
    NotReproducible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingRequirement {
    ValidatorUnavailable,
    RendererUnavailable,
    MissingInput,
    MissingDelivery,
    MissingDependency,
    UnknownAccountingUnit,
    HistoricalTimingUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliveryState {
    Recorded {
        raw: ByteEnvelope,
        disposition: RecordedDisposition,
        #[serde(deserialize_with = "required")]
        usage: Option<i64>,
        #[serde(deserialize_with = "required")]
        rejection: Option<RejectionMetadata>,
    },
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnitAccounting {
    pub unit: AccountingUnit,
    pub charged: i64,
    pub held: i64,
}
/// An original ledger observation without an established shared measurement unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnknownAccountingObservation {
    pub attempt_id: i64,
    pub charged: i64,
    pub held: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AttemptReplay {
    pub attempt_id: i64,
    pub reconstruction: ReplayStatus,
    pub validation: ReplayStatus,
    pub recorded_disposition: Option<RecordedDisposition>,
    pub validator_provenance: Option<ValidatorProvenance>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplayReport {
    pub report_schema: &'static str,
    pub consistency: ReplayStatus,
    pub reconstruction: ReplayStatus,
    pub validation: ReplayStatus,
    pub recorded_disposition: Option<RecordedDisposition>,
    pub validator_provenance: Option<ValidatorProvenance>,
    pub missing_requirements: Vec<MissingRequirement>,
    pub accounting: Vec<UnitAccounting>,
    pub unknown_accounting: Vec<UnknownAccountingObservation>,
    pub source_bundle_sha256: Option<String>,
    pub reevaluated_validator: Option<String>,
    pub attempts: Vec<AttemptReplay>,
}

/// Caller identity must be authenticated by the embedding runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRequest {
    pub actor: String,
    pub scope: String,
    pub attempt: i64,
    pub purpose: BundlePurpose,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundlePurpose {
    CurrentRead,
    DiagnosticReplay,
}

/// Ephemeral exact selection, never a persisted or transferable access grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticReadRequest {
    pub actor: String,
    pub scope: String,
    pub root_attempt: i64,
    pub purpose: BundlePurpose,
    pub attempts: Vec<SelectedAttempt>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedAttempt {
    pub attempt: i64,
    pub actor: String,
    pub scope: String,
    pub branch: i64,
    pub head: Option<i64>,
    pub revision: i64,
    pub current_epoch: i64,
    pub captured_revision: i64,
    pub captured_epoch: i64,
    pub inputs: Vec<SelectedInput>,
}
/// Repeated occurrences retain their consuming attempt and compacted position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedInput {
    pub position: i64,
    pub occurrence_id: i64,
}

pub trait DiagnosticAuthorizer {
    /// Synchronous trusted policy; must not reenter/mutate this Store or use the
    /// network. Approval covers precisely this retained-snapshot selection.
    fn authorize(&self, request: &DiagnosticReadRequest) -> Result<bool, super::BundleError>;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyDiagnostics;
impl DiagnosticAuthorizer for DenyDiagnostics {
    fn authorize(&self, _: &DiagnosticReadRequest) -> Result<bool, super::BundleError> {
        Ok(false)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "bundle", rename_all = "snake_case")]
pub enum BundleExport {
    Complete(DecisionBundleV1),
    Redacted(RedactedBundleV1),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RedactedBundleV1 {
    pub requested_attempt: i64,
    pub reason: RedactionReason,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionReason {
    Unavailable,
}
