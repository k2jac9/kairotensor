-- The initial publication's attempt ID names one immutable support/v1 formula.
-- The verdict on each publication is historical, while missing is a projection.
CREATE TABLE support_bindings (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts(id),
    rule_id INTEGER NOT NULL REFERENCES attempts(id),
    verdict INTEGER NOT NULL CHECK (verdict IN (0, 1))
);
CREATE TABLE support_clauses (
    id INTEGER PRIMARY KEY,
    rule_id INTEGER NOT NULL REFERENCES attempts(id),
    arity INTEGER NOT NULL CHECK (arity > 0),
    missing INTEGER NOT NULL CHECK (missing BETWEEN 0 AND arity)
);
CREATE INDEX clauses_by_status ON support_clauses(rule_id, missing);

-- No evidence FK: an occurrence ID can be requested before it is observed.
-- Only live premises participate in the reverse index, including absent ones.
CREATE TABLE support_premises (
    clause_id INTEGER NOT NULL REFERENCES support_clauses(id),
    requested_id INTEGER NOT NULL CHECK (requested_id > 0),
    actor TEXT NOT NULL,
    scope TEXT NOT NULL,
    watching INTEGER NOT NULL DEFAULT 1 CHECK (watching IN (0, 1)),
    PRIMARY KEY (clause_id, requested_id)
) WITHOUT ROWID;
CREATE INDEX live_support_consumers
    ON support_premises(actor, scope, requested_id, clause_id) WHERE watching = 1;

-- Replacing a rule retires all its watches. Revising the answer to the same
-- rule preserves them, including the watches needed to recover lost support.
CREATE TRIGGER retire_support_watches AFTER UPDATE OF head ON branches
WHEN (SELECT rule_id FROM support_bindings WHERE attempt_id = OLD.head)
  IS NOT (SELECT rule_id FROM support_bindings WHERE attempt_id = NEW.head)
  AND EXISTS (SELECT 1 FROM support_bindings WHERE attempt_id = OLD.head)
BEGIN
    UPDATE support_premises SET watching = 0
    WHERE clause_id IN (
        SELECT id FROM support_clauses WHERE rule_id = (
            SELECT rule_id FROM support_bindings WHERE attempt_id = OLD.head
        )
    );
END;
