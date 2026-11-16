use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyName {
    AlwaysStop,
    AffordableRetry,
    EvidenceGatedRetry,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyArtifactV1 {
    pub schema_version: u32,
    pub policy_name: PolicyName,
    pub purpose: String,
    pub compatibility_contract_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyCandidate {
    pub version_id: String,
    pub actor: String,
    pub scope: String,
    pub purpose: String,
    pub artifact: Vec<u8>,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    SyntheticRegistryMechanics,
    ObservedPolicyReplay,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingUnit {
    ModelTokens,
    FixtureBytes,
    LogicalSupport,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Accepted,
    Refused,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Execution {
    Executed,
    NotExecuted,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptDisposition {
    Accepted,
    InvalidOutput,
    Stale,
    Failed,
    Uncertain,
    Refused,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigestedRecord {
    pub bytes: Vec<u8>,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenPolicy {
    pub policy_name: PolicyName,
    pub artifact: DigestedRecord,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenLabel {
    pub answerable: bool,
    pub admissible_support: bool,
    pub normalized_format: String,
    pub scorer_sha256: String,
    pub expected_normalized: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestOpportunity {
    pub opportunity_id: String,
    pub request: DigestedRecord,
    pub evidence_manifest: DigestedRecord,
    pub admitted: bool,
    pub consistent: bool,
    pub input_bound: u64,
    pub output_cap: u64,
    pub start_min_ms: u64,
    pub start_max_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyOpportunities {
    pub policy_name: PolicyName,
    pub opportunities: Vec<RequestOpportunity>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCase {
    pub case_id: String,
    pub family_id: String,
    pub split_id: String,
    pub initial_evidence: DigestedRecord,
    pub label: DigestedRecord,
    pub budget: u64,
    pub deadline_ms: u64,
    pub timing_start_ms: u64,
    pub timing_end_ms: u64,
    pub policy_opportunities: Vec<PolicyOpportunities>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonManifest {
    pub format: String,
    pub actor: String,
    pub scope: String,
    pub purpose: String,
    pub provenance: Provenance,
    pub accounting_unit: AccountingUnit,
    pub workload_sha256: String,
    pub prompt_sha256: String,
    pub runtime_sha256: String,
    pub model_sha256: String,
    pub tokenizer_sha256: String,
    pub validator_sha256: String,
    pub scorer_sha256: String,
    pub compatibility_contract_sha256: String,
    pub policies: Vec<FrozenPolicy>,
    pub cases: Vec<FrozenCase>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoringAssertion {
    pub response_sha256: String,
    pub scorer_sha256: String,
    pub label_sha256: String,
    pub normalized: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedAttempt {
    pub execution_id: String,
    pub opportunity_id: String,
    pub request_sha256: String,
    pub evidence_manifest_sha256: String,
    pub execution: Execution,
    pub disposition: AttemptDisposition,
    #[serde(deserialize_with = "super::nullable")]
    pub response: Option<DigestedRecord>,
    #[serde(deserialize_with = "super::nullable")]
    pub measured_input: Option<u64>,
    #[serde(deserialize_with = "super::nullable")]
    pub measured_output: Option<u64>,
    pub retained_bound: u64,
    pub start_ms: u64,
    #[serde(deserialize_with = "super::nullable")]
    pub completed_ms: Option<u64>,
    pub validator_sha256: String,
    pub scorer_sha256: String,
    pub label_sha256: String,
    #[serde(deserialize_with = "super::nullable")]
    pub scoring: Option<ScoringAssertion>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRow {
    pub case_id: String,
    pub outcome: Outcome,
    #[serde(deserialize_with = "super::nullable")]
    pub correct: Option<bool>,
    #[serde(deserialize_with = "super::nullable")]
    pub eligible: Option<bool>,
    pub answerable: bool,
    #[serde(deserialize_with = "super::nullable")]
    pub completed_ms: Option<u64>,
    #[serde(deserialize_with = "super::nullable")]
    pub charged: Option<u64>,
    pub reserved_bound: u64,
    pub false_refusal: bool,
    pub unsafe_acceptance: bool,
    pub deadline_miss: bool,
    pub simulated: bool,
    pub attempts: Vec<ObservedAttempt>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyEvaluation {
    pub evaluation_id: String,
    pub actor: String,
    pub scope: String,
    pub purpose: String,
    pub baseline_id: String,
    pub candidate_id: String,
    pub baseline_sha256: String,
    pub candidate_sha256: String,
    pub experiment_manifest: Vec<u8>,
    pub experiment_sha256: String,
    pub evaluator_version: String,
    pub evaluator_sha256: String,
    pub provenance: Provenance,
    pub baseline_rows: Vec<PolicyRow>,
    pub candidate_rows: Vec<PolicyRow>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyCounters {
    pub cases: u64,
    pub success: u64,
    pub false_refusal: u64,
    pub unsafe_acceptance: u64,
    pub cost: u64,
    pub deadline_misses: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedCounters {
    pub baseline: PolicyCounters,
    pub candidate: PolicyCounters,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionRefusal {
    Incomplete,
    Incomparable,
    NoImprovement,
    IncreasedFalseRefusal,
    IncreasedUnsafeAcceptance,
    IncreasedCost,
    IncreasedDeadlineMisses,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionDecision {
    Eligible {
        validated_counters: ValidatedCounters,
    },
    Refused(SelectionRefusal),
}

/// Trusted management request, never an authorization to send model work.
#[derive(Clone, Debug, Serialize)]
pub struct ActivationRequest {
    pub actor: String,
    pub scope: String,
    pub purpose: String,
    pub expected_active_revision: Option<i64>,
    pub action: ActivationAction,
}
#[derive(Clone, Debug, Serialize)]
pub enum ActivationAction {
    Bootstrap {
        baseline_id: String,
        baseline_manifest: Vec<u8>,
        baseline_manifest_sha256: String,
        reason: String,
    },
    Promote {
        candidate_id: String,
        evaluation_id: String,
    },
    Rollback {
        target_version_id: String,
        reason: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivePolicy {
    pub version_id: String,
    pub sha256: String,
    pub evidence_id: i64,
    pub compatibility_contract_sha256: String,
    pub revision: i64,
    pub baseline_manifest_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationResult {
    Activated { revision: i64 },
    Refused(ActivationRefusal),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationRefusal {
    RevisionConflict,
    AlreadyInitialized,
    MissingHead,
    InvalidArtifact,
    ScopeMismatch,
    Incompatible,
    BaselineMismatch,
    MissingBaselineEvaluation,
    EvaluationRefused(SelectionRefusal),
    NeverActivated,
    AdmissionMismatch,
    NoChange,
}
