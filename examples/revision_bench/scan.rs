//! Single-writer, full-recomputation comparator for a fresh benchmark database.
//! Each edit batches its membership change and all changed verdicts in one commit.
use rusqlite::{params, Connection, Result, TransactionBehavior};
use std::path::Path;
use std::time::{Duration, Instant};

use super::Registration;

pub struct Scan {
    db: Connection,
    evidence_count: usize,
    rules: Vec<Vec<Vec<usize>>>,
    answers: Vec<bool>,
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
        Some(message.to_owned()),
    )
}

// Membership is loaded once per snapshot. The rule catalogue has no reverse index.
fn admissions(db: &Connection, count: usize) -> Result<Vec<bool>> {
    let mut active = vec![false; count + 1]; // Occurrence IDs start at one.
    let mut query = db.prepare_cached("SELECT id FROM evidence WHERE admitted = 1")?;
    let ids = query.query_map([], |row| row.get::<_, i64>(0))?;
    for id in ids {
        active[id? as usize] = true;
    }
    Ok(active)
}

fn evaluate(rule: &[Vec<usize>], active: &[bool]) -> bool {
    rule.iter().any(|clause| {
        clause
            .iter()
            .all(|&id| active.get(id).copied().unwrap_or(false))
    })
}

impl Scan {
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.pragma_update(None, "foreign_keys", true)?;
        db.pragma_update(None, "synchronous", "FULL")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE evidence (
                 id INTEGER PRIMARY KEY,
                 source TEXT NOT NULL,
                 payload BLOB NOT NULL,
                 admitted INTEGER NOT NULL CHECK (admitted IN (0, 1))
             );
             CREATE TABLE admissions (
                 epoch INTEGER PRIMARY KEY,
                 evidence_id INTEGER NOT NULL REFERENCES evidence(id),
                 admitted INTEGER NOT NULL CHECK (admitted IN (0, 1))
             );
             CREATE TABLE rules (id INTEGER PRIMARY KEY);
             CREATE TABLE premises (
                 rule INTEGER NOT NULL REFERENCES rules(id),
                 clause INTEGER NOT NULL,
                 requested_id INTEGER NOT NULL CHECK (requested_id > 0),
                 PRIMARY KEY (rule, clause, requested_id)
             ) WITHOUT ROWID;
             CREATE TABLE verdicts (
                 rule INTEGER NOT NULL REFERENCES rules(id),
                 epoch INTEGER NOT NULL,
                 supported INTEGER NOT NULL CHECK (supported IN (0, 1)),
                 PRIMARY KEY (rule, epoch)
             ) WITHOUT ROWID;",
        )?;
        tx.commit()?;
        Ok(Self {
            db,
            evidence_count: 0,
            rules: Vec::new(),
            answers: Vec::new(),
        })
    }
}

impl super::Backend for Scan {
    fn seed(&mut self, evidence_count: usize) -> Result<()> {
        let count = i64::try_from(evidence_count).map_err(|_| invalid("too many observations"))?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut evidence = tx.prepare(
                "INSERT INTO evidence(id, source, payload, admitted) VALUES (?1, 'fixture', X'', 1)",
            )?;
            let mut event = tx.prepare(
                "INSERT INTO admissions(evidence_id, admitted) VALUES (?1, 1)",
            )?;
            for id in 1..=count {
                evidence.execute([id])?;
                event.execute([id])?;
            }
        }
        tx.commit()?;
        self.evidence_count = evidence_count;
        Ok(())
    }

    fn register(&mut self, rules: &[Vec<Vec<i64>>]) -> Result<Registration> {
        let start = Instant::now();
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active = admissions(&tx, self.evidence_count)?;
        let epoch: i64 = tx.query_row(
            "SELECT COALESCE(MAX(epoch), 0) FROM admissions",
            [],
            |row| row.get(0),
        )?;
        let mut catalog = Vec::with_capacity(rules.len());
        let mut answers = Vec::with_capacity(rules.len());
        {
            let mut new_rule = tx.prepare("INSERT INTO rules(id) VALUES (?1)")?;
            let mut premise = tx.prepare(
                "INSERT INTO premises(rule, clause, requested_id) VALUES (?1, ?2, ?3)",
            )?;
            let mut verdict = tx.prepare(
                "INSERT INTO verdicts(rule, epoch, supported) VALUES (?1, ?2, ?3)",
            )?;
            for (index, rule) in rules.iter().enumerate() {
                if rule.is_empty() || rule.iter().any(Vec::is_empty) {
                    return Err(invalid("every rule requires at least one nonempty clause"));
                }
                let rule_id = index as i64;
                new_rule.execute([rule_id])?;
                let mut cached = Vec::with_capacity(rule.len());
                for (c, clause) in rule.iter().enumerate() {
                    let mut ids = Vec::with_capacity(clause.len());
                    for &id in clause {
                        // The table rejects duplicate IDs within a clause and nonpositive IDs.
                        premise.execute(params![rule_id, c as i64, id])?;
                        ids.push(usize::try_from(id).map_err(|_| invalid("unsupported ID"))?);
                    }
                    cached.push(ids);
                }
                let supported = evaluate(&cached, &active);
                verdict.execute(params![rule_id, epoch, supported])?;
                catalog.push(cached);
                answers.push(supported);
            }
        }
        tx.commit()?;
        let publish = start.elapsed();
        self.rules = catalog;
        self.answers = answers.clone();
        // This arm returns the verdicts it just computed and persisted. Its separate
        // full-table read is confined to the existing untimed verification replay.
        Ok(Registration { initial: answers, publish, initial_read: Duration::ZERO })
    }

    fn edit(&mut self, id: i64, admitted: bool) -> Result<Vec<(usize, bool)>> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: bool = tx.query_row(
            "SELECT admitted FROM evidence WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        if old == admitted {
            tx.commit()?;
            return Ok(Vec::new());
        }
        tx.execute("UPDATE evidence SET admitted = ?2 WHERE id = ?1", params![id, admitted])?;
        tx.execute(
            "INSERT INTO admissions(evidence_id, admitted) VALUES (?1, ?2)",
            params![id, admitted],
        )?;
        let epoch = tx.last_insert_rowid();
        let active = admissions(&tx, self.evidence_count)?;
        let mut changes = Vec::new();
        {
            let mut verdict = tx.prepare_cached(
                "INSERT INTO verdicts(rule, epoch, supported) VALUES (?1, ?2, ?3)",
            )?;
            for (index, rule) in self.rules.iter().enumerate() {
                let supported = evaluate(rule, &active);
                if supported != self.answers[index] {
                    verdict.execute(params![index as i64, epoch, supported])?;
                    changes.push((index, supported));
                }
            }
        }
        tx.commit()?;
        for &(index, answer) in &changes {
            self.answers[index] = answer;
        }
        Ok(changes)
    }

    /// Check the complete *persisted* result for the untimed verification replay.
    fn read_all(&mut self) -> Result<Vec<bool>> {
        let mut query = self.db.prepare(
            "SELECT v.supported FROM verdicts v
             JOIN (SELECT rule, MAX(epoch) AS epoch FROM verdicts GROUP BY rule) latest
             USING (rule, epoch) ORDER BY rule",
        )?;
        let values = query.query_map([], |row| row.get(0))?;
        values.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;

    #[test]
    fn full_scan_persists_only_changed_verdicts_and_admissions() -> Result<()> {
        let mut scan = Scan::open(Path::new(":memory:"))?;
        scan.seed(3)?;
        let rules = vec![vec![vec![1, 2], vec![3]], vec![vec![1]], vec![vec![9]]];
        let registration = scan.register(&rules)?;
        assert_eq!(registration.initial, vec![true, true, false]);
        assert_eq!(registration.initial_read, Duration::ZERO);
        assert_eq!(scan.edit(1, false)?, vec![(1, false)]);
        assert!(scan.edit(1, false)?.is_empty());
        assert_eq!(scan.read_all()?, vec![true, false, false]);
        assert_eq!(scan.edit(3, false)?, vec![(0, false)]);
        assert_eq!(scan.edit(1, true)?, vec![(0, true), (1, true)]);
        assert_eq!(scan.read_all()?, vec![true, true, false]);
        let counts: (i64, i64) = scan.db.query_row(
            "SELECT (SELECT COUNT(*) FROM admissions), (SELECT COUNT(*) FROM verdicts)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(counts, (6, 7));
        Ok(())
    }
}
