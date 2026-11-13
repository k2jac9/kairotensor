//! Compare three single-writer revision paths on identical file-backed fixtures.
#[path = "revision_bench/indexed.rs"]
mod indexed;
#[path = "revision_bench/scan.rs"]
mod scan;
#[path = "revision_bench/workload.rs"]
mod workload;

use indexed::{EditMode, Indexed};
use scan::Scan;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workload::{generate, recompute, Workload};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Changes = Vec<(usize, bool)>;
const SEED: u64 = 20261113;

#[derive(Clone, Copy)]
enum Arm { Indexed, Batch, Scan }

// All six orders balance positions and immediate predecessors across a block.
const ORDERS: [[Arm; 3]; 6] = {
    use Arm::{Batch, Indexed, Scan};
    [
        [Indexed, Batch, Scan],
        [Batch, Scan, Indexed],
        [Scan, Indexed, Batch],
        [Scan, Batch, Indexed],
        [Batch, Indexed, Scan],
        [Indexed, Scan, Batch],
    ]
};

impl Arm {
    fn label(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::Batch => "batch",
            Self::Scan => "scan",
        }
    }

    fn open(self, path: &Path) -> rusqlite::Result<Box<dyn Backend>> {
        match self {
            Self::Indexed => Ok(Box::new(Indexed::open(path, EditMode::PerBranch)?)),
            Self::Batch => Ok(Box::new(Indexed::open(path, EditMode::Atomic)?)),
            Self::Scan => Ok(Box::new(Scan::open(path)?)),
        }
    }
}

trait Backend {
    fn seed(&mut self, n: usize) -> rusqlite::Result<()>;
    fn register(&mut self, rules: &[Vec<Vec<i64>>]) -> rusqlite::Result<Vec<bool>>;
    fn edit(&mut self, id: i64, admitted: bool) -> rusqlite::Result<Changes>;
    fn read_all(&mut self) -> rusqlite::Result<Vec<bool>>;
}

struct Expected {
    initial: Vec<bool>,
    edits: Vec<Changes>,
}

impl Expected {
    fn new(work: &Workload) -> Self {
        let mut admitted = vec![true; work.evidence_count + 1];
        let initial = recompute(&work.rules, &admitted);
        let mut previous = initial.clone();
        let mut edits = Vec::new();
        for &(id, value) in &work.edits {
            admitted[id as usize] = value;
            let next = recompute(&work.rules, &admitted);
            let changes = next.iter().zip(&previous).enumerate()
                .filter_map(|(i, (&new, &old))| (new != old).then_some((i, new)))
                .collect();
            edits.push(changes);
            previous = next;
        }
        Self { initial, edits }
    }
}

// Full persisted checks run on their own file, never between timed edits.
fn verify(mut db: Box<dyn Backend>, work: &Workload, oracle: &Expected) -> Result<()> {
    db.seed(work.evidence_count)?;
    assert_eq!(db.register(&work.rules)?, oracle.initial);
    let mut current = oracle.initial.clone();
    assert_eq!(db.read_all()?, current);
    for (&(id, admitted), expected) in work.edits.iter().zip(&oracle.edits) {
        assert_eq!(db.edit(id, admitted)?, *expected);
        for &(rule, value) in expected {
            current[rule] = value;
        }
        assert_eq!(db.read_all()?, current);
    }
    Ok(())
}

struct Sample {
    open: Duration,
    seed: Duration,
    register: Duration,
    edits: Duration,
    total: Duration,
}

fn measure(mut db: Box<dyn Backend>, work: &Workload, oracle: &Expected, start: Instant) -> Result<Sample> {
    let opened = Instant::now();
    db.seed(work.evidence_count)?;
    let seeded = Instant::now();
    let initial = db.register(&work.rules)?;
    let registered = Instant::now();
    let mut observed = Vec::with_capacity(work.edits.len());
    for &(id, admitted) in &work.edits {
        observed.push(db.edit(id, admitted)?);
    }
    let edited = Instant::now();
    drop(db);
    let total = start.elapsed();
    assert_eq!(initial, oracle.initial);
    assert_eq!(observed, oracle.edits);
    Ok(Sample {
        open: opened - start,
        seed: seeded - opened,
        register: registered - seeded,
        edits: edited - registered,
        total,
    })
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir().join(format!("kairotensor-bench-{}-{stamp}", std::process::id()));
        fs::create_dir(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn cell(dir: &Path, rules: usize, edits: usize, trials: usize, dense: bool) -> Result<()> {
    let scenario = if dense { "dense" } else { "sparse" };
    let work = generate(rules, edits, dense, SEED);
    let oracle = Expected::new(&work);
    let changes: usize = oracle.edits.iter().map(Vec::len).sum();
    for arm in ORDERS[0] {
        let path = dir.join(format!("verify-{}.db", arm.label()));
        verify(arm.open(&path)?, &work, &oracle)?;
        fs::remove_file(path)?;
    }
    for trial in 0..trials {
        let order = ORDERS[trial % ORDERS.len()];
        let order_label = order.map(Arm::label).join("/");
        for arm in order {
            let path = dir.join(format!("{scenario}-{rules}-{edits}-{trial}-{}.db", arm.label()));
            let start = Instant::now();
            let s = measure(arm.open(&path)?, &work, &oracle, start)?;
            let bytes = fs::metadata(&path)?.len();
            println!("{scenario},{rules},{edits},{trial},{},{},{},{},{},{},{},{bytes},{changes}",
                order_label, arm.label(), s.open.as_micros(), s.seed.as_micros(),
                s.register.as_micros(), s.edits.as_micros(), s.total.as_micros());
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let scratch = Scratch::new()?;
    eprintln!("revision_bench: SQLite {}, seed {SEED}, file-backed in {}", rusqlite::version(), scratch.0.display());
    println!("scenario,rules,edits,trial,order,arm,open_us,seed_us,register_us,edit_us,total_us,db_bytes,changes");
    match args.as_slice() {
        [] => {
            for dense in [false, true] {
                for edits in [1, 32] { cell(&scratch.0, 128, edits, 6, dense)?; }
            }
            cell(&scratch.0, 1024, 128, 6, false)?;
        }
        [scenario, rules, edits, trials] => {
            let dense = match scenario.as_str() {
                "sparse" => false, "dense" => true, _ => return Err("scenario must be sparse or dense".into()),
            };
            let (n, e, t) = (rules.parse()?, edits.parse()?, trials.parse()?);
            if n == 0 || t == 0 { return Err("rules and trials must be positive".into()); }
            cell(&scratch.0, n, e, t, dense)?;
        }
        _ => return Err("usage: revision_bench [sparse|dense RULES EDITS TRIALS]".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_arm_orders_are_represented() {
        let mut orders = std::collections::BTreeSet::new();
        for order in ORDERS {
            let labels = order.map(Arm::label);
            let mut sorted = labels;
            sorted.sort();
            assert_eq!(sorted, ["batch", "indexed", "scan"]);
            assert!(orders.insert(labels));
        }
        assert_eq!(orders.len(), 6);
    }

    #[test]
    fn file_backed_arms_match_the_oracle() -> Result<()> {
        let scratch = Scratch::new()?;
        for dense in [false, true] { cell(&scratch.0, 8, 6, 2, dense)?; }
        Ok(())
    }
}
