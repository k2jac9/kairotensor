//! Opt-in prepared-request operations for examples that reserve exact model usage.
#[path = "adapter.rs"]
mod basic;
pub use basic::Adapter;
use basic::{Reply, Worker};
use serde_json::json;
use std::error::Error;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Single-use quote issued by the live worker that owns the prepared tensors.
pub struct Prepared {
    handle: String,
    pub input_tokens: u64,
    pub max_new_tokens: u64,
}

pub struct PreparedReply {
    pub reply: Reply,
    pub hit_cap: bool,
    pub ended_eos: bool,
}

impl Adapter {
    fn live_worker(&mut self) -> Result<&mut Worker> {
        self.worker.as_mut().ok_or_else(|| "exact preflight requires the live worker".into())
    }

    /// Tokenize without generating. This is the exact prompt ID count for the
    /// pinned model and template. Keep the frozen bytes for dispatch accounting.
    pub fn prepare(&mut self, request: &[u8]) -> Result<Prepared> {
        let request_utf8 = std::str::from_utf8(request)?;
        let command = json!({"op":"prepare", "request_utf8":request_utf8});
        let result = self.live_worker()?.exchange(&serde_json::to_vec(&command)?)?;
        Ok(Prepared {
            handle: result["handle"].as_str().ok_or("missing preparation handle")?.to_owned(),
            input_tokens: result["input_tokens"].as_u64().ok_or("missing preflight input count")?,
            max_new_tokens: result["max_new_tokens"].as_u64().ok_or("missing preflight output cap")?,
        })
    }

    /// Use only after a successful reservation. The worker consumes the handle
    /// before generating, including when a response is lost or generation fails.
    pub fn generate_prepared(&mut self, prepared: Prepared) -> Result<PreparedReply> {
        let start = Instant::now();
        let command = json!({"op":"generate", "handle":prepared.handle,
            "input_tokens":prepared.input_tokens, "max_new_tokens":prepared.max_new_tokens});
        let response = self.live_worker()?.exchange(&serde_json::to_vec(&command)?)?;
        let mut reply = Worker::reply(&response)?;
        let output = reply.output_tokens.ok_or("missing output count")?;
        let hit_cap = response["hit_cap"].as_bool().ok_or("missing output-cap indicator")?;
        let ended_eos = response["ended_eos"].as_bool().ok_or("missing EOS indicator")?;
        if reply.input_tokens != Some(prepared.input_tokens) || output > prepared.max_new_tokens
            || hit_cap != (output == prepared.max_new_tokens)
        {
            return Err("generation did not match prepared quote".into());
        }
        reply.roundtrip_us = start.elapsed().as_micros() as u64;
        Ok(PreparedReply { reply, hit_cap, ended_eos })
    }

    pub fn discard(&mut self, prepared: Prepared) -> Result<()> {
        let command = json!({"op":"discard", "handle":prepared.handle});
        let response = self.live_worker()?.exchange(&serde_json::to_vec(&command)?)?;
        if response["discarded"].as_str() != Some(prepared.handle.as_str()) {
            return Err("worker did not confirm discard".into());
        }
        Ok(())
    }

}
