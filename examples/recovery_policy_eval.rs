//! Collect one Store-backed trajectory for paired policy replay.
//!
//! Default output is synthetic byte-token mechanics, never model efficacy.
//! --live opts into the existing pinned Qwen adapter; collection never fits a policy.
#[allow(dead_code)]
#[path = "corrected_handoff/prepared_adapter.rs"]
mod adapter;
#[path = "recovery_policy_eval/tests.rs"]
#[cfg(test)]
mod tests;

use kairotensor::{CapturedInputs, Disposition, RecoveryStart, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CORPUS: &str = include_str!("recovery_policy_eval/corpus-v1.json");
const CORPUS_V2: &str = include_str!("recovery_policy_eval/corpus-v2.json");
const PROMPT_V2: &str = include_str!("recovery_policy_eval/prompt-v2.json");
const PROMPT_V2_FREEZE: &str = include_str!("recovery_policy_eval/prompt-v2-freeze.json");
const CORPUS_V2_FREEZE: &str = include_str!("recovery_policy_eval/corpus-v2-freeze.json");
const ACTOR: &str = "recovery-policy-eval";
const SCOPE: &str = "ci-report";
const BRANCH: &str = "verdict";
const VALIDATOR: &str = "independent-report-verdict/1";
const FEEDBACK: &str = "Return only complete JSON matching the current admitted reports: release, passed/total tests, and pass/fail status. Do not choose between conflicting reports.";
const SYSTEM: &str = "Read ci_reports. Return only JSON string fields release, tests, status. Use a report only when exactly one complete report is present. Copy release. Format tests as passed/total. status is pass only when passed equals total; otherwise fail. Do not invent a verdict for conflicting or incomplete reports.";
const MODEL: &str = "Qwen/Qwen2.5-1.5B-Instruct";
const REVISION: &str = "989aa7980e4cf806f80c7fef2b1adb7bc71aa306";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Report {
    release: String,
    passed: u64,
    total: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Verdict {
    release: String,
    tests: String,
    status: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    case_id: String,
    family: String,
    split: String,
    reports: Vec<Report>,
    retry_reports: Option<Vec<Report>>,
    initial_reference: Option<Verdict>,
    retry_reference: Option<Verdict>,
    fixture_outputs: [String; 2],
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    schema_version: i64,
    corpus_id: String,
    validator_version: String,
    prompt_version: String,
    budget_tokens: i64,
    output_cap: i64,
    deadline_ms: i64,
    cases: Vec<Case>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prompt {
    schema_version: i64,
    prompt_version: String,
    validator_version: String,
    system_prompt: String,
    feedback_prompt: String,
}
fn load_prompt(version: &str) -> Result<Prompt> {
    let prompt = match version {
        "report-bundle/1" => Prompt {
            schema_version: 1,
            prompt_version: version.into(),
            validator_version: VALIDATOR.into(),
            system_prompt: SYSTEM.into(),
            feedback_prompt: FEEDBACK.into(),
        },
        "report-bundle/2" => serde_json::from_str(PROMPT_V2)?,
        _ => return Err("unknown prompt version".into()),
    };
    if prompt.schema_version != 1
        || prompt.validator_version != VALIDATOR
        || prompt.prompt_version != version
    {
        return Err("incompatible frozen prompt".into());
    }
    Ok(prompt)
}
fn corpus_source(version: &str) -> Result<&'static str> {
    match version {
        "v1" => Ok(CORPUS),
        "v2" => Ok(CORPUS_V2),
        _ => Err("corpus version must be v1 or v2".into()),
    }
}
fn selected_corpus(version: &str, split: &str) -> Result<Corpus> {
    if !matches!(split, "all" | "calibration") {
        return Err("split must be all or calibration".into());
    }
    let mut corpus = load_corpus_version(version)?;
    if split == "calibration" {
        corpus.cases.retain(|case| case.split == "calibration");
    }
    Ok(corpus)
}

fn reference_for(reports: &[Report]) -> Option<Verdict> {
    let [report] = reports else { return None };
    if report.release.is_empty() || report.total == 0 || report.passed > report.total {
        return None;
    }
    Some(Verdict {
        release: report.release.clone(),
        tests: format!("{}/{}", report.passed, report.total),
        status: if report.passed == report.total {
            "pass"
        } else {
            "fail"
        }
        .into(),
    })
}
fn validate_answer(output: &str, reference: Option<&Verdict>) -> bool {
    reference.is_some_and(|expected| {
        serde_json::from_str::<Verdict>(output).ok().as_ref() == Some(expected)
    })
}
#[cfg(test)]
fn load_corpus() -> Result<Corpus> {
    load_corpus_version("v1")
}
fn load_corpus_version(version: &str) -> Result<Corpus> {
    let corpus: Corpus = serde_json::from_str(corpus_source(version)?)?;
    if corpus.schema_version != 1
        || corpus.validator_version != VALIDATOR
        || corpus.corpus_id != format!("ci-report-recovery-{version}")
        || corpus.prompt_version
            != match version {
                "v1" => "report-bundle/1",
                "v2" => "report-bundle/2",
                _ => unreachable!(),
            }
        || corpus.output_cap != 96
        || corpus.budget_tokens != 4096
        || corpus.deadline_ms != 60000
    {
        return Err("unexpected frozen corpus configuration".into());
    }
    let mut ids = BTreeSet::new();
    let mut families = BTreeMap::new();
    let mut splits = BTreeSet::new();
    for case in &corpus.cases {
        if case.case_id.trim().is_empty()
            || case.family.trim().is_empty()
            || !ids.insert(&case.case_id)
            || !matches!(case.split.as_str(), "calibration" | "heldout")
        {
            return Err("invalid/duplicate corpus identity or split".into());
        }
        splits.insert(case.split.as_str());
        if families
            .insert(&case.family, &case.split)
            .is_some_and(|split| split != &case.split)
        {
            return Err("family crosses corpus splits".into());
        }
        if reference_for(&case.reports) != case.initial_reference
            || reference_for(case.retry_reports.as_ref().unwrap_or(&case.reports))
                != case.retry_reference
        {
            return Err(format!(
                "independent reference disagrees with evidence: {}",
                case.case_id
            )
            .into());
        }
    }
    if splits != BTreeSet::from(["calibration", "heldout"]) {
        return Err("both splits are required".into());
    }
    Ok(corpus)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Context {
    request: String,
    evidence_id: String,
    admitted: bool,
    consistent: bool,
    input_tokens: i64,
    output_cap: i64,
    elapsed_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Attempt {
    request: String,
    evidence_id: String,
    input_tokens: i64,
    output_cap: i64,
    disposition: String,
    correct: Option<bool>,
    elapsed_ms: i64,
    charged_tokens: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cell {
    context: Context,
    attempt: Option<Attempt>,
}

struct Reply {
    text: String,
    input: i64,
    output: i64,
    details: Value,
}
trait Backend {
    type Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)>;
    fn generate(&mut self, prepared: Self::Prepared, fixture: &str) -> Result<Reply>;
    fn discard(&mut self, prepared: Self::Prepared) -> Result<()>;
}
struct Fixture;
impl Backend for Fixture {
    type Prepared = (i64, i64);
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        let request = serde_json::to_vec(
            &json!({"adapter":"recovery-byte-token-fixture/1","messages":messages,"output_cap":cap}),
        )?;
        let input = i64::try_from(request.len())?;
        Ok((request, input, (input, cap)))
    }
    fn generate(&mut self, (input, cap): Self::Prepared, fixture: &str) -> Result<Reply> {
        // Fixture vocabulary consists of UTF-8 bytes. Truncate at a valid text boundary.
        let mut end = fixture.len().min(usize::try_from(cap)?);
        while !fixture.is_char_boundary(end) {
            end -= 1;
        }
        Ok(Reply {
            text: fixture[..end].into(),
            input,
            output: i64::try_from(end)?,
            details: json!({"adapter":"recovery-byte-token-fixture/1","synthetic_response":true}),
        })
    }
    fn discard(&mut self, _: Self::Prepared) -> Result<()> {
        Ok(())
    }
}
impl Backend for adapter::Adapter {
    type Prepared = adapter::Prepared;
    fn prepare(&mut self, messages: Value, cap: i64) -> Result<(Vec<u8>, i64, Self::Prepared)> {
        let request = self.freeze_with_cap(messages, u64::try_from(cap)?)?;
        let prepared = adapter::Adapter::prepare(self, &request)?;
        if prepared.max_new_tokens != cap as u64 {
            self.discard(prepared)?;
            return Err("preflight cap mismatch".into());
        }
        Ok((request, i64::try_from(prepared.input_tokens)?, prepared))
    }
    fn generate(&mut self, prepared: Self::Prepared, _: &str) -> Result<Reply> {
        let response = self.generate_prepared(prepared)?;
        let reply = response.reply;
        Ok(Reply {
            text: reply.text,
            input: i64::try_from(reply.input_tokens.ok_or("missing input usage")?)?,
            output: i64::try_from(reply.output_tokens.ok_or("missing output usage")?)?,
            details: json!({"generation_us":reply.generation_us,"roundtrip_us":reply.roundtrip_us,
                "hit_cap":response.hit_cap,"ended_eos":response.ended_eos,"synthetic_response":false}),
        })
    }
    fn discard(&mut self, prepared: Self::Prepared) -> Result<()> {
        adapter::Adapter::discard(self, prepared)
    }
}

/// Atomic snapshots keep the last complete raw artifact readable on ordinary errors.
/// SQLite remains the authority if the process dies between claim and snapshot.
struct Journal {
    path: Option<PathBuf>,
    value: Value,
}
impl Journal {
    fn event(&mut self, event: Value) -> Result<()> {
        self.value["events"]
            .as_array_mut()
            .ok_or("invalid event journal")?
            .push(event);
        self.persist()
    }
    fn persist(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let temp = suffix(path, ".writing");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(&self.value)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}
fn suffix(path: &Path, value: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(value);
    PathBuf::from(name)
}
fn ns(start: Instant) -> Result<i64> {
    Ok(i64::try_from(start.elapsed().as_nanos())?)
}
fn ledger(store: &Store, budget: i64) -> Result<Value> {
    let state = store.token_budget_state(budget)?;
    if state.spent < 0 || state.reserved < 0 || state.spent + state.reserved > state.limit {
        return Err("Store budget conservation violation".into());
    }
    Ok(
        json!({"budget_id":budget,"limit":state.limit,"spent":state.spent,"reserved":state.reserved}),
    )
}
fn admit(store: &mut Store, scope: &str, reports: &[Report]) -> Result<Vec<i64>> {
    let bytes: Vec<_> = reports
        .iter()
        .map(serde_json::to_vec)
        .collect::<std::result::Result<_, _>>()?;
    let entries: Vec<_> = bytes.iter().map(|b| ("ci-report", b.as_slice())).collect();
    Ok(store.observe_and_admit_many(ACTOR, scope, &entries)?)
}
fn messages(inputs: &CapturedInputs, reason: Option<&str>, system: &str) -> Result<Value> {
    let reports: Vec<Value> = inputs
        .iter()
        .map(|(_, bytes)| serde_json::from_slice(bytes))
        .collect::<std::result::Result<_, _>>()?;
    let mut payload = json!({"ci_reports":reports});
    if let Some(reason) = reason {
        payload["validation_error"] = json!(reason);
    }
    Ok(
        json!([{"role":"system","content":system},{"role":"user","content":serde_json::to_string(&payload)?}]),
    )
}

struct Operation<'a> {
    start: Instant,
    previous_ns: i64,
    recovery: i64,
    budget: i64,
    scope: &'a str,
    case_id: &'a str,
    deadline_ms: i64,
    cap: i64,
    system_prompt: &'a str,
    feedback_prompt: &'a str,
}
fn collect_attempt<B: Backend>(
    store: &mut Store,
    backend: &mut B,
    journal: &mut Journal,
    operation: &mut Operation<'_>,
    candidates: &[i64],
    reference: Option<&Verdict>,
    fixture: &str,
) -> Result<Cell> {
    let (attempt_id, inputs, reason) = match store
        .begin_recovery_attempt(operation.recovery, candidates)?
    {
        RecoveryStart::New {
            attempt,
            inputs,
            retry_reason,
        } => (attempt, inputs, retry_reason),
        RecoveryStart::Existing(_) => {
            return Err(
                "existing operation is not permission to resend; inspect durable ledger".into(),
            )
        }
        RecoveryStart::Finished => return Err("attempt requested after terminal recovery".into()),
    };
    let epoch = store.epoch(ACTOR, operation.scope)?;
    let evidence_id = format!(
        "{}:{}:epoch-{}:inputs-{}",
        operation.case_id,
        attempt_id,
        epoch,
        inputs
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let admitted = !inputs.is_empty() && inputs.len() == candidates.len();
    let reports: Vec<Report> = inputs
        .iter()
        .map(|(_, bytes)| serde_json::from_slice(bytes))
        .collect::<std::result::Result<_, _>>()?;
    let consistent = reference_for(&reports).is_some();
    let payload = messages(&inputs, reason.as_deref(), operation.system_prompt)?;
    let (request, input, prepared) = match backend.prepare(payload, operation.cap) {
        Ok(quote) => quote,
        Err(error) => {
            let cancelled = store.cancel_recovery_attempt(attempt_id)?;
            journal.event(json!({"phase":"preparation_failed","case_id":operation.case_id,"attempt_id":attempt_id,
                "error":error.to_string(),"cancelled":cancelled,"operation_elapsed_ns":ns(operation.start)?,"ledger":ledger(store, operation.budget)?}))?;
            return Err(error);
        }
    };
    if input < 0 || input.checked_add(operation.cap).is_none() {
        backend.discard(prepared)?;
        store.cancel_recovery_attempt(attempt_id)?;
        return Err("invalid backend quote".into());
    }
    let mut context = Context {
        request: String::from_utf8(request.clone())?,
        evidence_id,
        admitted,
        consistent,
        input_tokens: input,
        output_cap: operation.cap,
        elapsed_ms: 0,
    };
    journal.event(json!({"phase":"prepared","case_id":operation.case_id,"attempt_id":attempt_id,
        "context":context,"evidence":inputs.iter().map(|(id, bytes)| json!({"id":id,"payload_utf8":String::from_utf8_lossy(bytes)})).collect::<Vec<_>>(),
        "operation_elapsed_ns":ns(operation.start)?,"ledger":ledger(store, operation.budget)?}))?;

    let refusal = if !admitted {
        Some("ineligible_evidence")
    } else if ns(operation.start)? / 1_000_000 >= operation.deadline_ms {
        Some("deadline_before_reservation")
    } else {
        None
    };
    if let Some(reason) = refusal {
        backend.discard(prepared)?;
        if !store.cancel_recovery_attempt(attempt_id)? {
            return Err("unclaimed attempt could not be cancelled".into());
        }
        return declined(store, journal, operation, attempt_id, context, reason);
    }
    let granted = match store.reserve_recovery_attempt(attempt_id, &request, input, operation.cap) {
        Ok(value) => value,
        Err(error) => {
            backend.discard(prepared)?;
            store.cancel_recovery_attempt(attempt_id)?;
            return Err(error.into());
        }
    };
    if !granted {
        backend.discard(prepared)?;
        return declined(
            store,
            journal,
            operation,
            attempt_id,
            context,
            "budget_denied",
        );
    }
    if ns(operation.start)? / 1_000_000 >= operation.deadline_ms {
        backend.discard(prepared)?;
        if !store.cancel_recovery_attempt(attempt_id)? {
            return Err("reservation cancellation lost its claim race".into());
        }
        return declined(
            store,
            journal,
            operation,
            attempt_id,
            context,
            "deadline_after_reservation",
        );
    }
    let claimed = match store.claim_recovery_dispatch(attempt_id) {
        Ok(Some(claimed)) if claimed == request => true,
        Ok(_) => false,
        Err(error) => {
            backend.discard(prepared)?;
            store.cancel_recovery_attempt(attempt_id)?;
            return Err(error.into());
        }
    };
    if !claimed {
        backend.discard(prepared)?;
        return Err("claim lost or persisted request mismatch; never resend".into());
    }
    journal.event(
        json!({"phase":"claimed","case_id":operation.case_id,"attempt_id":attempt_id,
        "operation_elapsed_ns":ns(operation.start)?,"ledger":ledger(store, operation.budget)?}),
    )?;
    // Admission includes durable claim AND the pre-I/O journal flush. A deadline
    // crossed after claim cannot release the reservation or make a complete replay.
    let admission_ns = ns(operation.start)?;
    context.elapsed_ms = admission_ns / 1_000_000 - operation.previous_ns / 1_000_000;
    if admission_ns / 1_000_000 >= operation.deadline_ms {
        backend.discard(prepared)?;
        journal.event(json!({"phase":"claimed_deadline_expired","case_id":operation.case_id,"attempt_id":attempt_id,
            "context":context,"operation_elapsed_ns":ns(operation.start)?,"ledger":ledger(store, operation.budget)?,
            "disposition":"uncertain","actual_model_call":false}))?;
        return Err(
            "deadline crossed after durable claim; reservation retained and replay withheld".into(),
        );
    }
    let previous_calls = journal.value["actual_backend_calls"]
        .as_u64()
        .ok_or("missing backend call counter")?;
    journal.value["actual_backend_calls"] = json!(previous_calls + 1);
    let response = match backend.generate(prepared, fixture) {
        Ok(reply) => reply,
        Err(error) => {
            let uncertain = Attempt {
                request: context.request.clone(),
                evidence_id: context.evidence_id.clone(),
                input_tokens: input,
                output_cap: operation.cap,
                disposition: "uncertain".into(),
                correct: None,
                elapsed_ms: ns(operation.start)? / 1_000_000 - admission_ns / 1_000_000,
                charged_tokens: None,
            };
            journal.event(json!({"phase":"generation_failed","case_id":operation.case_id,"attempt_id":attempt_id,
                "context":context,"attempt":uncertain,"error":error.to_string(),"admission_ns":admission_ns,
                "operation_elapsed_ns":ns(operation.start)?,"ledger":ledger(store, operation.budget)?}))?;
            return Err(error);
        }
    };
    // Persist raw delivery before validation/settlement so an interrupted settlement
    // retains the received bytes without pretending that unknown usage was zero.
    journal.event(json!({"phase":"received","case_id":operation.case_id,"attempt_id":attempt_id,
        "response":response.text,"input_tokens":response.input,"output_tokens":response.output,"details":response.details,
        "admission_ns":admission_ns,"operation_elapsed_ns":ns(operation.start)?}))?;
    if response.input != input || response.output < 0 || response.output > operation.cap {
        return Err(
            "response usage disagrees with exact prepared quote; reservation retained".into(),
        );
    }
    let usage = input.checked_add(response.output).ok_or("usage overflow")?;
    let correct = validate_answer(&response.text, reference);
    let disposition = if correct {
        store.settle(attempt_id, response.text.as_bytes(), usage)?
    } else {
        store.reject_invalid(
            attempt_id,
            response.text.as_bytes(),
            usage,
            VALIDATOR,
            operation.feedback_prompt,
        )?
    };
    // reject_invalid preserves the validation disposition even if its separate
    // applicability field is stale. A replay must not unlock a retry for that
    // stale delivery. Inspect the committed database rather than infer freshness.
    let store_disposition = match disposition {
        Disposition::Accepted => "accepted",
        Disposition::InvalidOutput => "invalid_output",
        Disposition::StaleView => "stale_view",
        Disposition::StaleBranch => "stale_branch",
    };
    let applicability: Option<String> =
        if let Some(database) = journal.value["manifest"]["database"].as_str() {
            let audit = rusqlite::Connection::open_with_flags(
                database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let (stored, applicability): (String, Option<String>) = audit.query_row(
                "SELECT disposition,applicability FROM deliveries WHERE attempt_id=?1",
                [attempt_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if stored != store_disposition {
                return Err("committed delivery disagrees with settlement return".into());
            }
            applicability
        } else if disposition == Disposition::InvalidOutput {
            // Only in-memory unit-test journals omit the database. They cannot
            // have an independent concurrent connection to change the branch.
            Some(
                if store.epoch(ACTOR, operation.scope)? != epoch {
                    "stale_view"
                } else {
                    "current"
                }
                .into(),
            )
        } else {
            None
        };
    let stale_invalid =
        disposition == Disposition::InvalidOutput && applicability.as_deref() != Some("current");
    let disposition = match disposition {
        Disposition::Accepted => "accepted",
        Disposition::InvalidOutput if stale_invalid => "stale",
        Disposition::InvalidOutput => "invalid_output",
        Disposition::StaleView | Disposition::StaleBranch => "stale",
    };
    let receipt_ns = ns(operation.start)?;
    let attempt = Attempt {
        request: context.request.clone(),
        evidence_id: context.evidence_id.clone(),
        input_tokens: input,
        output_cap: operation.cap,
        disposition: disposition.into(),
        correct: Some(correct),
        elapsed_ms: receipt_ns / 1_000_000 - admission_ns / 1_000_000,
        charged_tokens: Some(usage),
    };
    operation.previous_ns = receipt_ns;
    journal.event(
        json!({"phase":"settled","case_id":operation.case_id,"attempt_id":attempt_id,
        "context":context,"attempt":attempt,"admission_ns":admission_ns,"receipt_ns":receipt_ns,
        "stored_delivery":{"disposition":store_disposition,"applicability":applicability,
            "validator":if correct {None} else {Some(VALIDATOR)},"reason":if correct {None} else {Some(operation.feedback_prompt)}},
        "ledger":ledger(store, operation.budget)?}),
    )?;
    Ok(Cell {
        context,
        attempt: Some(attempt),
    })
}
fn declined(
    store: &Store,
    journal: &mut Journal,
    operation: &Operation<'_>,
    attempt_id: i64,
    mut context: Context,
    reason: &str,
) -> Result<Cell> {
    let end_ns = ns(operation.start)?;
    context.elapsed_ms = end_ns / 1_000_000 - operation.previous_ns / 1_000_000;
    journal.event(json!({"phase":"declined","case_id":operation.case_id,"attempt_id":attempt_id,
        "context":context,"reason":reason,"operation_elapsed_ns":end_ns,"ledger":ledger(store, operation.budget)?}))?;
    Ok(Cell {
        context,
        attempt: None,
    })
}

fn collect_case<B: Backend>(
    store: &mut Store,
    backend: &mut B,
    journal: &mut Journal,
    corpus: &Corpus,
    case: &Case,
) -> Result<Value> {
    let start = Instant::now();
    let prompt = load_prompt(&corpus.prompt_version)?;
    let scope = format!("{SCOPE}/{}", case.case_id);
    let mut candidates = admit(store, &scope, &case.reports)?;
    let budget = store.create_token_budget(corpus.budget_tokens)?;
    let recovery = store.create_recovery(
        &format!("{}/{}", corpus.corpus_id, case.case_id),
        budget,
        ACTOR,
        &scope,
        BRANCH,
    )?;
    let mut operation = Operation {
        start,
        previous_ns: 0,
        recovery,
        budget,
        scope: &scope,
        case_id: &case.case_id,
        deadline_ms: corpus.deadline_ms,
        cap: corpus.output_cap,
        system_prompt: &prompt.system_prompt,
        feedback_prompt: &prompt.feedback_prompt,
    };
    journal.event(json!({"phase":"case_started","case_id":case.case_id,"budget_id":budget,"recovery_id":recovery,
        "ledger":ledger(store,budget)?}))?;
    let initial = collect_attempt(
        store,
        backend,
        journal,
        &mut operation,
        &candidates,
        case.initial_reference.as_ref(),
        &case.fixture_outputs[0],
    )?;
    let unlocked = initial.attempt.as_ref().is_some_and(|attempt| {
        attempt.disposition == "invalid_output"
            && initial.context.elapsed_ms + attempt.elapsed_ms < corpus.deadline_ms
    });
    let retries = if unlocked {
        // Fresh candidate capture follows genuine admission changes. The original
        // rejected delivery is already committed before the evidence changes.
        if let Some(reports) = &case.retry_reports {
            for &id in &candidates {
                store.set_admitted(ACTOR, &scope, id, false)?;
            }
            candidates = admit(store, &scope, reports)?;
        }
        let retry = collect_attempt(
            store,
            backend,
            journal,
            &mut operation,
            &candidates,
            case.retry_reference.as_ref(),
            &case.fixture_outputs[1],
        )?;
        let gated = Cell {
            context: retry.context.clone(),
            attempt: if retry.context.consistent {
                retry.attempt.clone()
            } else {
                None
            },
        };
        json!({"affordable_retry":retry,"evidence_gated_retry":gated})
    } else {
        json!({"affordable_retry":null,"evidence_gated_retry":null})
    };
    let final_ledger = ledger(store, budget)?;
    let charged = initial
        .attempt
        .as_ref()
        .and_then(|a| a.charged_tokens)
        .unwrap_or(0)
        + retries["affordable_retry"]["attempt"]["charged_tokens"]
            .as_i64()
            .unwrap_or(0);
    if final_ledger["spent"] != charged || final_ledger["reserved"] != 0 {
        return Err("complete trajectory does not reconcile with Store ledger".into());
    }
    journal.event(
        json!({"phase":"case_completed","case_id":case.case_id,"ledger":final_ledger,
        "operation_elapsed_ns":ns(start)?}),
    )?;
    Ok(
        json!({"case_id":case.case_id,"family":case.family,"split":case.split,
        "budget_tokens":corpus.budget_tokens,"deadline_ms":corpus.deadline_ms,
        "initial_context":initial.context,"initial":initial.attempt,"retries":retries}),
    )
}

fn collect<B: Backend>(
    store: &mut Store,
    backend: &mut B,
    journal: &mut Journal,
    corpus: &Corpus,
    live: bool,
) -> Result<Value> {
    let mut cases = Vec::new();
    for case in &corpus.cases {
        let record = collect_case(store, backend, journal, corpus, case)?;
        cases.push(record);
        journal.value["completed_cases"] = json!(cases);
        journal.persist()?;
    }
    Ok(
        json!({"schema_version":2,"mode":if live { "recorded_policy_replay" } else { "synthetic_policy_fixture" },
        "provenance":format!("{}; {}; real monotonic Store-backed timings; independent static reference labels {}; raw manifest records exact corpus, prompts, responses, and ledgers. One affordable trajectory supplies matched policy cells; no separate gated draw or timing measurement. {}",
            corpus.corpus_id, if live { "live pinned Qwen local CPU" } else { "deterministic UTF-8 byte-token fixture, zero model calls" }, VALIDATOR,
            if live { "This small corpus does not establish production reliability." } else { "Synthetic response outcomes do not measure model efficacy." }),
        "cases":cases}),
    )
}

fn make_journal(
    path: Option<PathBuf>,
    corpus: &Corpus,
    live: bool,
    database: &Path,
) -> Result<Journal> {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned());
    let version = match corpus.corpus_id.as_str() {
        "ci-report-recovery-v1" => "v1",
        "ci-report-recovery-v2" => "v2",
        _ => return Err("unknown corpus identity".into()),
    };
    let split = if corpus.cases.iter().all(|case| case.split == "calibration") {
        "calibration"
    } else {
        "all"
    };
    let prompt = load_prompt(&corpus.prompt_version)?;
    // Calibration-only artifacts contain no heldout inputs, outputs or references,
    // even inside embedded source snapshots or the corpus_utf8 field.
    let corpus_utf8 = if split == "all" {
        corpus_source(version)?.to_owned()
    } else {
        serde_json::to_string_pretty(corpus)?
    };
    let mut journal = Journal {
        path,
        value: json!({"artifact_version":1,"status":"running","events":[],"completed_cases":[],
        "actual_backend_calls":0,"mode":if live {"live_qwen"} else {"deterministic_byte_token_fixture"},
        "manifest": {"corpus":corpus,"corpus_utf8":corpus_utf8,"validator_version":VALIDATOR,"system_prompt":prompt.system_prompt,"feedback_prompt":prompt.feedback_prompt,
            "selection":{"split":split,"corpus_version":version,"case_count":corpus.cases.len()},
            "corpus_encoding":if split=="all" {"exact_frozen_source"} else {"selected_cases_only"},
            "prompt_definition":prompt,
            "prompt_definition_utf8":if version=="v2" {Some(PROMPT_V2)} else {None},
            "prompt_freeze":if version=="v2" {Some(serde_json::from_str::<Value>(PROMPT_V2_FREEZE)?)} else {None},
            "corpus_freeze":if version=="v2" {Some(serde_json::from_str::<Value>(CORPUS_V2_FREEZE)?)} else {None},
            "model":if live {Some(MODEL)} else {None},"model_revision":if live {Some(REVISION)} else {None},
            "tokenizer_revision":if live {Some(REVISION)} else {None},
            "generation":if live {json!({"do_sample":false,"num_beams":1,"max_new_tokens":corpus.output_cap,"dtype":"bfloat16"})} else {Value::Null},
            "fixture_vocabulary":if live {None} else {Some("UTF-8 bytes; not model tokens")},
            "started_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
            "working_directory_commit_best_effort":commit,"runner_source_utf8":include_str!("recovery_policy_eval.rs"),
            "adapter_source_utf8":include_str!("corrected_handoff/adapter.rs"),
            "prepared_adapter_source_utf8":include_str!("corrected_handoff/prepared_adapter.rs"),"worker_source_utf8":include_str!("../scripts/handoff_model.py"),
            "platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"logical_parallelism":std::thread::available_parallelism().ok().map(|n|n.get()),
                "cpuinfo":fs::read_to_string("/proc/cpuinfo").ok(),"kernel":fs::read_to_string("/proc/version").ok()},
            "database":database.to_string_lossy(),
            "timing":"Instant monotonic nanosecond boundaries. Replay milliseconds are differences of cumulative floored boundaries. Preparation includes Store setup, quoting, reservation, claim, and pre-I/O journal persistence; receipt includes raw response persistence, independent validation, and settlement. Model loading is separate and excluded from per-case latency. Retry context includes the preceding post-receipt journal overhead. One measured trajectory is replayed, not separate policy runtimes.",
            "policy_gate":"consistent iff exactly one complete report with nonempty release, positive total, and passed<=total; Store eligibility always enforced",
            "limitations":"Hand-authored small report corpus. Fixture outputs are deliberately scripted. No fitted policy, no heldout tuning, no confidence or production-reliability inference. No timeout preemption of already-started model generation; late received results remain charged."}}),
    };
    journal.persist()?;
    // Kept as a separate field so missing worker runtime versions are never invented.
    journal.value["manifest"]["live_runtime_metadata"] = json!(if live {
        "pinned worker startup metadata is emitted to stderr; retain stderr with this run"
    } else {
        "not applicable"
    });
    Ok(journal)
}

#[cfg(test)]
fn execute(trace: &Path, live: bool) -> Result<()> {
    execute_selected(trace, live, "v1", "all")
}
fn execute_selected(trace: &Path, live: bool, version: &str, split: &str) -> Result<()> {
    if live {
        execute_selected_backend(trace, true, version, split, || adapter::Adapter::new(true))
    } else {
        execute_selected_backend(trace, false, version, split, || Ok(Fixture))
    }
}
#[cfg(test)]
fn execute_backend<B: Backend>(
    trace: &Path,
    live: bool,
    initialize: impl FnOnce() -> Result<B>,
) -> Result<()> {
    execute_selected_backend(trace, live, "v1", "all", initialize)
}
fn execute_selected_backend<B: Backend>(
    trace: &Path,
    live: bool,
    version: &str,
    split: &str,
    initialize: impl FnOnce() -> Result<B>,
) -> Result<()> {
    let corpus = selected_corpus(version, split)?;
    if let Some(parent) = trace.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let raw = suffix(trace, ".raw.json");
    let database = suffix(trace, ".sqlite");
    // Reserve the output path first. A failed collection leaves this empty marker,
    // never a misleading complete replay. All destinations are exclusively created.
    let mut output = if split == "all" {
        Some(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(trace)?,
        )
    } else {
        if trace.try_exists()? {
            return Err("calibration destination already contains a trace".into());
        }
        None
    };
    let _raw_guard = OpenOptions::new().write(true).create_new(true).open(&raw)?;
    let _db_guard = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&database)?;
    let mut journal = make_journal(Some(raw.clone()), &corpus, live, &database)?;
    let result = (|| -> Result<Value> {
        let mut store = Store::open(&database)?;
        let startup = Instant::now();
        let mut backend = initialize()?;
        journal.value["manifest"]["backend_initialization_ns"] = json!(ns(startup)?);
        collect(&mut store, &mut backend, &mut journal, &corpus, live)
    })();
    match result {
        Ok(document) => {
            journal.value["status"] = json!("complete");
            journal.value["new_model_calls"] = if live {
                journal.value["actual_backend_calls"].clone()
            } else {
                json!(0)
            };
            journal.persist()?;
            if let Some(output) = output.as_mut() {
                output.write_all(&serde_json::to_vec_pretty(&document)?)?;
                output.write_all(b"\n")?;
                output.sync_all()?;
            }
            println!(
                "{}",
                json!({"status":"complete","trace":if split=="all" {Some(trace)} else {None},"raw":raw,"database":database,
                "artifact_kind":if split=="all" {"complete_policy_trace"} else {"calibration_only"},"split":split,"corpus_version":version,
                "cases":corpus.cases.len(),"new_model_calls":journal.value["new_model_calls"],
                "actual_backend_calls":journal.value["actual_backend_calls"],"mode":journal.value["mode"]})
            );
            Ok(())
        }
        Err(error) => {
            journal.value["status"] = json!("failed");
            journal.value["error"] = json!(error.to_string());
            journal.value["new_model_calls"] = if live {
                journal.value["actual_backend_calls"].clone()
            } else {
                json!(0)
            };
            if let Err(persist_error) = journal.persist() {
                eprintln!(
                    "raw artifact update failed: {persist_error}; inspect durable database {}",
                    database.display()
                );
            }
            Err(format!(
                "{error}; incomplete replay withheld; partial artifact {} and ledger {}",
                raw.display(),
                database.display()
            )
            .into())
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Options {
    trace: PathBuf,
    live: bool,
    corpus_version: String,
    split: String,
}
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options> {
    let mut args = args.into_iter();
    let mut options = Options {
        trace: "recovery-policy-run/trace.json".into(),
        live: false,
        corpus_version: "v1".into(),
        split: "all".into(),
    };
    let mut seen = BTreeSet::new();
    while let Some(arg) = args.next() {
        if !seen.insert(arg.clone()) {
            return Err(format!("duplicate option: {arg}").into());
        }
        if arg == "--live" {
            options.live = true;
            continue;
        }
        if !matches!(arg.as_str(), "--trace" | "--corpus-version" | "--split") {
            return Err(format!("unknown option: {arg}").into());
        }
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or_else(|| format!("{arg} requires a value"))?;
        match arg.as_str() {
            "--trace" => options.trace = value.into(),
            "--corpus-version" => options.corpus_version = value,
            "--split" => options.split = value,
            _ => unreachable!(),
        }
    }
    if !matches!(options.corpus_version.as_str(), "v1" | "v2") {
        return Err("--corpus-version must be v1 or v2".into());
    }
    if !matches!(options.split.as_str(), "all" | "calibration") {
        return Err("--split must be all or calibration".into());
    }
    Ok(options)
}
fn main() -> Result<()> {
    let options = parse_args(std::env::args().skip(1))?;
    execute_selected(
        &options.trace,
        options.live,
        &options.corpus_version,
        &options.split,
    )
}
