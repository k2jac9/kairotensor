//! Immutable policy attestations. Registration neither authenticates execution nor activates a policy.
mod evaluation;
mod types;
use crate::evidence_digest::sha256_hex;
pub use evaluation::*;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
pub use types::*;
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024;
pub const MAX_EVALUATION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CASES: usize = 10_000;
fn bad(message: &str) -> rusqlite::Error {
    crate::invalid(message)
}
fn require(ok: bool, message: &str) -> rusqlite::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(bad(message))
    }
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn nonempty(s: &str) -> bool {
    !s.trim().is_empty()
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], limit: usize) -> rusqlite::Result<T> {
    require(bytes.len() <= limit, "policy record size limit")?;
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    require(depth <= 64, "policy JSON depth limit")?;
                }
                b']' | b'}' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| bad("malformed policy record"))?;
                }
                _ => {}
            }
        }
    }
    serde_json::from_slice(bytes).map_err(|_| bad("malformed policy record"))
}
struct SizeWriter(usize);
impl Write for SizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_EVALUATION_BYTES)
            .ok_or_else(|| io::Error::other("policy record size limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded<T: Serialize>(v: &T) -> rusqlite::Result<()> {
    serde_json::to_writer(SizeWriter(0), v).map_err(|_| bad("policy record size limit"))
}
fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
pub fn validate_policy_artifact(bytes: &[u8]) -> rusqlite::Result<PolicyArtifactV1> {
    let a: PolicyArtifactV1 = parse(bytes, MAX_ARTIFACT_BYTES)?;
    require(
        a.schema_version == 1 && nonempty(&a.purpose) && digest(&a.compatibility_contract_sha256),
        "invalid policy artifact",
    )?;
    Ok(a)
}
pub fn validate_policy_candidate(c: &PolicyCandidate) -> rusqlite::Result<PolicyArtifactV1> {
    bounded(c)?;
    require(
        [&c.version_id, &c.actor, &c.scope, &c.purpose]
            .iter()
            .all(|s| nonempty(s)),
        "empty candidate identity",
    )?;
    let a = validate_policy_artifact(&c.artifact)?;
    require(
        c.purpose == a.purpose && digest(&c.sha256) && sha256_hex(&c.artifact) == c.sha256,
        "candidate identity mismatch",
    )?;
    Ok(a)
}

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

// These column specifications are code-owned, never SQL supplied by a caller.
// Query type and byte length before copying any potentially corrupted persisted value.
fn preflight_fields(
    conn: &Connection,
    from: &str,
    key: &str,
    id: &str,
    fields: &[(&str, &str, usize)],
) -> rusqlite::Result<bool> {
    let columns = fields
        .iter()
        .map(|(name, _, _)| format!("typeof({name}), length(CAST({name} AS BLOB))"))
        .collect::<Vec<_>>()
        .join(",");
    conn.query_row(
        &format!("SELECT {columns} FROM {from} WHERE {key} = ?1"),
        [id],
        |row| {
            for (i, (_, kind, max)) in fields.iter().enumerate() {
                let actual: String = row.get(2 * i)?;
                let len: Option<i64> = row.get(2 * i + 1)?;
                require(
                    actual == *kind && len.is_some_and(|n| n >= 0 && n <= *max as i64),
                    "invalid persisted policy field type or byte length",
                )?;
            }
            Ok(true)
        },
    )
    .optional()
    .map(|v| v.unwrap_or(false))
}
fn encoded<T: Serialize>(value: &T) -> rusqlite::Result<Vec<u8>> {
    bounded(value)?;
    serde_json::to_vec(value).map_err(|_| bad("policy record encoding"))
}

fn stored_candidate(conn: &Connection, id: &str) -> rusqlite::Result<Option<PolicyCandidate>> {
    let from = "policy_versions v LEFT JOIN evidence e ON e.id = v.evidence_id";
    if !preflight_fields(
        conn,
        from,
        "v.version_id",
        id,
        &[
            ("v.version_id", "text", MAX_EVALUATION_BYTES),
            ("v.actor", "text", MAX_EVALUATION_BYTES),
            ("v.scope", "text", MAX_EVALUATION_BYTES),
            ("v.purpose", "text", MAX_EVALUATION_BYTES),
            ("v.artifact_sha256", "text", 64),
            ("v.compatibility_contract_sha256", "text", 64),
            ("e.source", "text", 64),
            ("e.payload", "blob", MAX_ARTIFACT_BYTES),
        ],
    )? {
        return Ok(None);
    }
    let (candidate, contract, source) = conn.query_row(
        &format!("SELECT v.version_id,v.actor,v.scope,v.purpose,v.artifact_sha256,e.payload,v.compatibility_contract_sha256,e.source FROM {from} WHERE v.version_id=?1"),
        [id],
        |r| {
            Ok((
                PolicyCandidate {
                    version_id: r.get(0)?, actor: r.get(1)?, scope: r.get(2)?,
                    purpose: r.get(3)?, sha256: r.get(4)?, artifact: r.get(5)?,
                },
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        },
    )?;
    let artifact = validate_policy_candidate(&candidate)?;
    require(
        artifact.compatibility_contract_sha256 == contract && source == "policy-registry/1",
        "persisted policy artifact metadata mismatch",
    )?;
    Ok(Some(candidate))
}

// The D1 validator checks shape and strict manifest structure even when it refuses
// comparison early. Unknown evaluator/unit row semantics remain unevaluated. The
// original object is always passed unchanged; no identity is relabelled to approve it.
fn bind_evaluation(conn: &Connection, e: &PolicyEvaluation) -> rusqlite::Result<()> {
    let _decision = evaluate_policy_comparison(e)?;
    let manifest: ComparisonManifest = parse(&e.experiment_manifest, MAX_EVALUATION_BYTES)?;
    require(
        manifest.actor == e.actor
            && manifest.scope == e.scope
            && manifest.purpose == e.purpose
            && manifest.provenance == e.provenance
            && sha256_hex(&e.experiment_manifest) == e.experiment_sha256,
        "evaluation manifest identity mismatch",
    )?;
    require(
        e.baseline_id != e.candidate_id
            || (e.baseline_sha256 == e.candidate_sha256 && e.baseline_rows == e.candidate_rows),
        "baseline-only evaluation must contain identical rows",
    )?;
    for (id, digest) in [
        (&e.baseline_id, &e.baseline_sha256),
        (&e.candidate_id, &e.candidate_sha256),
    ] {
        let candidate =
            stored_candidate(conn, id)?.ok_or_else(|| bad("unregistered policy version"))?;
        require(
            candidate.actor == e.actor
                && candidate.scope == e.scope
                && candidate.purpose == e.purpose
                && &candidate.sha256 == digest,
            "evaluation registered version identity mismatch",
        )?;
        require(
            manifest
                .policies
                .iter()
                .any(|p| &p.artifact.sha256 == digest && p.artifact.bytes == candidate.artifact),
            "evaluation must bind exact registered artifact bytes",
        )?;
    }
    Ok(())
}

fn stored_evaluation(conn: &Connection, id: &str) -> rusqlite::Result<Option<PolicyEvaluation>> {
    if !preflight_fields(
        conn,
        "policy_evaluations",
        "evaluation_id",
        id,
        &[
            ("evaluation_id", "text", MAX_EVALUATION_BYTES),
            ("actor", "text", MAX_EVALUATION_BYTES),
            ("scope", "text", MAX_EVALUATION_BYTES),
            ("purpose", "text", MAX_EVALUATION_BYTES),
            ("baseline_id", "text", MAX_EVALUATION_BYTES),
            ("candidate_id", "text", MAX_EVALUATION_BYTES),
            ("baseline_sha256", "text", 64),
            ("candidate_sha256", "text", 64),
            ("experiment_sha256", "text", 64),
            ("evaluator_version", "text", MAX_EVALUATION_BYTES),
            ("evaluator_sha256", "text", 64),
            ("evaluation_sha256", "text", 64),
            ("evaluation_bytes", "blob", MAX_EVALUATION_BYTES),
            ("experiment_manifest", "blob", MAX_EVALUATION_BYTES),
            ("baseline_rows", "blob", MAX_EVALUATION_BYTES),
            ("candidate_rows", "blob", MAX_EVALUATION_BYTES),
        ],
    )? {
        return Ok(None);
    }
    let e = conn.query_row(
        "SELECT evaluation_bytes, evaluation_id, actor, scope, purpose,
        baseline_id, candidate_id, baseline_sha256, candidate_sha256, experiment_sha256,
        evaluator_version, evaluator_sha256, evaluation_sha256, experiment_manifest,
        baseline_rows, candidate_rows FROM policy_evaluations WHERE evaluation_id=?1",
        [id],
        |r| {
            let raw: Vec<u8> = r.get(0)?;
            let e = parse_policy_evaluation(&raw)?;
            for (i, expected) in [
                &e.evaluation_id,
                &e.actor,
                &e.scope,
                &e.purpose,
                &e.baseline_id,
                &e.candidate_id,
                &e.baseline_sha256,
                &e.candidate_sha256,
                &e.experiment_sha256,
                &e.evaluator_version,
                &e.evaluator_sha256,
            ]
            .iter()
            .enumerate()
            {
                require(
                    r.get::<_, String>(i + 1)? == **expected,
                    "persisted evaluation metadata mismatch",
                )?;
            }
            require(
                r.get::<_, String>(12)? == evaluation_sha256(&e)?
                    && r.get::<_, Vec<u8>>(13)? == e.experiment_manifest
                    && r.get::<_, Vec<u8>>(14)? == encoded(&e.baseline_rows)?
                    && r.get::<_, Vec<u8>>(15)? == encoded(&e.candidate_rows)?
                    && raw == encoded(&e)?,
                "persisted evaluation bytes or digest mismatch",
            )?;
            Ok(e)
        },
    )?;
    bind_evaluation(conn, &e)?;
    Ok(Some(e))
}

impl crate::Store {
    /// Register one immutable scoped artifact and its fresh, unadmitted occurrence.
    /// The trusted runtime must authenticate management authority before calling.
    pub fn register_policy_candidate(
        &mut self,
        candidate: &PolicyCandidate,
    ) -> rusqlite::Result<()> {
        let artifact = validate_policy_candidate(candidate)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = stored_candidate(&tx, &candidate.version_id)? {
            require(
                existing == *candidate,
                "conflicting policy version registration",
            )?;
        } else {
            let evidence_id = tx
                .prepare_cached(crate::INSERT_OBSERVATION)?
                .insert(params!["policy-registry/1", candidate.artifact])?;
            tx.execute("INSERT INTO policy_versions(version_id,actor,scope,purpose,evidence_id,compatibility_contract_sha256,artifact_sha256) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![candidate.version_id,candidate.actor,candidate.scope,candidate.purpose,evidence_id,artifact.compatibility_contract_sha256,candidate.sha256])?;
        }
        tx.commit()
    }

    /// Persist exact attestation contents. A stored evaluation does not imply
    /// superiority, authenticated observation, semantic validation of unknown
    /// evaluator assertions, or admission. Identical retries do not append rows.
    pub fn record_policy_evaluation(
        &mut self,
        evaluation: &PolicyEvaluation,
    ) -> rusqlite::Result<()> {
        let digest = evaluation_sha256(evaluation)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        bind_evaluation(&tx, evaluation)?;
        if let Some(existing) = stored_evaluation(&tx, &evaluation.evaluation_id)? {
            require(
                existing == *evaluation,
                "conflicting policy evaluation registration",
            )?;
        } else {
            let e = evaluation;
            tx.execute("INSERT INTO policy_evaluations(evaluation_id,actor,scope,purpose,baseline_id,candidate_id,baseline_sha256,candidate_sha256,experiment_manifest,experiment_sha256,evaluator_version,evaluator_sha256,baseline_rows,candidate_rows,evaluation_bytes,evaluation_sha256) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![e.evaluation_id,e.actor,e.scope,e.purpose,e.baseline_id,e.candidate_id,e.baseline_sha256,e.candidate_sha256,e.experiment_manifest,e.experiment_sha256,e.evaluator_version,e.evaluator_sha256,encoded(&e.baseline_rows)?,encoded(&e.candidate_rows)?,encoded(e)?,digest])?;
        }
        tx.commit()
    }
}

// All callers hold a read snapshot or the activation writer transaction. Preflight
// the scoped head before copying strings, then reuse exact artifact validation.
fn read_head(
    conn: &Connection,
    actor: &str,
    scope: &str,
    purpose: &str,
) -> rusqlite::Result<Option<ActivePolicy>> {
    let valid = conn.query_row("SELECT typeof(version_id),length(CAST(version_id AS BLOB)),typeof(revision),revision,typeof(baseline_manifest_sha256),length(CAST(baseline_manifest_sha256 AS BLOB)) FROM policy_heads WHERE actor=?1 AND scope=?2 AND purpose=?3",params![actor,scope,purpose],|r| {
        require(r.get::<_,String>(0)? == "text" && r.get::<_,i64>(1)? <= MAX_EVALUATION_BYTES as i64 && r.get::<_,String>(2)? == "integer" && r.get::<_,i64>(3)? > 0 && r.get::<_,String>(4)? == "text" && r.get::<_,i64>(5)? == 64,"invalid policy head")
    }).optional()?;
    if valid.is_none() {
        return Ok(None);
    }
    let (id, revision, manifest): (String,i64,String) = conn.query_row("SELECT version_id,revision,baseline_manifest_sha256 FROM policy_heads WHERE actor=?1 AND scope=?2 AND purpose=?3",params![actor,scope,purpose],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let c = stored_candidate(conn, &id)?.ok_or_else(|| bad("missing active artifact"))?;
    require(
        c.actor == actor && c.scope == scope && c.purpose == purpose && digest(&manifest),
        "invalid scoped policy head",
    )?;
    let a = validate_policy_candidate(&c)?;
    let evidence_id = conn.query_row(
        "SELECT evidence_id FROM policy_versions WHERE version_id=?1",
        [&id],
        |r| r.get(0),
    )?;
    Ok(Some(ActivePolicy {
        version_id: id,
        sha256: c.sha256,
        evidence_id,
        compatibility_contract_sha256: a.compatibility_contract_sha256,
        revision,
        baseline_manifest_sha256: manifest,
    }))
}
fn scope_matches(c: &PolicyCandidate, r: &ActivationRequest) -> bool {
    c.actor == r.actor && c.scope == r.scope && c.purpose == r.purpose
}
fn consistent_admissions(
    conn: &Connection,
    r: &ActivationRequest,
    head: Option<&ActivePolicy>,
) -> rusqlite::Result<bool> {
    // An inactive third occurrence is just as inconsistent as an admitted target.
    let mut stmt=conn.prepare("SELECT v.evidence_id FROM policy_versions v WHERE v.actor=?1 AND v.scope=?2 AND v.purpose=?3 AND (SELECT admitted FROM admission_events a WHERE a.actor=v.actor AND a.scope=v.scope AND a.evidence_id=v.evidence_id ORDER BY epoch DESC LIMIT 1)=1 ORDER BY v.evidence_id LIMIT 2")?;
    let ids = stmt
        .query_map(params![r.actor, r.scope, r.purpose], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(match head {
        Some(h) => ids == [h.evidence_id],
        None => ids.is_empty(),
    })
}
fn baseline_recorded(conn: &Connection, e: &PolicyEvaluation) -> rusqlite::Result<bool> {
    // Stream IDs with bounded preflight, so a malformed stored attestation never
    // becomes valid evidence through its matching projection alone.
    let mut stmt=conn.prepare("SELECT evaluation_id,typeof(evaluation_id),length(CAST(evaluation_id AS BLOB)) FROM policy_evaluations WHERE actor=?1 AND scope=?2 AND purpose=?3 AND baseline_id=?4 AND candidate_id=?4")?;
    let mut rows = stmt.query(params![e.actor, e.scope, e.purpose, e.baseline_id])?;
    while let Some(row) = rows.next()? {
        require(
            row.get::<_, String>(1)? == "text"
                && row.get::<_, i64>(2)? <= MAX_EVALUATION_BYTES as i64,
            "invalid baseline evaluation identity",
        )?;
        let id: String = row.get(0)?;
        let b = stored_evaluation(conn, &id)?.ok_or_else(|| bad("missing baseline evaluation"))?;
        if b.baseline_id == b.candidate_id
            && b.baseline_sha256 == b.candidate_sha256
            && b.baseline_rows == b.candidate_rows
            && evaluate_policy_comparison(&b)?
                == SelectionDecision::Refused(SelectionRefusal::NoImprovement)
            && paired_baseline_matches(&b, e)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl crate::Store {
    /// Metadata from one consistent snapshot; this confers no dispatch authority.
    pub fn active_policy(
        &self,
        actor: &str,
        scope: &str,
        purpose: &str,
    ) -> rusqlite::Result<Option<ActivePolicy>> {
        crate::validate_view(actor, scope)?;
        require(nonempty(purpose), "empty policy purpose")?;
        let tx = self.conn.unchecked_transaction()?;
        let head = read_head(&tx, actor, scope, purpose)?;
        tx.commit()?;
        Ok(head)
    }

    /// Trusted embedding code authenticates management authority. Every gate and
    /// write shares one Immediate transaction; old claims and holds are untouched.
    pub fn activate_policy(&mut self, r: &ActivationRequest) -> rusqlite::Result<ActivationResult> {
        use ActivationAction::*;
        use ActivationRefusal::*;
        bounded(r)?;
        crate::validate_view(&r.actor, &r.scope)?;
        require(nonempty(&r.purpose), "empty policy purpose")?;
        let (target, action, reason, evaluation_id) = match &r.action {
            Bootstrap {
                baseline_id,
                baseline_manifest_sha256,
                reason,
                ..
            } => {
                require(
                    r.expected_active_revision.is_none()
                        && digest(baseline_manifest_sha256)
                        && nonempty(reason),
                    "invalid bootstrap request",
                )?;
                (baseline_id, "bootstrap", reason.as_str(), None)
            }
            Promote {
                candidate_id,
                evaluation_id,
            } => {
                require(nonempty(evaluation_id), "empty evaluation identity")?;
                (
                    candidate_id,
                    "promote",
                    "validated paired comparison",
                    Some(evaluation_id.as_str()),
                )
            }
            Rollback {
                target_version_id,
                reason,
            } => {
                require(nonempty(reason), "empty rollback reason")?;
                (target_version_id, "rollback", reason.as_str(), None)
            }
        };
        require(nonempty(target), "empty policy target")?;
        if action != "bootstrap" {
            require(
                r.expected_active_revision.is_some_and(|v| v > 0),
                "expected positive active revision required",
            )?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        macro_rules! refuse {
            ($r:expr) => {
                return Ok(ActivationResult::Refused($r))
            };
        }
        let head = read_head(&tx, &r.actor, &r.scope, &r.purpose)?;
        if action == "bootstrap" {
            if head.is_some() {
                refuse!(AlreadyInitialized);
            }
        } else {
            let Some(h) = head.as_ref() else {
                refuse!(MissingHead);
            };
            if r.expected_active_revision != Some(h.revision) {
                refuse!(RevisionConflict);
            }
            if target == &h.version_id {
                refuse!(NoChange);
            }
        }
        let Some(c) = stored_candidate(&tx, target)? else {
            refuse!(InvalidArtifact);
        };
        if !scope_matches(&c, r) {
            refuse!(ScopeMismatch);
        }
        let artifact = validate_policy_candidate(&c)?;
        if head.as_ref().is_some_and(|h| {
            h.compatibility_contract_sha256 != artifact.compatibility_contract_sha256
        }) {
            refuse!(Incompatible);
        }
        if !consistent_admissions(&tx, r, head.as_ref())? {
            refuse!(AdmissionMismatch);
        }
        let manifest_sha = match &r.action {
            Bootstrap {
                baseline_manifest,
                baseline_manifest_sha256,
                ..
            } => {
                if sha256_hex(baseline_manifest) != *baseline_manifest_sha256 {
                    refuse!(InvalidArtifact);
                }
                let (m, _) = match evaluation::validate_manifest(baseline_manifest) {
                    Ok(m) => m,
                    Err(_) => {
                        refuse!(InvalidArtifact);
                    }
                };
                if m.actor != r.actor || m.scope != r.scope || m.purpose != r.purpose {
                    refuse!(ScopeMismatch);
                }
                if m.provenance == Provenance::SyntheticRegistryMechanics
                    && m.purpose != "fixture/recovery-policy"
                {
                    refuse!(Incompatible);
                }
                if m.compatibility_contract_sha256 != artifact.compatibility_contract_sha256 {
                    refuse!(Incompatible);
                }
                if !m
                    .policies
                    .iter()
                    .any(|p| p.artifact.sha256 == c.sha256 && p.artifact.bytes == c.artifact)
                {
                    refuse!(InvalidArtifact);
                }
                baseline_manifest_sha256.clone()
            }
            Promote { evaluation_id, .. } => {
                // Scope-bound lookup precedes decoding the persisted evaluation.
                let in_scope:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM policy_evaluations WHERE evaluation_id=?1 AND actor=?2 AND scope=?3 AND purpose=?4)",params![evaluation_id,r.actor,r.scope,r.purpose],|r|r.get(0))?;
                if !in_scope {
                    refuse!(ScopeMismatch);
                }
                let Some(e) = stored_evaluation(&tx, evaluation_id)? else {
                    refuse!(InvalidArtifact);
                };
                let h = head.as_ref().unwrap();
                if e.baseline_id != h.version_id || e.baseline_sha256 != h.sha256 {
                    refuse!(BaselineMismatch);
                }
                if e.candidate_id != c.version_id || e.candidate_sha256 != c.sha256 {
                    refuse!(InvalidArtifact);
                }
                if e.experiment_sha256 != h.baseline_manifest_sha256 {
                    refuse!(BaselineMismatch);
                }
                match evaluate_policy_comparison(&e)? {
                    SelectionDecision::Eligible { .. } => {}
                    SelectionDecision::Refused(reason) => {
                        refuse!(EvaluationRefused(reason));
                    }
                }
                if !baseline_recorded(&tx, &e)? {
                    refuse!(MissingBaselineEvaluation);
                }
                h.baseline_manifest_sha256.clone()
            }
            Rollback { .. } => {
                let activated:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM policy_events WHERE actor=?1 AND scope=?2 AND purpose=?3 AND target_version_id=?4)",params![r.actor,r.scope,r.purpose,target],|r|r.get(0))?;
                if !activated {
                    refuse!(NeverActivated);
                }
                head.as_ref().unwrap().baseline_manifest_sha256.clone()
            }
        };
        let revision = head
            .as_ref()
            .map_or(Some(1), |h| h.revision.checked_add(1))
            .ok_or_else(|| bad("policy revision overflow"))?;
        let evidence_id = tx.query_row(
            "SELECT evidence_id FROM policy_versions WHERE version_id=?1",
            [target],
            |r| r.get(0),
        )?;
        let mut edits = Vec::with_capacity(2);
        if let Some(h) = &head {
            edits.push(crate::AdmissionEdit {
                evidence: h.evidence_id,
                admitted: false,
            });
        }
        edits.push(crate::AdmissionEdit {
            evidence: evidence_id,
            admitted: true,
        });
        crate::support::apply_admission_edits(&tx, &r.actor, &r.scope, &edits)?;
        if let Some(h) = &head {
            let changed=tx.execute("UPDATE policy_heads SET version_id=?4,revision=?5 WHERE actor=?1 AND scope=?2 AND purpose=?3 AND revision=?6",params![r.actor,r.scope,r.purpose,target,revision,h.revision])?;
            require(changed == 1, "policy head CAS failed")?;
        } else {
            tx.execute("INSERT INTO policy_heads(actor,scope,purpose,version_id,revision,baseline_manifest_sha256) VALUES(?1,?2,?3,?4,?5,?6)",params![r.actor,r.scope,r.purpose,target,revision,manifest_sha])?;
        }
        tx.execute("INSERT INTO policy_events(actor,scope,purpose,revision,action,previous_version_id,target_version_id,evaluation_id,manifest_sha256,reason) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![r.actor,r.scope,r.purpose,revision,action,head.as_ref().map(|h|h.version_id.as_str()),target,evaluation_id,manifest_sha,reason])?;
        tx.commit()?;
        Ok(ActivationResult::Activated { revision })
    }
}
