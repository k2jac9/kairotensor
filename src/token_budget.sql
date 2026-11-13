-- A trusted caller assigns one or more ordinary inference attempts to a budget.
-- All amounts count input plus generated tokens, irrespective of token price.
CREATE TABLE token_budgets (
    id INTEGER PRIMARY KEY,
    token_limit INTEGER NOT NULL CHECK (typeof(token_limit) = 'integer' AND token_limit >= 0)
);

-- Keep each admission decision, including denials. Only granted, unsettled
-- attempts hold capacity. The existing deliveries table owns all actual usage.
CREATE TABLE token_reservations (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts(id),
    budget_id INTEGER NOT NULL REFERENCES token_budgets(id),
    input_tokens INTEGER NOT NULL CHECK (typeof(input_tokens) = 'integer' AND input_tokens >= 0),
    output_cap INTEGER NOT NULL CHECK (typeof(output_cap) = 'integer' AND output_cap > 0),
    granted INTEGER NOT NULL CHECK (granted IN (0, 1)),
    CHECK (typeof(input_tokens + output_cap) = 'integer')
);
CREATE INDEX reservations_by_budget ON token_reservations(budget_id, granted);

-- Both settlement APIs write a delivery under the same lock as head publication.
-- A denied dispatch has no cost to settle. A known successful or rejected model
-- response reports its entire preflight input plus the generated tokens.
CREATE TRIGGER validate_reserved_usage BEFORE INSERT ON deliveries
WHEN EXISTS (
    SELECT 1 FROM token_reservations r WHERE r.attempt_id = NEW.attempt_id
      AND (r.granted = 0 OR NEW.usage < r.input_tokens OR NEW.usage > r.input_tokens + r.output_cap)
)
BEGIN
    SELECT RAISE(ABORT, 'usage conflicts with token reservation');
END;
