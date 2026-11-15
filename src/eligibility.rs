//! Payload-free diagnostics for the authoritative current-read gate.
use rusqlite::{Connection, Result};

use crate::attempts::{find_branch, Branch};
use crate::support::{inspect_current, SupportStatus};
use crate::{view_epoch, Store};

/// Eligibility under the Store contract, not a claim that an output is true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EligibilityState {
    Empty,
    Ready,
    NeedsReview,
}

/// The proof used at this snapshot; model reuse requires explicit resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EligibilityProof {
    CoarseEpoch,
    SupportV1,
}

/// Fixed reason codes never contain evidence or free-form validation text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EligibilityReason {
    NoHead,
    CoarseViewChanged,
    SupportVerdictChanged,
}

/// Metadata from one read transaction. This is not a reusable authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentExplanation {
    pub state: EligibilityState,
    pub head_id: Option<i64>,
    pub branch_revision: Option<i64>,
    pub captured_epoch: Option<i64>,
    pub current_epoch: i64,
    pub proof_kind: Option<EligibilityProof>,
    pub reason: Option<EligibilityReason>,
}

// The caller must retain its transaction through any subsequent payload load.
// This evaluator never selects output, request, evidence, or validator text.
pub(crate) fn evaluate(
    db: &Connection,
    branch: Option<&Branch>,
    current_epoch: i64,
) -> Result<CurrentExplanation> {
    let mut explanation = CurrentExplanation {
        state: EligibilityState::Empty,
        head_id: branch.and_then(|b| b.head),
        branch_revision: branch.map(|b| b.revision),
        captured_epoch: None,
        current_epoch,
        proof_kind: None,
        reason: Some(EligibilityReason::NoHead),
    };
    let Some((branch, head)) = branch.and_then(|b| b.head.map(|head| (b, head))) else {
        return Ok(explanation);
    };
    // Missing/nonaccepted delivery or a head belonging to another branch is a
    // malformed stored head, even if its epoch would otherwise be stale.
    let captured_epoch: i64 = db.query_row(
        "SELECT a.view_epoch FROM attempts a
         JOIN deliveries d ON d.attempt_id = a.id AND d.disposition = 'accepted'
         WHERE a.id = ?1 AND a.branch_id = ?2",
        rusqlite::params![head, branch.id],
        |row| row.get(0),
    )?;
    explanation.captured_epoch = Some(captured_epoch);
    let (state, proof, reason) = if captured_epoch == current_epoch {
        (EligibilityState::Ready, EligibilityProof::CoarseEpoch, None)
    } else {
        match inspect_current(db, head)? {
            SupportStatus::SameVerdict => {
                (EligibilityState::Ready, EligibilityProof::SupportV1, None)
            }
            SupportStatus::ChangedVerdict => (
                EligibilityState::NeedsReview,
                EligibilityProof::SupportV1,
                Some(EligibilityReason::SupportVerdictChanged),
            ),
            SupportStatus::NotSupport => (
                EligibilityState::NeedsReview,
                EligibilityProof::CoarseEpoch,
                Some(EligibilityReason::CoarseViewChanged),
            ),
        }
    };
    explanation.state = state;
    explanation.proof_kind = Some(proof);
    explanation.reason = reason;
    Ok(explanation)
}

impl Store {
    /// Explain the current head without loading its payload or changing history.
    /// The embedding runtime authenticates actor/scope and diagnostic access.
    /// A later read evaluates its own snapshot; this result grants no access.
    pub fn explain_current(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
    ) -> Result<CurrentExplanation> {
        let tx = self.conn.transaction()?;
        let branch = find_branch(&tx, actor, scope, branch)?;
        let result = evaluate(&tx, branch.as_ref(), view_epoch(&tx, actor, scope)?)?;
        tx.commit()?;
        Ok(result)
    }
}
