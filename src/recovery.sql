-- A stable caller-owned key identifies one logical recovery, not one process.
CREATE TABLE recovery_runs (
    id INTEGER PRIMARY KEY,
    recovery_key TEXT NOT NULL UNIQUE CHECK (length(trim(recovery_key)) > 0),
    budget_id INTEGER NOT NULL REFERENCES token_budgets(id),
    actor TEXT NOT NULL CHECK (length(actor) > 0),
    scope TEXT NOT NULL CHECK (length(scope) > 0),
    branch TEXT NOT NULL CHECK (length(branch) > 0)
);

CREATE TABLE recovery_attempts (
    recovery_id INTEGER NOT NULL REFERENCES recovery_runs(id),
    ordinal INTEGER NOT NULL CHECK (ordinal IN (0, 1)),
    attempt_id INTEGER NOT NULL UNIQUE REFERENCES attempts(id),
    state TEXT NOT NULL CHECK (state IN ('planned', 'reserved', 'sent', 'cancelled', 'denied')),
    request BLOB CHECK (request IS NULL OR typeof(request) = 'blob'),
    PRIMARY KEY (recovery_id, ordinal),
    CHECK (state IN ('planned', 'cancelled') OR request IS NOT NULL)
);

-- Ordinary attempts retain their existing settlement contract. A recovery
-- attempt, however, cannot publish or charge until the single-use send claim.
CREATE TRIGGER reject_unclaimed_recovery_delivery BEFORE INSERT ON deliveries
WHEN EXISTS (SELECT 1 FROM recovery_attempts r
             WHERE r.attempt_id = NEW.attempt_id AND r.state <> 'sent')
BEGIN
    SELECT RAISE(ABORT, 'recovery delivery requires a committed send claim');
END;
