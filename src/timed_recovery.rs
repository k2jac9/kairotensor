//! Explicit opt-in deadline-aware admission for durable recovery operations.
use crate::{invalid, Disposition, Store};
use rusqlite::{params, Connection, OptionalExtension, Result, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReadiness {
    Cold,
    Warm,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRuntime {
    pub model: String,
    pub model_revision: String,
    pub tokenizer_revision: String,
    pub runtime_version: String,
    pub dtype: String,
    pub decoding: String,
    pub cpu_fingerprint: String,
    pub threads: i64,
    pub prompt_fingerprint: String,
    pub readiness: RecoveryReadiness,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCalibration {
    pub id: String,
    pub max_duration_ms: i64,
    pub sample_count: i64,
    pub max_input_tokens: i64,
    pub output_cap: i64,
    pub measured_at_ms: i64,
    pub expires_at_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTiming {
    pub clock_domain: String,
    pub start_ms: i64,
    pub deadline_ms: i64,
    pub runtime: RecoveryRuntime,
    pub calibration: RecoveryCalibration,
    pub safety_margin_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryClock {
    pub domain: String,
    pub now_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPreparation {
    pub runtime: Option<RecoveryRuntime>,
    pub calibration_id: Option<String>,
    pub request: Vec<u8>,
    pub input_tokens: i64,
    pub output_cap: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingRefusal {
    UnknownRuntime,
    RuntimeMismatch,
    UnknownCalibration,
    CalibrationMismatch,
    QuoteMismatch,
    UncoveredQuote,
    StaleEvidence,
    InvalidBudget,
    ClockUnavailable,
    ClockDomainMismatch,
    ClockRegressed,
    CalibrationExpired,
    DeadlineExpired,
    InsufficientTime,
    InvalidTiming,
}
#[derive(Debug, PartialEq, Eq)]
pub enum TimedRecoveryClaim {
    Dispatched {
        request: Vec<u8>,
        decision_id: i64,
        observed_ms: i64,
        estimated_finish_ms: i64,
    },
    Refused {
        decision_id: i64,
        reason: TimingRefusal,
    },
    NotReserved,
}
impl TimingRefusal {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnknownRuntime => "unknown_runtime",
            Self::RuntimeMismatch => "runtime_mismatch",
            Self::UnknownCalibration => "unknown_calibration",
            Self::CalibrationMismatch => "calibration_mismatch",
            Self::QuoteMismatch => "quote_mismatch",
            Self::UncoveredQuote => "uncovered_quote",
            Self::StaleEvidence => "stale_evidence",
            Self::InvalidBudget => "invalid_budget",
            Self::ClockUnavailable => "clock_unavailable",
            Self::ClockDomainMismatch => "clock_domain_mismatch",
            Self::ClockRegressed => "clock_regressed",
            Self::CalibrationExpired => "calibration_expired",
            Self::DeadlineExpired => "deadline_expired",
            Self::InsufficientTime => "insufficient_time",
            Self::InvalidTiming => "invalid_timing",
        }
    }
}
fn nonblank(value: &str) -> bool {
    !value.trim().is_empty()
        && !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "unknown" | "unspecified" | "n/a" | "none"
        )
}
fn fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| invalid("cannot encode timing binding"))
}
fn decode(value: &str) -> Result<RecoveryTiming> {
    serde_json::from_str(value).map_err(|_| invalid("invalid stored timing binding"))
}
fn validate(t: &RecoveryTiming) -> Result<()> {
    let r = &t.runtime;
    let c = &t.calibration;
    if ![
        t.clock_domain.as_str(),
        r.model.as_str(),
        r.model_revision.as_str(),
        r.tokenizer_revision.as_str(),
        r.runtime_version.as_str(),
        r.dtype.as_str(),
        r.decoding.as_str(),
        c.id.as_str(),
    ]
    .into_iter()
    .all(nonblank)
        || !fingerprint(&r.cpu_fingerprint)
        || !fingerprint(&r.prompt_fingerprint)
        || r.threads <= 0
        || t.start_ms < 0
        || t.deadline_ms <= t.start_ms
        || t.safety_margin_ms < 0
        || c.max_duration_ms <= 0
        || c.sample_count <= 0
        || c.max_input_tokens < 0
        || c.output_cap <= 0
        || c.measured_at_ms < 0
        || c.measured_at_ms > t.start_ms
        || c.expires_at_ms <= t.start_ms
        || t.start_ms
            .checked_add(c.max_duration_ms)
            .and_then(|v| v.checked_add(t.safety_margin_ms))
            .is_none()
    {
        return Err(invalid("invalid or unknown recovery timing profile"));
    }
    Ok(())
}
pub(crate) fn is_timed_run(db: &Connection, recovery: i64) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM recovery_timing_bindings WHERE recovery_id=?1)",
        [recovery],
        |r| r.get(0),
    )
}
pub(crate) fn is_timed_attempt(db: &Connection, attempt: i64) -> Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM recovery_attempts a JOIN recovery_timing_bindings t ON t.recovery_id=a.recovery_id WHERE a.attempt_id=?1)",[attempt],|r|r.get(0))
}
impl Store {
    /// Opt in at creation, never adopt a legacy operation. The whole timing record
    /// is immutable and idempotent only for the exact same key and bindings.
    /// Clock, runtime and calibration identities are trusted caller attestations,
    /// not authenticated hardware facts. Calibration timestamps use this domain.
    pub fn create_timed_recovery(
        &mut self,
        key: &str,
        budget: i64,
        actor: &str,
        scope: &str,
        branch: &str,
        timing: &RecoveryTiming,
    ) -> Result<i64> {
        crate::validate_view(actor, scope)?;
        validate(timing)?;
        if key.trim().is_empty() || branch.is_empty() {
            return Err(invalid("recovery key and branch must be nonempty"));
        }
        let configuration = encode(timing)?;
        let runtime_json = encode(&timing.runtime)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing:Option<(i64,bool)>=tx.query_row("SELECT id,(budget_id=?2 AND actor=?3 AND scope=?4 AND branch=?5) FROM recovery_runs WHERE recovery_key=?1",params![key,budget,actor,scope,branch],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((id, same)) = existing {
            let prior: Option<String> = tx
                .query_row(
                    "SELECT configuration FROM recovery_timing_bindings WHERE recovery_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if !same || prior.as_deref() != Some(configuration.as_str()) {
                return Err(invalid(
                    "recovery key has different or legacy timing bindings",
                ));
            }
            return Ok(id);
        }
        tx.execute("INSERT INTO recovery_runs(recovery_key,budget_id,actor,scope,branch) VALUES(?1,?2,?3,?4,?5)",params![key,budget,actor,scope,branch])?;
        let id = tx.last_insert_rowid();
        let c = &timing.calibration;
        tx.execute("INSERT INTO recovery_timing_bindings(recovery_id,configuration,clock_domain,start_ms,deadline_ms,runtime_json,calibration_id,max_duration_ms,safety_margin_ms,max_input_tokens,output_cap,measured_at_ms,expires_at_ms,last_observed_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?4)",params![id,configuration,timing.clock_domain,timing.start_ms,timing.deadline_ms,runtime_json,c.id,c.max_duration_ms,timing.safety_margin_ms,c.max_input_tokens,c.output_cap,c.measured_at_ms,c.expires_at_ms])?;
        tx.commit()?;
        Ok(id)
    }

    /// Read the original window and exact profile for a restart. Reading it is
    /// not permission to dispatch, extend the deadline, or reset the clock domain.
    pub fn recovery_timing(&self, recovery: i64) -> Result<Option<RecoveryTiming>> {
        self.conn
            .query_row(
                "SELECT configuration FROM recovery_timing_bindings WHERE recovery_id=?1",
                [recovery],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| decode(&s))
            .transpose()
    }

    /// Check an existing unsent reservation immediately before the single-use
    /// send claim. The clock callback runs AFTER acquiring the Immediate lock and
    /// completing DB eligibility/profile/quote/budget checks, never before queueing.
    /// It must promptly sample the named monotonic domain, not return a saved tick.
    /// A same-domain process restart is allowed only if the caller can attest
    /// continuity; unknown/changed domains and observed regressions fail closed.
    ///
    /// Refusals are audited and retain the cancellable unsent hold. Database or
    /// callback panic failures roll back; no successful return means no permission.
    /// The callback's ordinary error records ClockUnavailable. A successful claim
    /// is potentially sent forever until a measured delivery; recheck the deadline
    /// before actual I/O, but do NOT release/resend a claimed hold if time expired.
    /// Commit/fsync/transport can still delay completion: the measured maximum plus
    /// margin is empirical admission evidence, not an on-time completion guarantee.
    /// Calibration expiry is checked at admission, not the estimated finish.
    pub fn claim_timed_recovery_dispatch<F>(
        &mut self,
        attempt: i64,
        prepared: &RecoveryPreparation,
        clock: F,
    ) -> Result<TimedRecoveryClaim>
    where
        F: FnOnce() -> Result<RecoveryClock>,
    {
        let supplied_runtime = prepared.runtime.as_ref().map(encode).transpose()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (recovery, state, request): (i64, String, Option<Vec<u8>>) = tx.query_row(
            "SELECT recovery_id,state,request FROM recovery_attempts WHERE attempt_id=?1",
            [attempt],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let (configuration,floor):(String,i64)=tx.query_row("SELECT configuration,last_observed_ms FROM recovery_timing_bindings WHERE recovery_id=?1",[recovery],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if state != "reserved" {
            return Ok(TimedRecoveryClaim::NotReserved);
        }
        let t = decode(&configuration)?;
        validate(&t)?;
        let request = request.ok_or_else(|| invalid("reserved recovery has no request"))?;
        let (budget,input,cap,granted):(i64,i64,i64,bool)=tx.query_row("SELECT budget_id,input_tokens,output_cap,granted FROM token_reservations WHERE attempt_id=?1",[attempt],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let (_, eligibility) = crate::attempts::applicability(&tx, attempt)?;
        let balance = crate::token_budget::budget_state(&tx, budget)?;
        let quote = input
            .checked_add(cap)
            .ok_or_else(|| invalid("stored reservation overflow"))?;
        let accounting_valid = granted
            && balance.reserved >= quote
            && balance
                .spent
                .checked_add(balance.reserved)
                .is_some_and(|sum| sum <= balance.limit);
        let precondition = if prepared.runtime.is_none() {
            Some(TimingRefusal::UnknownRuntime)
        } else if prepared.runtime.as_ref() != Some(&t.runtime) {
            Some(TimingRefusal::RuntimeMismatch)
        } else if prepared.calibration_id.is_none() {
            Some(TimingRefusal::UnknownCalibration)
        } else if prepared.calibration_id.as_deref() != Some(t.calibration.id.as_str()) {
            Some(TimingRefusal::CalibrationMismatch)
        } else if prepared.request != request
            || prepared.input_tokens != input
            || prepared.output_cap != cap
        {
            Some(TimingRefusal::QuoteMismatch)
        } else if input > t.calibration.max_input_tokens || cap != t.calibration.output_cap {
            Some(TimingRefusal::UncoveredQuote)
        } else if eligibility != Disposition::Accepted {
            Some(TimingRefusal::StaleEvidence)
        } else if !accounting_valid {
            Some(TimingRefusal::InvalidBudget)
        } else {
            None
        };
        // This is the final potentially blocking caller step under the writer
        // lock. All remaining writes form the one durable admission/claim fence.
        let observed = clock().ok();
        let mut finish = None;
        let clock_reason = match observed.as_ref() {
            None => Some(TimingRefusal::ClockUnavailable),
            Some(c) if c.domain != t.clock_domain => Some(TimingRefusal::ClockDomainMismatch),
            Some(c) if c.now_ms < 0 => Some(TimingRefusal::InvalidTiming),
            Some(c) if c.now_ms < floor || c.now_ms < t.start_ms => {
                Some(TimingRefusal::ClockRegressed)
            }
            Some(c) => {
                tx.execute(
                    "UPDATE recovery_timing_bindings SET last_observed_ms=?2 WHERE recovery_id=?1",
                    params![recovery, c.now_ms],
                )?;
                finish = c
                    .now_ms
                    .checked_add(t.calibration.max_duration_ms)
                    .and_then(|v| v.checked_add(t.safety_margin_ms));
                if finish.is_none() {
                    Some(TimingRefusal::InvalidTiming)
                } else if c.now_ms < t.calibration.measured_at_ms
                    || c.now_ms >= t.calibration.expires_at_ms
                {
                    Some(TimingRefusal::CalibrationExpired)
                } else if c.now_ms >= t.deadline_ms {
                    Some(TimingRefusal::DeadlineExpired)
                } else if finish.is_some_and(|v| v >= t.deadline_ms) {
                    Some(TimingRefusal::InsufficientTime)
                } else {
                    None
                }
            }
        };
        let refusal = clock_reason.or(precondition);
        let reason = refusal.map(TimingRefusal::as_str).unwrap_or("admitted");
        tx.execute("INSERT INTO recovery_admission_decisions(recovery_id,attempt_id,granted,reason,runtime_json,calibration_id,request,input_tokens,output_cap,observed_clock_domain,observed_now_ms,estimated_finish_ms,budget_limit,spent_tokens,reserved_tokens) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![recovery,attempt,refusal.is_none(),reason,supplied_runtime,prepared.calibration_id,prepared.request,prepared.input_tokens,prepared.output_cap,observed.as_ref().map(|c|c.domain.as_str()),observed.as_ref().map(|c|c.now_ms),finish,balance.limit,balance.spent,balance.reserved])?;
        let decision_id = tx.last_insert_rowid();
        if let Some(reason) = refusal {
            tx.commit()?;
            return Ok(TimedRecoveryClaim::Refused {
                decision_id,
                reason,
            });
        }
        tx.execute(
            "UPDATE recovery_attempts SET state='sent' WHERE attempt_id=?1",
            [attempt],
        )?;
        tx.execute(
            "INSERT INTO dispatches(attempt_id,request) VALUES(?1,?2)",
            params![attempt, request],
        )?;
        tx.commit()?;
        Ok(TimedRecoveryClaim::Dispatched {
            request,
            decision_id,
            observed_ms: observed.expect("admitted clock").now_ms,
            estimated_finish_ms: finish.expect("admitted estimate"),
        })
    }
}
