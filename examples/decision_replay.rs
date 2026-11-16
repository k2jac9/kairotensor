//! Separate-process fixture export and capability-free recorded replay.
#[allow(dead_code)]
#[path = "incident_store_bridge/adapter.rs"]
mod adapter;

use kairotensor::decision_bundle::{
    parse_decision_bundle, validate_recorded_bundle, BundleExport, BundlePurpose, BundleRequest,
    DenyDiagnostics, MAX_BUNDLE_BYTES,
};
use std::io::{Read, Write};
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// This branch holds only an input file and the pure codec/replay API. In
// particular it never constructs a Store, adapter, runtime or backend callback.
fn replay(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_BUNDLE_BYTES as u64 {
        return Err("decision bundle limit exceeded".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BUNDLE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err("decision bundle limit exceeded".into());
    }
    let bundle = parse_decision_bundle(&bytes)?;
    println!("{}", serde_json::to_string(&validate_recorded_bundle(&bundle))?);
    Ok(())
}

fn export_fixture(path: &Path) -> Result<()> {
    // Exclusive creation also refuses existing files and symbolic links.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut fixture = adapter::Fixture::new(kairotensor::Store::in_memory()?)?;
    let prepared = adapter::prepare_incident_attempt(
        &mut fixture.store,
        fixture.recovery,
        &fixture.ids,
        &fixture.contract,
    )?;
    if !fixture.store.reserve_recovery_attempt(
        prepared.attempt,
        &prepared.request,
        prepared.preparation.input_tokens,
        prepared.preparation.output_cap,
    )? {
        return Err("fixture reservation refused".into());
    }
    let outcome = fixture.store.run_timed_recovery(
        prepared.attempt,
        &prepared.preparation,
        || adapter::tick(200),
        |request| {
            // Offline, format-valid canned bytes; no model or semantic claim.
            Ok::<_, String>(prepared.measure(request, br#"{"decision":"abstain","owner":null,"reason":"undetermined_cause","evidence_ids":["e01"]}"#))
        },
    )?;
    if !matches!(outcome, kairotensor::RecoveryRun::Settled { disposition: kairotensor::Disposition::Accepted, .. }) {
        return Err("fixture did not settle accepted".into());
    }
    let export = fixture.store.export_decision_bundle(
        &BundleRequest {
            actor: adapter::ACTOR.into(),
            scope: adapter::SCOPE.into(),
            attempt: prepared.attempt,
            purpose: BundlePurpose::CurrentRead,
        },
        &DenyDiagnostics,
    )?;
    let BundleExport::Complete(bundle) = export else {
        return Err("fixture unavailable (redacted)".into());
    };
    file.write_all(&bundle.to_json()?)?;
    file.sync_all()?;
    println!("{}", serde_json::to_string(&validate_recorded_bundle(&bundle))?);
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [mode, path] if mode == "export-fixture" => export_fixture(Path::new(path)),
        [mode, path] if mode == "replay" => replay(Path::new(path)),
        _ => Err("usage: decision_replay export-fixture PATH | replay PATH".into()),
    }
}
