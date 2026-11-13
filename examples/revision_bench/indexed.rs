use kairotensor::{Current, Store, SupportRegistration};
use rusqlite::Result;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use super::Registration;

const ACTOR: &str = "benchmark";
const SCOPE: &str = "fixture";

#[derive(Clone, Copy)]
pub enum IndexedMode {
    PerBranch,
    Atomic,
    BulkSetup,
}

pub struct Indexed {
    store: Store,
    mode: IndexedMode,
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
    pub fn open(path: &Path, mode: IndexedMode) -> Result<Self> {
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
        if matches!(self.mode, IndexedMode::BulkSetup) {
            let observations = vec![("fixture", &b""[..]); evidence_count];
            let ids = self.store.observe_and_admit_many(ACTOR, SCOPE, &observations)?;
            assert_eq!(ids.len(), evidence_count);
            for (position, id) in ids.into_iter().enumerate() {
                assert_eq!(usize::try_from(id).ok(), Some(position + 1));
            }
            return Ok(());
        }
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

    fn register(&mut self, rules: &[Vec<Vec<i64>>]) -> Result<Registration> {
        assert!(self.branches.is_empty(), "benchmark rules are registered once");
        let branches: Vec<String> = (0..rules.len()).map(|position| position.to_string()).collect();
        let (heads, publish) = if matches!(self.mode, IndexedMode::BulkSetup) {
            let declarations: Vec<_> = branches.iter().zip(rules).map(|(branch, rule)| {
                SupportRegistration { branch: branch.as_str(), alternatives: rule.as_slice() }
            }).collect();
            let start = Instant::now();
            let heads = self.store.publish_support_many(ACTOR, SCOPE, &declarations)?;
            (heads, start.elapsed())
        } else {
            let mut heads = Vec::with_capacity(rules.len());
            let start = Instant::now();
            for (branch, rule) in branches.iter().zip(rules) {
                heads.push(self.store.publish_support(ACTOR, SCOPE, branch, rule)?);
            }
            (heads, start.elapsed())
        };
        assert_eq!(heads.len(), branches.len());
        let mut initial = Vec::with_capacity(heads.len());
        let start = Instant::now();
        for branch in &branches {
            initial.push(read_verdict(&mut self.store, branch)?);
        }
        let initial_read = start.elapsed();
        for (position, (branch, head)) in branches.into_iter().zip(heads).enumerate() {
            // Registration creates the first attempts in this fresh benchmark DB.
            assert_eq!(usize::try_from(head).ok(), Some(position + 1));
            self.branches.insert(branch, (position, head));
        }
        Ok(Registration { initial, publish, initial_read })
    }

    fn edit(&mut self, id: i64, admitted: bool) -> Result<Vec<(usize, bool)>> {
        let mut changed = match self.mode {
            IndexedMode::PerBranch => self.per_branch_edit(id, admitted)?,
            IndexedMode::Atomic | IndexedMode::BulkSetup => self.atomic_edit(id, admitted)?,
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
