//! Run with `cargo run --example selective_revision`.
use kairotensor::{Current, Store};
use rusqlite::Result;
use std::collections::BTreeSet;

const ACTOR: &str = "reviewer";
const SCOPE: &str = "test-run/42";
const REPORT: &str = "primary-report";
const RECORD: &str = "reproduction-record";

// Independent oracle for this single-writer fixture. Read current observations
// directly, then evaluate the fixed rule without consulting the support index.
fn recompute(store: &Store, alternatives: &[Vec<i64>]) -> Result<Current> {
    let mut admitted = BTreeSet::new();
    for &id in alternatives.iter().flatten() {
        if store.read_evidence(ACTOR, SCOPE, id)?.is_some() {
            admitted.insert(id);
        }
    }
    let supported = alternatives
        .iter()
        .any(|group| group.iter().all(|id| admitted.contains(id)));
    let bytes: &[u8] = if supported { b"supported" } else { b"unsupported" };
    Ok(Current::Ready(bytes.to_vec()))
}

fn main() -> Result<()> {
    let mut store = Store::in_memory()?;
    let report = store.observe("original-run/report", b"report for run 42")?;
    let manifest = store.observe("original-run/manifest", b"inputs for run 42")?;
    let rerun = store.observe("second-run/record", b"self-contained record for run 42")?;
    for id in [report, manifest, rerun] {
        store.set_admitted(ACTOR, SCOPE, id, true)?;
    }

    let report_rule = [vec![report]];
    let record_rule = [vec![report, manifest], vec![rerun]];
    let old_report = store.publish_support(ACTOR, SCOPE, REPORT, &report_rule)?;
    let old_record = store.publish_support(ACTOR, SCOPE, RECORD, &record_rule)?;
    store.set_admitted(ACTOR, SCOPE, report, false)?;

    let consumers = store.support_consumers(ACTOR, SCOPE, report)?;
    assert_eq!(consumers, vec![REPORT.to_owned(), RECORD.to_owned()]);
    println!("Withdrew the original report. Affected branches: {}.", consumers.join(", "));

    let usage = store.charged_usage()?;
    assert_eq!(store.read_current(ACTOR, SCOPE, RECORD)?, Current::Ready(b"supported".to_vec()));
    assert_eq!(store.revise_support(ACTOR, SCOPE, RECORD)?, old_record);
    assert_eq!(store.charged_usage()?, usage);
    println!("The second run still supplies a reproduction record. Kept attempt {old_record}.");

    assert_eq!(store.read_current(ACTOR, SCOPE, REPORT)?, Current::NeedsReview);
    let new_report = store.revise_support(ACTOR, SCOPE, REPORT)?;
    assert_ne!(new_report, old_report);
    assert_eq!(store.read_current(ACTOR, SCOPE, REPORT)?, Current::Ready(b"unsupported".to_vec()));
    assert_eq!(store.charged_usage()?, usage + 1);
    println!("The primary report has no admitted witness. Published attempt {new_report}.");

    for (branch, rule) in [(REPORT, report_rule.as_slice()), (RECORD, record_rule.as_slice())] {
        let expected = recompute(&store, rule)?;
        assert_eq!(store.read_current(ACTOR, SCOPE, branch)?, expected);
    }
    println!("Both results match full recomputation from admitted observations.");
    Ok(())
}

#[test]
fn selective_revision_matches_full_recomputation() -> Result<()> {
    main()
}
