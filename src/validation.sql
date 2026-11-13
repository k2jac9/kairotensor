-- Redefine the disposition constraint while retaining the original delivery rows.
-- No validation facts can be inferred for the earlier accepted or stale deliveries.
CREATE TABLE deliveries_v5 (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts (id),
    disposition TEXT NOT NULL
        CHECK (disposition IN ('accepted', 'stale_view', 'stale_branch', 'invalid_output')),
    output BLOB NOT NULL,
    usage INTEGER NOT NULL CHECK (typeof(usage) = 'integer' AND usage >= 0),
    validator TEXT,
    reason TEXT,
    applicability TEXT,
    CHECK (
        (disposition = 'invalid_output'
            AND validator IS NOT NULL AND length(trim(validator)) > 0
            AND reason IS NOT NULL AND length(trim(reason)) > 0
            AND applicability IS NOT NULL AND applicability IN ('current', 'stale_view', 'stale_branch'))
        OR
        (disposition <> 'invalid_output' AND validator IS NULL AND reason IS NULL AND applicability IS NULL)
    )
);
INSERT INTO deliveries_v5 (attempt_id, disposition, output, usage)
    SELECT attempt_id, disposition, output, usage FROM deliveries;
DROP TABLE deliveries;
ALTER TABLE deliveries_v5 RENAME TO deliveries;
