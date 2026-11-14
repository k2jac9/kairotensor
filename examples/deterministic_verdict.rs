//! A deterministic report verdict through authenticated Store evidence, not a model.
//! The default is an in-memory demonstration; persistence/races are covered by tests.
use kairotensor::{Current, Disposition, Store};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = Store::in_memory()?;
    let actor = "local-reviewer";
    let scope = "release";
    let evidence = store.observe(
        "ci-report",
        br#"{"release":"release-A","passed":11,"total":12}"#,
    )?;
    store.set_admitted(actor, scope, evidence, true)?;
    // Selection is complete-or-refuse, never "whatever remains visible".
    let missing = evidence + 100;
    assert!(store
        .begin_report_verdict(actor, scope, "plan", &[evidence, missing])
        .is_err());
    let attempt = store.begin_report_verdict(actor, scope, "plan", &[evidence])?;
    assert_eq!(store.commit_report_verdict(attempt)?, Disposition::Accepted);
    let Current::Ready(bytes) = store.read_current(actor, scope, "plan")? else {
        return Err("no currently eligible verdict".into());
    };
    println!(
        "{}",
        json!({"event":"current_read", "route":"deterministic_store_report_verdict",
        "attempt_id":attempt,"evidence_id":evidence,"verdict":serde_json::from_slice::<serde_json::Value>(&bytes)?,
        "model_calls":0,"charged_tokens":store.charged_usage()?})
    );

    // Redelivery is a historical receipt, not permission to show stale bytes.
    store.set_admitted(actor, scope, evidence, false)?;
    assert_eq!(store.commit_report_verdict(attempt)?, Disposition::Accepted);
    assert_eq!(
        store.read_current(actor, scope, "plan")?,
        Current::NeedsReview
    );
    println!(
        "{}",
        json!({"event":"after_revocation", "historical_receipt":"accepted",
        "current":"needs_review","model_calls":0,"charged_tokens":store.charged_usage()?})
    );
    Ok(())
}
