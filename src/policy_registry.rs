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
