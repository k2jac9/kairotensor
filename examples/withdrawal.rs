//! Run with `cargo run --example withdrawal`.
use kairotensor::{Current, Disposition, Store};
use std::error::Error;

const ACTOR: &str = "inventory-agent";
const SCOPE: &str = "warehouse";
const BRANCH: &str = "count";

// Each observation encodes one shelf count as an unsigned byte.
// This demo charges one work unit per addition performed by the reducer.
fn count_stock(inputs: &[(i64, Vec<u8>)]) -> Result<(u64, i64), Box<dyn Error>> {
    let mut total = 0u64;
    let mut additions = 0;
    for (_, bytes) in inputs {
        let [count] = bytes.as_slice() else {
            return Err("expected a single-byte shelf count".into());
        };
        total += u64::from(*count);
        additions += 1;
    }
    Ok((total, additions))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut store = Store::in_memory()?;
    let shelf_a = store.observe("stocktake/shelf-a", &[8])?;
    let shelf_b = store.observe("stocktake/shelf-b", &[4])?;
    let both = [shelf_a, shelf_b];
    for &id in &both {
        store.set_admitted(ACTOR, SCOPE, id, true)?;
    }

    let (initial, captured) = store.begin_from_candidates(ACTOR, SCOPE, BRANCH, &both)?;
    let (total, units) = count_stock(&captured)?;
    assert_eq!(
        store.settle(initial, total.to_string().as_bytes(), units)?,
        Disposition::Accepted
    );
    assert_eq!(
        store.read_current(ACTOR, SCOPE, BRANCH)?,
        Current::Ready(b"12".to_vec())
    );
    println!("Accepted stock count: {total} items.");

    // Another worker has already done its two additions. Its reply arrives
    // after the operator withdraws the shelf B count.
    let (pending, captured) = store.begin_from_candidates(ACTOR, SCOPE, BRANCH, &both)?;
    let (late_total, late_units) = count_stock(&captured)?;
    store.set_admitted(ACTOR, SCOPE, shelf_b, false)?;
    assert_eq!(store.read_evidence(ACTOR, SCOPE, shelf_b)?, None);
    assert_eq!(
        store.read_current(ACTOR, SCOPE, BRANCH)?,
        Current::NeedsReview
    );
    assert!(store.begin_resume(ACTOR, SCOPE, BRANCH, &[]).is_err());
    println!("Withdrew shelf B. The previous total is blocked for reads and resume.");

    assert_eq!(
        store.settle(pending, late_total.to_string().as_bytes(), late_units)?,
        Disposition::StaleView
    );
    println!("Rejected the late count and charged its {late_units} additions.");

    // Submit the same candidate list. This capture now contains only shelf A.
    let (rebuild, captured) = store.begin_from_candidates(ACTOR, SCOPE, BRANCH, &both)?;
    assert_eq!(captured, vec![(shelf_a, vec![8])]);
    let (revised_total, rebuild_units) = count_stock(&captured)?;
    assert_eq!(
        store.settle(rebuild, revised_total.to_string().as_bytes(), rebuild_units)?,
        Disposition::Accepted
    );
    assert_eq!(
        store.read_current(ACTOR, SCOPE, BRANCH)?,
        Current::Ready(b"8".to_vec())
    );
    let charged = store.charged_usage()?;
    assert_eq!(charged, 5);
    println!("Rebuilt stock count: {revised_total} items. All attempts used {charged} additions.");
    Ok(())
}

#[test]
fn withdrawal_demo_matches_expected_counts() -> Result<(), Box<dyn Error>> {
    main()
}
