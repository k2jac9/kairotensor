use rusqlite::{params, Connection, OptionalExtension, Result, Transaction, TransactionBehavior};

use crate::{invalid, is_admitted, view_epoch, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Accepted,
    StaleView,
    StaleBranch,
    InvalidOutput,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::StaleView => "stale_view",
            Self::StaleBranch => "stale_branch",
            Self::InvalidOutput => "invalid_output",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "stale_view" => Ok(Self::StaleView),
            "stale_branch" => Ok(Self::StaleBranch),
            "invalid_output" => Ok(Self::InvalidOutput),
            _ => Err(invalid("unknown stored disposition")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Current {
    Empty,
    NeedsReview,
    Ready(Vec<u8>),
}

/// Ordered (observation occurrence ID, exact payload) pairs captured together.
pub type CapturedInputs = Vec<(i64, Vec<u8>)>;

pub(crate) struct Branch {
    pub(crate) id: i64,
    pub(crate) head: Option<i64>,
    pub(crate) revision: i64,
}

pub(crate) fn find_branch(
    db: &Connection,
    actor: &str,
    scope: &str,
    name: &str,
) -> Result<Option<Branch>> {
    db.query_row(
        "SELECT id, head, revision FROM branches WHERE actor = ?1 AND scope = ?2 AND name = ?3",
        params![actor, scope, name],
        |r| {
            Ok(Branch {
                id: r.get(0)?,
                head: r.get(1)?,
                revision: r.get(2)?,
            })
        },
    )
    .optional()
}

pub(crate) fn ensure_branch(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    name: &str,
) -> Result<Branch> {
    tx.execute(
        "INSERT INTO branches (actor, scope, name) VALUES (?1, ?2, ?3)
         ON CONFLICT(actor, scope, name) DO NOTHING",
        params![actor, scope, name],
    )?;
    find_branch(tx, actor, scope, name)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

// The caller retains one SQLite snapshot throughout the gate and payload read.
fn current_head(db: &Connection, branch: Option<&Branch>, epoch: i64) -> Result<Current> {
    let explanation = crate::eligibility::evaluate(db, branch, epoch)?;
    match explanation.state {
        crate::EligibilityState::Empty => return Ok(Current::Empty),
        crate::EligibilityState::NeedsReview => return Ok(Current::NeedsReview),
        crate::EligibilityState::Ready => {}
    }
    let head = explanation.head_id.expect("ready gate has a head");
    #[cfg(test)]
    eligibility_snapshot_tests::before_payload(db, &explanation);
    db.query_row(
        "SELECT output FROM deliveries WHERE attempt_id = ?1 AND disposition = 'accepted'",
        [head],
        |r| r.get(0),
    )
    .map(Current::Ready)
}

pub(crate) fn record_attempt(
    tx: &Transaction<'_>,
    branch: &Branch,
    epoch: i64,
    inherited_from: Option<i64>,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO attempts (branch_id, parent, branch_revision, view_epoch, inherited_from)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![branch.id, branch.head, branch.revision, epoch, inherited_from],
    )?;
    Ok(tx.last_insert_rowid())
}

pub(crate) fn record_delivery(
    tx: &Transaction<'_>,
    attempt: i64,
    disposition: Disposition,
    output: &[u8],
    usage: i64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO deliveries (attempt_id, disposition, output, usage) VALUES (?1, ?2, ?3, ?4)",
        params![attempt, disposition.as_str(), output, usage],
    )?;
    Ok(())
}

// Both settlement paths identify redelivery by the original output, usage and
// declared validation decision. Applicability is determined only on first delivery.
pub(crate) fn existing_disposition(
    tx: &Transaction<'_>,
    attempt: i64,
    output: &[u8],
    usage: i64,
    validator: Option<&str>,
    reason: Option<&str>,
) -> Result<Option<Disposition>> {
    let prior: Option<(String, bool)> = tx.query_row(
        "SELECT disposition, (output = ?2 AND usage = ?3 AND validator IS ?4 AND reason IS ?5)
         FROM deliveries WHERE attempt_id = ?1",
        params![attempt, output, usage, validator, reason],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    match prior {
        None => Ok(None),
        Some((name, true)) => Disposition::from_db(&name).map(Some),
        Some((_, false)) => Err(invalid("conflicting final delivery for an already settled attempt")),
    }
}

/// The caller has already checked publication conditions under this writer lock.
pub(crate) fn accept(
    tx: &Transaction<'_>,
    branch_id: i64,
    attempt: i64,
    output: &[u8],
    usage: i64,
) -> Result<()> {
    record_delivery(tx, attempt, Disposition::Accepted, output, usage)?;
    tx.execute(
        "UPDATE branches SET head = ?1, revision = revision + 1 WHERE id = ?2",
        params![attempt, branch_id],
    )?;
    Ok(())
}

fn record_exact_inputs(
    tx: &Transaction<'_>,
    attempt: i64,
    actor: &str,
    scope: &str,
    ordered_inputs: &[i64],
) -> Result<()> {
    let mut insert = tx.prepare(
        "INSERT INTO attempt_inputs (attempt_id, position, evidence_id) VALUES (?1, ?2, ?3)",
    )?;
    for (index, &evidence) in ordered_inputs.iter().enumerate() {
        if !is_admitted(tx, actor, scope, evidence)? {
            return Err(invalid("input evidence is unavailable in this view"));
        }
        insert.execute(params![attempt, index as i64, evidence])?;
    }
    Ok(())
}

pub(crate) fn capture_candidates(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    branch: &str,
    candidates: &[i64],
) -> Result<(i64, CapturedInputs)> {
    let branch = ensure_branch(tx, actor, scope, branch)?;
    let attempt = record_attempt(tx, &branch, view_epoch(tx, actor, scope)?, None)?;
    let mut inputs = CapturedInputs::new();
    {
        let mut candidate = tx.prepare(
            "INSERT INTO attempt_candidates (attempt_id, position, requested_id)
             VALUES (?1, ?2, ?3)",
        )?;
        let mut selected = tx.prepare(
            "INSERT INTO attempt_inputs (attempt_id, position, evidence_id) VALUES (?1, ?2, ?3)",
        )?;
        let mut payload = tx.prepare("SELECT payload FROM evidence WHERE id = ?1")?;
        for (index, &id) in candidates.iter().enumerate() {
            candidate.execute(params![attempt, index as i64, id])?;
            if is_admitted(tx, actor, scope, id)? {
                selected.execute(params![attempt, inputs.len() as i64, id])?;
                inputs.push((id, payload.query_row([id], |row| row.get(0))?));
            }
        }
    }
    Ok((attempt, inputs))
}

// A separate complete-selection contract: existing candidate subset APIs keep
// their intentional filtering behavior. Callers must retain the transaction so
// any unavailable occurrence rolls back the whole provisional capture.
pub(crate) fn capture_complete_candidates(
    tx: &Transaction<'_>, actor: &str, scope: &str, branch: &str, candidates: &[i64],
) -> Result<(i64, CapturedInputs)> {
    let captured = capture_candidates(tx, actor, scope, branch, candidates)?;
    if captured.1.len() != candidates.len() {
        return Err(invalid("candidate evidence is incomplete or unavailable"));
    }
    Ok(captured)
}

// Called under the same writer lock as the dispatch record or final publication.
pub(crate) fn applicability(db: &Connection, attempt: i64) -> Result<(i64, Disposition)> {
    let (branch_id, actor, scope, captured_epoch, parent_matches) = db.query_row(
        "SELECT b.id, b.actor, b.scope, a.view_epoch,
                (a.parent IS b.head AND a.branch_revision = b.revision)
         FROM attempts a JOIN branches b ON b.id = a.branch_id WHERE a.id = ?1",
        [attempt],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
            ))
        },
    )?;
    let disposition = if captured_epoch != view_epoch(db, &actor, &scope)? {
        Disposition::StaleView
    } else if !parent_matches {
        Disposition::StaleBranch
    } else {
        Disposition::Accepted
    };
    Ok((branch_id, disposition))
}

pub(crate) fn pending_branch(db: &Connection, attempt: i64) -> Result<i64> {
    let completed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM deliveries WHERE attempt_id = ?1)",
        [attempt],
        |row| row.get(0),
    )?;
    if completed {
        return Err(invalid("cannot dispatch a settled attempt"));
    }
    let (branch, disposition) = applicability(db, attempt)?;
    if disposition != Disposition::Accepted {
        return Err(invalid("cannot dispatch a stale attempt"));
    }
    Ok(branch)
}

impl Store {
    /// Start from explicitly supplied evidence. The existing head is captured
    /// strictly as the expected publication parent; its bytes are never inherited.
    pub fn begin_attempt(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
        ordered_inputs: &[i64],
    ) -> Result<i64> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let branch = ensure_branch(&tx, actor, scope, branch)?;
        let attempt = record_attempt(&tx, &branch, view_epoch(&tx, actor, scope)?, None)?;
        record_exact_inputs(&tx, attempt, actor, scope, ordered_inputs)?;
        tx.commit()?;
        Ok(attempt)
    }

    /// Start a fresh computation over the admitted subset of a candidate list.
    /// Selection, ordered bytes, and publication preconditions share one snapshot.
    /// Missing and unadmitted candidates produce identical omissions from the result.
    pub fn begin_from_candidates(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
        candidates: &[i64],
    ) -> Result<(i64, CapturedInputs)> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let captured = capture_candidates(&tx, actor, scope, branch, candidates)?;
        tx.commit()?;
        Ok(captured)
    }

    /// Capture eligible accepted bytes and an explicit lineage edge in one
    /// transaction. Later revocation makes this attempt ineligible to publish.
    pub fn begin_resume(
        &mut self,
        actor: &str,
        scope: &str,
        branch: &str,
        ordered_inputs: &[i64],
    ) -> Result<(i64, Vec<u8>)> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let branch =
            find_branch(&tx, actor, scope, branch)?.ok_or_else(|| invalid("no head to resume"))?;
        let epoch = view_epoch(&tx, actor, scope)?;
        let output = match current_head(&tx, Some(&branch), epoch)? {
            Current::Empty => return Err(invalid("no head to resume")),
            Current::NeedsReview => return Err(invalid("head must be rebuilt before resume")),
            Current::Ready(output) => output,
        };
        let attempt = record_attempt(&tx, &branch, epoch, branch.head)?;
        record_exact_inputs(&tx, attempt, actor, scope, ordered_inputs)?;
        tx.commit()?;
        Ok((attempt, output))
    }

    /// Return only bytes eligible at this read's snapshot boundary.
    pub fn read_current(&mut self, actor: &str, scope: &str, branch: &str) -> Result<Current> {
        let tx = self.conn.transaction()?;
        let branch = find_branch(&tx, actor, scope, branch)?;
        let result = current_head(&tx, branch.as_ref(), view_epoch(&tx, actor, scope)?)?;
        tx.commit()?;
        Ok(result)
    }

    /// Freeze the exact serialized request immediately before an adapter submits
    /// it. The attempt must still be pending and match its captured view and parent.
    /// Calling again with the same bytes only verifies an existing capture while
    /// the attempt remains eligible. Submission and transport retries are the
    /// adapter's responsibility. Settlement checks the view and parent again.
    ///
    /// The trusted caller supplies the client-visible bytes for the previously
    /// captured inputs. Serialization, model parameters and template selection
    /// must already be fixed at this boundary.
    pub fn record_dispatch(&mut self, attempt: i64, request: &[u8]) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        pending_branch(&tx, attempt)?;
        if crate::recovery::is_managed(&tx, attempt)? {
            return Err(invalid("recovery dispatch requires the single-use send claim"));
        }
        if crate::model_reuse::is_opted_in(&tx, attempt)? {
            return Err(invalid("use resolve_model_attempt for an opted-in model attempt"));
        }
        if crate::token_budget::has_decision(&tx, attempt)? {
            return Err(invalid("budget decision already made; reconcile the original dispatch"));
        }
        let identical: Option<bool> = tx
            .query_row(
                "SELECT request = ?2 FROM dispatches WHERE attempt_id = ?1",
                params![attempt, request],
                |row| row.get(0),
            )
            .optional()?;
        match identical {
            Some(false) => return Err(invalid("conflicting request for an existing dispatch")),
            Some(true) => {}
            None => {
                tx.execute(
                    "INSERT INTO dispatches(attempt_id, request) VALUES (?1, ?2)",
                    params![attempt, request],
                )?;
            }
        }
        tx.commit()
    }

    /// Atomically record final output, cost, and a terminal disposition.
    /// Identical deliveries return the original disposition, even if the view
    /// has since changed. A conflicting retry fails without charging twice.
    pub fn settle(&mut self, attempt: i64, output: &[u8], usage: i64) -> Result<Disposition> {
        if usage < 0 {
            return Err(invalid("final usage must be nonnegative"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if crate::report_verdict::is_bound(&tx, attempt)? {
            return Err(invalid("deterministic attempts require commit_report_verdict"));
        }
        if let Some(prior) = existing_disposition(&tx, attempt, output, usage, None, None)? {
            return Ok(prior);
        }

        if crate::model_reuse::unresolved(&tx, attempt)? {
            return Err(invalid("resolve the model attempt before settling a delivery"));
        }
        let (branch_id, disposition) = applicability(&tx, attempt)?;
        if disposition == Disposition::Accepted {
            accept(&tx, branch_id, attempt, output, usage)?;
        } else {
            record_delivery(&tx, attempt, disposition, output, usage)?;
        }
        tx.commit()?;
        Ok(disposition)
    }

    /// Record a dispatched output rejected by the caller's versioned validator.
    /// The original bytes, cost, validator identity and reason share one terminal
    /// delivery. The branch head stays in place. Its applicability at settlement
    /// is recorded separately; a stale view takes precedence over a stale branch.
    ///
    /// The caller provides a stable validator/version and reason. Redelivery must
    /// repeat that decision, those exact strings, the bytes, and the reported usage.
    pub fn reject_invalid(
        &mut self,
        attempt: i64,
        output: &[u8],
        usage: i64,
        validator: &str,
        reason: &str,
    ) -> Result<Disposition> {
        if usage < 0 {
            return Err(invalid("final usage must be nonnegative"));
        }
        if validator.trim().is_empty() || reason.trim().is_empty() {
            return Err(invalid("validation rejection requires a validator and reason"));
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(prior) = existing_disposition(&tx, attempt, output, usage, Some(validator), Some(reason))? {
            return Ok(prior);
        }
        let (_, state) = applicability(&tx, attempt)?;
        let at_delivery = if state == Disposition::Accepted { "current" } else { state.as_str() };
        let recorded = tx.execute(
            "INSERT INTO deliveries (attempt_id, disposition, output, usage, validator, reason, applicability)
             SELECT attempt_id, 'invalid_output', ?2, ?3, ?4, ?5, ?6
             FROM dispatches WHERE attempt_id = ?1",
            params![attempt, output, usage, validator, reason, at_delivery],
        )?;
        if recorded != 1 {
            return Err(invalid("validation rejection requires a recorded dispatch"));
        }
        tx.commit()?;
        Ok(Disposition::InvalidOutput)
    }

    /// Total known final cost across every accepted and rejected delivery.
    pub fn charged_usage(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT coalesce(sum(usage), 0) FROM deliveries", [], |r| {
                r.get(0)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdrawal_blocks_pending_and_inherited_work_but_allows_a_clean_rebuild() -> Result<()> {
        let mut store = Store::in_memory()?;
        let evidence = store.observe("inventory@1", b"1")?;
        store.set_admitted("a", "lab", evidence, true)?;
        let original = store.begin_attempt("a", "lab", "main", &[evidence])?;
        assert_eq!(store.settle(original, b"one", 5)?, Disposition::Accepted);
        let (resume, old_output) = store.begin_resume("a", "lab", "main", &[])?;
        assert_eq!(old_output.as_slice(), b"one");
        assert_eq!(store.read_current("a", "lab", "main")?, Current::Ready(old_output));
        assert_eq!(store.read_current("b", "lab", "main")?, Current::Empty);
        assert_eq!(store.read_current("a", "other", "main")?, Current::Empty);

        store.set_admitted("a", "lab", evidence, false)?;
        assert_eq!(store.read_current("a", "lab", "main")?, Current::NeedsReview);
        assert!(store.begin_resume("a", "lab", "main", &[]).is_err());
        assert_eq!(store.settle(resume, b"obsolete", 3)?, Disposition::StaleView);
        assert_eq!(store.settle(original, b"one", 5)?, Disposition::Accepted);
        let fresh = store.begin_attempt("a", "lab", "main", &[])?;
        assert_eq!(store.settle(fresh, b"zero", 2)?, Disposition::Accepted);
        assert_eq!(store.read_current("a", "lab", "main")?, Current::Ready(b"zero".to_vec()));
        let inherited = |attempt: i64| {
            store.conn.query_row(
                "SELECT inherited_from FROM attempts WHERE id = ?1",
                [attempt],
                |r| r.get::<_, Option<i64>>(0),
            )
        };
        assert_eq!(inherited(resume)?, Some(original));
        assert_eq!(inherited(fresh)?, None);
        assert_eq!(store.charged_usage()?, 10);
        Ok(())
    }

    #[test]
    fn conflicting_deliveries_fail_and_identical_retries_are_idempotent() -> Result<()> {
        let mut store = Store::in_memory()?;
        let first = store.begin_attempt("a", "s", "main", &[])?;
        let stale = store.begin_attempt("a", "s", "main", &[])?;
        assert_eq!(store.settle(first, b"winner", 9)?, Disposition::Accepted);
        assert_eq!(store.settle(stale, b"later", 4)?, Disposition::StaleBranch);
        assert_eq!(store.settle(stale, b"later", 4)?, Disposition::StaleBranch);
        assert!(store.settle(stale, b"changed", 4).is_err());
        assert!(store.settle(stale, b"later", 5).is_err());
        assert!(store.settle(999, b"orphan", 3).is_err());
        assert_eq!(store.charged_usage()?, 13);
        assert_eq!(store.read_current("a", "s", "main")?, Current::Ready(b"winner".to_vec()));
        Ok(())
    }

    #[test]
    fn capture_preserves_order_and_duplicates_and_rolls_back_denied_inputs() -> Result<()> {
        let mut store = Store::in_memory()?;
        let first = store.observe("one", b"same")?;
        let second = store.observe("two", b"same")?;
        store.set_admitted("a", "s", first, true)?;
        assert!(store.begin_attempt("a", "s", "main", &[first, second]).is_err());
        let count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM attempts", [], |r| r.get(0))?;
        assert_eq!(count, 0);
        store.set_admitted("a", "s", second, true)?;
        let ordered = [second, first, second];
        let attempt = store.begin_attempt("a", "s", "main", &ordered)?;
        let mut stmt = store.conn.prepare(
            "SELECT evidence_id FROM attempt_inputs WHERE attempt_id = ?1 ORDER BY position",
        )?;
        let stored = stmt
            .query_map([attempt], |r| r.get::<_, i64>(0))?
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(stored.as_slice(), &ordered[..]);
        Ok(())
    }

    #[test]
    fn scoped_epoch_catches_new_members_of_an_empty_result() -> Result<()> {
        let mut store = Store::in_memory()?;
        let evidence = store.observe("source", b"new")?;
        let first = store.begin_attempt("a", "s", "main", &[])?;
        store.set_admitted("b", "s", evidence, true)?;
        store.set_admitted("a", "other", evidence, true)?;
        assert_eq!(store.settle(first, b"empty", 1)?, Disposition::Accepted);
        let second = store.begin_attempt("a", "s", "main", &[])?;
        store.set_admitted("a", "s", evidence, true)?;
        assert_eq!(store.settle(second, b"empty", 1)?, Disposition::StaleView);
        assert_eq!(store.read_current("a", "s", "main")?, Current::NeedsReview);
        Ok(())
    }

    #[test]
    fn stale_membership_and_accounting_survive_reopen() -> Result<()> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kairotensor-membership-{}-{stamp}.sqlite",
            std::process::id()
        ));
        let mut worker = Store::open(&path)?;
        let mut editor = Store::open(&path)?;
        let old = worker.observe("warehouse", b"4")?;
        let new = worker.observe("shop", b"8")?;
        editor.set_admitted("a", "s", old, true)?;
        let candidates = [old, 999, new, old];
        let (attempt, captured) =
            worker.begin_from_candidates("a", "s", "main", &candidates)?;
        assert_eq!(captured, vec![(old, b"4".to_vec()), (old, b"4".to_vec())]);
        {
            let mut query = worker.conn.prepare(
                "SELECT requested_id FROM attempt_candidates
                 WHERE attempt_id = ?1 ORDER BY position",
            )?;
            let stored = query
                .query_map([attempt], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>>>()?;
            assert_eq!(stored.as_slice(), &candidates[..]);
        }

        editor.set_admitted("a", "s", new, true)?;
        assert_eq!(worker.settle(attempt, b"8", 2)?, Disposition::StaleView);
        drop(worker);
        let mut restarted = Store::open(&path)?;
        assert_eq!(restarted.settle(attempt, b"8", 2)?, Disposition::StaleView);
        assert_eq!(restarted.charged_usage()?, 2);
        let (rebuilt, inputs) =
            restarted.begin_from_candidates("a", "s", "main", &candidates)?;
        assert_eq!(
            inputs,
            vec![
                (old, b"4".to_vec()),
                (new, b"8".to_vec()),
                (old, b"4".to_vec())
            ]
        );
        assert_eq!(restarted.settle(rebuilt, b"16", 3)?, Disposition::Accepted);
        assert_eq!(editor.charged_usage()?, 5);
        assert_eq!(
            editor.read_current("a", "s", "main")?,
            Current::Ready(b"16".to_vec())
        );
        drop(editor);
        drop(restarted);
        std::fs::remove_file(path).unwrap();
        Ok(())
    }
}

#[cfg(test)]
mod eligibility_snapshot_tests {
    use super::*;
    use crate::{CurrentExplanation, EligibilityState};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    };
    use std::time::Duration;

    type PayloadBoundary = Box<dyn FnOnce(&Connection, &CurrentExplanation)>;
    thread_local! {
        static PAYLOAD_BOUNDARY: RefCell<Option<PayloadBoundary>> = RefCell::new(None);
    }

    pub(super) fn before_payload(db: &Connection, explanation: &CurrentExplanation) {
        PAYLOAD_BOUNDARY.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook(db, explanation);
            }
        });
    }

    struct Database(PathBuf);
    impl Database {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "kairotensor-snapshot-{}-{}.sqlite",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    // Channel barriers have a timeout only to diagnose missing hooks/writer failures;
    // scheduling and the concurrent commit never depend on elapsed time or sleeps.
    fn concurrent_read(replace: bool) -> Result<()> {
        let file = Database::new();
        let mut reader = Store::open(&file.0)?;
        reader.conn.pragma_update(None, "journal_mode", "WAL")?;
        let id = reader.observe_and_admit_many("a", "s", &[("source", b"input")])?[0];
        let first = reader.begin_attempt("a", "s", "b", &[id])?;
        reader.settle(first, b"old snapshot output", 1)?;
        let mut writer = Store::open(&file.0)?;
        let (gate_tx, gate_rx) = mpsc::channel();
        let (commit_tx, commit_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || -> Result<Option<i64>> {
            gate_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("read reached gate barrier");
            let replacement = if replace {
                let next = writer.begin_attempt("a", "s", "b", &[id])?;
                writer.settle(next, b"new snapshot output", 2)?;
                Some(next)
            } else {
                writer.set_admitted("a", "s", id, false)?;
                None
            };
            commit_tx.send(()).unwrap();
            Ok(replacement)
        });
        let captured = Rc::new(RefCell::new(None));
        let captured_at_boundary = captured.clone();
        PAYLOAD_BOUNDARY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |db, explanation| {
                gate_tx.send(()).unwrap();
                commit_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("writer committed at barrier");
                assert!(
                    !db.is_autocommit(),
                    "payload boundary must retain an active transaction"
                );
                assert_eq!(
                    view_epoch(db, "a", "s").unwrap(),
                    explanation.current_epoch,
                    "payload boundary must retain the gate scope epoch"
                );
                let branch = find_branch(db, "a", "s", "b").unwrap().unwrap();
                assert_eq!(
                    branch.head, explanation.head_id,
                    "payload boundary must retain the gate head"
                );
                assert_eq!(
                    Some(branch.revision),
                    explanation.branch_revision,
                    "payload boundary must retain the gate branch revision"
                );
                *captured_at_boundary.borrow_mut() = Some(explanation.clone());
            }))
        });
        let result = reader.read_current("a", "s", "b")?;
        let replacement = worker.join().unwrap()?;
        let explanation = captured
            .borrow()
            .clone()
            .expect("actual read payload boundary was reached");
        assert_eq!(explanation.state, EligibilityState::Ready);
        assert_eq!(explanation.head_id, Some(first));
        assert_eq!(explanation.branch_revision, Some(1));
        assert_eq!(explanation.current_epoch, 1);
        assert_eq!(
            result,
            Current::Ready(b"old snapshot output".to_vec()),
            "gate metadata and payload must come from the same snapshot"
        );
        if let Some(replacement) = replacement {
            assert_eq!(
                reader.explain_current("a", "s", "b")?.head_id,
                Some(replacement)
            );
            assert_eq!(
                reader.explain_current("a", "s", "b")?.branch_revision,
                Some(2)
            );
            assert_eq!(
                reader.read_current("a", "s", "b")?,
                Current::Ready(b"new snapshot output".to_vec())
            );
        } else {
            assert_eq!(reader.explain_current("a", "s", "b")?.current_epoch, 2);
            assert_eq!(reader.read_current("a", "s", "b")?, Current::NeedsReview);
            assert!(reader.begin_resume("a", "s", "b", &[]).is_err());
        }
        Ok(())
    }

    #[test]
    fn revocation_snapshot_never_exposes_payload() -> Result<()> {
        concurrent_read(false)
    }

    #[test]
    fn head_replacement_keeps_metadata_and_output_in_one_snapshot() -> Result<()> {
        concurrent_read(true)
    }
}
