//! Local versioned evidence memory with actor-scoped admission and atomic publication.

mod attempts;
pub use attempts::{CapturedInputs, Current, Disposition};

use rusqlite::{ffi, params, Connection, Error, OptionalExtension, Result, TransactionBehavior};
use std::path::Path;
use std::time::Duration;

// "KTSM", stored in the SQLite header.
const APPLICATION_ID: i32 = 0x4b54_534d;
const SCHEMA_VERSION: i32 = 1;

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
            (APPLICATION_ID, SCHEMA_VERSION) => {}
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
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            _ => return Err(invalid("unrecognized database or unsupported schema version")),
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Record one acquisition. Repeated acquisitions, even of the same bytes, get new IDs.
    pub fn observe(&self, source: &str, payload: &[u8]) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO evidence(source, payload) VALUES (?1, ?2)",
            params![source, payload],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Change an observation's visibility for exactly one actor and scope.
    pub fn set_admitted(
        &mut self,
        actor: &str,
        scope: &str,
        evidence: i64,
        admitted: bool,
    ) -> Result<()> {
        if actor.is_empty() || scope.is_empty() {
            return Err(invalid("actor and scope must be nonempty"));
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.query_row(
            "SELECT 1 FROM evidence WHERE id = ?1",
            [evidence],
            |_| Ok(()),
        )?;
        if is_admitted(&tx, actor, scope, evidence)? != admitted {
            let epoch = view_epoch(&tx, actor, scope)?
                .checked_add(1)
                .ok_or_else(|| invalid("view epoch exhausted"))?;
            tx.execute(
                "INSERT INTO admission_events(actor, scope, epoch, evidence_id, admitted)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![actor, scope, epoch, evidence, admitted],
            )?;
        }
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
