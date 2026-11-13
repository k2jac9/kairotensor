//! Durable token ceilings for single-generation dispatches. The local adapter
//! must preflight the exact request and enforce the supplied generation cap.
use rusqlite::{params, Connection, Result, TransactionBehavior};

use crate::{attempts::pending_branch, invalid, Store};

/// Snapshot of a budget. Available tokens equal `limit - spent - reserved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBudgetState {
    pub limit: i64,
    pub spent: i64,
    pub reserved: i64,
}

pub(crate) fn has_decision(db: &Connection, attempt: i64) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM token_reservations WHERE attempt_id = ?1)",
        [attempt],
        |r| r.get(0),
    )
}

fn budget_state(db: &Connection, budget: i64) -> Result<TokenBudgetState> {
    db.query_row(
        "SELECT b.token_limit, COALESCE(SUM(d.usage), 0),
                COALESCE(SUM(CASE WHEN d.attempt_id IS NULL THEN r.input_tokens + r.output_cap END), 0)
         FROM token_budgets b
         LEFT JOIN token_reservations r ON r.budget_id = b.id AND r.granted = 1
         LEFT JOIN deliveries d ON d.attempt_id = r.attempt_id
         WHERE b.id = ?1 GROUP BY b.id",
        [budget],
        |r| Ok(TokenBudgetState { limit: r.get(0)?, spent: r.get(1)?, reserved: r.get(2)? }),
    )
}

impl Store {
    /// Create a budget local to this database. Only attempts submitted via
    /// `reserve_dispatch` are assigned to it. A token limit of zero is valid.
    pub fn create_token_budget(&mut self, limit: i64) -> Result<i64> {
        self.conn.prepare_cached("INSERT INTO token_budgets(token_limit) VALUES (?1)")?.insert([limit])
    }

    /// Read charged and outstanding token usage together in one SQLite snapshot.
    pub fn token_budget_state(&self, budget: i64) -> Result<TokenBudgetState> {
        budget_state(&self.conn, budget)
    }

    /// Authorize exactly one ordinary dispatch with a token ceiling. The caller
    /// supplies the frozen request, its exact tokenized input length, and the
    /// maximum number of new tokens the worker will generate from that input.
    ///
    /// Returns true after committing both the reservation and the frozen dispatch;
    /// false records a denial without consuming capacity. Either decision consumes
    /// this attempt's dispatch opportunity. Use a fresh attempt for a new decision.
    /// An uncertain send must be reconciled before any retransmission.
    ///
    /// Use `settle` or `reject_invalid` for known actual usage. A missing delivery
    /// leaves the entire reservation outstanding across restart.
    pub fn reserve_dispatch(
        &mut self,
        budget: i64,
        attempt: i64,
        request: &[u8],
        input_tokens: i64,
        output_cap: i64,
    ) -> Result<bool> {
        if input_tokens < 0 || output_cap <= 0 {
            return Err(invalid("token reservation requires nonnegative input and a positive output cap"));
        }
        let maximum = input_tokens.checked_add(output_cap)
            .ok_or_else(|| invalid("token reservation exceeds the integer range"))?;
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        pending_branch(&tx, attempt)?;
        if crate::model_reuse::is_opted_in(&tx, attempt)? {
            return Err(invalid("token reservations require an ordinary model attempt"));
        }
        let already_dispatched: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM dispatches WHERE attempt_id = ?1)", [attempt], |r| r.get(0),
        )?;
        if already_dispatched || has_decision(&tx, attempt)? {
            return Err(invalid("attempt already dispatched or has a token budget decision"));
        }
        let state = budget_state(&tx, budget)?;
        let granted = maximum <= state.limit - state.spent - state.reserved;
        tx.execute(
            "INSERT INTO token_reservations(attempt_id, budget_id, input_tokens, output_cap, granted)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![attempt, budget, input_tokens, output_cap, granted],
        )?;
        if granted {
            tx.execute("INSERT INTO dispatches(attempt_id, request) VALUES (?1, ?2)", params![attempt, request])?;
        }
        tx.commit()?;
        Ok(granted)
    }
}
