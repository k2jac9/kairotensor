use kairotensor::{Current, Disposition, Store};
use rusqlite::{Connection, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

const ACTOR: &str = "reviewer";
const SCOPE: &str = "release";

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!("kairotensor-atomic-{}-{time}.db", std::process::id())))
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn ready(supported: bool) -> Current {
    let bytes: &[u8] = if supported { b"supported" } else { b"unsupported" };
    Current::Ready(bytes.to_vec())
}

fn fixture(store: &mut Store) -> Result<BTreeMap<String, i64>> {
    for source in ["report", "manifest", "rerun"] {
        let id = store.observe(source, b"ok")?;
        store.set_admitted(ACTOR, SCOPE, id, true)?;
    }
    let mut heads = BTreeMap::new();
    for (name, clauses) in [
        ("sole", vec![vec![1]]),
        ("alternative", vec![vec![1, 2], vec![3]]),
        ("joint", vec![vec![1, 2]]),
        ("shared", vec![vec![1, 2], vec![1, 3]]),
    ] {
        heads.insert(name.to_owned(), store.publish_support(ACTOR, SCOPE, name, &clauses)?);
    }
    Ok(heads)
}

#[test]
fn batch_matches_individual_revisions_and_preserves_publication_lineage() -> Result<()> {
    let file = Database::new();
    let mut batch = Store::open(&file.0)?;
    let audit = Connection::open(&file.0)?;
    let mut single = Store::in_memory()?;
    let mut heads = fixture(&mut batch)?;
    let roots = heads.clone();
    let mut single_heads = fixture(&mut single)?;

    for (id, value) in [(1, false), (3, false), (1, true), (2, false), (2, true), (3, true)] {
        single.set_admitted(ACTOR, SCOPE, id, value)?;
        let mut expected = Vec::new();
        for name in single.support_consumers(ACTOR, SCOPE, id)? {
            let head = single.revise_support(ACTOR, SCOPE, &name)?;
            if head != single_heads[&name] {
                expected.push((name.clone(), single.read_current(ACTOR, SCOPE, &name)?));
                single_heads.insert(name, head);
            }
        }
        let before = batch.charged_usage()?;
        let changed = batch.set_admitted_and_revise(ACTOR, SCOPE, id, value)?;
        assert_eq!(changed.len(), expected.len());
        for (receipt, (name, new_value)) in changed.iter().zip(expected) {
            assert_eq!(receipt.branch, name);
            assert_eq!(ready(receipt.supported), new_value);
            let attempt = receipt.attempt_id;
            let (parent, rule, epoch, cost, verdict, revision_advanced): (i64, i64, i64, i64, bool, bool) =
                audit.query_row(
                    "SELECT a.parent, s.rule_id, a.view_epoch, d.usage, s.verdict,
                            b.revision = a.branch_revision + 1
                     FROM attempts a JOIN support_bindings s ON s.attempt_id = a.id
                     JOIN deliveries d ON d.attempt_id = a.id AND d.disposition = 'accepted'
                     JOIN branches b ON b.id = a.branch_id AND b.head = a.id
                     WHERE a.id = ?1 AND b.name = ?2",
                    rusqlite::params![attempt, name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                )?;
            assert_eq!(parent, heads[&name]);
            assert_eq!(rule, roots[&name]);
            assert_eq!(epoch, batch.epoch(ACTOR, SCOPE)?);
            assert_eq!(cost, 1);
            assert_eq!(verdict, receipt.supported);
            assert!(revision_advanced);
            heads.insert(name, attempt);
        }
        assert_eq!(batch.charged_usage()?, before + changed.len() as i64);
        assert_eq!(batch.charged_usage()?, single.charged_usage()?);
        for (name, head) in &heads {
            assert_eq!(batch.read_current(ACTOR, SCOPE, name)?, single.read_current(ACTOR, SCOPE, name)?);
            assert_eq!(batch.revise_support(ACTOR, SCOPE, name)?, *head);
        }
    }
    Ok(())
}

#[test]
fn failing_second_publication_rolls_back_the_entire_edit_then_retry_survives_reopen() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    let audit = Connection::open(&file.0)?;
    let id = store.observe("report", b"ok")?;
    store.set_admitted(ACTOR, SCOPE, id, true)?;
    let alpha = store.publish_support(ACTOR, SCOPE, "alpha", &[vec![id]])?;
    let beta = store.publish_support(ACTOR, SCOPE, "beta", &[vec![id]])?;
    // Fire only after the first new branch head is visible inside this very batch.
    audit.execute_batch(
        "CREATE TRIGGER fail_second BEFORE UPDATE OF head ON branches
         WHEN (SELECT COUNT(*) FROM branches WHERE revision = 2) = 1
         BEGIN SELECT RAISE(ABORT, 'injected second publication failure'); END;",
    )?;
    let error = store.set_admitted_and_revise(ACTOR, SCOPE, id, false).expect_err("batch must abort");
    assert!(error.to_string().contains("injected second publication failure"));
    assert_eq!(store.epoch(ACTOR, SCOPE)?, 1);
    assert_eq!(store.read_evidence(ACTOR, SCOPE, id)?, Some(b"ok".to_vec()));
    assert_eq!(store.charged_usage()?, 2);
    for (name, head) in [("alpha", alpha), ("beta", beta)] {
        assert_eq!(store.read_current(ACTOR, SCOPE, name)?, ready(true));
        assert_eq!(store.revise_support(ACTOR, SCOPE, name)?, head);
    }
    let counts: (i64, i64, i64, i64) = audit.query_row(
        "SELECT (SELECT COUNT(*) FROM attempts), (SELECT COUNT(*) FROM deliveries),
                (SELECT COUNT(*) FROM support_bindings), (SELECT SUM(missing) FROM support_clauses)",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    assert_eq!(counts, (2, 2, 2, 0));
    audit.execute_batch("DROP TRIGGER fail_second")?;
    let committed = store.set_admitted_and_revise(ACTOR, SCOPE, id, false)?;
    assert_eq!(committed.len(), 2);
    drop(store);
    let mut reopened = Store::open(&file.0)?;
    assert!(reopened.set_admitted_and_revise(ACTOR, SCOPE, id, false)?.is_empty());
    assert_eq!(reopened.epoch(ACTOR, SCOPE)?, 2);
    assert_eq!(reopened.charged_usage()?, 4);
    for receipt in committed {
        assert_eq!(reopened.read_current(ACTOR, SCOPE, &receipt.branch)?, ready(false));
        assert_eq!(reopened.revise_support(ACTOR, SCOPE, &receipt.branch)?, receipt.attempt_id);
    }
    Ok(())
}

#[test]
fn effective_edits_only_reconcile_scoped_support_consumers() -> Result<()> {
    let mut store = Store::in_memory()?;
    let a = store.observe("source A", b"A")?;
    let b = store.observe("source B", b"B")?;
    for (actor, scope) in [(ACTOR, SCOPE), ("other", SCOPE), (ACTOR, "other")] {
        for id in [a, b] { store.set_admitted(actor, scope, id, true)?; }
        store.publish_support(actor, scope, "joint", &[vec![a, b]])?;
    }
    let only_a = store.publish_support(ACTOR, SCOPE, "only-a", &[vec![a]])?;
    let (pending, _) = store.begin_resume(ACTOR, SCOPE, "joint", &[])?;
    store.publish_support(ACTOR, SCOPE, "opaque", &[vec![a, b]])?;
    let opaque = store.begin_attempt(ACTOR, SCOPE, "opaque", &[a, b])?;
    assert_eq!(store.settle(opaque, b"supported", 1)?, Disposition::Accepted);

    store.set_admitted(ACTOR, SCOPE, a, false)?;
    let before = store.charged_usage()?;
    assert!(store.set_admitted_and_revise(ACTOR, SCOPE, a, false)?.is_empty());
    assert_eq!(store.read_current(ACTOR, SCOPE, "joint")?, Current::NeedsReview);
    // 'joint' was already stale and is affected by B. 'only-a' stays pending review.
    let changed = store.set_admitted_and_revise(ACTOR, SCOPE, b, false)?;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].branch, "joint");
    assert!(!changed[0].supported);
    assert_eq!(store.charged_usage()?, before + 1);
    assert_eq!(store.read_current(ACTOR, SCOPE, "only-a")?, Current::NeedsReview);
    assert_ne!(store.revise_support(ACTOR, SCOPE, "only-a")?, only_a);
    assert_eq!(store.read_current(ACTOR, SCOPE, "opaque")?, Current::NeedsReview);
    assert!(store.begin_resume(ACTOR, SCOPE, "opaque", &[]).is_err());
    for (actor, scope) in [("other", SCOPE), (ACTOR, "other")] {
        assert_eq!(store.read_current(actor, scope, "joint")?, ready(true));
    }
    assert_eq!(store.settle(pending, b"late", 7)?, Disposition::StaleView);
    assert_eq!(store.settle(pending, b"late", 7)?, Disposition::StaleView);
    assert_eq!(store.charged_usage()?, before + 1 + 1 + 7);
    Ok(())
}

#[test]
fn concurrent_identical_edits_publish_and_charge_each_changed_head_once() -> Result<()> {
    let file = Database::new();
    let mut setup = Store::open(&file.0)?;
    let id = setup.observe("check", b"ok")?;
    setup.set_admitted(ACTOR, SCOPE, id, true)?;
    for name in ["alpha", "beta"] {
        setup.publish_support(ACTOR, SCOPE, name, &[vec![id]])?;
    }
    let writes = [Store::open(&file.0)?, Store::open(&file.0)?];
    let barrier = Arc::new(Barrier::new(writes.len()));
    let workers: Vec<_> = writes.into_iter().map(|mut writer| {
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || -> Result<usize> {
            barrier.wait();
            Ok(writer.set_admitted_and_revise(ACTOR, SCOPE, id, false)?.len())
        })
    }).collect();
    let mut published = Vec::new();
    for worker in workers {
        published.push(worker.join().expect("writer panicked")?);
    }
    published.sort();
    assert_eq!(published, [0, 2]);
    assert_eq!(setup.epoch(ACTOR, SCOPE)?, 2);
    assert_eq!(setup.charged_usage()?, 4);
    for name in ["alpha", "beta"] {
        assert_eq!(setup.read_current(ACTOR, SCOPE, name)?, ready(false));
    }
    Ok(())
}
