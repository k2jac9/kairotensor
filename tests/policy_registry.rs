use kairotensor::policy_registry::*;
use kairotensor::Store;
use rusqlite::{params, Connection, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "policy-registry-{}-{stamp}.sqlite",
            std::process::id()
        )))
    }
    fn conn(&self) -> Result<Connection> {
        let c = Connection::open(&self.0)?;
        c.pragma_update(None, "foreign_keys", true)?;
        Ok(c)
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn candidate(id: &str) -> PolicyCandidate {
    let artifact = br#"{"schema_version":1,"policy_name":"always_stop","purpose":"fixture/recovery-policy","compatibility_contract_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#.to_vec();
    PolicyCandidate {
        version_id: id.into(),
        actor: "actor".into(),
        scope: "scope".into(),
        purpose: "fixture/recovery-policy".into(),
        sha256: sha(&artifact),
        artifact,
    }
}
fn fixture() -> PolicyEvaluation {
    let output = std::process::Command::new("python3").args(["-c", "import sys,json;sys.path.insert(0,'scripts');from test_policy_evaluation_v2 import fixture;print(json.dumps(fixture()))"]).output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
fn register_pair(store: &mut Store, e: &PolicyEvaluation) -> Result<()> {
    let m: ComparisonManifest = serde_json::from_slice(&e.experiment_manifest).unwrap();
    for (id, digest) in [
        (&e.baseline_id, &e.baseline_sha256),
        (&e.candidate_id, &e.candidate_sha256),
    ] {
        let artifact = m
            .policies
            .iter()
            .find(|p| &p.artifact.sha256 == digest)
            .unwrap()
            .artifact
            .bytes
            .clone();
        store.register_policy_candidate(&PolicyCandidate {
            version_id: id.clone(),
            actor: e.actor.clone(),
            scope: e.scope.clone(),
            purpose: e.purpose.clone(),
            artifact,
            sha256: digest.clone(),
        })?;
    }
    Ok(())
}
fn count(c: &Connection, table: &str) -> Result<i64> {
    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
}
fn inactive(c: &Connection) -> Result<()> {
    for table in [
        "policy_heads",
        "policy_events",
        "admission_events",
        "attempts",
        "dispatches",
        "token_reservations",
    ] {
        assert_eq!(count(c, table)?, 0, "{table}");
    }
    Ok(())
}
#[test]
fn registration_is_durable_exact_idempotent_and_allocates_distinct_unadmitted_occurrences(
) -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let c = candidate("v1");
    s.register_policy_candidate(&c)?;
    s.register_policy_candidate(&c)?;
    drop(s);
    let mut s = Store::open(&db.0)?;
    s.register_policy_candidate(&c)?;
    let mut other = c.clone();
    other.version_id = "v2".into();
    s.register_policy_candidate(&other)?;
    let sql = db.conn()?;
    assert_eq!(count(&sql, "evidence")?, 2);
    assert_eq!(count(&sql, "policy_versions")?, 2);
    let mut stmt=sql.prepare("SELECT e.id,e.payload FROM policy_versions v JOIN evidence e ON e.id=v.evidence_id ORDER BY v.version_id")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?
        .collect::<Result<Vec<_>>>()?;
    assert_ne!(rows[0].0, rows[1].0);
    assert_eq!(rows[0].1, c.artifact);
    assert_eq!(rows[1].1, c.artifact);
    assert_eq!(s.read_evidence("actor", "scope", rows[0].0)?, None);
    assert_eq!(s.epoch("actor", "scope")?, 0);
    inactive(&sql)
}
#[test]
fn conflicting_registration_and_invalid_artifacts_leave_no_occurrences() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let c = candidate("v1");
    s.register_policy_candidate(&c)?;
    for field in ["actor", "scope", "purpose", "artifact", "sha256"] {
        let mut changed = c.clone();
        match field {
            "actor" => changed.actor = "other".into(),
            "scope" => changed.scope = "other".into(),
            "purpose" => changed.purpose = "other".into(),
            "artifact" => {
                changed.artifact.push(b' ');
                changed.sha256 = sha(&changed.artifact);
            }
            _ => changed.sha256 = "0".repeat(64),
        }
        assert!(s.register_policy_candidate(&changed).is_err(), "{field}");
    }
    for artifact in [b"{}".to_vec(), vec![b' '; MAX_ARTIFACT_BYTES + 1]] {
        let mut changed = candidate("new");
        changed.artifact = artifact;
        changed.sha256 = sha(&changed.artifact);
        assert!(s.register_policy_candidate(&changed).is_err());
    }
    let sql = db.conn()?;
    assert_eq!(count(&sql, "evidence")?, 1);
    assert_eq!(count(&sql, "policy_versions")?, 1);
    inactive(&sql)
}
#[test]
fn failed_version_insert_rolls_back_fresh_evidence() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let sql = db.conn()?;
    sql.execute_batch("CREATE TRIGGER fail_version BEFORE INSERT ON policy_versions BEGIN SELECT RAISE(ABORT,'injected constraint'); END;")?;
    assert!(s.register_policy_candidate(&candidate("v1")).is_err());
    assert_eq!(count(&sql, "evidence")?, 0);
    inactive(&sql)
}
#[test]
fn evaluations_bind_exact_registered_versions_and_preserve_manifest_and_rows() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let e = fixture();
    assert!(s.record_policy_evaluation(&e).is_err());
    register_pair(&mut s, &e)?;
    s.record_policy_evaluation(&e)?;
    s.record_policy_evaluation(&e)?;
    let sql = db.conn()?;
    let (raw,manifest,b,c):(Vec<u8>,Vec<u8>,Vec<u8>,Vec<u8>)=sql.query_row("SELECT evaluation_bytes,experiment_manifest,baseline_rows,candidate_rows FROM policy_evaluations",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    assert_eq!(parse_policy_evaluation(&raw)?, e);
    assert_eq!(manifest, e.experiment_manifest);
    assert_eq!(
        serde_json::from_slice::<Vec<PolicyRow>>(&b).unwrap(),
        e.baseline_rows
    );
    assert_eq!(
        serde_json::from_slice::<Vec<PolicyRow>>(&c).unwrap(),
        e.candidate_rows
    );
    drop(s);
    let mut s = Store::open(&db.0)?;
    s.record_policy_evaluation(&e)?;
    for field in [
        "actor",
        "scope",
        "purpose",
        "digest",
        "rows",
        "manifest",
        "candidate_id",
    ] {
        let mut bad = e.clone();
        match field {
            "actor" => bad.actor = "other".into(),
            "scope" => bad.scope = "other".into(),
            "purpose" => bad.purpose = "other".into(),
            "digest" => bad.candidate_sha256 = "0".repeat(64),
            "rows" => bad.candidate_rows[0].charged = Some(5),
            "manifest" => {
                bad.experiment_manifest.push(b' ');
                bad.experiment_sha256 = sha(&bad.experiment_manifest);
            }
            _ => bad.candidate_id = "missing".into(),
        }
        assert!(s.record_policy_evaluation(&bad).is_err(), "{field}");
    }
    assert_eq!(count(&sql, "policy_evaluations")?, 1);
    inactive(&sql)
}
#[test]
fn baseline_only_and_incomplete_evaluations_are_evidence_without_activation() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let e = fixture();
    register_pair(&mut s, &e)?;
    let mut baseline = e.clone();
    baseline.evaluation_id = "baseline".into();
    baseline.candidate_id = baseline.baseline_id.clone();
    baseline.candidate_sha256 = baseline.baseline_sha256.clone();
    baseline.candidate_rows = baseline.baseline_rows.clone();
    assert_eq!(
        evaluate_policy_comparison(&baseline)?,
        SelectionDecision::Refused(SelectionRefusal::NoImprovement)
    );
    s.record_policy_evaluation(&baseline)?;
    let mut bad = baseline.clone();
    bad.evaluation_id = "bad-baseline".into();
    bad.candidate_rows[0].charged = Some(5);
    assert!(s.record_policy_evaluation(&bad).is_err());
    let mut incomplete = e.clone();
    incomplete.evaluation_id = "incomplete".into();
    incomplete.candidate_rows[0].charged = None;
    assert_eq!(
        evaluate_policy_comparison(&incomplete)?,
        SelectionDecision::Refused(SelectionRefusal::Incomplete)
    );
    s.record_policy_evaluation(&incomplete)?;
    let mut malformed = e.clone();
    malformed.evaluation_id = "malformed".into();
    malformed.candidate_rows[0].false_refusal = true;
    assert!(s.record_policy_evaluation(&malformed).is_err());
    let mut unknown = e.clone();
    unknown.evaluation_id = "unknown-evaluator".into();
    unknown.evaluator_version = "future/2".into();
    s.record_policy_evaluation(&unknown)?;
    assert_eq!(count(&db.conn()?, "policy_evaluations")?, 3);
    inactive(&db.conn()?)
}
#[test]
fn immutable_registry_rows_and_monotonic_head_constraints() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let e = fixture();
    register_pair(&mut s, &e)?;
    s.record_policy_evaluation(&e)?;
    let sql = db.conn()?;
    for table in ["policy_versions", "policy_evaluations"] {
        assert!(sql
            .execute(&format!("UPDATE {table} SET actor='other'"), [])
            .is_err());
        assert!(sql.execute(&format!("DELETE FROM {table}"), []).is_err());
    }
    // Direct SQL here tests schema constraints only, never an activation API or admission.
    sql.execute(
        "INSERT INTO policy_heads VALUES ('actor','scope','fixture/recovery-policy','v1',1,?1)",
        [&e.experiment_sha256],
    )?;
    assert!(sql
        .execute("UPDATE policy_heads SET revision=1", [])
        .is_err());
    assert!(sql
        .execute("UPDATE policy_heads SET revision=3", [])
        .is_err());
    assert!(sql.execute("DELETE FROM policy_heads", []).is_err());
    sql.execute("INSERT INTO policy_events(actor,scope,purpose,revision,action,previous_version_id,target_version_id,evaluation_id,manifest_sha256,reason) VALUES ('actor','scope','fixture/recovery-policy',1,'bootstrap',NULL,'v1','e1',?1,'fixture only')",[&e.experiment_sha256])?;
    assert!(sql
        .execute("UPDATE policy_events SET reason='changed'", [])
        .is_err());
    assert!(sql.execute("DELETE FROM policy_events", []).is_err());
    assert!(sql
        .execute("INSERT INTO policy_events SELECT * FROM policy_events", [])
        .is_err());
    assert_eq!(count(&sql, "admission_events")?, 0);
    Ok(())
}
#[test]
fn existing_artifact_payload_is_bounded_and_verified_before_duplicate_acceptance() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let c = candidate("v1");
    s.register_policy_candidate(&c)?;
    let sql = db.conn()?;
    for payload in [b"corrupt".to_vec(), vec![0; MAX_ARTIFACT_BYTES + 1]] {
        sql.execute("UPDATE evidence SET payload=?1", params![payload])?;
        assert!(s.register_policy_candidate(&c).is_err());
    }
    sql.execute("UPDATE evidence SET payload='wrong-type'", [])?;
    assert!(s.register_policy_candidate(&c).is_err());
    assert_eq!(count(&sql, "evidence")?, 1);
    inactive(&sql)
}

#[test]
fn unknown_evaluator_is_retained_unchanged_and_never_relabelled_as_known() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let mut e = fixture();
    register_pair(&mut s, &e)?;
    e.evaluator_version = "unknown/99".into();
    e.evaluator_sha256 = "b".repeat(64);
    // Unsupported assertions are explicitly unevaluated, not interpreted by a cloned selector.
    e.candidate_rows[0].false_refusal = true;
    assert_eq!(
        evaluate_policy_comparison(&e)?,
        SelectionDecision::Refused(SelectionRefusal::Incomparable)
    );
    s.record_policy_evaluation(&e)?;
    let raw: Vec<u8> =
        db.conn()?
            .query_row("SELECT evaluation_bytes FROM policy_evaluations", [], |r| {
                r.get(0)
            })?;
    assert_eq!(raw, serde_json::to_vec(&e).unwrap());
    let persisted = parse_policy_evaluation(&raw)?;
    assert_eq!(
        evaluate_policy_comparison(&persisted)?,
        SelectionDecision::Refused(SelectionRefusal::Incomparable)
    );
    for kind in ["shape", "manifest", "identity"] {
        let mut bad = e.clone();
        bad.evaluation_id = format!("invalid-{kind}");
        match kind {
            "shape" => bad.candidate_rows[0].case_id = "".into(),
            "manifest" => bad.experiment_manifest = b"{}".to_vec(),
            _ => bad.scope = "other".into(),
        }
        assert!(s.record_policy_evaluation(&bad).is_err(), "{kind}");
    }
    assert_eq!(count(&db.conn()?, "policy_evaluations")?, 1);
    inactive(&db.conn()?)
}
#[test]
fn corrupted_persisted_evaluation_is_not_accepted_as_identical() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let e = fixture();
    register_pair(&mut s, &e)?;
    s.record_policy_evaluation(&e)?;
    let sql = db.conn()?;
    // Simulate host corruption outside the ordinary immutable-write boundary.
    sql.execute_batch(
        "DROP TRIGGER policy_evaluations_no_update; PRAGMA ignore_check_constraints=ON;",
    )?;
    for (column, original) in [
        ("evaluation_bytes", serde_json::to_vec(&e).unwrap()),
        ("experiment_manifest", e.experiment_manifest.clone()),
        (
            "baseline_rows",
            serde_json::to_vec(&e.baseline_rows).unwrap(),
        ),
        (
            "candidate_rows",
            serde_json::to_vec(&e.candidate_rows).unwrap(),
        ),
    ] {
        for expression in ["X'00'", "'text'", "zeroblob(16777217)"] {
            sql.execute(
                &format!("UPDATE policy_evaluations SET {column}={expression}"),
                [],
            )?;
            assert!(
                s.record_policy_evaluation(&e).is_err(),
                "{column} {expression}"
            );
        }
        sql.execute(
            &format!("UPDATE policy_evaluations SET {column}=?1"),
            [original],
        )?;
    }
    sql.execute(
        "UPDATE policy_evaluations SET evaluation_sha256=?1",
        ["0".repeat(64)],
    )?;
    assert!(s.record_policy_evaluation(&e).is_err());
    assert_eq!(count(&sql, "policy_evaluations")?, 1);
    inactive(&sql)
}
#[test]
fn sqlite_replace_cannot_rewrite_immutable_registry_or_reset_head_history() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let e = fixture();
    register_pair(&mut s, &e)?;
    s.record_policy_evaluation(&e)?;
    let sql = db.conn()?;
    for table in ["policy_versions", "policy_evaluations"] {
        assert!(
            sql.execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
                []
            )
            .is_err(),
            "{table}"
        );
    }
    sql.execute(
        "INSERT INTO policy_heads VALUES ('actor','scope','fixture/recovery-policy','v1',1,?1)",
        [&e.experiment_sha256],
    )?;
    sql.execute("UPDATE policy_heads SET revision=2", [])?;
    assert!(sql.execute("INSERT OR REPLACE INTO policy_heads VALUES ('actor','scope','fixture/recovery-policy','v1',1,?1)",[&e.experiment_sha256]).is_err());
    sql.execute("INSERT INTO policy_events VALUES ('actor','scope','fixture/recovery-policy',1,'bootstrap',NULL,'v1','e1',?1,'fixture only')",[&e.experiment_sha256])?;
    assert!(sql
        .execute(
            "INSERT OR REPLACE INTO policy_events SELECT * FROM policy_events",
            []
        )
        .is_err());
    Ok(())
}
#[test]
fn concurrent_identical_registrations_allocate_only_one_occurrence() -> Result<()> {
    let db = Database::new();
    drop(Store::open(&db.0)?);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers = (0..2)
        .map(|_| {
            let path = db.0.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || -> Result<()> {
                let mut s = Store::open(path)?;
                barrier.wait();
                s.register_policy_candidate(&candidate("same"))
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap()?;
    }
    assert_eq!(count(&db.conn()?, "policy_versions")?, 1);
    assert_eq!(count(&db.conn()?, "evidence")?, 1);
    inactive(&db.conn()?)
}

#[test]
fn evaluation_cannot_borrow_same_artifact_from_another_scope() -> Result<()> {
    let db = Database::new();
    let mut s = Store::open(&db.0)?;
    let mut e = fixture();
    register_pair(&mut s, &e)?;
    let manifest: ComparisonManifest = serde_json::from_slice(&e.experiment_manifest).unwrap();
    let frozen = manifest
        .policies
        .iter()
        .find(|p| p.artifact.sha256 == e.candidate_sha256)
        .unwrap();
    let foreign = PolicyCandidate {
        version_id: "foreign-version".into(),
        actor: e.actor.clone(),
        scope: "other-scope".into(),
        purpose: e.purpose.clone(),
        artifact: frozen.artifact.bytes.clone(),
        sha256: frozen.artifact.sha256.clone(),
    };
    s.register_policy_candidate(&foreign)?;
    e.candidate_id = foreign.version_id;
    // The pure evaluator cannot see registration scope; the Store must bind it.
    assert!(matches!(
        evaluate_policy_comparison(&e)?,
        SelectionDecision::Eligible { .. }
    ));
    assert!(s.record_policy_evaluation(&e).is_err());
    assert_eq!(count(&db.conn()?, "policy_evaluations")?, 0);
    inactive(&db.conn()?)
}

fn all_table_rows(conn: &Connection) -> Result<Vec<(String, Vec<Vec<rusqlite::types::Value>>)>> {
    let mut names =
        conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?;
    let tables = names
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    let mut snapshot = Vec::new();
    for table in tables {
        let quoted = format!("\"{}\"", table.replace('"', "\"\""));
        let columns = conn
            .prepare(&format!("SELECT * FROM {quoted}"))?
            .column_count();
        let order = (1..=columns)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut query = conn.prepare(&format!("SELECT * FROM {quoted} ORDER BY {order}"))?;
        let rows = query
            .query_map([], |r| (0..columns).map(|n| r.get(n)).collect())?
            .collect::<Result<Vec<_>>>()?;
        snapshot.push((table, rows));
    }
    Ok(snapshot)
}

fn rowid_replacement_refused(table: &str, alias: &str) -> Result<()> {
    let db = Database::new();
    let mut store = Store::open(&db.0)?;
    let sql = db.conn()?;
    sql.pragma_update(None, "recursive_triggers", false)?;
    assert!(sql.pragma_query_value(None, "foreign_keys", |r| r.get::<_, bool>(0))?);
    assert!(!sql.pragma_query_value(None, "recursive_triggers", |r| r.get::<_, bool>(0))?);
    let original = candidate("original");
    let evaluation = fixture();
    let statement = if table == "policy_versions" {
        store.register_policy_candidate(&original)?;
        // A valid fresh evidence ID avoids the declared evidence uniqueness guard.
        let fresh_evidence = store.observe("rowid-regression", &original.artifact)?;
        format!("INSERT OR REPLACE INTO policy_versions({alias},version_id,actor,scope,purpose,evidence_id,compatibility_contract_sha256,artifact_sha256)
            SELECT 1,'fresh-version',actor,scope,purpose,{fresh_evidence},compatibility_contract_sha256,artifact_sha256 FROM policy_versions WHERE version_id='original'")
    } else {
        register_pair(&mut store, &evaluation)?;
        store.record_policy_evaluation(&evaluation)?;
        format!("INSERT OR REPLACE INTO policy_evaluations({alias},evaluation_id,actor,scope,purpose,baseline_id,candidate_id,baseline_sha256,candidate_sha256,experiment_manifest,experiment_sha256,evaluator_version,evaluator_sha256,baseline_rows,candidate_rows,evaluation_bytes,evaluation_sha256)
            SELECT 1,'fresh-evaluation',actor,scope,purpose,baseline_id,candidate_id,baseline_sha256,candidate_sha256,experiment_manifest,experiment_sha256,evaluator_version,evaluator_sha256,baseline_rows,candidate_rows,evaluation_bytes,evaluation_sha256 FROM policy_evaluations WHERE evaluation_id='e1'")
    };
    assert_eq!(count(&sql, "pragma_foreign_key_check")?, 0);
    let before = all_table_rows(&sql)?;
    let result = sql.execute(&statement, []);
    assert!(
        result.is_err(),
        "{table} replacement through {alias} succeeded: {result:?}"
    );
    assert_eq!(
        all_table_rows(&sql)?,
        before,
        "{table} {alias} altered database rows"
    );
    assert_eq!(count(&sql, "pragma_foreign_key_check")?, 0);
    // The old immutable record remains usable through the ordinary exact retry API.
    if table == "policy_versions" {
        store.register_policy_candidate(&original)?;
    } else {
        store.record_policy_evaluation(&evaluation)?;
    }
    assert_eq!(all_table_rows(&sql)?, before);
    drop(store);
    drop(sql);
    let _reopened = Store::open(&db.0)?;
    assert_eq!(all_table_rows(&db.conn()?)?, before);
    assert_eq!(count(&db.conn()?, "pragma_foreign_key_check")?, 0);
    Ok(())
}

macro_rules! rowid_replacement_test {
    ($name:ident, $table:literal, $alias:literal) => {
        #[test]
        fn $name() -> Result<()> {
            rowid_replacement_refused($table, $alias)
        }
    };
}
rowid_replacement_test!(
    version_explicit_rowid_replace_preserves_history,
    "policy_versions",
    "rowid"
);
rowid_replacement_test!(
    version_explicit_underscore_rowid_replace_preserves_history,
    "policy_versions",
    "_rowid_"
);
rowid_replacement_test!(
    version_explicit_oid_replace_preserves_history,
    "policy_versions",
    "oid"
);
rowid_replacement_test!(
    evaluation_explicit_rowid_replace_preserves_history,
    "policy_evaluations",
    "rowid"
);
rowid_replacement_test!(
    evaluation_explicit_underscore_rowid_replace_preserves_history,
    "policy_evaluations",
    "_rowid_"
);
rowid_replacement_test!(
    evaluation_explicit_oid_replace_preserves_history,
    "policy_evaluations",
    "oid"
);
