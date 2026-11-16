//! Local versioned evidence memory with actor-scoped admission and atomic publication.

mod evidence_digest;
pub mod policy_registry;
pub mod incident_protocol;
pub mod decision_bundle;

mod attempts;
mod eligibility;
pub use eligibility::{CurrentExplanation, EligibilityProof, EligibilityReason, EligibilityState};
mod timed_recovery;
pub use timed_recovery::{RecoveryReadiness, RecoveryRuntime, RecoveryCalibration, RecoveryTiming, RecoveryClock, RecoveryPreparation, TimingRefusal, TimedRecoveryClaim};
mod report_verdict;
mod model_reuse;
mod support;
mod token_budget;
mod recovery;
pub use recovery::{RecoveryReservation, RecoveryStart};
pub use attempts::{CapturedInputs, Current, Disposition};
pub use model_reuse::ModelAction;
pub use support::{AdmissionEdit, BatchRevision, SupportRegistration, SupportRevision};
pub use token_budget::TokenBudgetState;

use rusqlite::{
    ffi, params, Connection, Error, OptionalExtension, Result, Transaction, TransactionBehavior,
};
use std::path::Path;
use std::time::Duration;

// "KTSM", stored in the SQLite header.
const APPLICATION_ID: i32 = 0x4b54_534d;
const SCHEMA_VERSION: i32 = 10;
const INSERT_OBSERVATION: &str = "INSERT INTO evidence(source, payload) VALUES (?1, ?2)";

/// A local trusted runtime owns the database and supplies authenticated actor/scope names.
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Open a Kairotensor database, initializing a new file atomically.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(conn: Connection) -> Result<Self> {
        Self::initialize_inner(
            conn,
            #[cfg(test)]
            None,
        )
    }

    fn initialize_inner(
        mut conn: Connection,
        #[cfg(test)] after_registry_ddl: Option<fn(&Transaction<'_>) -> Result<()>>,
    ) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application: i32 = tx.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match (application, version) {
            (APPLICATION_ID, 1..=SCHEMA_VERSION) => {}
            (0, 0) => {
                let populated: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*')",
                    [],
                    |row| row.get(0),
                )?;
                if populated {
                    return Err(invalid(
                        "refusing to initialize a nonempty unrecognized database",
                    ));
                }
                tx.execute_batch(include_str!("schema.sql"))?;
                tx.execute_batch(include_str!("attempts.sql"))?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            }
            _ => return Err(invalid("unrecognized database or unsupported schema version")),
        }
        if version < 2 {
            tx.execute_batch(include_str!("support.sql"))?;
        }
        if version < 3 {
            tx.execute_batch(include_str!("dispatch.sql"))?;
        }
        if version < 4 {
            tx.execute_batch(include_str!("model_reuse.sql"))?;
        }
        if version < 5 {
            tx.execute_batch(include_str!("validation.sql"))?;
        }
        if version < 6 {
            tx.execute_batch(include_str!("token_budget.sql"))?;
        }
        if version < 7 {
            tx.execute_batch(include_str!("recovery.sql"))?;
        }
        if version < 8 {
            tx.execute_batch(include_str!("report_verdict.sql"))?;
        }
        if version < 9 {
            tx.execute_batch(include_str!("timed_recovery.sql"))?;
        }
        if version < 10 {
            tx.execute_batch(include_str!("policy_registry.sql"))?;
            #[cfg(test)]
            if let Some(hook) = after_registry_ddl {
                hook(&tx)?;
            }
        }
        if version < SCHEMA_VERSION {
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Record one acquisition. Repeated acquisitions, even of the same bytes, get new IDs.
    pub fn observe(&self, source: &str, payload: &[u8]) -> Result<i64> {
        self.conn.prepare_cached(INSERT_OBSERVATION)?.insert(params![source, payload])
    }

    /// Acquire and admit fresh observations for one actor and scope in one transaction.
    /// Returned IDs and admission events follow input order. Repeating a successful
    /// call records new occurrences, including for identical sources and payloads.
    /// Support indexes are updated immediately; changed heads can be revised separately.
    pub fn observe_and_admit_many(
        &mut self,
        actor: &str,
        scope: &str,
        observations: &[(&str, &[u8])],
    ) -> Result<Vec<i64>> {
        validate_view(actor, scope)?;
        if observations.is_empty() {
            return Ok(Vec::new());
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut epoch = view_epoch(&tx, actor, scope)?;
        let mut ids = Vec::with_capacity(observations.len());
        {
            let mut insert = tx.prepare_cached(INSERT_OBSERVATION)?;
            for &(source, payload) in observations {
                let id = insert.insert(params![source, payload])?;
                // A fresh occurrence has no earlier admission in any view.
                epoch = append_admission(&tx, actor, scope, id, true, epoch)?;
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Admit existing occurrences atomically in input order. Only effective grants
    /// advance the view epoch. An unknown ID aborts the entire batch. As with
    /// `set_admitted`, readers of a changed support verdict must revise its head.
    pub fn admit_many(
        &mut self,
        actor: &str,
        scope: &str,
        evidence_ids: &[i64],
    ) -> Result<()> {
        validate_view(actor, scope)?;
        if evidence_ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for &id in evidence_ids {
            let _ = change_admission(&tx, actor, scope, id, true)?;
        }
        tx.commit()
    }

    /// Change an observation's visibility for exactly one actor and scope.
    pub fn set_admitted(
        &mut self,
        actor: &str,
        scope: &str,
        evidence: i64,
        admitted: bool,
    ) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let _ = change_admission(&tx, actor, scope, evidence, admitted)?;
        tx.commit()
    }

    /// Return the current actor/scope epoch. An empty view starts at zero.
    pub fn epoch(&self, actor: &str, scope: &str) -> Result<i64> {
        view_epoch(&self.conn, actor, scope)
    }

    /// Read an observation only when it is admitted at this statement's snapshot.
    pub fn read_evidence(
        &self,
        actor: &str,
        scope: &str,
        evidence: i64,
    ) -> Result<Option<Vec<u8>>> {
        self.conn
            .query_row(
                "SELECT payload FROM evidence WHERE id = ?3 AND (
                    SELECT admitted FROM admission_events
                    WHERE actor = ?1 AND scope = ?2 AND evidence_id = ?3
                    ORDER BY epoch DESC LIMIT 1
                 ) = 1",
                params![actor, scope, evidence],
                |row| row.get(0),
            )
            .optional()
    }
}

fn validate_view(actor: &str, scope: &str) -> Result<()> {
    if actor.is_empty() || scope.is_empty() {
        return Err(invalid("actor and scope must be nonempty"));
    }
    Ok(())
}

/// Apply an effective membership change and return its epoch to the writer.
/// Repeating the current membership leaves the journal and projections alone.
fn change_admission(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    evidence: i64,
    admitted: bool,
) -> Result<Option<i64>> {
    validate_view(actor, scope)?;
    tx.query_row(
        "SELECT 1 FROM evidence WHERE id = ?1",
        [evidence],
        |_| Ok(()),
    )?;
    if is_admitted(tx, actor, scope, evidence)? == admitted {
        return Ok(None);
    }
    let previous = view_epoch(tx, actor, scope)?;
    append_admission(tx, actor, scope, evidence, admitted, previous).map(Some)
}

/// Record a known state transition. The caller owns the writer lock and the
/// previous epoch of this view.
fn append_admission(
    tx: &Transaction<'_>,
    actor: &str,
    scope: &str,
    evidence: i64,
    admitted: bool,
    previous: i64,
) -> Result<i64> {
    let epoch = previous
        .checked_add(1)
        .ok_or_else(|| invalid("view epoch exhausted"))?;
    tx.prepare_cached(
        "INSERT INTO admission_events(actor, scope, epoch, evidence_id, admitted)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?.execute(params![actor, scope, epoch, evidence, admitted])?;
    support::on_admission(tx, actor, scope, evidence, admitted)?;
    Ok(epoch)
}

pub(crate) fn view_epoch(db: &Connection, actor: &str, scope: &str) -> Result<i64> {
    db.query_row(
        "SELECT COALESCE(MAX(epoch), 0) FROM admission_events WHERE actor = ?1 AND scope = ?2",
        params![actor, scope],
        |row| row.get(0),
    )
}

pub(crate) fn is_admitted(db: &Connection, actor: &str, scope: &str, evidence: i64) -> Result<bool> {
    let latest = db
        .query_row(
            "SELECT admitted FROM admission_events WHERE actor = ?1 AND scope = ?2
             AND evidence_id = ?3 ORDER BY epoch DESC LIMIT 1",
            params![actor, scope, evidence],
            |row| row.get(0),
        )
        .optional()?;
    Ok(latest.unwrap_or(false))
}

pub(crate) fn invalid(message: &str) -> Error {
    Error::SqliteFailure(
        ffi::Error::new(ffi::SQLITE_CONSTRAINT),
        Some(message.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn identical_bytes_preserve_distinct_occurrences() -> Result<()> {
        let mut store = Store::in_memory()?;
        let first = store.observe("sensor-a@3", &[0, 0xff])?;
        let second = store.observe("sensor-a@3", &[0, 0xff])?;
        assert_ne!(first, second);

        store.set_admitted("reviewer", "lab", first, true)?;
        assert_eq!(
            store.read_evidence("reviewer", "lab", first)?,
            Some(vec![0, 0xff])
        );
        assert_eq!(store.read_evidence("reviewer", "lab", second)?, None);
        Ok(())
    }

    #[test]
    fn epochs_track_effective_changes_for_each_actor_and_scope() -> Result<()> {
        let mut store = Store::in_memory()?;
        let evidence = store.observe("source@1", b"input")?;
        assert_eq!(store.epoch("a", "first")?, 0);
        store.set_admitted("a", "first", evidence, false)?;
        store.set_admitted("a", "first", evidence, true)?;
        store.set_admitted("a", "first", evidence, true)?;
        assert_eq!(store.epoch("a", "first")?, 1);
        store.set_admitted("b", "first", evidence, true)?;
        store.set_admitted("a", "first", evidence, false)?;
        assert_eq!(store.epoch("a", "first")?, 2);
        assert_eq!(store.epoch("b", "first")?, 1);
        assert_eq!(store.epoch("a", "other")?, 0);
        assert_eq!(store.read_evidence("a", "first", evidence)?, None);
        assert_eq!(
            store.read_evidence("b", "first", evidence)?,
            Some(b"input".to_vec())
        );

        store.set_admitted("a", "first", evidence, true)?;
        let mut events = store.conn.prepare(
            "SELECT admitted FROM admission_events
             WHERE actor = 'a' AND scope = 'first' ORDER BY epoch",
        )?;
        let history = events
            .query_map([], |row| row.get::<_, bool>(0))?
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(history, vec![true, false, true]);
        Ok(())
    }

    #[test]
    fn unknown_evidence_cannot_change_a_view() -> Result<()> {
        let mut store = Store::in_memory()?;
        for admitted in [true, false] {
            assert!(store.set_admitted("a", "scope", 99, admitted).is_err());
        }
        assert_eq!(store.epoch("a", "scope")?, 0);
        Ok(())
    }

    #[test]
    fn reopen_preserves_evidence_and_its_admission() -> Result<()> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kairotensor-{}-{stamp}.sqlite",
            std::process::id()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        drop(file);
        let mut writer = Store::open(&path)?;
        let evidence = writer.observe("source@4", b"revision")?;
        writer.set_admitted("a", "scope", evidence, true)?;
        drop(writer);
        let reader = Store::open(&path)?;
        assert_eq!(reader.epoch("a", "scope")?, 1);
        assert_eq!(
            reader.read_evidence("a", "scope", evidence)?,
            Some(b"revision".to_vec())
        );
        drop(reader);
        std::fs::remove_file(path).unwrap();
        Ok(())
    }

    #[test]
    fn migrate_v1_preserves_opaque_heads_pending_work_and_charges() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(include_str!("schema.sql"))?;
        conn.execute_batch(include_str!("attempts.sql"))?;
        conn.pragma_update(None, "application_id", APPLICATION_ID)?;
        conn.pragma_update(None, "user_version", 1)?;
        // Populate the exact v1 layout. The old deliveries have no proof metadata,
        // even when their bytes happen to match a built-in rule's output.
        conn.execute_batch(
            "INSERT INTO evidence VALUES (1, 'release-log', X'01');
             INSERT INTO admission_events VALUES ('reviewer', 'release', 1, 1, 1);
             INSERT INTO branches (id, actor, scope, name) VALUES (1, 'reviewer', 'release', 'old');
             INSERT INTO attempts VALUES (1, 1, NULL, 0, 1, NULL);
             INSERT INTO attempt_inputs VALUES (1, 0, 1);
             INSERT INTO deliveries VALUES (1, 'accepted', CAST('supported' AS BLOB), 3);
             UPDATE branches SET head = 1, revision = 1 WHERE id = 1;
             INSERT INTO attempts VALUES (2, 1, 1, 1, 1, 1);",
        )?;
        let mut store = Store::initialize(conn)?;
        assert_eq!(store.read_current("reviewer", "release", "old")?,
                   Current::Ready(b"supported".to_vec()));
        assert_eq!(store.charged_usage()?, 3);
        store.set_admitted("reviewer", "release", 1, false)?;
        assert_eq!(store.read_current("reviewer", "release", "old")?, Current::NeedsReview);
        assert!(store.revise_support("reviewer", "release", "old").is_err());
        assert_eq!(store.settle(2, b"late", 4)?, Disposition::StaleView);
        assert_eq!(store.settle(2, b"late", 4)?, Disposition::StaleView);
        assert_eq!(store.charged_usage()?, 7);
        // The same migrated file can immediately register a new guarded branch.
        store.publish_support("reviewer", "release", "new", &[vec![1]])?;
        assert_eq!(store.read_current("reviewer", "release", "new")?,
                   Current::Ready(b"unsupported".to_vec()));
        let reopened = Store::initialize(store.conn)?;
        let version: i32 = reopened.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        Ok(())
    }

    #[test]
    fn refuses_unknown_schema_versions_and_foreign_databases() -> Result<()> {
        let sqlite = Connection::open_in_memory()?;
        sqlite.pragma_update(None, "application_id", APPLICATION_ID)?;
        sqlite.pragma_update(None, "user_version", SCHEMA_VERSION + 1)?;
        assert!(Store::initialize(sqlite).is_err());
        let unrelated = Connection::open_in_memory()?;
        unrelated.execute("CREATE TABLE unrelated(x INTEGER)", [])?;
        assert!(Store::initialize(unrelated).is_err());
        Ok(())
    }
}

mod recovery_runtime;
pub use recovery_runtime::{ReplyValidation, MeasuredRecoveryReply, RecoveryRun};

mod boot_clock;
pub use boot_clock::LinuxBootClock;

#[cfg(test)]
mod registry_migration_tests {
    use super::*;
    use std::path::PathBuf;

    fn packet() -> PathBuf {
        let root = std::env::var_os("KAIROTENSOR_D2_ARTIFACTS")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = root.join(format!("registry-v9-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    // Build the actual historical layout, never a v10 file with a relabelled header.
    fn populated_v9(path: &Path) -> Result<()> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        for sql in [
            include_str!("schema.sql"),
            include_str!("attempts.sql"),
            include_str!("support.sql"),
            include_str!("dispatch.sql"),
            include_str!("model_reuse.sql"),
            include_str!("validation.sql"),
            include_str!("token_budget.sql"),
            include_str!("recovery.sql"),
            include_str!("report_verdict.sql"),
            include_str!("timed_recovery.sql"),
        ] {
            conn.execute_batch(sql)?;
        }
        conn.pragma_update(None, "application_id", APPLICATION_ID)?;
        conn.pragma_update(None, "user_version", 9)?;
        // Existing Store APIs populate the old schema without initializing/migrating it.
        let mut store = Store { conn };
        let evidence = store.observe("v9-fixture", b"old exact evidence\0\xff")?;
        store.set_admitted("actor", "scope", evidence, true)?;
        let budget = store.create_token_budget(100)?;
        for (key, disposition) in [
            ("accepted", Some(Disposition::Accepted)),
            ("stale", Some(Disposition::StaleView)),
            ("unresolved", None),
        ] {
            let run = store.create_recovery(key, budget, "actor", "scope", key)?;
            let attempt = match store.begin_recovery_attempt(run, &[evidence])? {
                RecoveryStart::New { attempt, .. } => attempt,
                other => panic!("{other:?}"),
            };
            assert!(store.reserve_recovery_attempt(attempt, b"v9 exact request\0\xff", 4, 8)?);
            assert_eq!(
                store.claim_recovery_dispatch(attempt)?,
                Some(b"v9 exact request\0\xff".to_vec())
            );
            if disposition == Some(Disposition::StaleView) {
                store.set_admitted("actor", "scope", evidence, false)?;
            }
            if let Some(expected) = disposition {
                assert_eq!(
                    store.settle(attempt, b"v9 exact output\0\xff", 7)?,
                    expected
                );
            }
        }
        assert_eq!(store.charged_usage()?, 14);
        assert_eq!(
            store.token_budget_state(budget)?,
            TokenBudgetState {
                limit: 100,
                spent: 14,
                reserved: 12
            }
        );
        assert_eq!(store.epoch("actor", "scope")?, 2);
        Ok(())
    }
    // Every old table, column, row and schema object, including dispatch receipts/holds.
    fn snapshot(conn: &Connection) -> Result<Vec<(String, Vec<Vec<rusqlite::types::Value>>)>> {
        let mut names = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'policy_%' ORDER BY name")?;
        let tables = names
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>>>()?;
        let mut result = Vec::new();
        for table in tables {
            let mut stmt = conn.prepare(&format!(
                "SELECT * FROM \"{}\" ORDER BY {}",
                table.replace('"', "\"\""),
                (1..=conn
                    .prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))?
                    .column_count())
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ))?;
            let cols = stmt.column_count();
            let rows = stmt
                .query_map([], |r| (0..cols).map(|i| r.get(i)).collect())?
                .collect::<Result<Vec<_>>>()?;
            result.push((table, rows));
        }
        let mut stmt = conn.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'policy_%' AND tbl_name NOT LIKE 'policy_%' ORDER BY type,name")?;
        result.push((
            "schema".into(),
            stmt.query_map([], |r| (0..4).map(|i| r.get(i)).collect())?
                .collect::<Result<Vec<_>>>()?,
        ));
        Ok(result)
    }
    #[test]
    fn populated_v9_migration_reopen_and_backup_restore_preserve_every_old_row() -> Result<()> {
        let packet = packet();
        let backup = packet.join("original-v9.sqlite");
        populated_v9(&backup)?;
        let bytes = std::fs::read(&backup).unwrap();
        let hash = crate::evidence_digest::sha256_hex(&bytes);
        std::fs::write(packet.join("original-v9.sha256"), &hash).unwrap();
        let before = snapshot(&Connection::open(&backup)?)?;
        std::fs::write(packet.join("old-state.txt"), format!("{before:#?}")).unwrap();
        for name in ["migrated.sqlite", "restored-from-backup.sqlite"] {
            let copy = packet.join(name);
            std::fs::copy(&backup, &copy).unwrap();
            for _ in 0..2 {
                let store = Store::open(&copy)?;
                assert_eq!(
                    store
                        .conn
                        .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))?,
                    10
                );
                assert_eq!(snapshot(&store.conn)?, before);
                assert_eq!(store.charged_usage()?, 14);
                assert_eq!(
                    store.token_budget_state(1)?,
                    TokenBudgetState {
                        limit: 100,
                        spent: 14,
                        reserved: 12
                    }
                );
                assert_eq!(store.epoch("actor", "scope")?, 2);
                assert_eq!(
                    store
                        .conn
                        .query_row("SELECT COUNT(*) FROM policy_versions", [], |r| r
                            .get::<_, i64>(0))?,
                    0
                );
                assert_eq!(
                    store.conn.query_row(
                        "SELECT COUNT(*) FROM pragma_foreign_key_check",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
            }
        }
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        eprintln!(
            "D2 v9 backup/restore evidence: {} sha256={hash}",
            packet.display()
        );
        Ok(())
    }
    #[test]
    fn interrupted_registry_migration_rolls_back_ddl_and_preserves_v9_backup() -> Result<()> {
        let packet = packet();
        let backup = packet.join("original-v9.sqlite");
        populated_v9(&backup)?;
        let bytes = std::fs::read(&backup).unwrap();
        let hash = crate::evidence_digest::sha256_hex(&bytes);
        std::fs::write(packet.join("original-v9.sha256"), &hash).unwrap();
        let copy = packet.join("interrupted.sqlite");
        std::fs::copy(&backup, &copy).unwrap();
        let before = snapshot(&Connection::open(&copy)?)?;
        let failure = Store::initialize_inner(
            Connection::open(&copy)?,
            Some(|tx| {
                assert_eq!(
                    tx.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))?,
                    9
                );
                assert_eq!(tx.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name LIKE 'policy_%'", [], |r|r.get::<_,i64>(0))?, 4);
                Err(invalid(
                    "injected after registry DDL before migration commit",
                ))
            }),
        );
        assert!(failure.is_err());
        let old = Connection::open(&copy)?;
        assert_eq!(
            old.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))?,
            9
        );
        assert_eq!(snapshot(&old)?, before);
        assert_eq!(
            old.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'policy_%'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        drop(old);
        assert_eq!(std::fs::read(&copy).unwrap(), bytes);
        let reopened = Store::open(&copy)?;
        assert_eq!(snapshot(&reopened.conn)?, before);
        assert_eq!(std::fs::read(&backup).unwrap(), bytes);
        eprintln!(
            "D2 interrupted migration evidence: {} sha256={hash}",
            packet.display()
        );
        Ok(())
    }
}
