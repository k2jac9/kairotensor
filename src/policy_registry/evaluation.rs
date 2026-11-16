use super::*;
use std::collections::{HashMap, HashSet};
pub const SELECTION_CONTRACT: &[u8] = b"policy-selection/1\ncomplete-paired-frozen-attestations;success=strict-increase;false-refusal,unsafe-acceptance,charged-plus-reserved,deadline-miss=nonincrease;synthetic=fixture/recovery-policy-only\n";
pub const SELECTION_CONTRACT_SHA256: &str =
    "df97ae41b2648bef15a11bf1950883597b057df1940fe43347d485fedf8590b3";
fn add(a: u64, b: u64) -> rusqlite::Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| bad("policy accounting overflow"))
}
fn record(r: &DigestedRecord) -> rusqlite::Result<()> {
    require(
        r.bytes.len() <= MAX_EVALUATION_BYTES
            && digest(&r.sha256)
            && sha256_hex(&r.bytes) == r.sha256,
        "record digest mismatch",
    )
}
fn shape(e: &PolicyEvaluation) -> rusqlite::Result<()> {
    require(
        e.baseline_rows.len() <= MAX_CASES && e.candidate_rows.len() <= MAX_CASES,
        "case limit",
    )?;
    require(
        e.experiment_manifest.len() <= MAX_EVALUATION_BYTES,
        "manifest size limit",
    )?;
    bounded(e)?;
    require(
        [
            &e.evaluation_id,
            &e.actor,
            &e.scope,
            &e.purpose,
            &e.baseline_id,
            &e.candidate_id,
            &e.evaluator_version,
        ]
        .iter()
        .all(|s| nonempty(s)),
        "empty evaluation identity",
    )?;
    require(
        [
            &e.baseline_sha256,
            &e.candidate_sha256,
            &e.experiment_sha256,
            &e.evaluator_sha256,
        ]
        .iter()
        .all(|s| digest(s)),
        "invalid evaluation digest",
    )?;
    for row in e.baseline_rows.iter().chain(&e.candidate_rows) {
        require(
            nonempty(&row.case_id) && row.attempts.len() <= 2,
            "invalid row identity or attempt count",
        )?;
        if let Some(c) = row.charged {
            add(c, row.reserved_bound)?;
        }
        for a in &row.attempts {
            require(
                nonempty(&a.execution_id) && nonempty(&a.opportunity_id),
                "empty attempt identity",
            )?;
            require(
                [
                    &a.request_sha256,
                    &a.evidence_manifest_sha256,
                    &a.validator_sha256,
                    &a.scorer_sha256,
                    &a.label_sha256,
                ]
                .iter()
                .all(|s| digest(s)),
                "invalid attempt digest",
            )?;
            if let Some(r) = &a.response {
                record(r)?;
            }
            if let Some(s) = &a.scoring {
                require(
                    [&s.response_sha256, &s.scorer_sha256, &s.label_sha256]
                        .iter()
                        .all(|v| digest(v)),
                    "invalid scoring digest",
                )?;
            }
        }
    }
    Ok(())
}
pub fn parse_policy_evaluation(bytes: &[u8]) -> rusqlite::Result<PolicyEvaluation> {
    let e = parse(bytes, MAX_EVALUATION_BYTES)?;
    shape(&e)?;
    Ok(e)
}
/// Digest of domain prefix plus sorted-key, compact UTF-8 JSON; exact byte payloads stay arrays.
pub fn evaluation_sha256(e: &PolicyEvaluation) -> rusqlite::Result<String> {
    shape(e)?;
    let value = serde_json::to_value(e).map_err(|_| bad("evaluation encoding"))?;
    let mut raw = b"policy-evaluation/2\n".to_vec();
    raw.extend(serde_json::to_vec(&value).map_err(|_| bad("evaluation encoding"))?);
    Ok(sha256_hex(&raw))
}
/// Exact frozen-baseline projection; does not imply either comparison is eligible.
pub fn paired_baseline_matches(
    recorded: &PolicyEvaluation,
    paired: &PolicyEvaluation,
) -> rusqlite::Result<bool> {
    shape(recorded)?;
    shape(paired)?;
    Ok(recorded.actor == paired.actor
        && recorded.scope == paired.scope
        && recorded.purpose == paired.purpose
        && recorded.baseline_id == paired.baseline_id
        && recorded.baseline_sha256 == paired.baseline_sha256
        && recorded.experiment_manifest == paired.experiment_manifest
        && recorded.experiment_sha256 == paired.experiment_sha256
        && recorded.evaluator_version == paired.evaluator_version
        && recorded.evaluator_sha256 == paired.evaluator_sha256
        && recorded.provenance == paired.provenance
        && recorded.baseline_rows == paired.baseline_rows)
}
pub(super) fn validate_manifest(
    bytes: &[u8],
) -> rusqlite::Result<(ComparisonManifest, Vec<FrozenLabel>)> {
    let m: ComparisonManifest = parse(bytes, MAX_EVALUATION_BYTES)?;
    require(
        m.format == "policy-comparison/1"
            && !m.cases.is_empty()
            && m.cases.len() <= MAX_CASES
            && !m.policies.is_empty()
            && m.policies.len() <= 3,
        "invalid manifest format or count",
    )?;
    require(
        [&m.actor, &m.scope, &m.purpose].iter().all(|s| nonempty(s)),
        "empty manifest identity",
    )?;
    require(
        [
            &m.workload_sha256,
            &m.prompt_sha256,
            &m.runtime_sha256,
            &m.model_sha256,
            &m.tokenizer_sha256,
            &m.validator_sha256,
            &m.scorer_sha256,
            &m.compatibility_contract_sha256,
        ]
        .iter()
        .all(|s| digest(s)),
        "invalid manifest digest",
    )?;
    let mut names = HashSet::new();
    let mut artifacts = HashSet::new();
    for p in &m.policies {
        record(&p.artifact)?;
        let a = validate_policy_artifact(&p.artifact.bytes)?;
        require(
            names.insert(p.policy_name) && artifacts.insert(&p.artifact.sha256),
            "duplicate frozen policy",
        )?;
        require(
            a.policy_name == p.policy_name
                && a.purpose == m.purpose
                && a.compatibility_contract_sha256 == m.compatibility_contract_sha256,
            "frozen artifact identity mismatch",
        )?;
    }
    let mut cases = HashSet::new();
    let mut families = HashMap::new();
    let mut labels = Vec::with_capacity(m.cases.len());
    for c in &m.cases {
        require(
            [&c.case_id, &c.family_id, &c.split_id]
                .iter()
                .all(|s| nonempty(s))
                && cases.insert(&c.case_id),
            "invalid or duplicate case identity",
        )?;
        require(
            c.timing_start_ms <= c.deadline_ms && c.deadline_ms <= c.timing_end_ms,
            "invalid timing boundaries",
        )?;
        if let Some(split) = families.insert(&c.family_id, &c.split_id) {
            require(split == &c.split_id, "family crosses splits")?;
        }
        record(&c.initial_evidence)?;
        record(&c.label)?;
        let l: FrozenLabel = parse(&c.label.bytes, MAX_ARTIFACT_BYTES)?;
        require(
            l.normalized_format == "opaque-bytes/1" && l.scorer_sha256 == m.scorer_sha256,
            "invalid frozen label identity",
        )?;
        let mut policies = HashSet::new();
        require(
            c.policy_opportunities.len() == names.len(),
            "missing frozen policy opportunities",
        )?;
        for p in &c.policy_opportunities {
            require(
                names.contains(&p.policy_name) && policies.insert(p.policy_name),
                "unknown or duplicate opportunity policy",
            )?;
            let expected = if p.policy_name == PolicyName::AlwaysStop {
                1
            } else {
                2
            };
            require(
                p.opportunities.len() == expected,
                "invalid frozen opportunity count",
            )?;
            let mut ids = HashSet::new();
            for (i, o) in p.opportunities.iter().enumerate() {
                require(
                    nonempty(&o.opportunity_id) && ids.insert(&o.opportunity_id),
                    "duplicate opportunity identity",
                )?;
                record(&o.request)?;
                record(&o.evidence_manifest)?;
                add(o.input_bound, o.output_cap)?;
                require(
                    o.start_min_ms >= c.timing_start_ms
                        && o.start_min_ms <= o.start_max_ms
                        && o.start_max_ms <= c.timing_end_ms,
                    "invalid opportunity timing",
                )?;
                if i == 0 {
                    require(
                        o.evidence_manifest == c.initial_evidence,
                        "initial evidence differs",
                    )?;
                }
            }
        }
        // All named policies start from the same frozen opportunity, excluding local ID.
        let first = &c.policy_opportunities[0].opportunities[0];
        for p in &c.policy_opportunities[1..] {
            let mut o = p.opportunities[0].clone();
            o.opportunity_id = first.opportunity_id.clone();
            require(o == *first, "different initial policy opportunities")?;
        }
        labels.push(l);
    }
    Ok((m, labels))
}
fn refused(r: SelectionRefusal) -> SelectionDecision {
    SelectionDecision::Refused(r)
}

pub fn evaluate_policy_comparison(e: &PolicyEvaluation) -> rusqlite::Result<SelectionDecision> {
    shape(e)?;
    let (m, labels) = validate_manifest(&e.experiment_manifest)?;
    if e.actor != m.actor
        || e.scope != m.scope
        || e.purpose != m.purpose
        || e.provenance != m.provenance
        || sha256_hex(&e.experiment_manifest) != e.experiment_sha256
        || e.evaluator_version != "policy-selection/1"
        || e.evaluator_sha256 != SELECTION_CONTRACT_SHA256
        || !matches!(
            m.accounting_unit,
            AccountingUnit::ModelTokens | AccountingUnit::FixtureBytes
        )
        || (e.provenance == Provenance::SyntheticRegistryMechanics
            && e.purpose != "fixture/recovery-policy")
    {
        return Ok(refused(SelectionRefusal::Incomparable));
    }
    let base = m
        .policies
        .iter()
        .find(|p| p.artifact.sha256 == e.baseline_sha256);
    let cand = m
        .policies
        .iter()
        .find(|p| p.artifact.sha256 == e.candidate_sha256);
    let (Some(base), Some(cand)) = (base, cand) else {
        return Ok(refused(SelectionRefusal::Incomparable));
    };
    let mut executions = HashMap::new();
    let b = rows(
        &m,
        &labels,
        &e.baseline_rows,
        base.policy_name,
        &mut executions,
    )?;
    let c = rows(
        &m,
        &labels,
        &e.candidate_rows,
        cand.policy_name,
        &mut executions,
    )?;
    if e.baseline_id == e.candidate_id
        && (e.baseline_sha256 != e.candidate_sha256 || e.baseline_rows != e.candidate_rows)
    {
        return Ok(refused(SelectionRefusal::Incomparable));
    }
    let (b, c) = match (b, c) {
        (Ok(b), Ok(c)) => (b, c),
        (Err(SelectionRefusal::Incomparable), _) | (_, Err(SelectionRefusal::Incomparable)) => {
            return Ok(refused(SelectionRefusal::Incomparable))
        }
        (Err(r), _) | (_, Err(r)) => return Ok(refused(r)),
    };
    let reason = if c.success <= b.success {
        Some(SelectionRefusal::NoImprovement)
    } else if c.false_refusal > b.false_refusal {
        Some(SelectionRefusal::IncreasedFalseRefusal)
    } else if c.unsafe_acceptance > b.unsafe_acceptance {
        Some(SelectionRefusal::IncreasedUnsafeAcceptance)
    } else if c.cost > b.cost {
        Some(SelectionRefusal::IncreasedCost)
    } else if c.deadline_misses > b.deadline_misses {
        Some(SelectionRefusal::IncreasedDeadlineMisses)
    } else {
        None
    };
    Ok(match reason {
        Some(r) => refused(r),
        None => SelectionDecision::Eligible {
            validated_counters: ValidatedCounters {
                baseline: b,
                candidate: c,
            },
        },
    })
}
type CheckedCounters = Result<PolicyCounters, SelectionRefusal>;
fn rows(
    m: &ComparisonManifest,
    labels: &[FrozenLabel],
    rows: &[PolicyRow],
    policy: PolicyName,
    executions: &mut HashMap<String, ExecutionFacts>,
) -> rusqlite::Result<CheckedCounters> {
    let mut refusal = if rows.len() != m.cases.len() {
        Some(SelectionRefusal::Incomplete)
    } else {
        None
    };
    let mut counters = PolicyCounters::default();
    for ((r, c), l) in rows.iter().zip(&m.cases).zip(labels) {
        if r.case_id != c.case_id {
            refusal = Some(SelectionRefusal::Incomparable);
            continue;
        }
        require(r.answerable == l.answerable, "fabricated answerability")?;
        require(
            r.false_refusal
                == (r.outcome == Outcome::Refused && l.answerable && l.admissible_support),
            "fabricated false refusal",
        )?;
        if r.outcome != Outcome::Accepted {
            require(!r.unsafe_acceptance, "fabricated unsafe acceptance")?;
        }
        if let (Some(correct), Some(eligible)) = (r.correct, r.eligible) {
            require(
                r.unsafe_acceptance == (r.outcome == Outcome::Accepted && (!correct || !eligible)),
                "fabricated unsafe acceptance",
            )?;
        }
        if let Some(completed) = r.completed_ms {
            require(
                completed >= c.timing_start_ms
                    && completed <= c.timing_end_ms
                    && r.deadline_miss == (completed > c.deadline_ms),
                "fabricated deadline miss",
            )?;
        }
        if let Some(charged) = r.charged {
            require(
                add(charged, r.reserved_bound)? <= c.budget,
                "row exceeds frozen budget",
            )?;
        }
        if m.provenance == Provenance::SyntheticRegistryMechanics {
            require(
                r.simulated && r.attempts.is_empty(),
                "synthetic rows require simulated counters only",
            )?;
            if let Some(eligible) = r.eligible {
                require(
                    eligible == l.admissible_support,
                    "fabricated synthetic eligibility",
                )?;
            }
        } else {
            require(!r.simulated, "observed row cannot simulate counters")?;
            if let Some(reason) = observed(m, c, l, r, policy, executions)? {
                if reason == SelectionRefusal::Incomparable {
                    refusal = Some(reason);
                } else {
                    refusal.get_or_insert(reason);
                }
                continue;
            }
        }
        let (Some(correct), Some(eligible), Some(completed), Some(charged)) =
            (r.correct, r.eligible, r.completed_ms, r.charged)
        else {
            refusal.get_or_insert(SelectionRefusal::Incomplete);
            continue;
        };
        if r.outcome == Outcome::Unknown {
            refusal.get_or_insert(SelectionRefusal::Incomplete);
            continue;
        }
        require(
            completed >= c.timing_start_ms && completed <= c.timing_end_ms,
            "row outside timing window",
        )?;
        require(
            r.false_refusal
                == (r.outcome == Outcome::Refused && l.answerable && l.admissible_support),
            "fabricated false refusal",
        )?;
        require(
            r.unsafe_acceptance == (r.outcome == Outcome::Accepted && (!correct || !eligible)),
            "fabricated unsafe acceptance",
        )?;
        require(
            r.deadline_miss == (completed > c.deadline_ms),
            "fabricated deadline miss",
        )?;
        let cost = add(charged, r.reserved_bound)?;
        require(cost <= c.budget, "row exceeds frozen budget")?;
        counters.cases = add(counters.cases, 1)?;
        counters.success = add(
            counters.success,
            u64::from(r.outcome == Outcome::Accepted && correct && eligible && !r.deadline_miss),
        )?;
        counters.false_refusal = add(counters.false_refusal, r.false_refusal as u64)?;
        counters.unsafe_acceptance = add(counters.unsafe_acceptance, r.unsafe_acceptance as u64)?;
        counters.deadline_misses = add(counters.deadline_misses, r.deadline_miss as u64)?;
        counters.cost = add(counters.cost, cost)?;
    }
    Ok(match refusal {
        Some(r) => Err(r),
        None => Ok(counters),
    })
}
fn observed(
    m: &ComparisonManifest,
    c: &FrozenCase,
    l: &FrozenLabel,
    r: &PolicyRow,
    policy: PolicyName,
    executions: &mut HashMap<String, ExecutionFacts>,
) -> rusqlite::Result<Option<SelectionRefusal>> {
    let ops = &c
        .policy_opportunities
        .iter()
        .find(|p| p.policy_name == policy)
        .unwrap()
        .opportunities;
    if r.attempts.is_empty() {
        return Ok(Some(SelectionRefusal::Incomplete));
    }
    require(r.attempts.len() <= ops.len(), "too many observed attempts")?;
    if let Some(reason) = check_observed_facts(m, c, l, r, ops, policy, executions)? {
        return Ok(Some(reason));
    }
    let mut charged = 0;
    let mut previous_end = c.timing_start_ms;
    let mut result = (
        Outcome::Refused,
        false,
        l.admissible_support,
        c.timing_start_ms,
    );
    let mut ids = HashSet::new();
    for (i, a) in r.attempts.iter().enumerate() {
        let o = &ops[i];
        if a.opportunity_id != o.opportunity_id
            || a.request_sha256 != o.request.sha256
            || a.evidence_manifest_sha256 != o.evidence_manifest.sha256
            || a.validator_sha256 != m.validator_sha256
            || a.scorer_sha256 != m.scorer_sha256
            || a.label_sha256 != c.label.sha256
        {
            return Ok(Some(SelectionRefusal::Incomparable));
        }
        require(
            ids.insert(&a.execution_id),
            "reused execution in one policy path",
        )?;
        if i > 0 {
            require(
                r.attempts[i - 1].execution == Execution::Executed
                    && r.attempts[i - 1].disposition == AttemptDisposition::InvalidOutput,
                "only settled invalid output unlocks retry",
            )?;
        }
        require(
            a.start_ms >= previous_end
                && a.start_ms >= o.start_min_ms
                && a.start_ms <= o.start_max_ms,
            "attempt outside timing window",
        )?;
        let quote = add(o.input_bound, o.output_cap)?;
        let gate = o.admitted
            && !(i > 0 && policy == PolicyName::EvidenceGatedRetry && !o.consistent)
            && a.start_ms < c.deadline_ms
            && quote
                <= c.budget
                    .checked_sub(charged)
                    .ok_or_else(|| bad("budget exceeded"))?;
        if a.execution == Execution::Unknown {
            require(
                a.measured_input.is_none()
                    && a.measured_output.is_none()
                    && a.response.is_none()
                    && a.scoring.is_none()
                    && a.retained_bound == quote
                    && a.disposition == AttemptDisposition::Uncertain
                    && gate
                    && i + 1 == r.attempts.len(),
                "invalid unknown execution",
            )?;
            require(
                r.charged.is_none() && r.reserved_bound == quote && r.outcome == Outcome::Unknown,
                "unknown row accounting",
            )?;
            return Ok(Some(SelectionRefusal::Incomplete));
        }
        if a.execution == Execution::NotExecuted {
            require(
                !gate
                    && a.disposition == AttemptDisposition::Refused
                    && a.measured_input == Some(0)
                    && a.measured_output == Some(0)
                    && a.retained_bound == 0
                    && a.response.is_none()
                    && a.scoring.is_none()
                    && a.completed_ms == Some(a.start_ms)
                    && i + 1 == r.attempts.len(),
                "invalid nonexecution attestation",
            )?;
            result = (
                Outcome::Refused,
                false,
                o.admitted && l.admissible_support,
                a.start_ms,
            );
        } else {
            require(gate, "execution violates frozen admission")?;
            if let Some(v) = a.measured_input {
                require(v <= o.input_bound, "input exceeds quote")?;
            }
            if let Some(v) = a.measured_output {
                require(v <= o.output_cap, "output exceeds cap")?;
            }
            let (Some(input), Some(output)) = (a.measured_input, a.measured_output) else {
                require(
                    a.retained_bound == quote
                        && r.charged.is_none()
                        && r.reserved_bound == quote
                        && r.outcome == Outcome::Unknown,
                    "unknown usage must retain full bound",
                )?;
                require(
                    i + 1 == r.attempts.len(),
                    "unknown usage cannot unlock retry",
                )?;
                return Ok(Some(SelectionRefusal::Incomplete));
            };
            require(
                a.retained_bound == 0 && a.disposition != AttemptDisposition::Uncertain,
                "settled usage has retained hold or uncertain disposition",
            )?;
            charged = add(charged, add(input, output)?)?;
            let Some(end) = a.completed_ms else {
                return Ok(Some(SelectionRefusal::Incomplete));
            };
            require(
                end >= a.start_ms && end <= c.timing_end_ms,
                "invalid completion time",
            )?;
            previous_end = end;
            let correct = if let Some(response) = &a.response {
                let Some(scoring) = &a.scoring else {
                    return Ok(Some(SelectionRefusal::Incomplete));
                };
                if scoring.response_sha256 != response.sha256
                    || scoring.scorer_sha256 != m.scorer_sha256
                    || scoring.label_sha256 != c.label.sha256
                {
                    return Ok(Some(SelectionRefusal::Incomparable));
                }
                scoring.normalized == l.expected_normalized
            } else {
                if a.scoring.is_some() || a.disposition != AttemptDisposition::Failed {
                    return Ok(Some(SelectionRefusal::Incomplete));
                }
                false
            };
            let outcome = match a.disposition {
                AttemptDisposition::Accepted => Outcome::Accepted,
                AttemptDisposition::Failed => Outcome::Failed,
                _ => Outcome::Refused,
            };
            result = (
                outcome,
                correct,
                o.admitted && l.admissible_support && a.disposition != AttemptDisposition::Stale,
                end,
            );
            if a.disposition == AttemptDisposition::InvalidOutput
                && i + 1 < ops.len()
                && i + 1 == r.attempts.len()
            {
                return Ok(Some(SelectionRefusal::Incomplete));
            }
            if a.disposition != AttemptDisposition::InvalidOutput {
                require(
                    i + 1 == r.attempts.len(),
                    "terminal disposition cannot unlock retry",
                )?;
            }
        }
    }
    require(
        (r.outcome == Outcome::Unknown || r.outcome == result.0)
            && r.correct.is_none_or(|v| v == result.1)
            && r.eligible.is_none_or(|v| v == result.2)
            && r.completed_ms.is_none_or(|v| v == result.3)
            && r.charged.is_none_or(|v| v == charged)
            && r.reserved_bound == 0,
        "fabricated observed row totals",
    )?;
    if r.outcome == Outcome::Unknown
        || r.correct.is_none()
        || r.eligible.is_none()
        || r.completed_ms.is_none()
        || r.charged.is_none()
    {
        return Ok(Some(SelectionRefusal::Incomplete));
    }
    Ok(None)
}

// Cumulative facts are used only to detect contradictions, never to complete a row.
struct ExecutionFacts {
    case_id: String,
    input_bound: u64,
    output_cap: u64,
    observation: ObservedAttempt,
}
fn merge_known<T: Clone + PartialEq>(known: &mut Option<T>, supplied: &Option<T>) -> bool {
    match (known.as_ref(), supplied.as_ref()) {
        (Some(left), Some(right)) => left == right,
        (None, Some(value)) => {
            *known = Some(value.clone());
            true
        }
        _ => true,
    }
}
fn shared_execution_consistent(
    c: &FrozenCase,
    o: &RequestOpportunity,
    a: &ObservedAttempt,
    executions: &mut HashMap<String, ExecutionFacts>,
) -> bool {
    let Some(facts) = executions.get_mut(&a.execution_id) else {
        executions.insert(
            a.execution_id.clone(),
            ExecutionFacts {
                case_id: c.case_id.clone(),
                input_bound: o.input_bound,
                output_cap: o.output_cap,
                observation: a.clone(),
            },
        );
        return true;
    };
    let prior = &mut facts.observation;
    if facts.case_id != c.case_id
        || facts.input_bound != o.input_bound
        || facts.output_cap != o.output_cap
        || prior.request_sha256 != a.request_sha256
        || prior.evidence_manifest_sha256 != a.evidence_manifest_sha256
        || prior.validator_sha256 != a.validator_sha256
        || prior.scorer_sha256 != a.scorer_sha256
        || prior.label_sha256 != a.label_sha256
        || prior.start_ms != a.start_ms
    {
        return false;
    }
    if prior.execution != Execution::Unknown
        && a.execution != Execution::Unknown
        && prior.execution != a.execution
    {
        return false;
    }
    if prior.disposition != AttemptDisposition::Uncertain
        && a.disposition != AttemptDisposition::Uncertain
        && prior.disposition != a.disposition
    {
        return false;
    }
    if !merge_known(&mut prior.response, &a.response)
        || !merge_known(&mut prior.scoring, &a.scoring)
        || !merge_known(&mut prior.measured_input, &a.measured_input)
        || !merge_known(&mut prior.measured_output, &a.measured_output)
        || !merge_known(&mut prior.completed_ms, &a.completed_ms)
    {
        return false;
    }
    if prior.execution == Execution::Unknown {
        prior.execution = a.execution;
    }
    if prior.disposition == AttemptDisposition::Uncertain {
        prior.disposition = a.disposition;
    }
    // A response and its scoring assertion can arrive in separate partial records.
    match (&prior.response, &prior.scoring) {
        (Some(response), Some(scoring)) => response.sha256 == scoring.response_sha256,
        _ => true,
    }
}
fn check_observed_facts(
    m: &ComparisonManifest,
    c: &FrozenCase,
    l: &FrozenLabel,
    r: &PolicyRow,
    ops: &[RequestOpportunity],
    policy: PolicyName,
    executions: &mut HashMap<String, ExecutionFacts>,
) -> rusqlite::Result<Option<SelectionRefusal>> {
    let mut ids = HashSet::new();
    let mut previous_start = c.timing_start_ms;
    let mut previous_end = Some(c.timing_start_ms);
    let mut known_charge = Some(0u64);
    let mut held = 0;
    for (i, (a, o)) in r.attempts.iter().zip(ops).enumerate() {
        if i > 0 {
            let previous = &r.attempts[i - 1];
            require(
                previous.execution == Execution::Executed
                    && previous.disposition == AttemptDisposition::InvalidOutput
                    && previous.measured_input.is_some()
                    && previous.measured_output.is_some(),
                "only settled invalid output unlocks retry",
            )?;
        }
        if i + 1 < r.attempts.len() {
            require(
                a.execution == Execution::Executed
                    && a.disposition == AttemptDisposition::InvalidOutput,
                "terminal disposition cannot unlock retry",
            )?;
        }
        require(
            ids.insert(&a.execution_id),
            "reused execution in one policy path",
        )?;
        if a.opportunity_id != o.opportunity_id
            || a.request_sha256 != o.request.sha256
            || a.evidence_manifest_sha256 != o.evidence_manifest.sha256
            || a.validator_sha256 != m.validator_sha256
            || a.scorer_sha256 != m.scorer_sha256
            || a.label_sha256 != c.label.sha256
        {
            return Ok(Some(SelectionRefusal::Incomparable));
        }
        require(
            a.start_ms >= previous_start
                && previous_end.is_none_or(|end| a.start_ms >= end)
                && a.start_ms >= o.start_min_ms
                && a.start_ms <= o.start_max_ms,
            "attempt outside timing window",
        )?;
        if let Some(end) = a.completed_ms {
            require(
                end >= a.start_ms && end <= c.timing_end_ms,
                "invalid completion time",
            )?;
        }
        previous_start = a.start_ms;
        previous_end = a.completed_ms;
        if let Some(input) = a.measured_input {
            require(input <= o.input_bound, "input exceeds quote")?;
        }
        if let Some(output) = a.measured_output {
            require(output <= o.output_cap, "output exceeds cap")?;
        }
        if let Some(scoring) = &a.scoring {
            if scoring.scorer_sha256 != m.scorer_sha256
                || scoring.label_sha256 != c.label.sha256
                || a.response
                    .as_ref()
                    .is_some_and(|response| response.sha256 != scoring.response_sha256)
            {
                return Ok(Some(SelectionRefusal::Incomparable));
            }
        }
        let quote = add(o.input_bound, o.output_cap)?;
        if let Some(spent) = known_charge {
            let remaining = c
                .budget
                .checked_sub(spent)
                .ok_or_else(|| bad("budget exceeded"))?;
            let gate = o.admitted
                && !(i > 0 && policy == PolicyName::EvidenceGatedRetry && !o.consistent)
                && a.start_ms < c.deadline_ms
                && quote <= remaining;
            require(
                gate == (a.execution != Execution::NotExecuted),
                "execution violates frozen admission",
            )?;
        }
        match a.execution {
            Execution::Unknown => require(
                a.measured_input.is_none()
                    && a.measured_output.is_none()
                    && a.response.is_none()
                    && a.scoring.is_none()
                    && a.disposition == AttemptDisposition::Uncertain
                    && a.retained_bound == quote,
                "invalid unknown execution",
            )?,
            Execution::NotExecuted => require(
                a.measured_input == Some(0)
                    && a.measured_output == Some(0)
                    && a.retained_bound == 0
                    && a.response.is_none()
                    && a.scoring.is_none()
                    && a.disposition == AttemptDisposition::Refused
                    && a.completed_ms == Some(a.start_ms),
                "invalid nonexecution attestation",
            )?,
            Execution::Executed => {
                if a.measured_input.is_some() && a.measured_output.is_some() {
                    require(
                        a.retained_bound == 0 && a.disposition != AttemptDisposition::Uncertain,
                        "invalid settled usage",
                    )?;
                } else {
                    require(
                        a.retained_bound == quote,
                        "unknown usage must retain full bound",
                    )?;
                }
            }
        }
        known_charge = match (known_charge, a.measured_input, a.measured_output) {
            (Some(total), Some(input), Some(output)) => Some(add(total, add(input, output)?)?),
            _ => None,
        };
        held = add(held, a.retained_bound)?;
        if !shared_execution_consistent(c, o, a, executions) {
            return Ok(Some(SelectionRefusal::Incomparable));
        }
    }
    let terminal = r.attempts.last().unwrap();
    let missing_retry = terminal.execution == Execution::Executed
        && terminal.disposition == AttemptDisposition::InvalidOutput
        && r.attempts.len() < ops.len();
    if let (Some(supplied), Some(known)) = (r.charged, known_charge) {
        require(supplied >= known, "row charge below known attempt prefix")?;
    }
    if let (Some(supplied), Some(known)) = (r.completed_ms, terminal.completed_ms) {
        require(
            supplied >= known,
            "row completion before known attempt prefix",
        )?;
    }
    if !missing_retry {
        let known_outcome = match (terminal.execution, terminal.disposition) {
            (Execution::Unknown, _) | (_, AttemptDisposition::Uncertain) => Outcome::Unknown,
            (Execution::NotExecuted, _) => Outcome::Refused,
            (_, AttemptDisposition::Accepted) => Outcome::Accepted,
            (_, AttemptDisposition::Failed) => Outcome::Failed,
            _ => Outcome::Refused,
        };
        require(
            r.outcome == Outcome::Unknown || r.outcome == known_outcome,
            "fabricated observed outcome",
        )?;
        if terminal.disposition == AttemptDisposition::Failed
            && terminal.response.is_none()
            && terminal.scoring.is_none()
        {
            require(
                r.correct.is_none_or(|v| !v),
                "failed response cannot imply correctness",
            )?;
        }
        if terminal.execution != Execution::Unknown
            && terminal.disposition != AttemptDisposition::Uncertain
        {
            if let Some(eligible) = r.eligible {
                let opportunity = &ops[r.attempts.len() - 1];
                require(
                    eligible
                        == (opportunity.admitted
                            && l.admissible_support
                            && terminal.disposition != AttemptDisposition::Stale),
                    "fabricated observed eligibility",
                )?;
            }
        }
        require(r.reserved_bound == held, "fabricated observed hold")?;
        if let (Some(supplied), Some(known)) = (r.charged, known_charge) {
            require(supplied == known, "fabricated observed charge")?;
        }
        if known_charge.is_none() {
            require(r.charged.is_none(), "unknown charge must remain unknown")?;
        }
        if let (Some(supplied), Some(known)) = (r.completed_ms, terminal.completed_ms) {
            require(supplied == known, "fabricated observed completion")?;
        }
        if let (Some(supplied), Some(scoring)) = (r.correct, &terminal.scoring) {
            require(
                supplied == (scoring.normalized == l.expected_normalized),
                "fabricated observed correctness",
            )?;
        }
    }
    Ok(None)
}
