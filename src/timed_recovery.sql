-- Explicit opt-in. Legacy operations are never adopted by this migration.
-- configuration is the exact versioned Rust record; columns duplicate its
-- admission-critical fields so SQLite can enforce the send fence too.
CREATE TABLE recovery_timing_bindings (
    recovery_id INTEGER PRIMARY KEY REFERENCES recovery_runs(id),
    configuration TEXT NOT NULL,
    clock_domain TEXT NOT NULL CHECK(length(trim(clock_domain))>0),
    start_ms INTEGER NOT NULL CHECK(typeof(start_ms)='integer' AND start_ms>=0),
    deadline_ms INTEGER NOT NULL CHECK(typeof(deadline_ms)='integer' AND deadline_ms>start_ms),
    runtime_json TEXT NOT NULL,
    calibration_id TEXT NOT NULL,
    max_duration_ms INTEGER NOT NULL CHECK(typeof(max_duration_ms)='integer' AND max_duration_ms>0),
    safety_margin_ms INTEGER NOT NULL CHECK(typeof(safety_margin_ms)='integer' AND safety_margin_ms>=0),
    max_input_tokens INTEGER NOT NULL CHECK(typeof(max_input_tokens)='integer' AND max_input_tokens>=0),
    output_cap INTEGER NOT NULL CHECK(typeof(output_cap)='integer' AND output_cap>0),
    measured_at_ms INTEGER NOT NULL CHECK(typeof(measured_at_ms)='integer' AND measured_at_ms>=0 AND measured_at_ms<=start_ms),
    expires_at_ms INTEGER NOT NULL CHECK(typeof(expires_at_ms)='integer' AND expires_at_ms>start_ms),
    last_observed_ms INTEGER NOT NULL CHECK(typeof(last_observed_ms)='integer' AND last_observed_ms>=start_ms)
);
CREATE TRIGGER timing_requires_new_operation BEFORE INSERT ON recovery_timing_bindings
WHEN EXISTS(SELECT 1 FROM recovery_attempts WHERE recovery_id=NEW.recovery_id)
BEGIN SELECT RAISE(ABORT,'cannot attach timing to existing attempts'); END;
CREATE TRIGGER timing_configuration_is_immutable BEFORE UPDATE OF recovery_id,configuration,clock_domain,start_ms,deadline_ms,runtime_json,calibration_id,max_duration_ms,safety_margin_ms,max_input_tokens,output_cap,measured_at_ms,expires_at_ms ON recovery_timing_bindings
BEGIN SELECT RAISE(ABORT,'timing configuration is immutable'); END;
CREATE TRIGGER timing_floor_is_monotonic BEFORE UPDATE OF last_observed_ms ON recovery_timing_bindings
WHEN NEW.last_observed_ms<OLD.last_observed_ms
BEGIN SELECT RAISE(ABORT,'clock floor cannot move backward'); END;
CREATE TRIGGER timing_cannot_be_removed BEFORE DELETE ON recovery_timing_bindings
BEGIN SELECT RAISE(ABORT,'timing binding cannot be removed'); END;

-- A receipt is an audit record, not an independent send permission. Only the
-- successful claim transaction changes reserved->sent and records a dispatch.
CREATE TABLE recovery_admission_decisions (
    id INTEGER PRIMARY KEY,
    recovery_id INTEGER NOT NULL REFERENCES recovery_timing_bindings(recovery_id),
    attempt_id INTEGER NOT NULL REFERENCES recovery_attempts(attempt_id),
    granted INTEGER NOT NULL CHECK(granted IN (0,1)),
    reason TEXT NOT NULL,
    runtime_json TEXT,
    calibration_id TEXT,
    request BLOB NOT NULL CHECK(typeof(request)='blob'),
    input_tokens INTEGER NOT NULL CHECK(typeof(input_tokens)='integer'),
    output_cap INTEGER NOT NULL CHECK(typeof(output_cap)='integer'),
    observed_clock_domain TEXT,
    observed_now_ms INTEGER CHECK(observed_now_ms IS NULL OR typeof(observed_now_ms)='integer'),
    estimated_finish_ms INTEGER CHECK(estimated_finish_ms IS NULL OR typeof(estimated_finish_ms)='integer'),
    budget_limit INTEGER NOT NULL CHECK(typeof(budget_limit)='integer'),
    spent_tokens INTEGER NOT NULL CHECK(typeof(spent_tokens)='integer'),
    reserved_tokens INTEGER NOT NULL CHECK(typeof(reserved_tokens)='integer'),
    CHECK((granted=1 AND reason='admitted' AND observed_now_ms IS NOT NULL AND estimated_finish_ms>observed_now_ms)
       OR (granted=0 AND reason<>'admitted'))
);
CREATE UNIQUE INDEX one_timed_grant ON recovery_admission_decisions(attempt_id) WHERE granted=1;
CREATE TRIGGER admission_decisions_are_immutable BEFORE UPDATE ON recovery_admission_decisions
BEGIN SELECT RAISE(ABORT,'admission decisions are immutable'); END;
CREATE TRIGGER admission_decisions_cannot_be_removed BEFORE DELETE ON recovery_admission_decisions
BEGIN SELECT RAISE(ABORT,'admission decisions cannot be removed'); END;
CREATE TRIGGER timed_grant_requires_exact_bound_reservation BEFORE INSERT ON recovery_admission_decisions
WHEN NEW.granted=1 AND NOT EXISTS (
    SELECT 1 FROM recovery_attempts a JOIN recovery_timing_bindings t ON t.recovery_id=a.recovery_id
    JOIN token_reservations q ON q.attempt_id=a.attempt_id
    WHERE a.attempt_id=NEW.attempt_id AND a.recovery_id=NEW.recovery_id AND a.state='reserved'
      AND NEW.spent_tokens>=0 AND NEW.reserved_tokens>=q.input_tokens+q.output_cap
      AND typeof(NEW.spent_tokens+NEW.reserved_tokens)='integer'
      AND NEW.spent_tokens+NEW.reserved_tokens<=NEW.budget_limit
      AND q.granted=1 AND a.request=NEW.request AND q.input_tokens=NEW.input_tokens AND q.output_cap=NEW.output_cap
      AND NEW.runtime_json=t.runtime_json AND NEW.calibration_id=t.calibration_id
      AND NEW.input_tokens<=t.max_input_tokens AND NEW.output_cap=t.output_cap
      AND NEW.observed_clock_domain=t.clock_domain AND NEW.observed_now_ms=t.last_observed_ms
      AND NEW.observed_now_ms>=t.measured_at_ms AND NEW.observed_now_ms<t.expires_at_ms
      AND NEW.observed_now_ms>=t.start_ms AND NEW.observed_now_ms<t.deadline_ms
      AND typeof(NEW.observed_now_ms+t.max_duration_ms+t.safety_margin_ms)='integer'
      AND NEW.estimated_finish_ms=NEW.observed_now_ms+t.max_duration_ms+t.safety_margin_ms
      AND NEW.estimated_finish_ms<t.deadline_ms
)
BEGIN SELECT RAISE(ABORT,'timed grant does not match bound reservation'); END;
CREATE TRIGGER timed_state_requires_grant BEFORE UPDATE OF state ON recovery_attempts
WHEN NEW.state='sent' AND EXISTS(SELECT 1 FROM recovery_timing_bindings WHERE recovery_id=NEW.recovery_id)
 AND NOT EXISTS(SELECT 1 FROM recovery_admission_decisions d WHERE d.attempt_id=NEW.attempt_id AND d.granted=1 AND d.request=NEW.request)
BEGIN SELECT RAISE(ABORT,'timed dispatch requires a checked grant'); END;
CREATE TRIGGER timed_dispatch_requires_grant BEFORE INSERT ON dispatches
WHEN EXISTS(SELECT 1 FROM recovery_attempts a JOIN recovery_timing_bindings t ON t.recovery_id=a.recovery_id WHERE a.attempt_id=NEW.attempt_id)
 AND NOT EXISTS(SELECT 1 FROM recovery_admission_decisions d JOIN recovery_attempts a ON a.attempt_id=d.attempt_id
                WHERE d.attempt_id=NEW.attempt_id AND d.granted=1 AND d.request=NEW.request AND a.state='sent')
BEGIN SELECT RAISE(ABORT,'timed dispatch requires a checked grant'); END;
