use kairotensor::{Current, Store};
use rusqlite::Result;
use std::collections::HashMap;
use std::path::Path;

const ACTOR: &str = "benchmark";
const SCOPE: &str = "fixture";

#[derive(Clone, Copy)]
pub enum EditMode {
    PerBranch,
    Atomic,
}

pub struct Indexed {
    store: Store,
    mode: EditMode,
    // Rule position and last accepted attempt for each production branch name.
    branches: HashMap<String, (usize, i64)>,
}

fn read_verdict(store: &mut Store, branch: &str) -> Result<bool> {
    match store.read_current(ACTOR, SCOPE, branch)? {
        Current::Ready(bytes) if bytes.as_slice() == b"supported" => Ok(true),
        Current::Ready(bytes) if bytes.as_slice() == b"unsupported" => Ok(false),
        other => panic!("expected a published support verdict for {branch}, found {other:?}"),
    }
}

impl Indexed {
    pub fn open(path: &Path, mode: EditMode) -> Result<Self> {
        Ok(Self {
            store: Store::open(path)?,
            mode,
            branches: HashMap::new(),
        })
    }

    fn per_branch_edit(&mut self, id: i64, admitted: bool) -> Result<super::Changes> {
        self.store.set_admitted(ACTOR, SCOPE, id, admitted)?;
        let consumers = self.store.support_consumers(ACTOR, SCOPE, id)?;
        let mut changed = Vec::new();
        for branch in consumers {
            let (position, previous) = self.branches.get_mut(&branch).expect("unknown indexed branch");
            let head = self.store.revise_support(ACTOR, SCOPE, &branch)?;
            if head != *previous {
                changed.push((*position, read_verdict(&mut self.store, &branch)?));
                *previous = head;
            }
        }
        Ok(changed)
    }

    fn atomic_edit(&mut self, id: i64, admitted: bool) -> Result<super::Changes> {
        let revisions = self.store.set_admitted_and_revise(ACTOR, SCOPE, id, admitted)?;
        let mut changed = Vec::with_capacity(revisions.len());
        for revision in revisions {
            let (position, previous) = self.branches.get_mut(&revision.branch)
                .expect("unknown indexed branch");
            assert_ne!(revision.attempt_id, *previous);
            // Give both indexed arms the same post-publication read cost.
            let value = read_verdict(&mut self.store, &revision.branch)?;
            assert_eq!(value, revision.supported);
            changed.push((*position, value));
            *previous = revision.attempt_id;
        }
        Ok(changed)
    }
}

impl super::Backend for Indexed {
    fn seed(&mut self, evidence_count: usize) -> Result<()> {
        for ordinal in 1..=evidence_count {
            let id = self.store.observe("fixture", b"")?;
            assert_eq!(
                usize::try_from(id).ok(),
                Some(ordinal),
                "expected a fresh database"
            );
            self.store.set_admitted(ACTOR, SCOPE, id, true)?;
        }
        Ok(())
    }

    fn register(&mut self, rules: &[Vec<Vec<i64>>]) -> Result<Vec<bool>> {
        assert!(self.branches.is_empty(), "benchmark rules are registered once");
        let mut initial = Vec::with_capacity(rules.len());
        for (position, rule) in rules.iter().enumerate() {
            let branch = position.to_string();
            let head = self.store.publish_support(ACTOR, SCOPE, &branch, rule)?;
            initial.push(read_verdict(&mut self.store, &branch)?);
            self.branches.insert(branch, (position, head));
        }
        Ok(initial)
    }

    fn edit(&mut self, id: i64, admitted: bool) -> Result<Vec<(usize, bool)>> {
        let mut changed = match self.mode {
            EditMode::PerBranch => self.per_branch_edit(id, admitted)?,
            EditMode::Atomic => self.atomic_edit(id, admitted)?,
        };
        // SQLite orders branch names as text. All benchmark arms emit changes
        // in numeric rule order so the driver can compare the complete stream.
        changed.sort_unstable_by_key(|(position, _)| *position);
        Ok(changed)
    }

    fn read_all(&mut self) -> Result<Vec<bool>> {
        let mut verdicts = vec![false; self.branches.len()];
        for (branch, &(position, _)) in &self.branches {
            verdicts[position] = read_verdict(&mut self.store, branch)?;
        }
        Ok(verdicts)
    }
}
