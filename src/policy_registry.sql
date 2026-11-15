-- Scoped immutable attestations. Only the trusted runtime authenticates the caller.
CREATE TABLE policy_versions (
    version_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(version_id)) > 0),
    actor TEXT NOT NULL CHECK (length(trim(actor)) > 0),
    scope TEXT NOT NULL CHECK (length(trim(scope)) > 0),
    purpose TEXT NOT NULL CHECK (length(trim(purpose)) > 0),
    evidence_id INTEGER NOT NULL UNIQUE REFERENCES evidence(id),
    compatibility_contract_sha256 TEXT NOT NULL CHECK (length(compatibility_contract_sha256) = 64),
    artifact_sha256 TEXT NOT NULL CHECK (length(artifact_sha256) = 64),
    UNIQUE (version_id, actor, scope, purpose)
);
CREATE TABLE policy_evaluations (
    evaluation_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(evaluation_id)) > 0),
    actor TEXT NOT NULL CHECK (length(trim(actor)) > 0),
    scope TEXT NOT NULL CHECK (length(trim(scope)) > 0),
    purpose TEXT NOT NULL CHECK (length(trim(purpose)) > 0),
    baseline_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    baseline_sha256 TEXT NOT NULL CHECK (length(baseline_sha256) = 64),
    candidate_sha256 TEXT NOT NULL CHECK (length(candidate_sha256) = 64),
    experiment_manifest BLOB NOT NULL CHECK (typeof(experiment_manifest) = 'blob' AND length(experiment_manifest) <= 16777216),
    experiment_sha256 TEXT NOT NULL CHECK (length(experiment_sha256) = 64),
    evaluator_version TEXT NOT NULL CHECK (length(trim(evaluator_version)) > 0),
    evaluator_sha256 TEXT NOT NULL CHECK (length(evaluator_sha256) = 64),
    baseline_rows BLOB NOT NULL CHECK (typeof(baseline_rows) = 'blob' AND length(baseline_rows) <= 16777216),
    candidate_rows BLOB NOT NULL CHECK (typeof(candidate_rows) = 'blob' AND length(candidate_rows) <= 16777216),
    evaluation_bytes BLOB NOT NULL CHECK (typeof(evaluation_bytes) = 'blob' AND length(evaluation_bytes) <= 16777216),
    evaluation_sha256 TEXT NOT NULL CHECK (length(evaluation_sha256) = 64),
    FOREIGN KEY (baseline_id, actor, scope, purpose) REFERENCES policy_versions(version_id, actor, scope, purpose),
    FOREIGN KEY (candidate_id, actor, scope, purpose) REFERENCES policy_versions(version_id, actor, scope, purpose),
    UNIQUE (evaluation_id, actor, scope, purpose)
);
CREATE TABLE policy_heads (
    actor TEXT NOT NULL,
    scope TEXT NOT NULL,
    purpose TEXT NOT NULL,
    version_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (typeof(revision) = 'integer' AND revision > 0),
    baseline_manifest_sha256 TEXT NOT NULL CHECK (length(baseline_manifest_sha256) = 64),
    PRIMARY KEY (actor, scope, purpose),
    FOREIGN KEY (version_id, actor, scope, purpose) REFERENCES policy_versions(version_id, actor, scope, purpose)
) WITHOUT ROWID;
CREATE TABLE policy_events (
    actor TEXT NOT NULL,
    scope TEXT NOT NULL,
    purpose TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (typeof(revision) = 'integer' AND revision > 0),
    action TEXT NOT NULL CHECK (action IN ('bootstrap', 'promote', 'rollback')),
    previous_version_id TEXT,
    target_version_id TEXT NOT NULL,
    evaluation_id TEXT,
    manifest_sha256 TEXT NOT NULL CHECK (length(manifest_sha256) = 64),
    reason TEXT NOT NULL CHECK (length(trim(reason)) > 0),
    PRIMARY KEY (actor, scope, purpose, revision),
    FOREIGN KEY (previous_version_id, actor, scope, purpose) REFERENCES policy_versions(version_id, actor, scope, purpose),
    FOREIGN KEY (target_version_id, actor, scope, purpose) REFERENCES policy_versions(version_id, actor, scope, purpose),
    FOREIGN KEY (evaluation_id, actor, scope, purpose) REFERENCES policy_evaluations(evaluation_id, actor, scope, purpose)
) WITHOUT ROWID;

CREATE TRIGGER policy_versions_no_update BEFORE UPDATE ON policy_versions BEGIN SELECT RAISE(ABORT, 'immutable policy version'); END;
CREATE TRIGGER policy_versions_no_delete BEFORE DELETE ON policy_versions BEGIN SELECT RAISE(ABORT, 'immutable policy version'); END;
CREATE TRIGGER policy_evaluations_no_update BEFORE UPDATE ON policy_evaluations BEGIN SELECT RAISE(ABORT, 'immutable policy evaluation'); END;
CREATE TRIGGER policy_evaluations_no_delete BEFORE DELETE ON policy_evaluations BEGIN SELECT RAISE(ABORT, 'immutable policy evaluation'); END;
CREATE TRIGGER policy_events_no_update BEFORE UPDATE ON policy_events BEGIN SELECT RAISE(ABORT, 'immutable policy event'); END;
CREATE TRIGGER policy_events_no_delete BEFORE DELETE ON policy_events BEGIN SELECT RAISE(ABORT, 'immutable policy event'); END;
CREATE TRIGGER policy_heads_initial_revision BEFORE INSERT ON policy_heads WHEN NEW.revision <> 1
BEGIN SELECT RAISE(ABORT, 'policy head must start at revision one'); END;
CREATE TRIGGER policy_heads_monotonic BEFORE UPDATE ON policy_heads
WHEN NEW.actor <> OLD.actor OR NEW.scope <> OLD.scope OR NEW.purpose <> OLD.purpose
    OR OLD.revision = 9223372036854775807 OR NEW.revision <> OLD.revision + 1
BEGIN SELECT RAISE(ABORT, 'policy head revision must advance exactly once'); END;
CREATE TRIGGER policy_heads_no_delete BEFORE DELETE ON policy_heads BEGIN SELECT RAISE(ABORT, 'policy head history cannot reset'); END;
-- SQLite REPLACE deletes conflicting rows without delete triggers unless recursive
-- triggers are enabled. Refuse that path even with the default connection setting.
CREATE TRIGGER policy_versions_no_replace BEFORE INSERT ON policy_versions
WHEN EXISTS (SELECT 1 FROM policy_versions WHERE version_id = NEW.version_id OR evidence_id = NEW.evidence_id)
BEGIN SELECT RAISE(ABORT, 'immutable policy version'); END;
CREATE TRIGGER policy_evaluations_no_replace BEFORE INSERT ON policy_evaluations
WHEN EXISTS (SELECT 1 FROM policy_evaluations WHERE evaluation_id = NEW.evaluation_id)
BEGIN SELECT RAISE(ABORT, 'immutable policy evaluation'); END;
CREATE TRIGGER policy_events_no_replace BEFORE INSERT ON policy_events
WHEN EXISTS (SELECT 1 FROM policy_events WHERE actor = NEW.actor AND scope = NEW.scope AND purpose = NEW.purpose AND revision = NEW.revision)
BEGIN SELECT RAISE(ABORT, 'immutable policy event'); END;
CREATE TRIGGER policy_heads_no_replace BEFORE INSERT ON policy_heads
WHEN EXISTS (SELECT 1 FROM policy_heads WHERE actor = NEW.actor AND scope = NEW.scope AND purpose = NEW.purpose)
BEGIN SELECT RAISE(ABORT, 'policy head history cannot reset'); END;
