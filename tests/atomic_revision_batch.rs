use kairotensor::{AdmissionEdit, BatchRevision, Current, Disposition, Store, SupportRevision};
use rusqlite::{types::Value, Connection, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

const A: &str = "reviewer";
const S: &str = "release";
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!("kairotensor-batch-{}-{stamp}.db", std::process::id())))
    }
}
impl Drop for Database {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}
fn edit(evidence: i64, admitted: bool) -> AdmissionEdit { AdmissionEdit { evidence, admitted } }
fn ready(value: bool) -> Current {
    Current::Ready(if value { b"supported".to_vec() } else { b"unsupported".to_vec() })
}
fn applied(result: BatchRevision, expected: i64) -> Vec<SupportRevision> {
    match result {
        BatchRevision::Applied { epoch, changes } => { assert_eq!(epoch, expected); changes }
        other => panic!("expected Applied at {expected}, got {other:?}"),
    }
}
// Include every persistent table, table boundaries and every column, including
// history, indexes, charges, dispatches and unresolved budget reservations.
fn snapshot(db: &Connection) -> Result<BTreeMap<String, Vec<Vec<Value>>>> {
    let tables = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?
        .query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>>>()?;
    let mut result = BTreeMap::new();
    for table in tables {
        let mut stmt = db.prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))?;
        let columns = stmt.column_count();
        let mut rows = stmt.query_map([], |r| (0..columns).map(|i| r.get(i)).collect())?
            .collect::<Result<Vec<Vec<Value>>>>()?;
        rows.sort_by_key(|row| format!("{row:?}"));
        result.insert(table, rows);
    }
    Ok(result)
}
fn two(store: &mut Store) -> Result<()> {
    store.observe_and_admit_many(A, S, &[("one", b"1"), ("two", b"2")])?;
    Ok(())
}

#[test]
fn replacement_has_no_intermediate_verdict() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    two(&mut store)?;
    store.set_admitted(A, S, 2, false)?;
    let head = store.publish_support(A, S, "either", &[vec![1], vec![2]])?;
    let audit = Connection::open(&file.0)?;
    let charges = store.charged_usage()?;
    assert!(applied(store.revise_admissions_if_current(A, S, 3, &[edit(1, false), edit(2, true)])?, 5).is_empty());
    assert_eq!(store.revise_support(A, S, "either")?, head);
    assert_eq!(store.charged_usage()?, charges);
    assert_eq!(store.read_current(A, S, "either")?, ready(true));
    let events = audit.prepare("SELECT evidence_id, admitted FROM admission_events WHERE epoch > 3 ORDER BY epoch")?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)))?.collect::<Result<Vec<_>>>()?;
    assert_eq!(events, [(1, false), (2, true)]);
    Ok(())
}

#[test]
fn overlapping_consumers_publish_once_at_final_epoch() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    two(&mut store)?;
    let z = store.publish_support(A, S, "z", &[vec![1, 2], vec![1]])?;
    let a = store.publish_support(A, S, "a", &[vec![1, 2]])?;
    let before = store.charged_usage()?;
    let changes = applied(store.revise_admissions_if_current(A, S, 2, &[edit(2, false), edit(1, false)])?, 4);
    assert_eq!(changes.iter().map(|c| c.branch.as_str()).collect::<Vec<_>>(), ["a", "z"]);
    assert_eq!(store.charged_usage()?, before + 2);
    assert!(changes[0].attempt_id < changes[1].attempt_id);
    let audit = Connection::open(&file.0)?;
    for (change, root) in changes.iter().zip([a, z]) {
        let row: (i64, i64, i64, i64) = audit.query_row(
            "SELECT a.parent, a.view_epoch, s.rule_id, d.usage FROM attempts a JOIN support_bindings s ON s.attempt_id=a.id JOIN deliveries d ON d.attempt_id=a.id WHERE a.id=?1",
            [change.attempt_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        assert_eq!(row, (root, 4, root, 1));
        assert!(!change.supported);
    }
    Ok(())
}

#[test]
fn duplicate_edit_is_rejected() -> Result<()> {
    let mut store = Store::in_memory()?;
    two(&mut store)?;
    for values in [[false, false], [false, true]] {
        assert!(store.revise_admissions_if_current(A, S, 2, &[edit(1, values[0]), edit(1, values[1])]).is_err());
    }
    assert_eq!(store.epoch(A, S)?, 2);
    Ok(())
}

#[test]
fn stale_empty_and_noop_batches_conflict() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    two(&mut store)?;
    let audit = Connection::open(&file.0)?;
    let before = snapshot(&audit)?;
    for edits in [vec![], vec![edit(1, true)], vec![edit(9999, false)]] {
        assert_eq!(store.revise_admissions_if_current(A, S, 0, &edits)?, BatchRevision::EpochConflict { current_epoch: 2 });
    }
    for edits in [vec![], vec![edit(1, true), edit(2, true)]] {
        assert!(applied(store.revise_admissions_if_current(A, S, 2, &edits)?, 2).is_empty());
    }
    assert_eq!(snapshot(&audit)?, before);
    Ok(())
}

#[test]
fn noop_leaves_stale_consumers_alone() -> Result<()> {
    let mut store = Store::in_memory()?;
    two(&mut store)?;
    store.publish_support(A, S, "stale", &[vec![1]])?;
    store.publish_support(A, S, "changed", &[vec![2]])?;
    store.set_admitted(A, S, 1, false)?;
    let charge = store.charged_usage()?;
    let changes = applied(store.revise_admissions_if_current(A, S, 3, &[edit(1, false), edit(2, false)])?, 4);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].branch, "changed");
    assert_eq!(store.read_current(A, S, "stale")?, Current::NeedsReview);
    assert_eq!(store.charged_usage()?, charge + 1);
    Ok(())
}

#[test]
fn request_shape_and_epoch_precedence_are_stable() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    two(&mut store)?;
    let audit = Connection::open(&file.0)?;
    let before = snapshot(&audit)?;
    for (actor, scope, epoch, edits) in [
        ("", S, 0, vec![]), (A, "", 0, vec![]), (A, S, -1, vec![]),
        (A, S, 0, vec![edit(0, true)]), (A, S, 0, vec![edit(-2, true)]),
        (A, S, 0, vec![edit(1, false), edit(1, true)]),
        (A, S, 0, (1..=1025).map(|id| edit(id, true)).collect()),
    ] { assert!(store.revise_admissions_if_current(actor, scope, epoch, &edits).is_err()); }
    let boundary: Vec<_> = (1..=1024).map(|id| edit(id, true)).collect();
    assert_eq!(store.revise_admissions_if_current(A, S, 0, &boundary)?, BatchRevision::EpochConflict { current_epoch: 2 });
    // An abort trigger on the first edit distinguishes full prevalidation from
    // rollback following a late unknown-ID lookup.
    audit.execute_batch("CREATE TRIGGER reject_write BEFORE INSERT ON admission_events BEGIN SELECT RAISE(ABORT, 'mutation before existence validation'); END;")?;
    let error = store.revise_admissions_if_current(A, S, 2, &[edit(1, false), edit(9999, true)]).unwrap_err();
    assert!(matches!(error, rusqlite::Error::QueryReturnedNoRows));
    assert_eq!(snapshot(&audit)?, before);
    audit.execute_batch("DROP TRIGGER reject_write")?;
    for id in 3..=1024 { assert_eq!(store.observe("boundary", b"x")?, id); }
    let result = applied(store.revise_admissions_if_current(A, S, 2, &boundary)?, 1024);
    assert!(result.is_empty());
    Ok(())
}

fn rollback_after(trigger: &str, expected_error: &str) -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    two(&mut store)?;
    for name in ["a", "z"] { store.publish_support(A, S, name, &[vec![1, 2]])?; }
    let settled = store.begin_attempt(A, S, "opaque", &[1, 2])?;
    let budget = store.create_token_budget(100)?;
    store.reserve_dispatch(budget, settled, b"accepted request", 2, 8)?;
    assert_eq!(store.settle(settled, b"answer", 7)?, Disposition::Accepted);
    let pending = store.begin_attempt(A, S, "unresolved", &[])?;
    store.reserve_dispatch(budget, pending, b"unknown send", 4, 10)?;
    let audit = Connection::open(&file.0)?;
    audit.execute_batch(trigger)?;
    let before = snapshot(&audit)?;
    let held = store.token_budget_state(budget)?;
    assert_eq!((held.spent, held.reserved), (7, 14));
    let error = store.revise_admissions_if_current(A, S, 2, &[edit(1, false), edit(2, false)]).unwrap_err();
    assert!(error.to_string().contains(expected_error), "{error}");
    assert_eq!(snapshot(&audit)?, before);
    assert_eq!(store.token_budget_state(budget)?, held);
    drop(store);
    let mut store = Store::open(&file.0)?;
    assert_eq!(snapshot(&audit)?, before);
    assert_eq!(store.epoch(A, S)?, 2);
    assert_eq!(store.token_budget_state(budget)?, held);
    audit.execute_batch("DROP TRIGGER fail_batch")?;
    assert_eq!(applied(store.revise_admissions_if_current(A, S, 2, &[edit(1, false), edit(2, false)])?, 4).len(), 2);
    assert_eq!(store.token_budget_state(budget)?, held);
    Ok(())
}

#[test]
fn second_edit_failure_rolls_back_every_write() -> Result<()> {
    rollback_after("CREATE TRIGGER fail_batch BEFORE INSERT ON admission_events WHEN NEW.epoch=4 AND EXISTS(SELECT 1 FROM admission_events WHERE epoch=3 AND evidence_id=1 AND admitted=0) BEGIN SELECT RAISE(ABORT, 'second effective edit'); END;", "second effective edit")
}
#[test]
fn later_publication_failure_rolls_back_every_write() -> Result<()> {
    rollback_after("CREATE TRIGGER fail_batch BEFORE UPDATE OF head ON branches WHEN NEW.name='z' AND EXISTS(SELECT 1 FROM branches WHERE name='a' AND revision=2) BEGIN SELECT RAISE(ABORT, 'later publication'); END;", "later publication")
}

#[test]
fn concurrent_correction_has_one_winner() -> Result<()> {
    let file = Database::new();
    let mut store = Store::open(&file.0)?;
    two(&mut store)?;
    for name in ["a", "z"] { store.publish_support(A, S, name, &[vec![1, 2]])?; }
    let initial_charge = store.charged_usage()?;
    let connections = [Store::open(&file.0)?, Store::open(&file.0)?];
    let start = Arc::new(Barrier::new(3));
    let workers: Vec<_> = connections.into_iter().map(|mut writer| {
        let start = Arc::clone(&start);
        std::thread::spawn(move || { start.wait(); writer.revise_admissions_if_current(A, S, 2, &[edit(1, false), edit(2, false)]) })
    }).collect();
    start.wait();
    let mut wins = 0;
    let mut conflicts = 0;
    for worker in workers {
        match worker.join().unwrap()? {
            BatchRevision::Applied { epoch, changes } => { assert_eq!(epoch, 4); assert_eq!(changes.len(), 2); wins += 1; }
            BatchRevision::EpochConflict { current_epoch } => { assert_eq!(current_epoch, 4); conflicts += 1; }
        }
    }
    assert_eq!((wins, conflicts), (1, 1));
    assert_eq!(store.charged_usage()?, initial_charge + 2);
    applied(store.revise_admissions_if_current(A, S, 4, &[edit(1, true), edit(2, true)])?, 6);
    let audit = Connection::open(&file.0)?;
    let before = snapshot(&audit)?;
    assert_eq!(store.revise_admissions_if_current(A, S, 2, &[edit(1, false), edit(2, false)])?, BatchRevision::EpochConflict { current_epoch: 6 });
    assert_eq!(snapshot(&audit)?, before);
    assert_eq!(store.charged_usage()?, initial_charge + 4);
    Ok(())
}

fn rules() -> Vec<(&'static str, Vec<Vec<i64>>)> {
    vec![("a", vec![vec![1, 2], vec![3, 4]]), ("b", vec![vec![1], vec![2, 3]]), ("c", vec![vec![1, 2, 3, 4]]), ("d", vec![vec![2], vec![4]])]
}
fn fixture(store: &mut Store, mask: u8) -> Result<BTreeMap<String, i64>> {
    for id in 1..=4 {
        assert_eq!(store.observe("occurrence", b"same payload")?, id);
        store.set_admitted(A, S, id, mask & (1 << (id-1)) != 0)?;
    }
    rules().into_iter().map(|(name, groups)| Ok((name.to_owned(), store.publish_support(A, S, name, &groups)?))).collect()
}
fn verdict(groups: &[Vec<i64>], mask: u8) -> bool {
    groups.iter().any(|g| g.iter().all(|id| mask & (1 << (id-1)) != 0))
}
fn compare_transition(store: &mut Store, audit: &Connection, heads: &mut BTreeMap<String, i64>, roots: &BTreeMap<String, i64>, from: u8, to: u8, reverse: bool) -> Result<()> {
    let mut reference = Store::in_memory()?;
    fixture(&mut reference, to)?;
    let mut edits: Vec<_> = (1..=4).map(|id| edit(id, to & (1 << (id-1)) != 0)).collect();
    if reverse { edits.reverse(); }
    let before_epoch = store.epoch(A, S)?;
    let final_epoch = before_epoch + (from ^ to).count_ones() as i64;
    let charge = store.charged_usage()?;
    let changes = applied(store.revise_admissions_if_current(A, S, before_epoch, &edits)?, final_epoch);
    let expected: Vec<_> = rules().into_iter().filter(|(_, g)| verdict(g, from) != verdict(g, to)).map(|(n, _)| n).collect();
    assert_eq!(changes.iter().map(|c| c.branch.as_str()).collect::<Vec<_>>(), expected, "transition {from}->{to}");
    assert_eq!(store.charged_usage()?, charge + expected.len() as i64);
    for c in changes {
        let row: (i64, i64, i64, i64) = audit.query_row("SELECT a.parent, a.view_epoch, s.rule_id, d.usage FROM attempts a JOIN support_bindings s ON s.attempt_id=a.id JOIN deliveries d ON d.attempt_id=a.id WHERE a.id=?1", [c.attempt_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        assert_eq!(row, (heads[&c.branch], final_epoch, roots[&c.branch], 1));
        heads.insert(c.branch, c.attempt_id);
    }
    for (name, groups) in rules() {
        assert_eq!(store.read_current(A, S, name)?, reference.read_current(A, S, name)?);
        assert_eq!(store.read_current(A, S, name)?, ready(verdict(&groups, to)));
        assert_eq!(store.revise_support(A, S, name)?, heads[name]);
    }
    let events = audit.prepare("SELECT evidence_id, admitted FROM admission_events WHERE epoch>?1 ORDER BY epoch")?
        .query_map([before_epoch], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)))?.collect::<Result<Vec<_>>>()?;
    let expected_events: Vec<_> = edits.iter().filter(|e| (from ^ to) & (1 << (e.evidence-1)) != 0).map(|e| (e.evidence, e.admitted)).collect();
    assert_eq!(events, expected_events);
    Ok(())
}

#[test]
fn every_initial_and_final_four_occurrence_view_matches_fresh_store() -> Result<()> {
    for from in 0..16 {
        for to in 0..16 {
            let file = Database::new();
            let mut store = Store::open(&file.0)?;
            let mut heads = fixture(&mut store, from)?;
            let roots = heads.clone();
            let audit = Connection::open(&file.0)?;
            compare_transition(&mut store, &audit, &mut heads, &roots, from, to, (from + to) % 2 == 0)?;
        }
    }
    Ok(())
}

#[test]
fn seeded_edit_sequences_preserve_lineage_charges_and_final_views() -> Result<()> {
    for seed in [0x4b54_2026_u64, 0x5eed_1115, 0xdead_beef] {
        eprintln!("atomic correction sequence seed={seed:#x}");
        let file = Database::new();
        let mut store = Store::open(&file.0)?;
        let mut heads = fixture(&mut store, 0)?;
        let roots = heads.clone();
        let audit = Connection::open(&file.0)?;
        let mut random = seed;
        let mut from = 0;
        for step in 0..64 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let to = ((random >> 32) & 15) as u8;
            eprintln!("seed={seed:#x} step={step} transition={from}->{to}");
            compare_transition(&mut store, &audit, &mut heads, &roots, from, to, random & 1 != 0)?;
            from = to;
        }
    }
    Ok(())
}
