-- A typed local computation. The trusted runtime derives these immutable bytes
-- from a complete captured evidence manifest; callers cannot supply the output.
-- This table never adopts pre-existing dispatched, budgeted, or settled work.
CREATE TABLE deterministic_verdicts (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts(id),
    computation TEXT NOT NULL CHECK (computation = 'report-verdict/v1'),
    output BLOB NOT NULL CHECK (typeof(output) = 'blob')
);
CREATE TRIGGER deterministic_binding_requires_fresh_attempt BEFORE INSERT ON deterministic_verdicts
WHEN EXISTS (SELECT 1 FROM dispatches WHERE attempt_id=NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM token_reservations WHERE attempt_id=NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM deliveries WHERE attempt_id=NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM model_attempts WHERE attempt_id=NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM recovery_attempts WHERE attempt_id=NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'deterministic binding requires a fresh ordinary attempt'); END;
CREATE TRIGGER deterministic_binding_is_immutable BEFORE UPDATE ON deterministic_verdicts
BEGIN SELECT RAISE(ABORT, 'deterministic binding is immutable'); END;
CREATE TRIGGER deterministic_binding_cannot_be_removed BEFORE DELETE ON deterministic_verdicts
BEGIN SELECT RAISE(ABORT, 'deterministic binding cannot be removed'); END;
CREATE TRIGGER deterministic_no_dispatch BEFORE INSERT ON dispatches
WHEN EXISTS (SELECT 1 FROM deterministic_verdicts WHERE attempt_id=NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'deterministic attempts cannot dispatch'); END;
CREATE TRIGGER deterministic_no_reservation BEFORE INSERT ON token_reservations
WHEN EXISTS (SELECT 1 FROM deterministic_verdicts WHERE attempt_id=NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'deterministic attempts cannot reserve tokens'); END;
CREATE TRIGGER deterministic_no_model_opt_in BEFORE INSERT ON model_attempts
WHEN EXISTS (SELECT 1 FROM deterministic_verdicts WHERE attempt_id=NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'deterministic attempts cannot become model attempts'); END;
CREATE TRIGGER deterministic_no_recovery_opt_in BEFORE INSERT ON recovery_attempts
WHEN EXISTS (SELECT 1 FROM deterministic_verdicts WHERE attempt_id=NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'deterministic attempts cannot become recovery attempts'); END;
CREATE TRIGGER deterministic_delivery_is_bound BEFORE INSERT ON deliveries
WHEN EXISTS (SELECT 1 FROM deterministic_verdicts v WHERE v.attempt_id=NEW.attempt_id
             AND (NEW.usage <> 0 OR NEW.output <> v.output OR NEW.disposition='invalid_output'))
BEGIN SELECT RAISE(ABORT, 'deterministic delivery must match the zero-cost bound verdict'); END;
