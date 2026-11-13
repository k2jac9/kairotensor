//! Compare three revision or setup paths on identical file-backed fixtures.
#[path = "revision_bench/indexed.rs"]
mod indexed;
#[path = "revision_bench/scan.rs"]
mod scan;
#[path = "revision_bench/workload.rs"]
mod workload;

use indexed::{Indexed, IndexedMode};
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
enum Arm { Indexed, Batch, Bulk, Scan }

#[derive(Clone, Copy)]
enum Suite { Revision, Setup }

impl Suite {
    // Each three-arm trial uses a different order. A full block balances
    // position and immediate predecessor for every arm.
    fn orders(self) -> [[Arm; 3]; 6] {
        let [a, b, c] = match self {
            Self::Revision => [Arm::Indexed, Arm::Batch, Arm::Scan],
            Self::Setup => [Arm::Batch, Arm::Bulk, Arm::Scan],
        };
        [
            [a, b, c], [b, c, a], [c, a, b],
            [c, b, a], [b, a, c], [a, c, b],
        ]
    }
}

impl Arm {
    fn label(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::Batch => "batch",
            Self::Bulk => "bulk",
            Self::Scan => "scan",
        }
    }

    fn open(self, path: &Path) -> rusqlite::Result<Box<dyn Backend>> {
        match self {
            Self::Indexed => Ok(Box::new(Indexed::open(path, IndexedMode::PerBranch)?)),
            Self::Batch => Ok(Box::new(Indexed::open(path, IndexedMode::Atomic)?)),
            Self::Bulk => Ok(Box::new(Indexed::open(path, IndexedMode::BulkSetup)?)),
            Self::Scan => Ok(Box::new(Scan::open(path)?)),
        }
    }
}

// Both durations are nested inside the existing end-to-end register phase.
// Initial verdicts remain part of the benchmark's correctness contract.
struct Registration {
    initial: Vec<bool>,
    publish: Duration,
    initial_read: Duration,
}

trait Backend {
    fn seed(&mut self, n: usize) -> rusqlite::Result<()>;
    fn register(&mut self, rules: &[Vec<Vec<i64>>]) -> rusqlite::Result<Registration>;
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
    assert_eq!(db.register(&work.rules)?.initial, oracle.initial);
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
    publish: Duration,
    initial_read: Duration,
    edits: Duration,
    total: Duration,
}

fn measure(mut db: Box<dyn Backend>, work: &Workload, oracle: &Expected, start: Instant) -> Result<Sample> {
    let opened = Instant::now();
    db.seed(work.evidence_count)?;
    let seeded = Instant::now();
    let registration = db.register(&work.rules)?;
    let registered = Instant::now();
    let mut observed = Vec::with_capacity(work.edits.len());
    for &(id, admitted) in &work.edits {
        observed.push(db.edit(id, admitted)?);
    }
    let edited = Instant::now();
    drop(db);
    let total = start.elapsed();
    assert_eq!(registration.initial, oracle.initial);
    let register = registered - seeded;
    assert!(registration.publish + registration.initial_read <= register);
    assert_eq!(observed, oracle.edits);
    Ok(Sample {
        open: opened - start,
        seed: seeded - opened,
        register,
        publish: registration.publish,
        initial_read: registration.initial_read,
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

fn cell(dir: &Path, suite: Suite, rules: usize, edits: usize, trials: usize, dense: bool) -> Result<()> {
    let scenario = if dense { "dense" } else { "sparse" };
    let work = generate(rules, edits, dense, SEED);
    let oracle = Expected::new(&work);
    let changes: usize = oracle.edits.iter().map(Vec::len).sum();
    let orders = suite.orders();
    for arm in orders[0] {
        let path = dir.join(format!("verify-{}.db", arm.label()));
        verify(arm.open(&path)?, &work, &oracle)?;
        fs::remove_file(path)?;
    }
    for trial in 0..trials {
        let order = orders[trial % orders.len()];
        let order_label = order.map(Arm::label).join("/");
        for arm in order {
            let path = dir.join(format!("{scenario}-{rules}-{edits}-{trial}-{}.db", arm.label()));
            let start = Instant::now();
            let s = measure(arm.open(&path)?, &work, &oracle, start)?;
            let bytes = fs::metadata(&path)?.len();
            println!("{scenario},{rules},{edits},{trial},{},{},{},{},{},{},{},{bytes},{changes},{},{}",
                order_label, arm.label(), s.open.as_micros(), s.seed.as_micros(),
                s.register.as_micros(), s.edits.as_micros(), s.total.as_micros(),
                s.publish.as_micros(), s.initial_read.as_micros());
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let scratch = Scratch::new()?;
    eprintln!("revision_bench: SQLite {}, seed {SEED}, file-backed in {}", rusqlite::version(), scratch.0.display());
    println!("scenario,rules,edits,trial,order,arm,open_us,seed_us,register_us,edit_us,total_us,db_bytes,changes,publish_us,initial_read_us");
    let (suite, args) = match args.as_slice() {
        [command, rest @ ..] if command == "setup" => (Suite::Setup, rest),
        rest => (Suite::Revision, rest),
    };
    match args {
        [] => {
            for dense in [false, true] {
                for edits in [1, 32] { cell(&scratch.0, suite, 128, edits, 6, dense)?; }
            }
            cell(&scratch.0, suite, 1024, 128, 6, false)?;
        }
        [scenario, rules, edits, trials] => {
            let dense = match scenario.as_str() {
                "sparse" => false, "dense" => true, _ => return Err("scenario must be sparse or dense".into()),
            };
            let (n, e, t) = (rules.parse()?, edits.parse()?, trials.parse()?);
            if n == 0 || t == 0 { return Err("rules and trials must be positive".into()); }
            cell(&scratch.0, suite, n, e, t, dense)?;
        }
        _ => return Err("usage: revision_bench [setup] [sparse|dense RULES EDITS TRIALS]".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_arm_orders_are_represented() {
        for (suite, expected) in [
            (Suite::Revision, ["batch", "indexed", "scan"]),
            (Suite::Setup, ["batch", "bulk", "scan"]),
        ] {
            let mut orders = std::collections::BTreeSet::new();
            for order in suite.orders() {
                let labels = order.map(Arm::label);
                let mut sorted = labels;
                sorted.sort();
                assert_eq!(sorted, expected);
                assert!(orders.insert(labels));
            }
            assert_eq!(orders.len(), 6);
        }
    }

    #[test]
    fn file_backed_arms_match_the_oracle() -> Result<()> {
        let scratch = Scratch::new()?;
        for suite in [Suite::Revision, Suite::Setup] {
            for dense in [false, true] {
                cell(&scratch.0, suite, 8, 6, 2, dense)?;
            }
        }
        Ok(())
    }
}
