//! Explicit reuse of a previously accepted model result on a single branch.
use rusqlite::{params, Connection, OptionalExtension, Result, TransactionBehavior};

use crate::attempts::{accept, capture_candidates, pending_branch};
use crate::{invalid, CapturedInputs, Store};

/// Decision at the frozen request boundary. Only `Dispatch` calls the model.
#[derive(Debug, PartialEq, Eq)]
pub enum ModelAction {
    /// The request was frozen for the caller to send and subsequently settle.
    Dispatch,
    /// This attempt has already published the original result with zero new usage.
    Reused { origin_attempt: i64, output: Vec<u8> },
}

pub(crate) fn is_opted_in(db: &Connection, attempt: i64) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM model_attempts WHERE attempt_id = ?1)",
        [attempt],
        |row| row.get(0),
    )
}

// Normal settlement still handles every completed delivery, including duplicates.
// A pending opt-in cannot publish an unexecuted result through the generic API.
pub(crate) fn unresolved(db: &Connection, attempt: i64) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM model_attempts m
         WHERE m.attempt_id = ?1
           AND NOT EXISTS(SELECT 1 FROM dispatches d WHERE d.attempt_id = m.attempt_id))",
        [attempt],
        |row| row.get(0),
    )
}

// Candidate positions describe the selection query, including misses. Input
// positions describe the occurrences actually submitted. Each (kind, pos, id)
// must occur once in each manifest; grouping by position preserves duplicates.
fn same_manifests(db: &Connection, attempt: i64, origin: i64) -> Result<bool> {
    db.query_row(
        "SELECT NOT EXISTS (
             SELECT 1 FROM (
                 SELECT 0 AS kind, position, requested_id AS id FROM attempt_candidates
                 WHERE attempt_id IN (?1, ?2)
                 UNION ALL
                 SELECT 1, position, evidence_id FROM attempt_inputs
                 WHERE attempt_id IN (?1, ?2)
             ) GROUP BY kind, position, id HAVING COUNT(*) <> 2
         )",
        params![attempt, origin],
        |row| row.get(0),
    )
}

// pending_branch has already validated the candidate parent's identity and revision
// under this writer lock. Only its result can be reused; never search old history
// behind a newer opaque head. Re-check every admission since the real execution.
fn matching_origin(db: &Connection, attempt: i64, request: &[u8]) -> Result<Option<i64>> {
    let origin: Option<i64> = db.query_row(
        "SELECT original.id
         FROM attempts a
         JOIN branches b ON b.id = a.branch_id
         JOIN model_attempts current ON current.attempt_id = a.id
         JOIN model_attempts parent ON parent.attempt_id = a.parent
         LEFT JOIN model_reuses reuse ON reuse.attempt_id = parent.attempt_id
         JOIN attempts original ON original.id = COALESCE(reuse.origin_attempt, parent.attempt_id)
             AND original.branch_id = b.id
         JOIN model_attempts source ON source.attempt_id = original.id
         JOIN deliveries result ON result.attempt_id = original.id AND result.disposition = 'accepted'
         JOIN dispatches d ON d.attempt_id = original.id AND d.request = ?2
         WHERE a.id = ?1 AND a.inherited_from IS NULL AND original.inherited_from IS NULL
           AND current.contract = source.contract AND parent.contract = source.contract
           AND NOT EXISTS (
               SELECT 1 FROM attempt_candidates c JOIN admission_events e
                 ON e.actor = b.actor AND e.scope = b.scope AND e.evidence_id = c.requested_id
               WHERE c.attempt_id = a.id AND e.epoch > original.view_epoch AND e.epoch <= a.view_epoch
           )",
        params![attempt, request],
        |row| row.get(0),
    ).optional()?;
    if let Some(id) = origin {
        if same_manifests(db, attempt, id)? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

impl Store {
    /// Capture a fresh model request whose caller permits reuse of one realized result.
    /// All mutable evidence dependencies must be represented in the fixed, ordered
    /// candidate list. The nonempty contract identifies the complete assembly,
    /// validation and reuse semantics. No branch output is inherited.
    ///
    /// Use the ordinary attempt and dispatch APIs for independent samples, external
    /// effects, time-dependent answers or inputs whose dependencies cannot be closed.
    pub fn begin_model_attempt(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
        candidates: &[i64],
        contract: &str,
    ) -> Result<(i64, CapturedInputs)> {
        if contract.trim().is_empty() {
            return Err(invalid("model reuse requires a nonempty contract"));
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let captured = capture_candidates(&tx, actor, scope, branch, candidates)?;
        tx.execute(
            "INSERT INTO model_attempts(attempt_id, contract) VALUES (?1, ?2)",
            params![captured.0, contract],
        )?;
        tx.commit()?;
        Ok(captured)
    }

    /// Resolve once, immediately before model I/O, using the exact versioned
    /// request envelope the caller would dispatch. A hit atomically accepts the
    /// original bytes, records the originating execution and charges zero. A miss
    /// freezes the request for dispatch and normal post-execution settlement.
    ///
    /// Both outcomes are single-use. A caller recovering from an uncertain send
    /// must reconcile its transport; repeating this operation cannot authorize a
    /// second send or return a historical result after a subsequent withdrawal.
    pub fn resolve_model_attempt(&mut self, attempt: i64, request: &[u8]) -> Result<ModelAction> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let branch = pending_branch(&tx, attempt)?;
        if !is_opted_in(&tx, attempt)? {
            return Err(invalid("attempt did not opt in to model result reuse"));
        }
        let dispatched: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM dispatches WHERE attempt_id = ?1)",
            [attempt],
            |row| row.get(0),
        )?;
        if dispatched {
            return Err(invalid("request already resolved for dispatch; reconcile its delivery"));
        }
        let action = if let Some(origin) = matching_origin(&tx, attempt, request)? {
            let output: Vec<u8> = tx.query_row(
                "SELECT output FROM deliveries WHERE attempt_id = ?1", [origin], |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO model_reuses(attempt_id, origin_attempt) VALUES (?1, ?2)",
                params![attempt, origin],
            )?;
            accept(&tx, branch, attempt, &output, 0)?;
            ModelAction::Reused { origin_attempt: origin, output }
        } else {
            tx.execute(
                "INSERT INTO dispatches(attempt_id, request) VALUES (?1, ?2)",
                params![attempt, request],
            )?;
            ModelAction::Dispatch
        };
        tx.commit()?;
        Ok(action)
    }
}
