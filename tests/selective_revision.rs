use kairotensor::{Current, Disposition, Store};
use rusqlite::Result;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const ACTOR: &str = "reviewer";
const SCOPE: &str = "inspection";

fn ready(supported: bool) -> Current {
    let flag: &[u8] = if supported { b"supported" } else { b"unsupported" };
    Current::Ready(flag.to_vec())
}

// The oracle only knows which IDs the test admitted. It has no access to the
// store, indexed dependencies, cached output or previous answer.
fn recompute(alternatives: &[Vec<i64>], active: &HashSet<i64>) -> bool {
    alternatives.iter().any(|clause| clause.iter().all(|id| active.contains(id)))
}

#[test]
fn selective_revision_matches_recomputation_across_every_view() -> Result<()> {
    let mut store = Store::in_memory()?;
    let mut ids = Vec::new();
    for source in ["a", "b", "c", "d"] {
        // Byte equality must not collapse distinct admission dependencies.
        let id = store.observe(source, b"inspection passed")?;
        store.set_admitted(ACTOR, SCOPE, id, true)?;
        ids.push(id);
    }
    let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
    let mut queries = Vec::new();
    for (branch, alternatives) in [
        ("single", vec![vec![a]]),
        ("joint", vec![vec![a, b, c]]),
        ("alternative", vec![vec![a, b], vec![d]]),
        ("shared-premise", vec![vec![a, b], vec![a, c]]),
    ] {
        let head = store.publish_support(ACTOR, SCOPE, branch, &alternatives)?;
        queries.push((branch, alternatives, head, true));
    }

    for mask in (0u8..16).chain((0u8..16).rev()) {
        let mut active = HashSet::new();
        let mut fresh = Store::in_memory()?;
        for (bit, &id) in ids.iter().enumerate() {
            assert_eq!(fresh.observe("reference occurrence", b"inspection passed")?, id);
            let admitted = mask & (1 << bit) != 0;
            store.set_admitted(ACTOR, SCOPE, id, admitted)?;
            if admitted {
                fresh.set_admitted(ACTOR, SCOPE, id, true)?;
                active.insert(id);
            }
        }
        for (branch, alternatives, head, previous) in &mut queries {
            let expected = recompute(alternatives, &active);
            fresh.publish_support(ACTOR, SCOPE, branch, alternatives)?;
            let from_scratch = fresh.read_current(ACTOR, SCOPE, branch)?;
            assert_eq!(from_scratch, ready(expected), "fresh {branch} in view {mask}");
            let before = store.charged_usage()?;
            let current = store.read_current(ACTOR, SCOPE, branch)?;
            if expected == *previous {
                assert_eq!(current, ready(expected), "{branch} in view {mask}");
                assert_eq!(store.revise_support(ACTOR, SCOPE, branch)?, *head);
                assert_eq!(store.charged_usage()?, before);
            } else {
                assert_eq!(current, Current::NeedsReview, "{branch} in view {mask}");
                assert!(store.begin_resume(ACTOR, SCOPE, branch, &[]).is_err());
                let revised = store.revise_support(ACTOR, SCOPE, branch)?;
                assert_ne!(revised, *head);
                assert_eq!(store.charged_usage()?, before + 1);
                *head = revised;
                *previous = expected;
            }
            assert_eq!(store.read_current(ACTOR, SCOPE, branch)?, from_scratch);
        }
    }
    Ok(())
}

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        Self(std::env::temp_dir().join(format!(
            "kairotensor-selective-{}-{stamp}.sqlite",
            std::process::id()
        )))
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn surviving_proof_and_stale_children_reconcile_across_connections_and_restart() -> Result<()> {
    let file = TestDatabase::new();
    let mut worker = Store::open(&file.0)?;
    let mut editor = Store::open(&file.0)?;
    let a = worker.observe("check A", b"ok")?;
    let b = worker.observe("check B", b"ok")?;
    for id in [a, b] {
        editor.set_admitted(ACTOR, SCOPE, id, true)?;
    }
    let branch = "checks";
    let original = worker.publish_support(ACTOR, SCOPE, branch, &[vec![a], vec![b]])?;
    let (first_child, initial) = worker.begin_resume(ACTOR, SCOPE, branch, &[])?;
    assert_eq!(initial.as_slice(), b"supported");
    let initial_usage = worker.charged_usage()?;

    editor.set_admitted(ACTOR, SCOPE, a, false)?;
    assert_eq!(worker.revise_support(ACTOR, SCOPE, branch)?, original);
    let (second_child, surviving) = worker.begin_resume(ACTOR, SCOPE, branch, &[])?;
    assert_eq!(surviving.as_slice(), b"supported");
    // The parent can be reused on B. The old child is still an opaque computation.
    assert_eq!(worker.settle(first_child, b"child", 5)?, Disposition::StaleView);
    editor.set_admitted(ACTOR, SCOPE, b, false)?;
    assert_eq!(worker.read_current(ACTOR, SCOPE, branch)?, Current::NeedsReview);
    assert!(worker.begin_resume(ACTOR, SCOPE, branch, &[]).is_err());
    assert_eq!(worker.settle(second_child, b"another", 7)?, Disposition::StaleView);
    assert_eq!(worker.charged_usage()?, initial_usage + 12);
    let revised = editor.revise_support(ACTOR, SCOPE, branch)?;
    assert_ne!(revised, original);
    let final_usage = editor.charged_usage()?;
    drop(worker);
    drop(editor);

    let mut reopened = Store::open(&file.0)?;
    assert_eq!(reopened.revise_support(ACTOR, SCOPE, branch)?, revised);
    assert_eq!(reopened.read_current(ACTOR, SCOPE, branch)?, ready(false));
    let (_, bytes) = reopened.begin_resume(ACTOR, SCOPE, branch, &[])?;
    assert_eq!(bytes.as_slice(), b"unsupported");
    assert_eq!(reopened.settle(first_child, b"child", 5)?, Disposition::StaleView);
    assert!(reopened.settle(second_child, b"another", 8).is_err());
    assert_eq!(reopened.charged_usage()?, final_usage);
    reopened.set_admitted(ACTOR, SCOPE, a, true)?;
    assert_eq!(reopened.read_current(ACTOR, SCOPE, branch)?, Current::NeedsReview);
    reopened.revise_support(ACTOR, SCOPE, branch)?;
    assert_eq!(reopened.read_current(ACTOR, SCOPE, branch)?, ready(true));
    Ok(())
}

#[test]
fn reverse_dependencies_follow_current_rule_heads_and_scope_boundaries() -> Result<()> {
    let mut store = Store::in_memory()?;
    let a = store.observe("a", b"x")?;
    let b = store.observe("b", b"x")?;
    for (actor, scope) in [(ACTOR, SCOPE), ("other", SCOPE), (ACTOR, "other")] {
        for id in [a, b] {
            store.set_admitted(actor, scope, id, true)?;
        }
    }
    store.publish_support(ACTOR, SCOPE, "joint", &[vec![a, b], vec![a]])?;
    store.publish_support(ACTOR, SCOPE, "sole", &[vec![a]])?;
    store.publish_support("other", SCOPE, "outside", &[vec![a]])?;
    store.publish_support(ACTOR, "other", "outside", &[vec![a]])?;
    store.set_admitted(ACTOR, SCOPE, a, false)?;
    let mut impacted = store.support_consumers(ACTOR, SCOPE, a)?;
    impacted.sort();
    assert_eq!(impacted, vec!["joint".to_owned(), "sole".to_owned()]);
    assert_eq!(store.read_current("other", SCOPE, "outside")?, ready(true));
    assert_eq!(store.read_current(ACTOR, "other", "outside")?, ready(true));

    store.publish_support(ACTOR, SCOPE, "joint", &[vec![b]])?;
    assert_eq!(store.support_consumers(ACTOR, SCOPE, a)?, vec!["sole".to_owned()]);
    let opaque = store.begin_attempt(ACTOR, SCOPE, "sole", &[b])?;
    // Deliberately output the same bytes as an earlier certified answer.
    assert_eq!(store.settle(opaque, b"supported", 2)?, Disposition::Accepted);
    assert!(store.support_consumers(ACTOR, SCOPE, a)?.is_empty());
    store.set_admitted(ACTOR, SCOPE, a, true)?;
    assert_eq!(store.read_current(ACTOR, SCOPE, "joint")?, ready(true));
    assert_eq!(store.read_current(ACTOR, SCOPE, "sole")?, Current::NeedsReview);
    assert!(store.revise_support(ACTOR, SCOPE, "sole").is_err());
    assert!(store.begin_resume(ACTOR, SCOPE, "sole", &[]).is_err());
    Ok(())
}

#[test]
fn invalid_proof_publication_preserves_the_existing_head_and_ledger() -> Result<()> {
    let mut store = Store::in_memory()?;
    let allowed = store.observe("allowed", b"yes")?;
    store.set_admitted(ACTOR, SCOPE, allowed, true)?;
    let head = store.publish_support(ACTOR, SCOPE, "checks", &[vec![allowed]])?;
    let before = store.charged_usage()?;
    for invalid in [vec![], vec![vec![]], vec![vec![allowed, allowed]]] {
        assert!(store.publish_support(ACTOR, SCOPE, "checks", &invalid).is_err());
        assert_eq!(store.revise_support(ACTOR, SCOPE, "checks")?, head);
        assert_eq!(store.charged_usage()?, before);
    }
    Ok(())
}

#[test]
fn initially_missing_premises_count_only_after_admission_in_the_requesting_view() -> Result<()> {
    let mut store = Store::in_memory()?;
    let hidden = store.observe("hidden", b"private report")?;
    let future = hidden + 1;
    store.set_admitted("other", SCOPE, hidden, true)?;
    let policy = [vec![hidden, future]];
    let first = store.publish_support(ACTOR, SCOPE, "both", &policy)?;
    assert_eq!(store.read_current(ACTOR, SCOPE, "both")?, ready(false));
    assert_eq!(store.observe("new report", b"observed only")?, future);
    assert_eq!(store.revise_support(ACTOR, SCOPE, "both")?, first);
    store.set_admitted(ACTOR, SCOPE, future, true)?;
    assert_eq!(store.read_current(ACTOR, SCOPE, "both")?, ready(false));
    store.set_admitted(ACTOR, SCOPE, hidden, true)?;
    assert_eq!(store.read_current(ACTOR, SCOPE, "both")?, Current::NeedsReview);
    assert_ne!(store.revise_support(ACTOR, SCOPE, "both")?, first);
    assert_eq!(store.read_current(ACTOR, SCOPE, "both")?, ready(true));
    Ok(())
}

#[test]
fn restoring_a_witness_recertifies_its_head_but_never_an_older_in_flight_child() -> Result<()> {
    let mut store = Store::in_memory()?;
    let a = store.observe("check A", b"pass")?;
    store.set_admitted(ACTOR, SCOPE, a, true)?;
    let head = store.publish_support(ACTOR, SCOPE, "gate", &[vec![a]])?;
    let (child, _) = store.begin_resume(ACTOR, SCOPE, "gate", &[])?;
    let before = store.charged_usage()?;
    store.set_admitted(ACTOR, SCOPE, a, false)?;
    assert_eq!(store.read_current(ACTOR, SCOPE, "gate")?, Current::NeedsReview);
    store.set_admitted(ACTOR, SCOPE, a, true)?;
    assert_eq!(store.revise_support(ACTOR, SCOPE, "gate")?, head);
    assert_eq!(store.read_current(ACTOR, SCOPE, "gate")?, ready(true));
    assert_eq!(store.charged_usage()?, before);
    assert_eq!(store.settle(child, b"old child", 3)?, Disposition::StaleView);
    Ok(())
}
