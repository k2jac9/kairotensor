CREATE TABLE evidence (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL CHECK (length(source) > 0),
    payload BLOB NOT NULL
);

-- This history is also the authority for the current view. A no-op admission
-- never inserts a row, so the greatest epoch counts actual view changes.
CREATE TABLE admission_events (
    actor TEXT NOT NULL CHECK (length(actor) > 0),
    scope TEXT NOT NULL CHECK (length(scope) > 0),
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    evidence_id INTEGER NOT NULL REFERENCES evidence(id),
    admitted INTEGER NOT NULL CHECK (admitted IN (0, 1)),
    PRIMARY KEY (actor, scope, epoch)
) WITHOUT ROWID;

CREATE INDEX admission_by_evidence
    ON admission_events (actor, scope, evidence_id, epoch DESC);
