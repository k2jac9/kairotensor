-- Explicit opt-in for pure model requests assembled from a closed candidate list.
CREATE TABLE model_attempts (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts(id),
    contract TEXT NOT NULL CHECK (length(contract) > 0)
);

-- A reuse publication points directly to a dispatched execution, including
-- on consecutive hits. Only real dispatches have a row in dispatches.
CREATE TABLE model_reuses (
    attempt_id INTEGER PRIMARY KEY REFERENCES model_attempts(attempt_id),
    origin_attempt INTEGER NOT NULL REFERENCES dispatches(attempt_id),
    CHECK (origin_attempt < attempt_id)
);
