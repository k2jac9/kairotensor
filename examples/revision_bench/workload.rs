/// Both strategies consume the same immutable catalogue and ordered edits.
/// Occurrence IDs run from 1 through evidence_count and start admitted.
pub struct Workload {
    pub rules: Vec<Vec<Vec<i64>>>,
    pub evidence_count: usize,
    pub edits: Vec<(i64, bool)>,
}

// SplitMix64, used only to permute the order of the sparse keys reproducibly.
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut x = *state;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Every rule has two three-premise alternatives. Adjacent pairs share a local
/// key. The first rule in a pair requires both the global and local key under
/// either alternative; the second can also use stable evidence alone.
pub fn generate(rule_count: usize, edit_count: usize, dense: bool, seed: u64) -> Workload {
    assert!(rule_count > 0, "the workload requires at least one rule");
    let pair_count = rule_count / 2 + rule_count % 2;
    let evidence_count = 5 + pair_count;
    let first_local: i64 = 6;
    let global: i64 = 1;
    let mut rules = Vec::with_capacity(rule_count);
    for rule in 0..rule_count {
        let local = first_local + (rule / 2) as i64;
        let primary = vec![global, local, 4];
        let alternate = if rule % 2 == 0 {
            vec![global, local, 5]
        } else {
            vec![2, 3, 5]
        };
        rules.push(vec![primary, alternate]);
    }

    let mut keys: Vec<i64> = (0..pair_count).map(|i| first_local + i as i64).collect();
    let mut state = seed;
    for i in (1..keys.len()).rev() {
        let j = (next_random(&mut state) % (i as u64 + 1)) as usize;
        keys.swap(i, j);
    }
    let mut edits = Vec::with_capacity(edit_count);
    for i in 0..edit_count {
        let key = if dense { global } else { keys[(i / 2) % keys.len()] };
        edits.push((key, i % 2 == 1)); // withdraw, then readmit the same key
    }
    Workload { rules, evidence_count, edits }
}

/// Rebuild every verdict directly from a separately managed membership vector.
/// Index zero is unused.
pub fn recompute(rules: &[Vec<Vec<i64>>], admitted: &[bool]) -> Vec<bool> {
    rules.iter()
        .map(|rule| rule.iter().any(|group| group.iter().all(|&id| admitted[id as usize])))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fanout_changes_both_work_and_verdicts() {
        let sparse = generate(8, 20, false, 7);
        let dense = generate(8, 20, true, 7);
        assert_eq!(sparse.rules, dense.rules);
        assert_eq!(sparse.evidence_count, dense.evidence_count);
        for (work, affected, changed) in [(&sparse, 2, 1), (&dense, 8, 4)] {
            let mut admitted = vec![true; work.evidence_count + 1];
            let mut previous = recompute(&work.rules, &admitted);
            for &(id, enabled) in &work.edits {
                assert_ne!(admitted[id as usize], enabled);
                admitted[id as usize] = enabled;
                let current = recompute(&work.rules, &admitted);
                let consumers = work.rules.iter().filter(|r| r.iter().flatten().any(|&x| x == id));
                assert_eq!(consumers.count(), affected);
                assert_eq!(previous.iter().zip(&current).filter(|(a, b)| a != b).count(), changed);
                previous = current;
            }
        }
    }

    #[test]
    fn sparse_schedule_is_reproducible_prefix_with_full_pair_coverage() {
        let long = generate(7, 10, false, 11);
        assert_eq!(generate(7, 1, false, 11).edits, long.edits[..1]);
        let mut first_pass: Vec<i64> = long.edits[..8].chunks_exact(2).map(|pair| {
            assert_eq!(pair[0], (pair[1].0, false));
            assert!(pair[1].1);
            pair[0].0
        }).collect();
        first_pass.sort_unstable();
        assert_eq!(first_pass, vec![6, 7, 8, 9]);
        assert_eq!(long.edits[..2], long.edits[8..]);
        assert!(generate(1, 0, true, 0).edits.is_empty());
    }
}
