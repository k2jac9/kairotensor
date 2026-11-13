//! Selective revision for the closed support/v1 query: any group whose
//! observation IDs are all admitted is a witness. Only the resulting flag can
//! be reused under this rule. Observation payloads are never read by the rule.
use rusqlite::{params, Connection, OptionalExtension, Result, Transaction, TransactionBehavior};

use crate::attempts::{accept, ensure_branch, find_branch, record_attempt};
use crate::{invalid, is_admitted, view_epoch, Store};

fn output(supported: bool) -> &'static [u8] {
    if supported { b"supported" } else { b"unsupported" }
}

struct RuleState {
    rule: i64,
    published: bool,
    current: bool,
}

fn rule_state(db: &Connection, head: i64) -> Result<Option<RuleState>> {
    db.query_row(
        "SELECT s.rule_id, s.verdict, EXISTS (
             SELECT 1 FROM support_clauses c WHERE c.rule_id = s.rule_id AND c.missing = 0
         )
         FROM support_bindings s
         JOIN attempts a ON a.id = s.attempt_id
         JOIN branches b ON b.id = a.branch_id AND b.head = a.id
         WHERE s.attempt_id = ?1",
        [head],
        |row| {
            Ok(RuleState {
                rule: row.get(0)?,
                published: row.get(1)?,
                current: row.get(2)?,
            })
        },
    )
    .optional()
}

pub(crate) fn is_current(db: &Connection, head: i64) -> Result<bool> {
    Ok(rule_state(db, head)?.is_some_and(|state| state.current == state.published))
}

// Called exactly once per effective transition, in the admission writer's
// transaction. Distinct clauses may share an occurrence; each changes once.
pub(crate) fn on_admission(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    evidence: i64,
    admitted: bool,
) -> Result<()> {
    let delta: i64 = if admitted { -1 } else { 1 };
    tx.execute(
        "UPDATE support_clauses SET missing = missing + ?4 WHERE id IN (
             SELECT clause_id FROM support_premises
             WHERE actor = ?1 AND scope = ?2 AND requested_id = ?3 AND watching = 1
         )",
        params![actor, scope, evidence, delta],
    )?;
    Ok(())
}

fn bind(
    tx: &Transaction<'_>,
    attempt: i64,
    rule: i64,
    supported: bool,
) -> Result<()> {
    tx.execute(
        "INSERT INTO support_bindings(attempt_id, rule_id, verdict) VALUES (?1, ?2, ?3)",
        params![attempt, rule, supported],
    )?;
    Ok(())
}

impl Store {
    /// Publish a new support/v1 definition and its current flag. Groups are
    /// alternatives and each group requires all its distinct positive IDs.
    /// Unadmitted and missing IDs have identical membership semantics.
    /// Each registration is a new assertion, even for the same formula.
    pub fn publish_support(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
        alternatives: &[Vec<i64>],
    ) -> Result<i64> {
        if alternatives.is_empty() || alternatives.iter().any(Vec::is_empty) {
            return Err(invalid("support requires at least one nonempty group and no empty groups"));
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let branch = ensure_branch(&tx, actor, scope, branch)?;
        let attempt = record_attempt(&tx, &branch, view_epoch(&tx, actor, scope)?, None)?;
        let mut supported = false;
        let mut checks = 0;
        for group in alternatives {
            let arity = i64::try_from(group.len()).map_err(|_| invalid("group is too large"))?;
            let mut missing = 0_i64;
            for &id in group {
                if id <= 0 {
                    return Err(invalid("an observation occurrence ID must be positive"));
                }
                if !is_admitted(&tx, actor, scope, id)? {
                    missing += 1;
                }
            }
            tx.execute(
                "INSERT INTO support_clauses(rule_id, arity, missing) VALUES (?1, ?2, ?3)",
                params![attempt, arity, missing],
            )?;
            let clause = tx.last_insert_rowid();
            for &id in group {
                // The compound primary key rejects a duplicate within a group.
                tx.execute(
                    "INSERT INTO support_premises(clause_id, requested_id, actor, scope)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![clause, id, actor, scope],
                )?;
            }
            checks += arity;
            supported |= missing == 0;
        }
        bind(&tx, attempt, attempt, supported)?;
        accept(&tx, branch.id, attempt, output(supported), checks)?;
        tx.commit()?;
        Ok(attempt)
    }

    /// Return an eligible head unchanged or issue the revised flag atomically.
    /// A fresh answer charges one clause-existence query. Opaque heads cannot
    /// receive a support certificate or inherit the previous head's formula.
    pub fn revise_support(&mut self, actor: &str, scope: &str, branch: &str) -> Result<i64> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let branch = find_branch(&tx, actor, scope, branch)?
            .ok_or_else(|| invalid("no support rule at this branch head"))?;
        let head = branch.head.ok_or_else(|| invalid("no support rule at this branch head"))?;
        let state = rule_state(&tx, head)?
            .ok_or_else(|| invalid("no support rule at this branch head"))?;
        if state.current == state.published {
            tx.commit()?;
            return Ok(head);
        }
        let attempt = record_attempt(&tx, &branch, view_epoch(&tx, actor, scope)?, None)?;
        bind(&tx, attempt, state.rule, state.current)?;
        accept(&tx, branch.id, attempt, output(state.current), 1)?;
        tx.commit()?;
        Ok(attempt)
    }

    /// List the active support-rule branch names watching this occurrence in
    /// the named view. An unadmitted ID can be watched for later admission.
    pub fn support_consumers(&self, actor: &str, scope: &str, evidence: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT b.name FROM support_premises p
             JOIN support_clauses c ON c.id = p.clause_id
             JOIN attempts a ON a.id = c.rule_id
             JOIN branches b ON b.id = a.branch_id
             WHERE p.actor = ?1 AND p.scope = ?2 AND p.requested_id = ?3 AND p.watching = 1
             ORDER BY b.name",
        )?;
        let rows = stmt.query_map(params![actor, scope, evidence], |row| row.get(0))?;
        rows.collect()
    }
}
