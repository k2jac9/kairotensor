//! Offline typed Policy + Report fixture. No provider, adapter, or model calls.
use kairotensor::policy_registry::*;
use kairotensor::{CapturedInputs, Current, Disposition, RecoveryStart, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{error::Error, path::Path};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const PURPOSE: &str = "fixture/recovery-policy";
const CONTRACT: &str = "policy-report-fixture/1";
const CONTRACT_BYTES: &[u8] =
    b"policy-report-fixture/1:one-policy-then-reports;fixture-byte-runtime/1";
const FEEDBACK: &str = "Return complete JSON matching the captured Reports; do not invent a verdict for conflicting or incomplete reports.";
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Occurrence {
    role: Role,
    evidence_id: i64,
    sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Role {
    Policy,
    Report,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    actor: String,
    scope: String,
    purpose: String,
    active_version_id: String,
    active_revision: i64,
    active_evidence_id: i64,
    active_sha256: String,
    compatibility_contract_sha256: String,
    baseline_manifest_sha256: String,
    occurrences: Vec<Occurrence>,
}
struct Prepared {
    attempt: i64,
    branch: String,
    request: Vec<u8>,
}
struct Fixture {
    store: Store,
    evaluation: PolicyEvaluation,
    budget: i64,
    reports: Vec<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureInput {
    format: String,
    new_model_calls: u64,
    evaluation: PolicyEvaluation,
    #[serde(rename = "limitations")]
    _limitations: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    release: String,
    passed: u64,
    total: u64,
}
impl Fixture {
    fn open(path: &Path, evaluation: PolicyEvaluation) -> Result<Self> {
        if evaluation.purpose != PURPOSE
            || evaluation.provenance != Provenance::SyntheticRegistryMechanics
            || !matches!(
                evaluate_policy_comparison(&evaluation)?,
                SelectionDecision::Eligible { .. }
            )
        {
            return Err("fixture requires a qualifying explicitly synthetic evaluation".into());
        }
        let m: ComparisonManifest = serde_json::from_slice(&evaluation.experiment_manifest)?;
        if m.compatibility_contract_sha256 != sha(CONTRACT_BYTES) {
            return Err("unsupported fixture compatibility contract".into());
        }
        let mut store = Store::open(path)?;
        for (version_id, digest, expected) in [
            (
                &evaluation.baseline_id,
                &evaluation.baseline_sha256,
                PolicyName::AlwaysStop,
            ),
            (
                &evaluation.candidate_id,
                &evaluation.candidate_sha256,
                PolicyName::AffordableRetry,
            ),
        ] {
            let policy = m
                .policies
                .iter()
                .find(|p| &p.artifact.sha256 == digest)
                .ok_or("missing fixture policy")?;
            if policy.policy_name != expected {
                return Err("unexpected fixture policy name".into());
            }
            store.register_policy_candidate(&PolicyCandidate {
                version_id: version_id.clone(),
                actor: evaluation.actor.clone(),
                scope: evaluation.scope.clone(),
                purpose: evaluation.purpose.clone(),
                artifact: policy.artifact.bytes.clone(),
                sha256: digest.clone(),
            })?;
        }
        let request = ActivationRequest {
            actor: evaluation.actor.clone(),
            scope: evaluation.scope.clone(),
            purpose: PURPOSE.into(),
            expected_active_revision: None,
            action: ActivationAction::Bootstrap {
                baseline_id: evaluation.baseline_id.clone(),
                baseline_manifest: evaluation.experiment_manifest.clone(),
                baseline_manifest_sha256: evaluation.experiment_sha256.clone(),
                reason: "explicit synthetic fixture baseline; no superiority assertion".into(),
            },
        };
        if store.activate_policy(&request)? != (ActivationResult::Activated { revision: 1 }) {
            return Err("fixture bootstrap refused".into());
        }
        let mut baseline = evaluation.clone();
        baseline.evaluation_id = format!("{}-baseline", evaluation.evaluation_id);
        baseline.candidate_id = baseline.baseline_id.clone();
        baseline.candidate_sha256 = baseline.baseline_sha256.clone();
        baseline.candidate_rows = baseline.baseline_rows.clone();
        store.record_policy_evaluation(&baseline)?;
        store.record_policy_evaluation(&evaluation)?;
        let reports = store.observe_and_admit_many(
            &evaluation.actor,
            &evaluation.scope,
            &[(
                "fixture-report",
                br#"{"release":"fixture","passed":3,"total":3}"#,
            )],
        )?;
        let budget = store.create_token_budget(100_000)?;
        Ok(Self {
            store,
            evaluation,
            budget,
            reports,
        })
    }
    fn manifest(&self) -> Result<Manifest> {
        let e = &self.evaluation;
        let head = self
            .store
            .active_policy(&e.actor, &e.scope, PURPOSE)?
            .ok_or("missing active fixture policy")?;
        let mut occurrences = vec![Occurrence {
            role: Role::Policy,
            evidence_id: head.evidence_id,
            sha256: head.sha256.clone(),
        }];
        for id in &self.reports {
            let bytes = self
                .store
                .read_evidence(&e.actor, &e.scope, *id)?
                .ok_or("report unavailable")?;
            occurrences.push(Occurrence {
                role: Role::Report,
                evidence_id: *id,
                sha256: sha(&bytes),
            });
        }
        Ok(Manifest {
            format: CONTRACT.into(),
            actor: e.actor.clone(),
            scope: e.scope.clone(),
            purpose: PURPOSE.into(),
            active_version_id: head.version_id,
            active_revision: head.revision,
            active_evidence_id: head.evidence_id,
            active_sha256: head.sha256,
            compatibility_contract_sha256: head.compatibility_contract_sha256,
            baseline_manifest_sha256: head.baseline_manifest_sha256,
            occurrences,
        })
    }
    fn transition(&mut self, action: ActivationAction) -> Result<ActivationResult> {
        let e = &self.evaluation;
        let revision = self
            .store
            .active_policy(&e.actor, &e.scope, PURPOSE)?
            .ok_or("missing fixture head")?
            .revision;
        Ok(self.store.activate_policy(&ActivationRequest {
            actor: e.actor.clone(),
            scope: e.scope.clone(),
            purpose: PURPOSE.into(),
            expected_active_revision: Some(revision),
            action,
        })?)
    }
    fn promote(&mut self) -> Result<ActivationResult> {
        self.transition(ActivationAction::Promote {
            candidate_id: self.evaluation.candidate_id.clone(),
            evaluation_id: self.evaluation.evaluation_id.clone(),
        })
    }
    fn rollback(&mut self) -> Result<ActivationResult> {
        self.transition(ActivationAction::Rollback {
            target_version_id: self.evaluation.baseline_id.clone(),
            reason: "fixture rollback to previously active compatible baseline".into(),
        })
    }
}
fn render(manifest: &Manifest, inputs: &CapturedInputs, head: &ActivePolicy) -> Result<Value> {
    if manifest.format != CONTRACT
        || manifest.purpose != PURPOSE
        || manifest.active_version_id != head.version_id
        || manifest.active_revision != head.revision
        || manifest.active_evidence_id != head.evidence_id
        || manifest.active_sha256 != head.sha256
        || manifest.compatibility_contract_sha256 != head.compatibility_contract_sha256
        || manifest.compatibility_contract_sha256 != sha(CONTRACT_BYTES)
        || manifest.baseline_manifest_sha256 != head.baseline_manifest_sha256
    {
        return Err("selected active policy changed or incompatible contract".into());
    }
    if manifest.occurrences.len() < 2
        || manifest.occurrences[0].role != Role::Policy
        || manifest.occurrences[0].evidence_id != head.evidence_id
        || manifest.occurrences[0].sha256 != head.sha256
    {
        return Err("position zero must be the exact active policy occurrence".into());
    }
    // The legacy candidate API intentionally filters unavailable inputs. This
    // new complete typed contract refuses any filtering, reorder, or replacement.
    if inputs.len() != manifest.occurrences.len() {
        return Err("incomplete captured inputs".into());
    }
    let mut reports = Vec::new();
    for (position, (occurrence, (id, bytes))) in manifest.occurrences.iter().zip(inputs).enumerate()
    {
        if *id != occurrence.evidence_id || sha(bytes) != occurrence.sha256 {
            return Err("captured occurrence/digest mismatch".into());
        }
        if position == 0 {
            let artifact = validate_policy_artifact(bytes)?;
            if artifact.purpose != PURPOSE
                || artifact.compatibility_contract_sha256 != head.compatibility_contract_sha256
                || sha(bytes) != head.sha256
            {
                return Err("captured policy artifact is incompatible".into());
            }
        } else {
            if occurrence.role != Role::Report {
                return Err("only Report roles may follow the Policy".into());
            }
            let report: Report = serde_json::from_slice(bytes)?;
            if report.release.is_empty() || report.total == 0 || report.passed > report.total {
                return Err("invalid captured Report".into());
            }
            reports.push(report);
        }
    }
    Ok(json!({"ci_reports": reports}))
}
fn prepare(store: &mut Store, budget: i64, key: &str, manifest: &Manifest) -> Result<Prepared> {
    let binding = sha(&serde_json::to_vec(manifest)?);
    let branch = format!("{CONTRACT}/{binding}/{key}");
    let recovery =
        store.create_recovery(&branch, budget, &manifest.actor, &manifest.scope, &branch)?;
    let candidates: Vec<_> = manifest.occurrences.iter().map(|o| o.evidence_id).collect();
    let RecoveryStart::New {
        attempt, inputs, ..
    } = store.begin_recovery_attempt(recovery, &candidates)?
    else {
        return Err("fixture recovery already exists; never resend an ambiguous claim".into());
    };
    let preparation = (|| -> Result<Vec<u8>> {
        // Read again after capture, immediately before preparing. A subsequent
        // policy change is fenced by normal Store claim/settlement freshness.
        let head = store
            .active_policy(&manifest.actor, &manifest.scope, &manifest.purpose)?
            .ok_or("active policy unavailable")?;
        let content = render(manifest, &inputs, &head)?;
        Ok(serde_json::to_vec(
            &json!({"contract": CONTRACT, "computation_sha256": binding,
            "input_manifest": manifest, "runtime": {"identity":"fixture-byte-runtime/1", "accounting_unit":"fixture_bytes", "output_cap":96, "model":null},
            "feedback": FEEDBACK, "messages":[{"role":"user", "content":serde_json::to_string(&content)?}]}),
        )?)
    })();
    let request = match preparation {
        Ok(request) => request,
        Err(error) => {
            store.cancel_recovery_attempt(attempt)?;
            return Err(error);
        }
    };
    if !store.reserve_recovery_attempt(attempt, &request, i64::try_from(request.len())?, 96)? {
        return Err("fixture budget refused".into());
    }
    Ok(Prepared {
        attempt,
        branch,
        request,
    })
}
fn save_request(out: &Path, label: &str, prepared: &Prepared) -> Result<()> {
    std::fs::write(out.join(format!("{label}.request.json")), &prepared.request)?;
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 || args[0] != "--fixture" || args[2] != "--out" {
        return Err("usage: policy_activation --fixture SYNTHETIC.json --out NEW_DIRECTORY".into());
    }
    let bytes = std::fs::read(&args[1])?;
    if bytes.len() > MAX_EVALUATION_BYTES {
        return Err("fixture size limit".into());
    }
    let input: FixtureInput = serde_json::from_slice(&bytes)?;
    if input.format != "policy-activation-fixture/1" || input.new_model_calls != 0 {
        return Err("not an offline synthetic fixture".into());
    }
    let out = Path::new(&args[3]);
    // Create a new directory, refusing reuse of any existing/owner database.
    std::fs::create_dir(out)?;
    std::fs::write(out.join("fixture.json"), &bytes)?;
    let db = out.join("fixture.sqlite");
    let mut f = Fixture::open(&db, input.evaluation)?;
    let baseline = f.manifest()?;
    let historical = prepare(&mut f.store, f.budget, "historical", &baseline)?;
    save_request(out, "historical", &historical)?;
    if f.store.claim_recovery_dispatch(historical.attempt)? != Some(historical.request.clone()) {
        return Err("fixture claim refused".into());
    }
    // The fixed response matches the single captured Report. No semantic label
    // from the synthetic comparison is in any execution request.
    let answer = br#"{"release":"fixture","tests":"3/3","status":"pass"}"#;
    let usage = i64::try_from(historical.request.len() + answer.len())?;
    if f.store.settle(historical.attempt, answer, usage)? != Disposition::Accepted {
        return Err("fixture publication refused".into());
    }
    let pending = prepare(&mut f.store, f.budget, "preclaim", &baseline)?;
    save_request(out, "preclaim", &pending)?;
    let uncertain = prepare(&mut f.store, f.budget, "uncertain", &baseline)?;
    save_request(out, "uncertain", &uncertain)?;
    if f.store.claim_recovery_dispatch(uncertain.attempt)? != Some(uncertain.request.clone()) {
        return Err("uncertain fixture claim refused".into());
    }
    let before = f.store.token_budget_state(f.budget)?;
    let promoted = f.promote()?;
    if promoted != (ActivationResult::Activated { revision: 2 }) {
        return Err("fixture promotion refused".into());
    }
    let stale_claim = f
        .store
        .claim_recovery_dispatch(pending.attempt)
        .expect_err("policy change must fence old prepared work");
    if !stale_claim
        .to_string()
        .contains("cannot dispatch a stale attempt")
    {
        return Err(stale_claim.into());
    }
    let historical_receipt = f.store.settle(historical.attempt, answer, usage)?;
    let current = f
        .store
        .read_current(&baseline.actor, &baseline.scope, &historical.branch)?;
    if historical_receipt != Disposition::Accepted || current != Current::NeedsReview {
        return Err("historical/current gate mismatch".into());
    }
    if f.store.token_budget_state(f.budget)? != before {
        return Err("activation changed old accounting".into());
    }
    if !f.store.cancel_recovery_attempt(pending.attempt)? {
        return Err("unsent fixture cancellation refused".into());
    }
    let held = f.store.token_budget_state(f.budget)?;
    let rolled_back = f.rollback()?;
    if rolled_back != (ActivationResult::Activated { revision: 3 }) {
        return Err("fixture rollback refused".into());
    }
    let rollback = f.manifest()?;
    if rollback.active_evidence_id != baseline.active_evidence_id || rollback.active_revision != 3 {
        return Err("rollback identity/revision mismatch".into());
    }
    let final_prepared = prepare(&mut f.store, f.budget, "after-rollback", &rollback)?;
    save_request(out, "after-rollback", &final_prepared)?;
    f.store.cancel_recovery_attempt(final_prepared.attempt)?;
    if f.store
        .claim_recovery_dispatch(uncertain.attempt)?
        .is_some()
        || f.store.cancel_recovery_attempt(uncertain.attempt)?
    {
        return Err("ambiguous claim was reclaimed".into());
    }
    let budget = f.budget;
    drop(f);
    let reopened = Store::open(&db)?;
    if reopened.token_budget_state(budget)? != held {
        return Err("uncertain hold changed across reopen".into());
    }
    let summary = json!({"format":"policy-activation-result/1", "provenance":"synthetic_registry_mechanics", "new_model_calls":0,
        "limitations":"Synthetic registry attestations only; not feasible named-policy replay or measured superiority.", "bootstrap_revision":1, "promotion_revision":2, "rollback_revision":3,
        "baseline_manifest":baseline, "rollback_manifest":rollback, "historical_receipt":"historicalAccepted",
        "current_head":"NeedsReview", "preclaim_refusal":stale_claim.to_string(), "uncertain_attempt":uncertain.attempt,
        "budget_id":budget, "spent_fixture_bytes":held.spent, "retained_uncertain_bound":held.reserved,
        "database":db, "scope":"offline fixture only; incident/1 decision replay and diagnostics do not support this contract"});
    std::fs::write(
        out.join("result.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Result<Fixture> {
        let output = std::process::Command::new("python3")
            .args(["scripts/recovery_policy_eval.py", "--activation-fixture"])
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout)?;
        Fixture::open(
            Path::new(":memory:"),
            serde_json::from_value(value["evaluation"].clone())?,
        )
    }
    #[test]
    fn captures_policy_and_reports_but_renders_only_reports() -> Result<()> {
        let mut f = fixture()?;
        let m = f.manifest()?;
        let p = prepare(&mut f.store, f.budget, "render", &m)?;
        let request: Value = serde_json::from_slice(&p.request)?;
        assert_eq!(request["contract"], CONTRACT);
        assert_eq!(request["input_manifest"], serde_json::to_value(&m)?);
        let content: Value =
            serde_json::from_str(request["messages"][0]["content"].as_str().unwrap())?;
        assert_eq!(
            content,
            json!({"ci_reports":[{"release":"fixture", "passed":3, "total":3}]})
        );
        assert_eq!(f.store.claim_recovery_dispatch(p.attempt)?, Some(p.request));
        assert_eq!(m.occurrences.len(), 2);
        assert_eq!(m.occurrences[0].role, Role::Policy);
        Ok(())
    }
    #[test]
    fn substituted_identical_policy_occurrence_is_refused() -> Result<()> {
        let mut f = fixture()?;
        let mut m = f.manifest()?;
        let head = f
            .store
            .active_policy(&m.actor, &m.scope, &m.purpose)?
            .unwrap();
        let bytes = f
            .store
            .read_evidence(&m.actor, &m.scope, head.evidence_id)?
            .unwrap();
        let substitute =
            f.store
                .observe_and_admit_many(&m.actor, &m.scope, &[("substitute", &bytes)])?[0];
        m.occurrences[0].evidence_id = substitute;
        let err = prepare(&mut f.store, f.budget, "substitute", &m)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("active policy occurrence"), "{err}");
        assert_eq!(f.store.token_budget_state(f.budget)?.reserved, 0);
        Ok(())
    }
    #[test]
    fn malformed_roles_hashes_and_incomplete_capture_refuse_preparation() -> Result<()> {
        let mut f = fixture()?;
        let m = f.manifest()?;
        for n in 0..4 {
            let mut bad = m.clone();
            match n {
                0 => bad.occurrences.swap(0, 1),
                1 => bad.occurrences[1].role = Role::Policy,
                2 => bad.occurrences[1].sha256 = "0".repeat(64),
                _ => bad.occurrences[1].evidence_id = i64::MAX,
            }
            assert!(prepare(&mut f.store, f.budget, &format!("bad/{n}"), &bad).is_err());
        }
        assert_eq!(f.store.token_budget_state(f.budget)?.reserved, 0);
        Ok(())
    }
    #[test]
    fn rechecks_selection_after_lookup_and_before_preparation() -> Result<()> {
        let mut f = fixture()?;
        let old = f.manifest()?;
        assert_eq!(f.promote()?, ActivationResult::Activated { revision: 2 });
        assert!(prepare(&mut f.store, f.budget, "old-selection", &old).is_err());
        assert_eq!(f.store.token_budget_state(f.budget)?.reserved, 0);
        Ok(())
    }
    #[test]
    fn actual_store_refuses_policy_switch_between_prepare_and_claim() -> Result<()> {
        let mut f = fixture()?;
        let m = f.manifest()?;
        let p = prepare(&mut f.store, f.budget, "switch", &m)?;
        f.promote()?;
        let error = f
            .store
            .claim_recovery_dispatch(p.attempt)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot dispatch a stale attempt"), "{error}");
        assert!(f.store.cancel_recovery_attempt(p.attempt)?);
        Ok(())
    }
    #[test]
    fn policy_switch_after_claim_refuses_stale_publication_with_known_usage() -> Result<()> {
        let mut f = fixture()?;
        let m = f.manifest()?;
        let p = prepare(&mut f.store, f.budget, "known-delivery", &m)?;
        assert_eq!(
            f.store.claim_recovery_dispatch(p.attempt)?,
            Some(p.request.clone())
        );
        f.promote()?;
        let answer = br#"{"release":"fixture","tests":"3/3","status":"pass"}"#;
        let usage = i64::try_from(p.request.len() + answer.len())?;
        assert_eq!(
            f.store.settle(p.attempt, answer, usage)?,
            Disposition::StaleView
        );
        assert_eq!(
            f.store.read_current(&m.actor, &m.scope, &p.branch)?,
            Current::Empty
        );
        // This separate known receipt charges real fixture bytes; an unresolved
        // claimed attempt in the other test retains its bound instead.
        assert_eq!(f.store.token_budget_state(f.budget)?.spent, usage);
        assert_eq!(f.store.token_budget_state(f.budget)?.reserved, 0);
        Ok(())
    }
    #[test]
    fn rollback_advances_revision_and_old_receipt_is_historical_only() -> Result<()> {
        let mut f = fixture()?;
        let m = f.manifest()?;
        let p = prepare(&mut f.store, f.budget, "historical", &m)?;
        f.store.claim_recovery_dispatch(p.attempt)?.unwrap();
        let answer = br#"{"release":"fixture","tests":"3/3","status":"pass"}"#;
        let usage = i64::try_from(p.request.len() + answer.len())?;
        assert_eq!(
            f.store.settle(p.attempt, answer, usage)?,
            Disposition::Accepted
        );
        assert_eq!(
            f.store.read_current(&m.actor, &m.scope, &p.branch)?,
            Current::Ready(answer.to_vec())
        );
        f.promote()?;
        assert_eq!(
            f.store.settle(p.attempt, answer, usage)?,
            Disposition::Accepted
        );
        assert_eq!(
            f.store.read_current(&m.actor, &m.scope, &p.branch)?,
            Current::NeedsReview
        );
        assert_eq!(f.rollback()?, ActivationResult::Activated { revision: 3 });
        let rolled = f.manifest()?;
        assert_eq!(rolled.active_version_id, m.active_version_id);
        assert_eq!(rolled.active_evidence_id, m.active_evidence_id);
        assert_eq!(rolled.active_revision, m.active_revision + 2);
        assert_eq!(
            f.store.read_current(&m.actor, &m.scope, &p.branch)?,
            Current::NeedsReview
        );
        Ok(())
    }
    #[test]
    fn uncertain_claim_keeps_exact_hold_across_promotion_rollback_and_reopen() -> Result<()> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "policy-activation-{}-{stamp}.sqlite",
            std::process::id()
        ));
        let mut f = fixture()?;
        // Reuse the exact fixture attestation in a separate disposable database.
        f = Fixture::open(&path, f.evaluation.clone())?;
        let m = f.manifest()?;
        let p = prepare(&mut f.store, f.budget, "uncertain", &m)?;
        assert_eq!(
            f.store.claim_recovery_dispatch(p.attempt)?,
            Some(p.request.clone())
        );
        let before = f.store.token_budget_state(f.budget)?;
        assert!(before.reserved > 0);
        f.promote()?;
        f.rollback()?;
        assert_eq!(f.store.token_budget_state(f.budget)?, before);
        assert_eq!(f.store.claim_recovery_dispatch(p.attempt)?, None);
        assert!(!f.store.cancel_recovery_attempt(p.attempt)?);
        assert_eq!(
            f.store.read_current(&m.actor, &m.scope, &p.branch)?,
            Current::Empty
        );
        let budget = f.budget;
        drop(f);
        let reopened = Store::open(&path)?;
        assert_eq!(reopened.token_budget_state(budget)?, before);
        drop(reopened);
        std::fs::remove_file(path)?;
        Ok(())
    }
}
