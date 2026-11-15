//! Selective revision for the closed support/v1 query: any group whose
//! observation IDs are all admitted is a witness. Only the resulting flag can
//! be reused under this rule. Observation payloads are never read by the rule.
use rusqlite::{params, Connection, OptionalExtension, Result, Transaction, TransactionBehavior};
use std::collections::{hash_map::Entry, HashMap};

use crate::attempts::{accept, ensure_branch, find_branch, record_attempt, Branch};
use crate::{change_admission, invalid, is_admitted, validate_view, view_epoch, Store};

/// One named support/v1 rule to publish. Every observation in one group must
/// be admitted; any complete group supports the rule.
#[derive(Clone, Copy, Debug)]
pub struct SupportRegistration<'a> {
    /// Branch to receive the new accepted head.
    pub branch: &'a str,
    /// Ordered alternatives. Each group must contain distinct positive IDs.
    pub alternatives: &'a [Vec<i64>],
}

/// One accepted support/v1 publication made by an atomic admission edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportRevision {
    /// Branch name within the edited actor and scope.
    pub branch: String,
    /// Newly accepted attempt containing the published result and its charge.
    pub attempt_id: i64,
    /// Whether any declared witness group is complete at this edit.
    pub supported: bool,
}

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

pub(crate) enum SupportStatus {
    NotSupport,
    SameVerdict,
    ChangedVerdict,
}

pub(crate) fn inspect_current(db: &Connection, head: i64) -> Result<SupportStatus> {
    Ok(match rule_state(db, head)? {
        None => SupportStatus::NotSupport,
        Some(state) if state.current == state.published => SupportStatus::SameVerdict,
        Some(_) => SupportStatus::ChangedVerdict,
    })
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

fn publish_revision(
    tx: &Transaction<'_>,
    branch: &Branch,
    epoch: i64,
    rule: i64,
    supported: bool,
) -> Result<i64> {
    let attempt = record_attempt(tx, branch, epoch, None)?;
    bind(tx, attempt, rule, supported)?;
    accept(tx, branch.id, attempt, output(supported), 1)?;
    Ok(attempt)
}

struct ChangedHead {
    name: String,
    branch: Branch,
    rule: i64,
    supported: bool,
}

// Deduplicate rules before checking their verdicts: an occurrence can appear in
// several alternatives. Finish the SELECT before mutating the heads it reads.
fn changed_heads(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    evidence: i64,
) -> Result<Vec<ChangedHead>> {
    let mut select = tx.prepare_cached(
        "WITH affected(rule_id) AS (
             SELECT DISTINCT c.rule_id FROM support_premises p
             JOIN support_clauses c ON c.id = p.clause_id
             WHERE p.actor = ?1 AND p.scope = ?2 AND p.requested_id = ?3 AND p.watching = 1
         )
         SELECT b.name, b.id, b.head, b.revision, s.rule_id, s.verdict, EXISTS (
             SELECT 1 FROM support_clauses c WHERE c.rule_id = s.rule_id AND c.missing = 0
         )
         FROM affected
         JOIN attempts a ON a.id = affected.rule_id
         JOIN branches b ON b.id = a.branch_id
         JOIN support_bindings s ON s.attempt_id = b.head AND s.rule_id = affected.rule_id
         WHERE b.actor = ?1 AND b.scope = ?2
         ORDER BY b.name",
    )?;
    let mut rows = select.query(params![actor, scope, evidence])?;
    let mut changed = Vec::new();
    while let Some(row) = rows.next()? {
        let published: bool = row.get(5)?;
        let supported: bool = row.get(6)?;
        if published != supported {
            changed.push(ChangedHead {
                name: row.get(0)?,
                branch: Branch {
                    id: row.get(1)?,
                    head: row.get(2)?,
                    revision: row.get(3)?,
                },
                rule: row.get(4)?,
                supported,
            });
        }
    }
    Ok(changed)
}

// A bulk registration never changes admissions. Bind this cache to its single
// actor, scope and writer transaction; both missing and visible IDs can repeat.
fn cached_admissions(mut lookup: impl FnMut(i64) -> Result<bool>) -> impl FnMut(i64) -> Result<bool> {
    let mut entries = HashMap::new();
    move |id| match entries.entry(id) {
        Entry::Occupied(hit) => Ok(*hit.get()),
        Entry::Vacant(miss) => Ok(*miss.insert(lookup(id)?)),
    }
}

fn register_support(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    epoch: i64,
    rule: &SupportRegistration<'_>,
    mut admitted: impl FnMut(i64) -> Result<bool>,
) -> Result<i64> {
    if rule.alternatives.is_empty() || rule.alternatives.iter().any(Vec::is_empty) {
        return Err(invalid("support requires at least one nonempty group and no empty groups"));
    }
    let branch = ensure_branch(tx, actor, scope, rule.branch)?;
    let attempt = record_attempt(tx, &branch, epoch, None)?;
    let mut insert_clause = tx.prepare_cached(
        "INSERT INTO support_clauses(rule_id, arity, missing) VALUES (?1, ?2, ?3)",
    )?;
    let mut insert_premise = tx.prepare_cached(
        "INSERT INTO support_premises(clause_id, requested_id, actor, scope)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut supported = false;
    let mut checks = 0_i64;
    for group in rule.alternatives {
        let arity = i64::try_from(group.len()).map_err(|_| invalid("group is too large"))?;
        let mut missing = 0_i64;
        for &id in group {
            if id <= 0 {
                return Err(invalid("an observation occurrence ID must be positive"));
            }
            if !admitted(id)? {
                missing += 1;
            }
        }
        insert_clause.execute(params![attempt, arity, missing])?;
        let clause = tx.last_insert_rowid();
        for &id in group {
            // The compound primary key rejects a duplicate within a group.
            insert_premise.execute(params![clause, id, actor, scope])?;
        }
        checks = checks.checked_add(arity).ok_or_else(|| invalid("rule is too large"))?;
        supported |= missing == 0;
    }
    bind(tx, attempt, attempt, supported)?;
    accept(tx, branch.id, attempt, output(supported), checks)?;
    Ok(attempt)
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
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let epoch = view_epoch(&tx, actor, scope)?;
        let attempt = register_support(
            &tx, actor, scope, epoch, &SupportRegistration { branch, alternatives },
            |id| is_admitted(&tx, actor, scope, id),
        )?;
        tx.commit()?;
        Ok(attempt)
    }

    /// Publish an ordered catalogue of rules in one transaction and return their
    /// accepted attempt IDs in input order. All rules see the same admission epoch.
    /// Repeated branch names form successive publications, retaining each attempt,
    /// its parent, and its charge. A later error rolls back the entire call.
    ///
    /// An empty catalogue returns an empty list. Replaying a successful nonempty
    /// catalogue intentionally creates fresh assertions and charges.
    pub fn publish_support_many(
        &mut self,
        actor: &str,
        scope: &str,
        rules: &[SupportRegistration<'_>],
    ) -> Result<Vec<i64>> {
        validate_view(actor, scope)?;
        if rules.is_empty() {
            return Ok(Vec::new());
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let epoch = view_epoch(&tx, actor, scope)?;
        let mut attempts = Vec::with_capacity(rules.len());
        {
            let mut admitted = cached_admissions(|id| is_admitted(&tx, actor, scope, id));
            for rule in rules {
                attempts.push(register_support(&tx, actor, scope, epoch, rule, &mut admitted)?);
            }
        }
        tx.commit()?;
        Ok(attempts)
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
        let epoch = view_epoch(&tx, actor, scope)?;
        let attempt = publish_revision(&tx, &branch, epoch, state.rule, state.current)?;
        tx.commit()?;
        Ok(attempt)
    }

    /// Change one observation's admission and reconcile its current support/v1
    /// consumers in this actor's scope in one SQLite transaction. A reader
    /// using a single snapshot sees the state before or after the whole edit.
    ///
    /// Returns only newly published verdicts, ordered by branch name. Each
    /// receives a new accepted attempt and a one-unit charge. Consumers whose
    /// published verdict still holds keep their original attempts.
    ///
    /// Setting the existing admission is a no-op with an empty receipt.
    /// An immediate retry after commit therefore adds no charges. If another
    /// writer reversed the admission since then, the call is a new edit.
    pub fn set_admitted_and_revise(
        &mut self,
        actor: &str,
        scope: &str,
        evidence: i64,
        admitted: bool,
    ) -> Result<Vec<SupportRevision>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut revisions = Vec::new();
        if let Some(epoch) = change_admission(&tx, actor, scope, evidence, admitted)? {
            for head in changed_heads(&tx, actor, scope, evidence)? {
                let attempt_id =
                    publish_revision(&tx, &head.branch, epoch, head.rule, head.supported)?;
                revisions.push(SupportRevision {
                    branch: head.name,
                    attempt_id,
                    supported: head.supported,
                });
            }
        }
        tx.commit()?;
        Ok(revisions)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_positive_and_negative_memberships_each_require_one_lookup() -> Result<()> {
        let mut queried = Vec::new();
        {
            let mut admitted = cached_admissions(|id| {
                queried.push(id);
                Ok(id == 7)
            });
            for id in [7, 8, 8, 7, 7, 8, 9] {
                assert_eq!(admitted(id)?, id == 7);
            }
        }
        assert_eq!(queried, vec![7, 8, 9]);
        Ok(())
    }
}
