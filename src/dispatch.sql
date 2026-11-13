-- Exact client-visible request recorded at the pre-dispatch boundary.
-- attempt_inputs records the associated evidence occurrences in submitted order.
CREATE TABLE dispatches (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts(id),
    request BLOB NOT NULL CHECK (typeof(request) = 'blob')
);
