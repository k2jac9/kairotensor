//! One retained read snapshot, with authority and admission before payloads.
use super::*;
use crate::{attempts::Branch, eligibility, is_admitted, view_epoch, EligibilityState, Store};
use rusqlite::{Connection, OptionalExtension};

// Bound allocation even for malicious row counts and text with JSON escapes.
// Conservative encoding estimates can refuse a selection before final sealing.
struct Limits {
    remaining: usize,
}
impl Limits {
    fn new() -> Self {
        Self {
            remaining: MAX_BUNDLE_BYTES,
        }
    }
    fn take(&mut self, bytes: usize) -> Result<(), Error> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or(Error::LimitExceeded)?;
        Ok(())
    }
}
struct Selection {
    selected: SelectedAttempt,
    parent: Option<i64>,
    inherited: Option<i64>,
    reuse: Option<i64>,
    predecessor: Option<i64>,
}
impl Selection {
    fn dependencies(&self) -> impl Iterator<Item = i64> + '_ {
        [self.inherited, self.reuse, self.predecessor]
            .into_iter()
            .flatten()
    }
}

// All expressions are fixed internal SQL, never caller text. A probe reads only
// SQLite type/byte length; CAST avoids SQLite text-length's embedded-NUL rules.
fn field(
    db: &Connection,
    from: &str,
    column: &str,
    id: i64,
    kind: &str,
    expansion: usize,
    limits: &mut Limits,
) -> Result<Option<Vec<u8>>, Error> {
    let probe = format!("SELECT typeof({column}), length(CAST({column} AS BLOB)) FROM {from}");
    let shape: Option<(String, Option<i64>)> = db
        .query_row(&probe, [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((storage, length)) = shape else {
        return Ok(None);
    };
    if storage == "null" {
        return Ok(None);
    }
    if storage != kind {
        return Err(Error::Store(crate::invalid(
            "invalid export field storage class",
        )));
    }
    let length = usize::try_from(length.ok_or(Error::InconsistentLedger)?)
        .map_err(|_| Error::LimitExceeded)?;
    if length > MAX_PAYLOAD_BYTES {
        return Err(Error::LimitExceeded);
    }
    limits.take(
        length
            .checked_mul(expansion)
            .and_then(|n| n.checked_add(192))
            .ok_or(Error::LimitExceeded)?,
    )?;
    let sql = format!("SELECT CAST({column} AS BLOB) FROM {from}");
    Ok(Some(db.query_row(&sql, [id], |r| r.get(0))?))
}
fn text_field(
    db: &Connection,
    from: &str,
    column: &str,
    id: i64,
    limits: &mut Limits,
) -> Result<Option<String>, Error> {
    field(db, from, column, id, "text", 6, limits)?
        .map(|b| String::from_utf8(b).map_err(|_| Error::InvalidEncoding))
        .transpose()
}
fn envelope(
    db: &Connection,
    from: &str,
    column: &str,
    id: i64,
    kind: &str,
    limits: &mut Limits,
) -> Result<Option<ByteEnvelope>, Error> {
    field(db, from, column, id, kind, 2, limits)?
        .map(|b| ByteEnvelope::from_bytes(&b))
        .transpose()
}
fn required<T>(value: Option<T>) -> Result<T, Error> {
    value.ok_or(Error::InconsistentLedger)
}
fn pairs(
    db: &Connection,
    sql: &str,
    id: i64,
    limits: &mut Limits,
) -> Result<Vec<(i64, i64)>, Error> {
    let mut statement = db.prepare(sql)?;
    let mut rows = statement.query([id])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        limits.take(128)?;
        result.push((row.get(0)?, row.get(1)?));
    }
    Ok(result)
}
fn select(db: &Connection, id: i64, limits: &mut Limits) -> Result<Option<Selection>, Error> {
    let data: Option<(i64,Option<i64>,i64,i64,Option<i64>,Option<i64>,i64)> = db.query_row(
        "SELECT a.branch_id,a.parent,a.branch_revision,a.view_epoch,a.inherited_from,b.head,b.revision FROM attempts a JOIN branches b ON b.id=a.branch_id WHERE a.id=?1",[id],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    let Some((branch, parent, captured_revision, captured_epoch, inherited, head, revision)) = data
    else {
        return Ok(None);
    };
    limits.take(4096)?;
    let actor = required(text_field(
        db,
        "branches WHERE id=?1",
        "actor",
        branch,
        limits,
    )?)?;
    let scope = required(text_field(
        db,
        "branches WHERE id=?1",
        "scope",
        branch,
        limits,
    )?)?;
    let current_epoch = view_epoch(db, &actor, &scope)?;
    let inputs = pairs(
        db,
        "SELECT position,evidence_id FROM attempt_inputs WHERE attempt_id=?1 ORDER BY position",
        id,
        limits,
    )?
    .into_iter()
    .map(|(position, occurrence_id)| SelectedInput {
        position,
        occurrence_id,
    })
    .collect();
    let reuse = db
        .query_row(
            "SELECT origin_attempt FROM model_reuses WHERE attempt_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let predecessor = db.query_row("SELECT p.attempt_id FROM recovery_attempts a JOIN recovery_attempts p ON p.recovery_id=a.recovery_id AND p.ordinal=a.ordinal-1 WHERE a.attempt_id=?1",[id],|r|r.get(0)).optional()?;
    let ordinal: Option<i64> = db
        .query_row(
            "SELECT ordinal FROM recovery_attempts WHERE attempt_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    if ordinal.is_some_and(|n| n > 0) && predecessor.is_none() {
        return Ok(None);
    }
    Ok(Some(Selection {
        selected: SelectedAttempt {
            attempt: id,
            actor,
            scope,
            branch,
            head,
            revision,
            current_epoch,
            captured_revision,
            captured_epoch,
            inputs,
        },
        parent,
        inherited,
        reuse,
        predecessor,
    }))
}
fn visit(
    db: &Connection,
    id: i64,
    limits: &mut Limits,
    all: &mut Vec<Selection>,
    path: &mut Vec<i64>,
) -> Result<bool, Error> {
    if path.contains(&id) {
        return Ok(false);
    }
    if all.iter().any(|s| s.selected.attempt == id) {
        return Ok(true);
    }
    if all.len() >= MAX_ATTEMPTS {
        return Err(Error::LimitExceeded);
    }
    let Some(s) = select(db, id, limits)? else {
        return Ok(false);
    };
    let deps: Vec<_> = s.dependencies().collect();
    all.push(s);
    path.push(id);
    for dep in deps {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM deliveries WHERE attempt_id=?1)",
            [dep],
            |r| r.get(0),
        )?;
        if !exists || !visit(db, dep, limits, all, path)? {
            return Ok(false);
        }
    }
    path.pop();
    Ok(true)
}
impl Store {
    /// Export authorized bytes from one snapshot. An exported file never grants
    /// access, dispatch or import authority. No archive fallback is performed.
    pub fn export_decision_bundle(
        &mut self,
        request: &BundleRequest,
        authorizer: &impl DiagnosticAuthorizer,
    ) -> Result<BundleExport, Error> {
        let tx = self.conn.transaction()?;
        let result = export(&tx, request, authorizer);
        // Also commit read-only redactions; an SQL error is never a successful export.
        tx.commit()?;
        match result {
            Ok(Some(bundle)) => Ok(BundleExport::Complete(bundle)),
            Ok(None) | Err(Error::LimitExceeded) => Ok(BundleExport::Redacted(RedactedBundleV1 {
                requested_attempt: request.attempt,
                reason: RedactionReason::Unavailable,
            })),
            Err(error) => Err(error),
        }
    }
}
fn export(
    db: &Connection,
    request: &BundleRequest,
    authorizer: &impl DiagnosticAuthorizer,
) -> Result<Option<DecisionBundleV1>, Error> {
    let mut limits = Limits::new();
    limits.take(
        request
            .actor
            .len()
            .checked_add(request.scope.len())
            .and_then(|n| n.checked_mul(6))
            .ok_or(Error::LimitExceeded)?,
    )?;
    let mut selection = Vec::new();
    if !visit(
        db,
        request.attempt,
        &mut limits,
        &mut selection,
        &mut Vec::new(),
    )? {
        return Ok(None);
    }
    let root = &selection[0].selected;
    if root.actor != request.actor || root.scope != request.scope {
        return Ok(None);
    }
    if request.purpose == BundlePurpose::DiagnosticReplay {
        let exact = DiagnosticReadRequest {
            actor: request.actor.clone(),
            scope: request.scope.clone(),
            root_attempt: request.attempt,
            purpose: request.purpose,
            attempts: selection.iter().map(|s| s.selected.clone()).collect(),
        };
        if !authorizer.authorize(&exact)? {
            return Ok(None);
        }
    } else {
        for s in &selection {
            let a = &s.selected;
            if a.actor != request.actor || a.scope != request.scope || a.head != Some(a.attempt) {
                return Ok(None);
            }
            let branch = Branch {
                id: a.branch,
                head: a.head,
                revision: a.revision,
            };
            if eligibility::evaluate(db, Some(&branch), a.current_epoch)?.state
                != EligibilityState::Ready
            {
                return Ok(None);
            }
        }
    }
    for s in &selection {
        for input in &s.selected.inputs {
            if !is_admitted(
                db,
                &s.selected.actor,
                &s.selected.scope,
                input.occurrence_id,
            )? {
                return Ok(None);
            }
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM evidence WHERE id=?1)",
                [input.occurrence_id],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
        }
    }
    #[cfg(test)]
    snapshot_tests::before_payload(db, &selection);
    let mut bundle = DecisionBundleV1 {
        schema_version: 1,
        root_attempt: request.attempt,
        actor: request.actor.clone(),
        scope: request.scope.clone(),
        export_purpose: match request.purpose {
            BundlePurpose::CurrentRead => "current_read",
            BundlePurpose::DiagnosticReplay => "diagnostic_replay",
        }
        .into(),
        snapshot: BundleSnapshot {
            root_branch: root.branch,
            head: root.head,
            revision: root.revision,
            current_epoch: root.current_epoch,
        },
        attempts: Vec::new(),
        evidence: Vec::new(),
        integrity: IntegrityManifest {
            version: "decision-bundle-integrity/1".into(),
            sha256: String::new(),
        },
    };
    for s in &selection {
        let a = &s.selected;
        for input in &a.inputs {
            let scope = AuthorizedScope {
                actor: a.actor.clone(),
                scope: a.scope.clone(),
            };
            if let Some(e) = bundle
                .evidence
                .iter_mut()
                .find(|e| e.occurrence_id == input.occurrence_id)
            {
                if !e.authorized_scopes.contains(&scope) {
                    limits.take(scope.actor.len() * 6 + scope.scope.len() * 6 + 128)?;
                    e.authorized_scopes.push(scope);
                }
            } else {
                limits.take(scope.actor.len() * 6 + scope.scope.len() * 6 + 256)?;
                bundle.evidence.push(EvidenceRecord {
                    occurrence_id: input.occurrence_id,
                    source: required(envelope(
                        db,
                        "evidence WHERE id=?1",
                        "source",
                        input.occurrence_id,
                        "text",
                        &mut limits,
                    )?)?,
                    payload: required(envelope(
                        db,
                        "evidence WHERE id=?1",
                        "payload",
                        input.occurrence_id,
                        "blob",
                        &mut limits,
                    )?)?,
                    authorized_scopes: vec![scope],
                });
            }
        }
        bundle.attempts.push(load_attempt(db, s, &mut limits)?);
    }
    for attempt in &mut bundle.attempts {
        bind_incident(attempt, &bundle.evidence, &mut limits)?;
    }
    bundle.seal()?;
    Ok(Some(bundle))
}

fn load_attempt(
    db: &Connection,
    s: &Selection,
    limits: &mut Limits,
) -> Result<AttemptRecord, Error> {
    let a = &s.selected;
    let id = a.attempt;
    let delivery = if let Some(raw) = envelope(
        db,
        "deliveries WHERE attempt_id=?1",
        "output",
        id,
        "blob",
        limits,
    )? {
        let disposition = required(text_field(
            db,
            "deliveries WHERE attempt_id=?1",
            "disposition",
            id,
            limits,
        )?)?;
        let disposition = match disposition.as_str() {
            "accepted" => RecordedDisposition::Accepted,
            "stale_view" => RecordedDisposition::StaleView,
            "stale_branch" => RecordedDisposition::StaleBranch,
            "invalid_output" => RecordedDisposition::InvalidOutput,
            _ => return Err(Error::InconsistentLedger),
        };
        let usage = db.query_row(
            "SELECT usage FROM deliveries WHERE attempt_id=?1",
            [id],
            |r| r.get(0),
        )?;
        let rejection = if disposition == RecordedDisposition::InvalidOutput {
            Some(RejectionMetadata {
                validator: required(text_field(
                    db,
                    "deliveries WHERE attempt_id=?1",
                    "validator",
                    id,
                    limits,
                )?)?,
                reason: required(text_field(
                    db,
                    "deliveries WHERE attempt_id=?1",
                    "reason",
                    id,
                    limits,
                )?)?,
                applicability: required(text_field(
                    db,
                    "deliveries WHERE attempt_id=?1",
                    "applicability",
                    id,
                    limits,
                )?)?,
            })
        } else {
            None
        };
        DeliveryState::Recorded {
            raw,
            disposition,
            usage: Some(usage),
            rejection,
        }
    } else {
        DeliveryState::Unresolved
    };
    let deterministic = if let Some(version) = text_field(
        db,
        "deterministic_verdicts WHERE attempt_id=?1",
        "computation",
        id,
        limits,
    )? {
        Some(DeterministicContract {
            version,
            output: required(envelope(
                db,
                "deterministic_verdicts WHERE attempt_id=?1",
                "output",
                id,
                "blob",
                limits,
            )?)?,
        })
    } else {
        None
    };
    let support = load_support(db, id, limits)?;
    let recovery_key: Option<(i64, i64)> = db
        .query_row(
            "SELECT recovery_id,ordinal FROM recovery_attempts WHERE attempt_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let recovery = if let Some((recovery_id, ordinal)) = recovery_key {
        Some(RecoveryRecord {
            recovery_id,
            ordinal,
            state: required(text_field(
                db,
                "recovery_attempts WHERE attempt_id=?1",
                "state",
                id,
                limits,
            )?)?,
            request: envelope(
                db,
                "recovery_attempts WHERE attempt_id=?1",
                "request",
                id,
                "blob",
                limits,
            )?,
            predecessor_id: s.predecessor,
        })
    } else {
        None
    };
    let timing_configuration = if let Some((key, _)) = recovery_key {
        envelope(
            db,
            "recovery_timing_bindings WHERE recovery_id=?1",
            "configuration",
            key,
            "text",
            limits,
        )?
    } else {
        None
    };
    let immutable_runtime = if let Some((key, _)) = recovery_key {
        envelope(
            db,
            "recovery_timing_bindings WHERE recovery_id=?1",
            "runtime_json",
            key,
            "text",
            limits,
        )?
    } else {
        None
    };
    let reservation:Option<(i64,i64,i64,bool)>=db.query_row("SELECT budget_id,input_tokens,output_cap,granted FROM token_reservations WHERE attempt_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let charge = match &delivery {
        DeliveryState::Recorded { usage, .. } => required(*usage)?,
        DeliveryState::Unresolved => 0,
    };
    let hold_bound = match reservation {
        Some((_, input, cap, true))
            if matches!(delivery, DeliveryState::Unresolved)
                && !recovery.as_ref().is_some_and(|r| r.state == "cancelled") =>
        {
            input.checked_add(cap).ok_or(Error::InconsistentLedger)?
        }
        _ => 0,
    };
    let validator = if let DeliveryState::Recorded {
        rejection: Some(r), ..
    } = &delivery
    {
        Some(ValidatorBinding {
            version: r.validator.clone(),
            provenance: ValidatorProvenance::RecordedRejection,
        })
    } else {
        deterministic.as_ref().map(|d| ValidatorBinding {
            version: d.version.clone(),
            provenance: ValidatorProvenance::DeterministicBinding,
        })
    };
    let role = if deterministic
        .as_ref()
        .is_some_and(|d| d.version == "report-verdict/v1")
    {
        InputRole::Report
    } else if support.is_some() {
        InputRole::Evidence
    } else {
        InputRole::Unknown
    };
    Ok(AttemptRecord{actor:a.actor.clone(),scope:a.scope.clone(),id,branch:a.branch,captured_epoch:a.captured_epoch,captured_revision:a.captured_revision,parent:s.parent,inherited_from:s.inherited,
        candidates:pairs(db,"SELECT position,requested_id FROM attempt_candidates WHERE attempt_id=?1 ORDER BY position",id,limits)?.into_iter().map(|(position,evidence_id)|CandidateRecord{position,evidence_id}).collect(),
        inputs:a.inputs.iter().map(|i|InputRecord{position:i.position,evidence_id:i.occurrence_id,role}).collect(),
        dispatch_request:envelope(db,"dispatches WHERE attempt_id=?1","request",id,"blob",limits)?,delivery,validator,immutable_runtime,immutable_manifest:None,timing_configuration,
        accounting:AttemptAccounting{unit:if role!=InputRole::Unknown{AccountingUnit::LogicalSupport}else{AccountingUnit::Unknown},quote:reservation.map(|(_,input_tokens,output_cap,_)|ReservationQuote{input_tokens,output_cap}),granted:reservation.map(|r|r.3),budget_reference:reservation.map(|r|r.0),charge,hold_bound},
        reuse_origin:s.reuse,model_contract:text_field(db,"model_attempts WHERE attempt_id=?1","contract",id,limits)?,deterministic,support,recovery})
}
fn load_support(
    db: &Connection,
    id: i64,
    limits: &mut Limits,
) -> Result<Option<SupportContract>, Error> {
    let bound: Option<(i64, bool)> = db
        .query_row(
            "SELECT rule_id,verdict FROM support_bindings WHERE attempt_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((rule_id, verdict)) = bound else {
        return Ok(None);
    };
    let clauses = pairs(
        db,
        "SELECT id,id FROM support_clauses WHERE rule_id=?1 ORDER BY id",
        rule_id,
        limits,
    )?;
    let mut result = SupportContract {
        version: "support/v1".into(),
        rule_id,
        verdict,
        clauses: Vec::new(),
    };
    for (clause_id, _) in clauses {
        let premises=pairs(db,"SELECT requested_id,requested_id FROM support_premises WHERE clause_id=?1 ORDER BY requested_id",clause_id,limits)?;
        let mut clause = SupportClause {
            clause_id,
            premises: Vec::new(),
        };
        for (evidence_id, _) in premises {
            // Composite key bound in SQL literals (integers from SQLite, not text).
            let from = format!("support_premises WHERE clause_id={clause_id} AND requested_id=?1");
            clause.premises.push(SupportPremise {
                evidence_id,
                actor: required(text_field(db, &from, "actor", evidence_id, limits)?)?,
                scope: required(text_field(db, &from, "scope", evidence_id, limits)?)?,
            });
        }
        result.clauses.push(clause);
    }
    Ok(Some(result))
}

// Count borrowed values once before repeated source metadata is cloned. A
// context is individually bounded, but repeating its case ID can otherwise
// produce a much larger candidate manifest even when no fingerprint matches.
fn manifest_bound(
    context: &crate::incident_protocol::IncidentContextV1,
    runtime: &crate::RecoveryRuntime,
) -> Result<usize, Error> {
    fn encoded<T: Serialize + ?Sized>(value: &T) -> Result<usize, Error> {
        let mut counter = super::CountWriter { size: 0 };
        serde_json::to_writer(&mut counter, value).map_err(|_| Error::LimitExceeded)?;
        Ok(counter.size)
    }
    let mut size = 1024usize;
    let mut add = |amount: usize| -> Result<(), Error> {
        size = size
            .checked_add(amount)
            .filter(|n| *n <= MAX_PAYLOAD_BYTES)
            .ok_or(Error::LimitExceeded)?;
        Ok(())
    };
    add(encoded(runtime)?)?;
    for text in [
        &context.workload_version,
        &context.freeze_sha256,
        &context.prompt_sha256,
    ] {
        add(encoded(text)?)?;
    }
    let case = encoded(&context.case_id)?;
    let workload = encoded(&context.workload_version)?;
    for external in &context.evidence_ids {
        add(256)?; // keys, digest, integer occurrence ID, and punctuation
        add(case)?;
        add(workload)?;
        add(encoded(external)?)?;
    }
    Ok(size)
}

// B does not persist its manifest separately. Reconstruct its exact canonical
// bytes from retained inputs and bindings; only a matching fingerprint allows
// the declared contract, roles or fixture accounting unit to be attached.
fn bind_incident(
    a: &mut AttemptRecord,
    evidence: &[EvidenceRecord],
    limits: &mut Limits,
) -> Result<(), Error> {
    let (Some(runtime), Some(timing), Some(first)) = (
        &a.immutable_runtime,
        &a.timing_configuration,
        a.inputs.first(),
    ) else {
        return Ok(());
    };
    let Ok(runtime) = super::decode_strict::<crate::RecoveryRuntime>(&runtime.decode()?) else {
        return Ok(());
    };
    let Ok(timing) = super::decode_strict::<crate::RecoveryTiming>(&timing.decode()?) else {
        return Ok(());
    };
    let Some(context) = evidence
        .iter()
        .find(|e| e.occurrence_id == first.evidence_id)
    else {
        return Ok(());
    };
    let Ok(context_value) =
        crate::incident_protocol::IncidentContextV1::from_json(&context.payload.decode()?)
    else {
        return Ok(());
    };
    if a.inputs.len() != context_value.evidence_ids.len() + 1 || a.inputs.len() > 1024 {
        return Ok(());
    }
    let bound = manifest_bound(&context_value, &runtime)?;
    limits.take(
        bound
            .checked_mul(2)
            .and_then(|n| n.checked_add(192))
            .ok_or(Error::LimitExceeded)?,
    )?;
    let mut runtime_value = serde_json::to_value(&runtime).map_err(|_| Error::InvalidEncoding)?;
    runtime_value
        .as_object_mut()
        .ok_or(Error::InvalidEncoding)?
        .remove("prompt_fingerprint");
    let mut sources = Vec::new();
    for (input, external) in a.inputs.iter().skip(1).zip(&context_value.evidence_ids) {
        let Some(e) = evidence
            .iter()
            .find(|e| e.occurrence_id == input.evidence_id)
        else {
            return Ok(());
        };
        sources.push(serde_json::json!({"workload_version":context_value.workload_version,"case_id":context_value.case_id,"external_id":external,"occurrence_id":input.evidence_id,"payload_sha256":e.payload.sha256}));
    }
    let manifest=serde_json::to_vec(&serde_json::json!({
        "version":"incident-store-manifest/1","schema_version":1,
        "context":{"occurrence_id":first.evidence_id,"sha256":context.payload.sha256},
        "sources":sources,"workload_version":context_value.workload_version,
        "freeze_sha256":context_value.freeze_sha256,"prompt_sha256":context_value.prompt_sha256,
        "validator_version":"incident-routing-protocol/1","initial_renderer":"incident-request/1",
        "retry_renderer":"incident-request-retry/1","retry_feedback":"incident-routing-protocol/1:invalid_output",
        "output_cap":timing.calibration.output_cap,"runtime":runtime_value,
    })).map_err(|_|Error::InvalidEncoding)?;
    if sha256_hex(&manifest) != runtime.prompt_fingerprint {
        return Ok(());
    }
    a.immutable_manifest = Some(ByteEnvelope::from_bytes(&manifest)?);
    for (index, input) in a.inputs.iter_mut().enumerate() {
        input.role = if index == 0 {
            InputRole::Context
        } else {
            InputRole::Report
        };
    }
    if a.validator.is_none() {
        a.validator = Some(ValidatorBinding {
            version: "incident-routing-protocol/1".into(),
            provenance: ValidatorProvenance::DeclaredContract,
        });
    }
    if super::replay::fixture_runtime(a) {
        a.accounting.unit = AccountingUnit::FixtureBytes;
    }
    Ok(())
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::cell::RefCell;
    type Hook = Box<dyn FnOnce(&Connection, &[Selection])>;
    thread_local! {static BOUNDARY:RefCell<Option<Hook>>=RefCell::new(None);}
    pub(super) fn before_payload(db: &Connection, selection: &[Selection]) {
        BOUNDARY.with(|cell| {
            if let Some(hook) = cell.borrow_mut().take() {
                hook(db, selection)
            }
        });
    }

    fn dump(db: &Connection) -> Vec<String> {
        let mut names = db
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap();
        let names = names
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut result = Vec::new();
        for name in names {
            let mut statement = db
                .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                .unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                result.push(format!(
                    "{name}: {:?}",
                    (0..columns)
                        .map(|i| row.get_ref(i).unwrap())
                        .collect::<Vec<_>>()
                ));
            }
        }
        result.sort();
        result
    }
    struct Allow;
    impl DiagnosticAuthorizer for Allow {
        fn authorize(&self, _: &DiagnosticReadRequest) -> Result<bool, Error> {
            Ok(true)
        }
    }
    fn concurrent(change: u8) {
        use std::{sync::mpsc, time::Duration};
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kairotensor-export-wal-{}-{}.sqlite",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut reader = Store::open(&path).unwrap();
        reader
            .conn
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        let input = reader
            .observe_and_admit_many("a", "s", &[("source", b"old evidence")])
            .unwrap()[0];
        let root = reader.begin_attempt("a", "s", "main", &[input]).unwrap();
        let budget = reader.create_token_budget(100).unwrap();
        reader
            .reserve_dispatch(budget, root, b"old request", 3, 20)
            .unwrap();
        if change != 2 {
            reader.settle(root, b"old delivery", 5).unwrap();
        }
        let before = dump(&reader.conn);
        let expected = before.clone();
        let mut writer = Store::open(&path).unwrap();
        let (start_tx, start_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            start_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("payload barrier reached");
            match change {
                0 => {
                    writer.set_admitted("a", "s", input, false).unwrap();
                }
                1 => {
                    let next = writer.begin_attempt("a", "s", "main", &[input]).unwrap();
                    writer.settle(next, b"new head", 7).unwrap();
                }
                2 => {
                    writer.settle(root, b"new delivery", 6).unwrap();
                }
                _ => unreachable!(),
            }
            let committed = dump(&writer.conn);
            done_tx.send(committed).unwrap();
        });
        let observed = std::rc::Rc::new(RefCell::new(None));
        let observed_hook = observed.clone();
        BOUNDARY.with(|cell|*cell.borrow_mut()=Some(Box::new(move|db,selection|{
            start_tx.send(()).unwrap();
            let committed=done_rx.recv_timeout(Duration::from_secs(5)).expect("WAL writer commits while reader retains snapshot");
            assert!(!db.is_autocommit(),"payload fetch must retain the transaction");
            assert_eq!(selection.len(),1);let selected=&selection[0].selected;
            assert_eq!(selected.attempt,root);assert_eq!(selected.current_epoch,1);
            assert_eq!(view_epoch(db,"a","s").unwrap(),1);
            assert!(is_admitted(db,"a","s",input).unwrap());
            assert_eq!(dump(db),expected,"all selected rows, heads, admissions, holds and deliveries remain in original snapshot at payload boundary");
            assert_ne!(committed,expected,"writer must actually change committed rows");
            *observed_hook.borrow_mut()=Some(committed);
        })));
        let request = BundleRequest {
            actor: "a".into(),
            scope: "s".into(),
            attempt: root,
            purpose: if change == 2 {
                BundlePurpose::DiagnosticReplay
            } else {
                BundlePurpose::CurrentRead
            },
        };
        let result = reader.export_decision_bundle(&request, &Allow).unwrap();
        worker.join().unwrap();
        let BundleExport::Complete(b) = result else {
            panic!("retained admitted snapshot should export")
        };
        if change == 2 {
            assert_eq!(b.attempts[0].delivery, DeliveryState::Unresolved);
            assert_eq!(b.attempts[0].accounting.hold_bound, 23);
        } else {
            assert!(
                matches!(&b.attempts[0].delivery,DeliveryState::Recorded{raw,..} if raw.decode().unwrap()==b"old delivery")
            );
            assert_eq!(b.snapshot.head, Some(root));
        }
        assert!(reader.conn.is_autocommit());
        let committed = observed
            .borrow()
            .clone()
            .expect("actual payload boundary executed");
        assert_eq!(
            dump(&reader.conn),
            committed,
            "export introduces no writes after concurrent commit"
        );
        drop(reader);
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(
            dump(&reopened.conn),
            committed,
            "reopen preserves all rows and holds"
        );
        let fresh = reopened.export_decision_bundle(&request, &Allow).unwrap();
        if change < 2 {
            assert!(matches!(fresh, BundleExport::Redacted(_)));
        } else {
            let BundleExport::Complete(b) = fresh else {
                panic!()
            };
            assert_eq!(b.attempts[0].accounting.hold_bound, 0);
            assert_eq!(b.attempts[0].accounting.charge, 6);
        }
        assert_eq!(
            dump(&reopened.conn),
            committed,
            "fresh export is also read-only"
        );
    }
    #[test]
    fn withdrawal_retains_live_snapshot_through_payload_fetch() {
        concurrent(0)
    }
    #[test]
    fn head_change_retains_live_snapshot_through_payload_fetch() {
        concurrent(1)
    }
    #[test]
    fn settlement_retains_live_snapshot_through_payload_fetch() {
        concurrent(2)
    }
}
