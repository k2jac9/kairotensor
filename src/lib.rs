//! Local versioned evidence memory with actor-scoped admission and atomic publication.

mod attempts;
mod support;
pub use attempts::{CapturedInputs, Current, Disposition};
pub use support::{SupportRegistration, SupportRevision};

use rusqlite::{
    ffi, params, Connection, Error, OptionalExtension, Result, Transaction, TransactionBehavior,
};
use std::path::Path;
use std::time::Duration;

// "KTSM", stored in the SQLite header.
const APPLICATION_ID: i32 = 0x4b54_534d;
const SCHEMA_VERSION: i32 = 3;
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

    fn initialize(mut conn: Connection) -> Result<Self> {
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
