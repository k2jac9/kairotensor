use kairotensor::{Current, Disposition, Store, SupportRegistration};
use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const ACTOR: &str = "reviewer";
const SCOPE: &str = "release";

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let salt = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        let path = std::env::temp_dir().join(format!(
            "kairotensor-bulk-{}-{}-{}.db", std::process::id(), salt.as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self(path)
    }
}

impl Drop for Database {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

fn registration<'a>(branch: &'a str, alternatives: &'a [Vec<i64>]) -> SupportRegistration<'a> {
    SupportRegistration { branch, alternatives }
}

#[test]
fn ordered_acquisitions_and_grants_keep_occurrence_and_epoch_history() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let raw = store.observe("old", b"original")?;
    let items: &[(&str, &[u8])] = &[("same source", &[0, 255]), ("same source", &[0, 255])];
    let ids = store.observe_and_admit_many(ACTOR, SCOPE, items)?;
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(store.read_evidence(ACTOR, SCOPE, ids[0])?, Some(vec![0, 255]));
    assert_eq!(store.read_evidence("other", SCOPE, ids[1])?, None);
    assert_eq!(store.read_evidence(ACTOR, "other", ids[1])?, None);
    store.admit_many(ACTOR, SCOPE, &[ids[1], raw, raw, ids[0]])?;
    store.admit_many(ACTOR, SCOPE, &[raw])?;
    assert!(store.observe_and_admit_many(ACTOR, SCOPE, &[])?.is_empty());
    store.admit_many(ACTOR, SCOPE, &[])?;
    assert!(store.publish_support_many(ACTOR, SCOPE, &[])?.is_empty());
    assert_eq!(store.epoch(ACTOR, SCOPE)?, 3);
    assert_eq!(store.charged_usage()?, 0);
    drop(store);

    let db = Connection::open(&file.0)?;
    let mut events = db.prepare(
        "SELECT epoch, evidence_id, admitted FROM admission_events ORDER BY epoch"
    )?;
    let history = events.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?))
    })?.collect::<Result<Vec<_>>>()?;
    assert_eq!(history, vec![(1, ids[0], true), (2, ids[1], true), (3, raw, true)]);
    let mut restarted = Store::open(&file.0)?;
    let more = restarted.observe_and_admit_many(ACTOR, SCOPE, items)?;
    assert!(more.iter().all(|id| !ids.contains(id)));
    assert_eq!(restarted.epoch(ACTOR, SCOPE)?, 5);
    Ok(())
}

#[test]
fn failed_acquisitions_and_grants_restore_edges_epochs_and_pending_work() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let audit = Connection::open(&file.0)?;
    let first = store.observe("staged", b"prior")?;
    let future = first + 1;
    let old = store.publish_support(ACTOR, SCOPE, "gate", &[vec![first], vec![future]])?;
    let pending = store.begin_attempt(ACTOR, SCOPE, "worker", &[])?;
    // Firing on the later INSERT requires the previous admission and watch
    // update to already be visible inside the acquisition transaction.
    audit.execute_batch(
        "CREATE TRIGGER reject_later BEFORE INSERT ON evidence
         WHEN NEW.source = 'fail' AND EXISTS (SELECT 1 FROM support_clauses WHERE missing = 0)
         BEGIN SELECT RAISE(ABORT, 'injected acquisition failure'); END;"
    )?;
    let entries: &[(&str, &[u8])] = &[("new", b"first"), ("fail", b"second")];
    let error = store.observe_and_admit_many(ACTOR, SCOPE, entries).unwrap_err();
    assert!(error.to_string().contains("injected acquisition failure"));
    assert!(store.admit_many(ACTOR, SCOPE, &[first, future + 100]).is_err());
    assert_eq!(store.epoch(ACTOR, SCOPE)?, 0);
    assert_eq!(store.read_evidence(ACTOR, SCOPE, first)?, None);
    assert_eq!(store.revise_support(ACTOR, SCOPE, "gate")?, old);
    assert_eq!(store.read_current(ACTOR, SCOPE, "gate")?, Current::Ready(b"unsupported".to_vec()));
    let state: (i64, i64, i64) = audit.query_row(
        "SELECT (SELECT COUNT(*) FROM evidence), (SELECT COUNT(*) FROM admission_events),
                (SELECT SUM(missing) FROM support_clauses)",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    )?;
    assert_eq!(state, (1, 0, 2));
    assert_eq!(store.settle(pending, b"still valid", 5)?, Disposition::Accepted);

    audit.execute_batch("DROP TRIGGER reject_later")?;
    let captured = store.begin_attempt(ACTOR, SCOPE, "worker", &[])?;
    let ids = store.observe_and_admit_many(ACTOR, SCOPE, &entries[..1])?;
    assert_eq!(ids, vec![future]);
    assert_eq!(store.read_current(ACTOR, SCOPE, "gate")?, Current::NeedsReview);
    assert_eq!(store.settle(captured, b"late", 3)?, Disposition::StaleView);
    let renewed = store.revise_support(ACTOR, SCOPE, "gate")?;
    assert_ne!(renewed, old);
    store.admit_many(ACTOR, SCOPE, &[first, first])?;
    assert_eq!(store.revise_support(ACTOR, SCOPE, "gate")?, renewed);
    assert_eq!(store.epoch(ACTOR, SCOPE)?, 2);
    Ok(())
}

#[test]
fn ordered_replacements_match_single_publications_and_survive_restart() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let mut single = Store::in_memory()?;
    for db in [&mut store, &mut single] {
        let id = db.observe("report", b"original")?;
        db.set_admitted(ACTOR, SCOPE, id, true)?;
        let opaque = db.begin_attempt(ACTOR, SCOPE, "gate", &[id])?;
        assert_eq!(db.settle(opaque, b"opaque", 4)?, Disposition::Accepted);
    }

    let with_missing = [vec![1, 9]];
    let visible = [vec![1]];
    let declarations = [
        registration("gate", &with_missing),
        registration("other", &with_missing),
        registration("gate", &visible),
    ];
    let expected: Vec<_> = declarations.iter().map(|r| {
        single.publish_support(ACTOR, SCOPE, r.branch, r.alternatives)
    }).collect::<Result<_>>()?;
    let heads = store.publish_support_many(ACTOR, SCOPE, &declarations)?;
    assert_eq!(heads, expected);
    assert_eq!(store.charged_usage()?, single.charged_usage()?);
    assert_eq!(store.charged_usage()?, 9);
    drop(store);

    let mut reopened = Store::open(&file.0)?;
    assert_eq!(reopened.revise_support(ACTOR, SCOPE, "gate")?, heads[2]);
    assert_eq!(reopened.revise_support(ACTOR, SCOPE, "other")?, heads[1]);
    assert_eq!(reopened.support_consumers(ACTOR, SCOPE, 9)?, vec!["other"]);
    assert_eq!(reopened.read_current(ACTOR, SCOPE, "gate")?, Current::Ready(b"supported".to_vec()));
    assert_eq!(reopened.read_current("other", SCOPE, "gate")?, Current::Empty);
    let audit = Connection::open(&file.0)?;
    let lineage: (i64, i64, i64, i64, i64) = audit.query_row(
        "SELECT a.parent, a.branch_revision, a.view_epoch, s.rule_id, d.usage FROM attempts a
         JOIN support_bindings s ON s.attempt_id = a.id JOIN deliveries d ON d.attempt_id = a.id
         WHERE a.id = ?1 AND d.disposition = 'accepted'",
        [heads[2]], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    )?;
    assert_eq!(lineage, (heads[0], 2, 1, heads[2], 1));
    Ok(())
}

#[test]
fn a_late_invalid_registration_rolls_back_retired_watches_heads_and_charges() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let id = store.observe("witness", b"ok")?;
    store.set_admitted(ACTOR, SCOPE, id, true)?;
    let original = store.publish_support(ACTOR, SCOPE, "gate", &[vec![id]])?;
    let replacement = [vec![id + 10]];
    let second_replacement = [vec![id + 20]];
    let malformed = [vec![id, id]];
    let rules = [
        registration("gate", &replacement),
        registration("gate", &second_replacement),
        registration("new", &malformed),
    ];
    assert!(store.publish_support_many(ACTOR, SCOPE, &rules).is_err());
    assert_eq!(store.revise_support(ACTOR, SCOPE, "gate")?, original);
    assert_eq!(store.read_current(ACTOR, SCOPE, "new")?, Current::Empty);
    assert_eq!(store.support_consumers(ACTOR, SCOPE, id)?, vec!["gate"]);
    assert!(store.support_consumers(ACTOR, SCOPE, id + 10)?.is_empty());
    assert!(store.support_consumers(ACTOR, SCOPE, id + 20)?.is_empty());
    assert_eq!(store.charged_usage()?, 1);
    let audit = Connection::open(&file.0)?;
    let counts: (i64, i64, i64, i64, i64) = audit.query_row(
        "SELECT (SELECT COUNT(*) FROM attempts), (SELECT COUNT(*) FROM deliveries),
                (SELECT COUNT(*) FROM support_bindings), (SELECT COUNT(*) FROM support_clauses),
                (SELECT COUNT(*) FROM branches)",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    )?;
    assert_eq!(counts, (1, 1, 1, 1, 1));
    let committed = store.publish_support_many(ACTOR, SCOPE, &rules[..1])?;
    assert_eq!(committed.len(), 1);
    assert_ne!(committed[0], original);
    assert!(store.support_consumers(ACTOR, SCOPE, id)?.is_empty());
    Ok(())
}
