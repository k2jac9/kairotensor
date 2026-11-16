#[path = "incident_store_bridge/adapter.rs"]
mod adapter;
#[cfg(test)]
#[path = "incident_store_bridge/tests.rs"]
mod tests;

fn requests_report() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use adapter::{prepare_incident_attempt, sha256, Fixture};
    use serde_json::{json, Value};

    let cases: Value = serde_json::from_slice(include_bytes!(
        "../workloads/incident-routing-v1/cases.json"
    ))?;
    let mut requests = Vec::new();
    for case in cases["cases"].as_array().ok_or("missing frozen cases")? {
        let case_id = case["case_id"].as_str().ok_or("missing frozen case ID")?;
        // Each fixture has its own Store. Occurrence IDs are local to that Store.
        let mut fixture = Fixture::for_case(kairotensor::Store::in_memory()?, case_id)?;
        let prepared = prepare_incident_attempt(
            &mut fixture.store,
            fixture.recovery,
            &fixture.ids,
            &fixture.contract,
        )?;
        requests.push(json!({
            "case_id": case_id,
            "request_sha256": sha256(&prepared.request),
            "request": String::from_utf8(prepared.request)?,
            "manifest_sha256": prepared.manifest_sha256,
            "captured_ids": prepared.captured_ids,
        }));
    }
    Ok(json!({
        "schema_version": 1, "version": "incident-store-requests/1",
        "mode": "synthetic_store_incident_fixture", "new_model_calls": 0,
        "accounting_unit": "UTF-8 bytes", "capture": "Store",
        "freeze_sha256": sha256(include_bytes!("../workloads/incident-routing-v1/freeze.json")),
        "requests": requests,
    }))
}

fn export_requests(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let report = requests_report()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    serde_json::to_writer(&mut file, &report)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => adapter::demo(),
        [flag, path] if flag == "--requests" => export_requests(std::path::Path::new(path)),
        _ => Err("usage: incident_store_bridge [--requests PATH]".into()),
    }
}
