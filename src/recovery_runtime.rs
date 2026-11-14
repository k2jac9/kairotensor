use crate::{Disposition, RecoveryClock, RecoveryPreparation, Store, TimingRefusal};
use rusqlite::Result;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyValidation {
    Valid,
    Invalid { validator: String, reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredRecoveryReply {
    pub output: Vec<u8>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub validation: ReplyValidation,
}
#[derive(Debug)]
pub enum RecoveryRun<E> {
    NotReserved,
    Refused {
        decision_id: i64,
        reason: TimingRefusal,
    },
    ClaimedButNotInvoked {
        attempt: i64,
        decision_id: i64,
        reason: TimingRefusal,
    },
    BackendFailed {
        attempt: i64,
        decision_id: i64,
        error: E,
    },
    Settled {
        attempt: i64,
        decision_id: i64,
        disposition: Disposition,
    },
    SettlementFailed {
        attempt: i64,
        decision_id: i64,
        reply: MeasuredRecoveryReply,
        error: rusqlite::Error,
    },
}
impl Store {
    /// Concrete fail-closed Linux boot-clock route. No wall-clock fallback.
    pub fn run_timed_recovery_linux<B, E>(
        &mut self,
        attempt: i64,
        preparation: &RecoveryPreparation,
        backend: B,
    ) -> Result<RecoveryRun<E>>
    where
        B: FnOnce(&[u8]) -> std::result::Result<MeasuredRecoveryReply, E>,
    {
        self.run_timed_recovery(
            attempt,
            preparation,
            || {
                crate::LinuxBootClock::sample()
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            },
            backend,
        )
    }

    /// Single local backend invocation, never an exactly-once network guarantee.
    /// The backend is trusted to validate output and report measured token counts.
    /// Errors/panics after claim retain its hold. No clock can guarantee completion.
    pub fn run_timed_recovery<C, B, E>(
        &mut self,
        attempt: i64,
        preparation: &RecoveryPreparation,
        mut clock: C,
        backend: B,
    ) -> Result<RecoveryRun<E>>
    where
        C: FnMut() -> Result<RecoveryClock>,
        B: FnOnce(&[u8]) -> std::result::Result<MeasuredRecoveryReply, E>,
    {
        // Read immutable original binding before claiming; no database work occurs
        // between the final clock sample and backend invocation.
        let recovery: i64 = self.conn.query_row(
            "SELECT recovery_id FROM recovery_attempts WHERE attempt_id=?1",
            [attempt],
            |r| r.get(0),
        )?;
        let timing = self
            .recovery_timing(recovery)?
            .ok_or_else(|| crate::invalid("runtime wrapper requires timed recovery"))?;
        let (request, decision_id, observed_ms) =
            match self.claim_timed_recovery_dispatch(attempt, preparation, &mut clock)? {
                crate::TimedRecoveryClaim::NotReserved => return Ok(RecoveryRun::NotReserved),
                crate::TimedRecoveryClaim::Refused {
                    decision_id,
                    reason,
                } => {
                    return Ok(RecoveryRun::Refused {
                        decision_id,
                        reason,
                    })
                }
                crate::TimedRecoveryClaim::Dispatched {
                    request,
                    decision_id,
                    observed_ms,
                    ..
                } => (request, decision_id, observed_ms),
            };
        let refusal = match clock() {
            Err(_) => Some(TimingRefusal::ClockUnavailable),
            Ok(now) if now.domain != timing.clock_domain => {
                Some(TimingRefusal::ClockDomainMismatch)
            }
            Ok(now) if now.now_ms < observed_ms => Some(TimingRefusal::ClockRegressed),
            Ok(now) if now.now_ms >= timing.deadline_ms => Some(TimingRefusal::DeadlineExpired),
            Ok(now)
                if now.now_ms < timing.calibration.measured_at_ms
                    || now.now_ms >= timing.calibration.expires_at_ms =>
            {
                Some(TimingRefusal::CalibrationExpired)
            }
            Ok(now) => match now
                .now_ms
                .checked_add(timing.calibration.max_duration_ms)
                .and_then(|v| v.checked_add(timing.safety_margin_ms))
            {
                None => Some(TimingRefusal::InvalidTiming),
                Some(finish) if finish >= timing.deadline_ms => {
                    Some(TimingRefusal::InsufficientTime)
                }
                Some(_) => None,
            },
        };
        if let Some(reason) = refusal {
            return Ok(RecoveryRun::ClaimedButNotInvoked {
                attempt,
                decision_id,
                reason,
            });
        }
        let reply = match backend(&request) {
            Ok(reply) => reply,
            Err(error) => {
                return Ok(RecoveryRun::BackendFailed {
                    attempt,
                    decision_id,
                    error,
                })
            }
        };
        let usage = reply.input_tokens.checked_add(reply.output_tokens);
        let disposition = if reply.input_tokens != preparation.input_tokens
            || reply.input_tokens < 0
            || reply.output_tokens < 0
            || reply.output_tokens > preparation.output_cap
            || usage.is_none()
        {
            Err(crate::invalid("measured usage does not fit frozen quote"))
        } else {
            match &reply.validation {
                ReplyValidation::Valid => self.settle(attempt, &reply.output, usage.unwrap()),
                ReplyValidation::Invalid { validator, reason } => {
                    self.reject_invalid(attempt, &reply.output, usage.unwrap(), validator, reason)
                }
            }
        };
        Ok(match disposition {
            Ok(disposition) => RecoveryRun::Settled {
                attempt,
                decision_id,
                disposition,
            },
            Err(error) => RecoveryRun::SettlementFailed {
                attempt,
                decision_id,
                reply,
                error,
            },
        })
    }
}
