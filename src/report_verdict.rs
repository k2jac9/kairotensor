//! Typed zero-model report computation over complete Store-captured evidence.
//!
//! Schema v8 persists the versioned output binding and excludes inference and
//! token reservations for this attempt. Publication still uses normal captured
//! epoch/parent preconditions. A historical delivery is not a current read grant.
use rusqlite::{params, Connection, Result, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::attempts::{
    accept, applicability, capture_complete_candidates, existing_disposition, record_delivery,
};
use crate::{invalid, Disposition, Store};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    release: String,
    passed: u64,
    total: u64,
}

#[derive(Serialize)]
struct Verdict {
    release: String,
    tests: String,
    status: &'static str,
}

fn derive(payload: &[u8]) -> Result<Vec<u8>> {
    // Deserialize directly into a strict struct: a Value would lose duplicate keys.
    let report: Report =
        serde_json::from_slice(payload).map_err(|_| invalid("invalid report evidence"))?;
    if report.release.is_empty() || report.total == 0 || report.passed > report.total {
        return Err(invalid("invalid report evidence"));
    }
    serde_json::to_vec(&Verdict {
        release: report.release,
        tests: format!("{}/{}", report.passed, report.total),
        status: if report.passed == report.total {
            "pass"
        } else {
            "fail"
        },
    })
    .map_err(|_| invalid("cannot encode report verdict"))
}

pub(crate) fn is_bound(db: &Connection, attempt: i64) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM deterministic_verdicts WHERE attempt_id=?1)",
        [attempt],
        |r| r.get(0),
    )
}

impl Store {
    /// Capture every requested occurrence and derive one complete report verdict.
    /// Missing and unadmitted candidates share a generic failure, never a filtered
    /// subset. Exactly one occurrence is required, so duplicate IDs also refuse.
    /// No caller-supplied payload, admission flag, reference or output is accepted.
    /// An error rolls back the branch, capture and output binding together.
    pub fn begin_report_verdict(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
        candidates: &[i64],
    ) -> Result<i64> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (attempt, inputs) = capture_complete_candidates(&tx, actor, scope, branch, candidates)?;
        let [(_, payload)] = inputs.as_slice() else {
            return Err(invalid(
                "exactly one complete report occurrence is required",
            ));
        };
        let output = derive(payload)?;
        tx.execute("INSERT INTO deterministic_verdicts(attempt_id,computation,output) VALUES (?1,'report-verdict/v1',?2)", params![attempt, output])?;
        tx.commit()?;
        Ok(attempt)
    }

    /// Publish only the persisted derived output, charging zero tokens. Captured
    /// view/branch changes produce a zero-cost stale delivery, never a new head.
    /// Duplicate commits return the original HISTORICAL disposition, including
    /// Accepted after revocation. Use read_current for current eligibility/bytes.
    /// This cannot settle or release another model/recovery attempt's unknown hold.
    pub fn commit_report_verdict(&mut self, attempt: i64) -> Result<Disposition> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let output: Vec<u8> = tx.query_row("SELECT output FROM deterministic_verdicts WHERE attempt_id=?1 AND computation='report-verdict/v1'", [attempt], |r|r.get(0))?;
        if let Some(prior) = existing_disposition(&tx, attempt, &output, 0, None, None)? {
            return Ok(prior);
        }
        let (branch, disposition) = applicability(&tx, attempt)?;
        if disposition == Disposition::Accepted {
            accept(&tx, branch, attempt, &output, 0)?;
        } else {
            record_delivery(&tx, attempt, disposition, &output, 0)?;
        }
        tx.commit()?;
        Ok(disposition)
    }
}
