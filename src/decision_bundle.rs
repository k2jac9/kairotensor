//! Bounded, pure decision exports. Digests establish internal consistency only;
//! neither parsing nor replay authenticates execution or grants access.
mod replay;
mod types;
use crate::evidence_digest::sha256_hex;
pub use replay::{reevaluate_bundle, validate_recorded_bundle};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    io::{self, Write},
};
pub use types::*;

pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ATTEMPTS: usize = 128;
pub const MAX_JSON_DEPTH: usize = 64;

#[derive(Debug)]
pub enum BundleError {
    InvalidSchema,
    InvalidEncoding,
    LimitExceeded,
    InconsistentLedger,
    Store(rusqlite::Error),
}
impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidSchema => "invalid decision bundle schema",
            Self::InvalidEncoding => "invalid decision bundle encoding",
            Self::LimitExceeded => "decision bundle limit exceeded",
            Self::InconsistentLedger => "inconsistent decision ledger",
            Self::Store(_) => "decision bundle store error",
        })
    }
}
impl std::error::Error for BundleError {}
impl From<rusqlite::Error> for BundleError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}
use BundleError as Error;

impl ByteEnvelope {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::LimitExceeded);
        }
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            write!(hex, "{byte:02x}").unwrap();
        }
        Ok(Self {
            hex,
            byte_len: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        })
    }
    pub fn decode(&self) -> Result<Vec<u8>, Error> {
        if self.byte_len > MAX_PAYLOAD_BYTES as u64 || self.hex.len() > MAX_PAYLOAD_BYTES * 2 {
            return Err(Error::LimitExceeded);
        }
        if self.hex.len() != self.byte_len as usize * 2
            || !lower_hex(&self.hex)
            || !digest_shape(&self.sha256)
        {
            return Err(Error::InvalidEncoding);
        }
        let bytes: Vec<u8> = self
            .hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| nibble(pair[0]) * 16 + nibble(pair[1]))
            .collect();
        if sha256_hex(&bytes) != self.sha256 {
            return Err(Error::InconsistentLedger);
        }
        Ok(bytes)
    }
}
fn nibble(b: u8) -> u8 {
    if b <= b'9' {
        b - b'0'
    } else {
        b - b'a' + 10
    }
}
fn lower_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn digest_shape(s: &str) -> bool {
    s.len() == 64 && lower_hex(s)
}

// This first pass writes to a counter, not an owned JSON tree or cloned record.
// Serde stops on the first overflowing write, including escape expansion.
struct CountWriter {
    size: usize,
}
impl Write for CountWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.size = self
            .size
            .checked_add(bytes.len())
            .filter(|&n| n <= MAX_BUNDLE_BYTES)
            .ok_or_else(|| io::Error::other("bundle limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded_value<T: Serialize>(value: &T) -> Result<serde_json::Value, Error> {
    serde_json::to_writer(&mut CountWriter { size: 0 }, value).map_err(|_| Error::LimitExceeded)?;
    serde_json::to_value(value).map_err(|_| Error::InvalidSchema)
}
fn canonical(value: &serde_json::Value) -> Result<Vec<u8>, Error> {
    // Default serde_json maps are BTreeMaps: recursively sorted UTF-8 keys.
    serde_json::to_vec(value).map_err(|_| Error::InvalidSchema)
}
fn check_depth(raw: &[u8]) -> Result<(), Error> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &b in raw {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > MAX_JSON_DEPTH {
                        return Err(Error::LimitExceeded);
                    }
                }
                b']' | b'}' => {
                    depth = depth.checked_sub(1).ok_or(Error::InvalidSchema)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
pub(super) fn decode_strict<'a, T: Deserialize<'a>>(raw: &'a [u8]) -> Result<T, Error> {
    if raw.len() > MAX_BUNDLE_BYTES {
        return Err(Error::LimitExceeded);
    }
    std::str::from_utf8(raw).map_err(|_| Error::InvalidEncoding)?;
    check_depth(raw)?;
    serde_json::from_slice(raw).map_err(|_| Error::InvalidSchema)
}
pub fn parse_decision_bundle(bytes: &[u8]) -> Result<DecisionBundleV1, Error> {
    let bundle: DecisionBundleV1 = decode_strict(bytes)?;
    check_bundle(&bundle)?;
    Ok(bundle)
}
impl DecisionBundleV1 {
    /// Canonical complete bytes, with integrity checked. Not the transport file digest.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        check_bundle(self).and_then(|v| canonical(&v))
    }
    /// Bind the current ordered export. This is an integrity operation, not authorization.
    pub fn seal(&mut self) -> Result<(), Error> {
        if self.attempts.len() > MAX_ATTEMPTS {
            return Err(Error::LimitExceeded);
        }
        preflight_envelopes(self)?;
        let mut value = bounded_value(self)?;
        value["integrity"]
            .as_object_mut()
            .ok_or(Error::InvalidSchema)?
            .remove("sha256");
        let hash = sha256_hex(&canonical(&value)?);
        // Check a bounded candidate before mutating the caller's manifest.
        value["integrity"]["sha256"] = hash.into();
        let candidate: Self = serde_json::from_value(value).map_err(|_| Error::InvalidSchema)?;
        check_bundle(&candidate)?;
        self.integrity.sha256 = candidate.integrity.sha256;
        Ok(())
    }
}
fn positive(id: i64) -> Result<(), Error> {
    if id > 0 {
        Ok(())
    } else {
        Err(Error::InconsistentLedger)
    }
}
fn nonnegative(n: i64) -> Result<(), Error> {
    if n >= 0 {
        Ok(())
    } else {
        Err(Error::InconsistentLedger)
    }
}
fn nonempty(s: &str) -> Result<(), Error> {
    if !s.is_empty() {
        Ok(())
    } else {
        Err(Error::InconsistentLedger)
    }
}
fn unique_positions<'a>(positions: impl Iterator<Item = (i64, i64)> + 'a) -> Result<(), Error> {
    let mut prior = None;
    for (position, id) in positions {
        nonnegative(position)?;
        positive(id)?;
        if prior.is_some_and(|p| p >= position) {
            return Err(Error::InconsistentLedger);
        }
        prior = Some(position);
    }
    Ok(())
}
fn check_accounting(a: &AttemptRecord) -> Result<(), Error> {
    let q = &a.accounting;
    nonnegative(q.charge)?;
    nonnegative(q.hold_bound)?;
    let bound = match (&q.quote, q.granted, q.budget_reference) {
        (None, None, None) => None,
        (Some(quote), Some(granted), Some(budget)) => {
            positive(budget)?;
            nonnegative(quote.input_tokens)?;
            positive(quote.output_cap)?;
            let cap = quote
                .input_tokens
                .checked_add(quote.output_cap)
                .ok_or(Error::InconsistentLedger)?;
            Some((quote.input_tokens, cap, granted))
        }
        _ => return Err(Error::InconsistentLedger),
    };
    match &a.delivery {
        DeliveryState::Unresolved => {
            if q.charge != 0
                || q.hold_bound
                    != bound
                        .filter(|b| {
                            b.2 && !a.recovery.as_ref().is_some_and(|r| r.state == "cancelled")
                        })
                        .map_or(0, |b| b.1)
            {
                return Err(Error::InconsistentLedger);
            }
        }
        DeliveryState::Recorded {
            usage,
            disposition,
            rejection,
            ..
        } => {
            let usage = usage.ok_or(Error::InconsistentLedger)?;
            nonnegative(usage)?;
            if usage != q.charge || q.hold_bound != 0 {
                return Err(Error::InconsistentLedger);
            }
            if bound.is_some_and(|(input, cap, granted)| !granted || usage < input || usage > cap) {
                return Err(Error::InconsistentLedger);
            }
            match (disposition, rejection) {
                (RecordedDisposition::InvalidOutput, Some(r)) => {
                    nonempty(&r.validator)?;
                    nonempty(&r.reason)?;
                    if !matches!(
                        r.applicability.as_str(),
                        "current" | "stale_view" | "stale_branch"
                    ) {
                        return Err(Error::InconsistentLedger);
                    }
                }
                (RecordedDisposition::InvalidOutput, None) => {
                    return Err(Error::InconsistentLedger)
                }
                (_, Some(_)) => return Err(Error::InconsistentLedger),
                _ => {}
            }
        }
    }
    Ok(())
}
fn check_bundle(b: &DecisionBundleV1) -> Result<serde_json::Value, Error> {
    if b.attempts.len() > MAX_ATTEMPTS {
        return Err(Error::LimitExceeded);
    }
    // Bound direct constructions before any clone, hash buffer, or owned Value.
    preflight_envelopes(b)?;
    let mut value = bounded_value(b)?;
    if b.schema_version != 1 || b.integrity.version != "decision-bundle-integrity/1" {
        return Err(Error::InvalidSchema);
    }
    positive(b.root_attempt)?;
    positive(b.snapshot.root_branch)?;
    if let Some(id) = b.snapshot.head {
        positive(id)?;
    }
    nonnegative(b.snapshot.revision)?;
    nonnegative(b.snapshot.current_epoch)?;
    nonempty(&b.actor)?;
    nonempty(&b.scope)?;
    nonempty(&b.export_purpose)?;
    if !digest_shape(&b.integrity.sha256) {
        return Err(Error::InvalidEncoding);
    }
    let mut ids = HashSet::new();
    for e in &b.evidence {
        positive(e.occurrence_id)?;
        if !ids.insert(e.occurrence_id) {
            return Err(Error::InconsistentLedger);
        }
        let mut scopes = HashSet::new();
        if e.authorized_scopes.is_empty() {
            return Err(Error::InconsistentLedger);
        }
        for scope in &e.authorized_scopes {
            nonempty(&scope.actor)?;
            nonempty(&scope.scope)?;
            if !scopes.insert((&scope.actor, &scope.scope)) {
                return Err(Error::InconsistentLedger);
            }
        }
        e.source.decode()?;
        e.payload.decode()?;
    }
    ids.clear();
    for a in &b.attempts {
        nonempty(&a.actor)?;
        nonempty(&a.scope)?;
        positive(a.id)?;
        positive(a.branch)?;
        nonnegative(a.captured_epoch)?;
        nonnegative(a.captured_revision)?;
        if !ids.insert(a.id) {
            return Err(Error::InconsistentLedger);
        }
        for id in [a.parent, a.inherited_from, a.reuse_origin]
            .into_iter()
            .flatten()
        {
            positive(id)?;
        }
        if a.inherited_from.is_some() && a.inherited_from != a.parent {
            return Err(Error::InconsistentLedger);
        }
        unique_positions(a.inputs.iter().map(|i| (i.position, i.evidence_id)))?;
        unique_positions(a.candidates.iter().map(|i| (i.position, i.evidence_id)))?;
        for input in &a.inputs {
            if b.evidence
                .iter()
                .find(|e| e.occurrence_id == input.evidence_id)
                .is_some_and(|e| {
                    !e.authorized_scopes
                        .iter()
                        .any(|s| s.actor == a.actor && s.scope == a.scope)
                })
            {
                return Err(Error::InconsistentLedger);
            }
        }
        for raw in [
            &a.dispatch_request,
            &a.immutable_runtime,
            &a.immutable_manifest,
            &a.timing_configuration,
        ]
        .into_iter()
        .flatten()
        {
            raw.decode()?;
        }
        if let DeliveryState::Recorded { raw, .. } = &a.delivery {
            raw.decode()?;
        }
        check_accounting(a)?;
        check_metadata(a)?;
        if let Some(d) = &a.deterministic {
            nonempty(&d.version)?;
            d.output.decode()?;
            if a.dispatch_request.is_some()
                || a.accounting.quote.is_some()
                || a.model_contract.is_some()
                || a.recovery.is_some()
                || a.accounting.charge != 0
            {
                return Err(Error::InconsistentLedger);
            }
            if let DeliveryState::Recorded {
                raw, disposition, ..
            } = &a.delivery
            {
                if *raw != d.output || *disposition == RecordedDisposition::InvalidOutput {
                    return Err(Error::InconsistentLedger);
                }
            }
        }
        if a.reuse_origin.is_some()
            && (a.model_contract.is_none()
                || a.dispatch_request.is_some()
                || a.accounting.charge != 0)
        {
            return Err(Error::InconsistentLedger);
        }
        if let Some(s) = &a.support {
            nonempty(&s.version)?;
            positive(s.rule_id)?;
            let mut clauses = HashSet::new();
            for c in &s.clauses {
                positive(c.clause_id)?;
                if !clauses.insert(c.clause_id) || c.premises.is_empty() {
                    return Err(Error::InconsistentLedger);
                }
                let mut premises = HashSet::new();
                for p in &c.premises {
                    positive(p.evidence_id)?;
                    nonempty(&p.actor)?;
                    nonempty(&p.scope)?;
                    if !premises.insert(p.evidence_id) {
                        return Err(Error::InconsistentLedger);
                    }
                }
            }
        }
        if let Some(r) = &a.recovery {
            positive(r.recovery_id)?;
            nonnegative(r.ordinal)?;
            nonempty(&r.state)?;
            if let Some(request) = &r.request {
                request.decode()?;
            }
            if let Some(dispatched) = &a.dispatch_request {
                if r.request.as_ref() != Some(dispatched) || r.state != "sent" {
                    return Err(Error::InconsistentLedger);
                }
            }
            if matches!(r.state.as_str(), "reserved" | "sent" | "denied") && r.request.is_none() {
                return Err(Error::InconsistentLedger);
            }
            if r.state == "sent" && a.dispatch_request.is_none() {
                return Err(Error::InconsistentLedger);
            }
            let reservation_valid = match r.state.as_str() {
                "planned" => a.accounting.quote.is_none() && r.request.is_none(),
                "reserved" | "sent" => a.accounting.granted == Some(true),
                "denied" => a.accounting.granted == Some(false),
                "cancelled" => a.accounting.granted != Some(false),
                _ => true, // Retain unknown historical phase; renderer will refuse.
            };
            if !reservation_valid {
                return Err(Error::InconsistentLedger);
            }

            if let Some(id) = r.predecessor_id {
                positive(id)?;
            }
            if matches!(a.delivery, DeliveryState::Recorded { .. }) && r.state != "sent" {
                return Err(Error::InconsistentLedger);
            }
        }
    }
    check_recovery_runs(b)?;
    for a in &b.attempts {
        if let Some(origin) = a
            .reuse_origin
            .and_then(|id| b.attempts.iter().find(|d| d.id == id))
        {
            if let (
                DeliveryState::Recorded { raw, .. },
                DeliveryState::Recorded {
                    raw: prior,
                    disposition,
                    ..
                },
            ) = (&a.delivery, &origin.delivery)
            {
                if raw != prior
                    || *disposition != RecordedDisposition::Accepted
                    || origin.dispatch_request.is_none()
                    || a.model_contract != origin.model_contract
                    || a.inputs != origin.inputs
                    || a.candidates != origin.candidates
                {
                    return Err(Error::InconsistentLedger);
                }
            }
        }
    }
    let root = b
        .attempts
        .iter()
        .find(|a| a.id == b.root_attempt)
        .ok_or(Error::InconsistentLedger)?;
    if root.branch != b.snapshot.root_branch || root.actor != b.actor || root.scope != b.scope {
        return Err(Error::InconsistentLedger);
    }
    value["integrity"].as_object_mut().unwrap().remove("sha256");
    if sha256_hex(&canonical(&value)?) != b.integrity.sha256 {
        return Err(Error::InconsistentLedger);
    }
    value["integrity"]["sha256"] = b.integrity.sha256.clone().into();
    // Aggregate arithmetic checked independently for each unit; no global budget totals.
    for unit in [
        AccountingUnit::LogicalSupport,
        AccountingUnit::FixtureBytes,
        AccountingUnit::ModelTokens,
        AccountingUnit::Unknown,
    ] {
        let (mut charge, mut held) = (0i64, 0i64);
        for a in b.attempts.iter().filter(|a| a.accounting.unit == unit) {
            charge = charge
                .checked_add(a.accounting.charge)
                .ok_or(Error::InconsistentLedger)?;
            held = held
                .checked_add(a.accounting.hold_bound)
                .ok_or(Error::InconsistentLedger)?;
        }
        charge.checked_add(held).ok_or(Error::InconsistentLedger)?;
    }
    Ok(value)
}

// A recovery's primary key and immutable run facts are shared across phases.
// Quotes are per-attempt: an unsent phase may have no reservation, or a quote
// that timed admission refused. Only a recorded sent phase requires coverage.
fn check_recovery_runs(bundle: &DecisionBundleV1) -> Result<(), Error> {
    let mut keys = HashSet::new();
    let mut runs: HashMap<i64, &AttemptRecord> = HashMap::new();
    let mut budgets = HashMap::new();
    for attempt in &bundle.attempts {
        let Some(recovery) = &attempt.recovery else {
            continue;
        };
        if !keys.insert((recovery.recovery_id, recovery.ordinal)) {
            return Err(Error::InconsistentLedger);
        }
        if let Some(prior) = runs.insert(recovery.recovery_id, attempt) {
            if attempt.actor != prior.actor
                || attempt.scope != prior.scope
                || attempt.branch != prior.branch
                || attempt.timing_configuration != prior.timing_configuration
                || attempt.immutable_runtime != prior.immutable_runtime
                || attempt.immutable_manifest != prior.immutable_manifest
            {
                return Err(Error::InconsistentLedger);
            }
        }
        // Absence here means this phase has no token_reservations row; it does
        // not erase or contradict a budget reference recorded by another phase.
        if let Some(budget) = attempt.accounting.budget_reference {
            if budgets
                .insert(recovery.recovery_id, budget)
                .is_some_and(|prior| prior != budget)
            {
                return Err(Error::InconsistentLedger);
            }
        }
        if recovery.state == "sent" {
            if let Some(configuration) = &attempt.timing_configuration {
                let timing: crate::RecoveryTiming = decode_strict(&configuration.decode()?)
                    .map_err(|_| Error::InconsistentLedger)?;
                let quote = attempt
                    .accounting
                    .quote
                    .as_ref()
                    .ok_or(Error::InconsistentLedger)?;
                if quote.output_cap != timing.calibration.output_cap
                    || quote.input_tokens > timing.calibration.max_input_tokens
                {
                    return Err(Error::InconsistentLedger);
                }
            }
        }
    }
    Ok(())
}

// Check every envelope's borrowed lengths before the outer JSON allocation.
fn preflight_envelopes(b: &DecisionBundleV1) -> Result<(), Error> {
    fn check(e: &ByteEnvelope) -> Result<(), Error> {
        if e.byte_len > MAX_PAYLOAD_BYTES as u64 || e.hex.len() > MAX_PAYLOAD_BYTES * 2 {
            Err(Error::LimitExceeded)
        } else {
            Ok(())
        }
    }
    for e in &b.evidence {
        check(&e.source)?;
        check(&e.payload)?;
    }
    for a in &b.attempts {
        for e in [
            &a.dispatch_request,
            &a.immutable_runtime,
            &a.immutable_manifest,
            &a.timing_configuration,
        ]
        .into_iter()
        .flatten()
        {
            check(e)?;
        }
        if let DeliveryState::Recorded { raw, .. } = &a.delivery {
            check(raw)?;
        }
        if let Some(d) = &a.deterministic {
            check(&d.output)?;
        }
        if let Some(r) = &a.recovery {
            if let Some(raw) = &r.request {
                check(raw)?;
            }
        }
    }
    Ok(())
}
fn check_metadata(a: &AttemptRecord) -> Result<(), Error> {
    let report = a
        .deterministic
        .as_ref()
        .is_some_and(|d| d.version == "report-verdict/v1");
    let support = a
        .support
        .as_ref()
        .is_some_and(|s| s.version == "support/v1");
    let incident = a
        .immutable_manifest
        .as_ref()
        .and_then(|e| e.decode().ok())
        .is_some_and(|raw| replay::recognized_incident_manifest(&raw));
    for (index, input) in a.inputs.iter().enumerate() {
        let expected = if report {
            InputRole::Report
        } else if incident {
            if index == 0 {
                InputRole::Context
            } else {
                InputRole::Report
            }
        } else if support {
            InputRole::Evidence
        } else {
            InputRole::Unknown
        };
        if input.role != expected {
            return Err(Error::InconsistentLedger);
        }
    }
    match a.accounting.unit {
        AccountingUnit::Unknown => {}
        AccountingUnit::LogicalSupport if report || support => {}
        AccountingUnit::FixtureBytes if incident && replay::fixture_runtime(a) => {}
        _ => return Err(Error::InconsistentLedger),
    }
    if let Some(v) = &a.validator {
        nonempty(&v.version)?;
        let valid = match v.provenance {
            ValidatorProvenance::Unknown => true,
            ValidatorProvenance::DeterministicBinding => a
                .deterministic
                .as_ref()
                .is_some_and(|d| d.version == v.version),
            ValidatorProvenance::DeclaredContract => a.immutable_manifest.is_some(),
            ValidatorProvenance::RecordedRejection => {
                matches!(&a.delivery,DeliveryState::Recorded{rejection:Some(r),..} if r.validator==v.version)
            }
        };
        if !valid {
            return Err(Error::InconsistentLedger);
        }
    }
    Ok(())
}
