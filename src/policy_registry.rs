//! Pure policy attestations. Validation does not authenticate execution or activate a Store.
mod evaluation;
mod types;
use crate::evidence_digest::sha256_hex;
pub use evaluation::*;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
pub use types::*;
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024;
pub const MAX_EVALUATION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CASES: usize = 10_000;
fn bad(message: &str) -> rusqlite::Error {
    crate::invalid(message)
}
fn require(ok: bool, message: &str) -> rusqlite::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(bad(message))
    }
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn nonempty(s: &str) -> bool {
    !s.trim().is_empty()
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], limit: usize) -> rusqlite::Result<T> {
    require(bytes.len() <= limit, "policy record size limit")?;
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &b in bytes {
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
                    require(depth <= 64, "policy JSON depth limit")?;
                }
                b']' | b'}' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| bad("malformed policy record"))?;
                }
                _ => {}
            }
        }
    }
    serde_json::from_slice(bytes).map_err(|_| bad("malformed policy record"))
}
struct SizeWriter(usize);
impl Write for SizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_EVALUATION_BYTES)
            .ok_or_else(|| io::Error::other("policy record size limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded<T: Serialize>(v: &T) -> rusqlite::Result<()> {
    serde_json::to_writer(SizeWriter(0), v).map_err(|_| bad("policy record size limit"))
}
fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
pub fn validate_policy_artifact(bytes: &[u8]) -> rusqlite::Result<PolicyArtifactV1> {
    let a: PolicyArtifactV1 = parse(bytes, MAX_ARTIFACT_BYTES)?;
    require(
        a.schema_version == 1 && nonempty(&a.purpose) && digest(&a.compatibility_contract_sha256),
        "invalid policy artifact",
    )?;
    Ok(a)
}
pub fn validate_policy_candidate(c: &PolicyCandidate) -> rusqlite::Result<PolicyArtifactV1> {
    bounded(c)?;
    require(
        [&c.version_id, &c.actor, &c.scope, &c.purpose]
            .iter()
            .all(|s| nonempty(s)),
        "empty candidate identity",
    )?;
    let a = validate_policy_artifact(&c.artifact)?;
    require(
        c.purpose == a.purpose && digest(&c.sha256) && sha256_hex(&c.artifact) == c.sha256,
        "candidate identity mismatch",
    )?;
    Ok(a)
}
