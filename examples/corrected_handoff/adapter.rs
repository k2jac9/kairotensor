//! Freeze one client-visible JSON request and give those exact bytes to the adapter.
use serde_json::{json, Value};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MODEL: &str = "Qwen/Qwen2.5-1.5B-Instruct";
const REVISION: &str = "989aa7980e4cf806f80c7fef2b1adb7bc71aa306";

pub struct Reply {
    pub text: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub generation_us: u64,
    pub roundtrip_us: u64,
}

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

pub struct Adapter {
    worker: Option<Worker>,
}

impl Adapter {
    pub fn new(live: bool) -> Result<Self> {
        Ok(Self { worker: if live { Some(Worker::start()?) } else { None } })
    }

    pub fn freeze(&self, messages: Value) -> Result<Vec<u8>> {
        self.freeze_with_cap(messages, 96)
    }

    pub fn freeze_with_cap(&self, messages: Value, cap: u64) -> Result<Vec<u8>> {
        if !(1..=96).contains(&cap) {
            return Err("output cap must be between 1 and 96".into());
        }
        let request = if self.worker.is_some() {
            json!({
                "adapter": "transformers-cpu/v1", "model": MODEL, "revision": REVISION,
                "messages": messages, "dtype": "bfloat16", "add_generation_prompt": true,
                "generation": {"do_sample": false, "num_beams": 1, "max_new_tokens": cap}
            })
        } else {
            if cap != 96 {
                return Err("variable caps require the live worker".into());
            }
            json!({"adapter": "fixture/v1", "messages": messages})
        };
        Ok(serde_json::to_vec(&request)?)
    }

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

    pub fn dispatch(&mut self, request: &[u8]) -> Result<Reply> {
        let start = Instant::now();
        let mut reply = match self.worker.as_mut() {
            Some(worker) => Worker::reply(&worker.exchange(request)?)?,
            None => {
                let envelope: Value = serde_json::from_slice(request)?;
                if envelope["adapter"] != "fixture/v1" {
                    return Err("unexpected fixture adapter envelope".into());
                }
                let messages = envelope["messages"].as_array().ok_or("missing messages")?;
                let user = messages.last().filter(|m| m["role"] == "user")
                    .and_then(|m| m["content"].as_str()).ok_or("missing final user message")?;
                let data: Value = serde_json::from_str(user)?;
                let text = fixture_answer(&data)?.to_string();
                Reply {
                    text, input_tokens: None, output_tokens: None,
                    generation_us: start.elapsed().as_micros() as u64, roundtrip_us: 0,
                }
            }
        };
        reply.roundtrip_us = start.elapsed().as_micros() as u64;
        Ok(reply)
    }
}

// The fixture mirrors the two structured extraction tasks without token estimates.
fn fixture_answer(data: &Value) -> Result<Value> {
    fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
        value[name].as_str().ok_or_else(|| format!("missing fixture field {name}").into())
    }
    if let Some(report) = data.get("ci_report") {
        let failed = report["failing_cases"].as_u64().ok_or("missing failing_cases")?;
        return Ok(json!({
            "release": field(report, "release")?, "tests": field(report, "test_summary")?,
            "status": if failed == 0 { "pass" } else { "fail" }
        }));
    }
    let requirement = &data["requirement"];
    let report = &data["test_certificate"];
    let release = field(requirement, "release")?;
    if release != field(report, "release")? {
        return Err("release and verified report disagree".into());
    }
    Ok(json!({
        "release": release, "target": field(requirement, "target")?,
        "approval": field(requirement, "approval")?, "tests": field(report, "tests")?,
        "action": if report["status"] == "fail" { "hold" } else { "deploy" }
    }))
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Worker {
    fn start() -> Result<Self> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/handoff_model.py");
        let mut child = Command::new("python3")
            .arg("-u").arg(path).args([MODEL, REVISION])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("missing worker stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("missing worker stdout")?);
        let mut worker = Self { child, stdin, stdout };
        let ready = worker.read_json()?;
        if ready["ready"] != true || ready["model"] != MODEL || ready["revision"] != REVISION {
            return Err("worker initialization did not confirm pinned model".into());
        }
        eprintln!("model worker ready: {ready}");
        Ok(worker)
    }

    fn read_json(&mut self) -> Result<Value> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err("model worker exited before sending a response".into());
        }
        Ok(serde_json::from_str(&line)?)
    }

    fn exchange(&mut self, request: &[u8]) -> Result<Value> {
        if request.contains(&b'\n') {
            return Err("request must be one JSONL record".into());
        }
        self.stdin.write_all(request)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let response = self.read_json()?;
        if let Some(error) = response.get("error").and_then(Value::as_str) {
            return Err(format!("model worker: {error}").into());
        }
        Ok(response)
    }

    fn reply(response: &Value) -> Result<Reply> {
        let count = |key: &str| -> Result<u64> {
            Ok(response[key].as_u64().ok_or("missing worker measurement")?)
        };
        Ok(Reply {
            text: response["text"].as_str().ok_or("missing model output")?.to_owned(),
            input_tokens: Some(count("input_tokens")?),
            output_tokens: Some(count("output_tokens")?),
            generation_us: count("generation_us")?,
            roundtrip_us: 0,
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Reap the worker even when a checklist or I/O check ends the run early.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
