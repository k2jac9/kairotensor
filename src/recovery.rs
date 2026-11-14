//! Durable, bounded recovery and a claim-before-send dispatch fence.
use crate::{
    attempts::{capture_candidates, capture_complete_candidates, pending_branch},
    invalid,
    token_budget::budget_state,
    CapturedInputs, Store,
};
use rusqlite::{params, Connection, OptionalExtension, Result, TransactionBehavior};

#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryStart {
    New {
        attempt: i64,
        inputs: CapturedInputs,
        retry_reason: Option<String>,
    },
    Existing(i64),
    Finished,
}

/// Exact persisted bytes and quote. Reading this is not permission to send.
#[derive(Debug, PartialEq, Eq)]
pub struct RecoveryReservation {
    pub request: Vec<u8>,
    pub input_tokens: i64,
    pub output_cap: i64,
}

pub(crate) fn is_managed(db: &Connection, attempt: i64) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM recovery_attempts WHERE attempt_id = ?1)",
        [attempt],
        |r| r.get(0),
    )
}

impl Store {
    /// Create or look up one logical operation. Reusing its key requires exactly
    /// the same budget and actor/scope/branch binding. Persist the key at the caller;
    /// inventing a new key deliberately creates a different operation.
    pub fn create_recovery(
        &mut self,
        key: &str,
        budget: i64,
        actor: &str,
        scope: &str,
        branch: &str,
    ) -> Result<i64> {
        crate::validate_view(actor, scope)?;
        if key.trim().is_empty() || branch.is_empty() {
            return Err(invalid("recovery key and branch must be nonempty"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(i64, bool)> = tx
            .query_row(
                "SELECT id, (budget_id = ?2 AND actor = ?3 AND scope = ?4 AND branch = ?5)
             FROM recovery_runs WHERE recovery_key = ?1",
                params![key, budget, actor, scope, branch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, matches)) = existing {
            if crate::timed_recovery::is_timed_run(&tx, id)? {
                return Err(invalid(
                    "timed recovery requires exact timed creation bindings",
                ));
            }
            if !matches {
                return Err(invalid("recovery key is already bound to different inputs"));
            }
            return Ok(id);
        }
        tx.execute("INSERT INTO recovery_runs(recovery_key,budget_id,actor,scope,branch) VALUES (?1,?2,?3,?4,?5)",
                   params![key,budget,actor,scope,branch])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }
    /// Allocate the original attempt or the single retry after invalid_output.
    /// A restart/concurrent caller sees Existing rather than receiving a second
    /// send opportunity. Cancellation and denial terminate the latest slot.
    pub fn begin_recovery_attempt(
        &mut self,
        recovery: i64,
        candidates: &[i64],
    ) -> Result<RecoveryStart> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (actor, scope, branch): (String, String, String) = tx.query_row(
            "SELECT actor,scope,branch FROM recovery_runs WHERE id=?1",
            [recovery],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let prior: Option<(i64,i64,String,Option<String>,Option<String>)> = tx.query_row(
            "SELECT r.attempt_id,r.ordinal,r.state,d.disposition,d.reason FROM recovery_attempts r
             LEFT JOIN deliveries d ON d.attempt_id=r.attempt_id WHERE r.recovery_id=?1 ORDER BY r.ordinal DESC LIMIT 1",
            [recovery], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (ordinal, retry_reason) = match prior {
            None => (0, None),
            Some((attempt, _, state, None, _))
                if matches!(state.as_str(), "planned" | "reserved" | "sent") =>
            {
                return Ok(RecoveryStart::Existing(attempt))
            }
            Some((_, 0, state, Some(disposition), reason))
                if state == "sent" && disposition == "invalid_output" =>
            {
                (1, reason)
            }
            Some(_) => return Ok(RecoveryStart::Finished),
        };
        let (attempt, inputs) = if crate::timed_recovery::is_timed_run(&tx, recovery)? {
            capture_complete_candidates(&tx, &actor, &scope, &branch, candidates)?
        } else {
            capture_candidates(&tx, &actor, &scope, &branch, candidates)?
        };
        tx.execute("INSERT INTO recovery_attempts(recovery_id,ordinal,attempt_id,state) VALUES (?1,?2,?3,'planned')",
                   params![recovery,ordinal,attempt])?;
        tx.commit()?;
        Ok(RecoveryStart::New {
            attempt,
            inputs,
            retry_reason,
        })
    }
    /// Freeze a quote and reserve capacity without authorizing network/model I/O.
    /// Preparation must have used exactly these bytes. Duplicate reservations
    /// (even identical ones) fail, preventing callers from overwriting the quote.
    pub fn reserve_recovery_attempt(
        &mut self,
        attempt: i64,
        request: &[u8],
        input: i64,
        cap: i64,
    ) -> Result<bool> {
        if input < 0 || cap <= 0 {
            return Err(invalid("invalid recovery token quote"));
        }
        let maximum = input
            .checked_add(cap)
            .ok_or_else(|| invalid("token reservation exceeds the integer range"))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (budget,state):(i64,String)=tx.query_row(
            "SELECT w.budget_id,r.state FROM recovery_attempts r JOIN recovery_runs w ON w.id=r.recovery_id WHERE r.attempt_id=?1",
            [attempt],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if state != "planned" {
            return Err(invalid(
                "recovery attempt already has a reservation decision",
            ));
        }
        pending_branch(&tx, attempt)?;
        let balance = budget_state(&tx, budget)?;
        let granted = maximum <= balance.limit - balance.spent - balance.reserved;
        tx.execute("INSERT INTO token_reservations(attempt_id,budget_id,input_tokens,output_cap,granted) VALUES (?1,?2,?3,?4,?5)",params![attempt,budget,input,cap,granted])?;
        tx.execute(
            "UPDATE recovery_attempts SET state=?2,request=?3 WHERE attempt_id=?1",
            params![
                attempt,
                if granted { "reserved" } else { "denied" },
                request
            ],
        )?;
        tx.commit()?;
        Ok(granted)
    }

    /// Inspect an unsent reservation to re-prepare it after a restart. Check the
    /// new preparation's request/count/cap against these immutable values BEFORE
    /// claiming. Discard preparation if another worker/cancellation wins.
    pub fn recovery_reservation(&self, attempt: i64) -> Result<Option<RecoveryReservation>> {
        self.conn.query_row("SELECT r.request,t.input_tokens,t.output_cap FROM recovery_attempts r
            JOIN token_reservations t ON t.attempt_id=r.attempt_id WHERE r.attempt_id=?1 AND r.state='reserved'",
            [attempt],|r|Ok(RecoveryReservation {request:r.get(0)?,input_tokens:r.get(1)?,output_cap:r.get(2)?})).optional()
    }

    /// The only send permission for a recovery attempt. Exactly one caller gets
    /// Some(bytes). Commit this BEFORE I/O, and submit exactly once. A crash or
    /// lost return after this claim is ambiguous: do not reclaim or auto-resend.
    pub fn claim_recovery_dispatch(&mut self, attempt: i64) -> Result<Option<Vec<u8>>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if crate::timed_recovery::is_timed_attempt(&tx, attempt)? {
            return Err(invalid("timed recovery requires deadline-aware claim"));
        }
        let (state, request): (String, Option<Vec<u8>>) = tx.query_row(
            "SELECT state,request FROM recovery_attempts WHERE attempt_id=?1",
            [attempt],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if state != "reserved" {
            return Ok(None);
        }
        pending_branch(&tx, attempt)?;
        let request = request.ok_or_else(|| invalid("reserved recovery has no request"))?;
        tx.execute(
            "UPDATE recovery_attempts SET state='sent' WHERE attempt_id=?1",
            [attempt],
        )?;
        tx.execute(
            "INSERT INTO dispatches(attempt_id,request) VALUES (?1,?2)",
            params![attempt, request],
        )?;
        tx.commit()?;
        Ok(Some(request))
    }

    /// Release only a planned/reserved attempt that has never been claimed.
    /// Works even if its evidence/branch is stale. Idempotent after cancellation;
    /// false for sent/denied. Never records fabricated usage or a delivery.
    pub fn cancel_recovery_attempt(&mut self, attempt: i64) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: String = tx.query_row(
            "SELECT state FROM recovery_attempts WHERE attempt_id=?1",
            [attempt],
            |r| r.get(0),
        )?;
        if state == "cancelled" {
            return Ok(true);
        }
        if !matches!(state.as_str(), "planned" | "reserved") {
            return Ok(false);
        }
        tx.execute(
            "UPDATE recovery_attempts SET state='cancelled' WHERE attempt_id=?1",
            [attempt],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_key_is_idempotent_but_cannot_change_its_budget() -> Result<()> {
        let mut store = Store::in_memory()?;
        let first = store.create_token_budget(100)?;
        let second = store.create_token_budget(100)?;
        let recovery = store.create_recovery("job", first, "actor", "scope", "branch")?;
        assert_eq!(
            store.create_recovery("job", first, "actor", "scope", "branch")?,
            recovery
        );
        assert!(store
            .create_recovery("job", second, "actor", "scope", "branch")
            .is_err());
        Ok(())
    }
}
