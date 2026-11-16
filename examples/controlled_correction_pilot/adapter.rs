//! Fixed request/IPC boundary. This module cannot open a network connection.
use super::{Result, REPORT_PROMPT, PLAN_PROMPT};
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
pub const MODEL: &str = "ornith-1.5:9b";
pub const DIGEST: &str = "e5df7dcdd8a263994df62d610317e07be0d6af23f96fcbd8543273058fce575e";
pub struct Reply { pub text: String, pub input: u64, pub output: u64 }
pub trait Backend { fn mode(&self)->&str; fn dispatch(&mut self, request:&[u8])->Result<Reply>; }
pub fn freeze(system:&str,evidence:Value)->Result<Vec<u8>> {
    if system!=REPORT_PROMPT && system!=PLAN_PROMPT { return Err("unexpected prompt".into()); }
    Ok(serde_json::to_vec(&json!({"model":MODEL,"runner":"ggml","stream":false,
        "think":false,"truncate":false,"shift":false,"keep_alive":"10m",
        "options":{"seed":0,"temperature":0,"top_k":1,"top_p":1,"num_ctx":2048,
            "num_predict":96,"num_thread":2,"num_gpu":0},
        "messages":[{"role":"system","content":system},
                    {"role":"user","content":serde_json::to_string(&evidence)?}]}))?)
}
pub struct Fixture;
impl Backend for Fixture {
    fn mode(&self)->&str { "offline_fixture" }
    fn dispatch(&mut self,request:&[u8])->Result<Reply> {
        let envelope:Value=serde_json::from_slice(request)?;
        let data:Value=serde_json::from_str(envelope["messages"][1]["content"].as_str().ok_or("missing evidence")?)?;
        let result=if let Some(r)=data.get("ci_report") {
            json!({"release":r["release"],"tests":r["test_summary"],
                "status":if r["failing_cases"]==0 {"pass"} else {"fail"}})
        } else {
            let (p,v)=(&data["requirement"],&data["test_certificate"]);
            json!({"release":p["release"],"target":p["target"],"approval":p["approval"],
                "tests":v["tests"],"action":if v["status"]=="pass" {"deploy"}else{"hold"}})
        };
        // Synthetic accounting units, deliberately unrelated to tokenizer measurements.
        Ok(Reply{text:result.to_string(),input:1,output:1})
    }
}
pub struct Bridge { ordinal:u64 }
impl Bridge { pub fn new()->Self {Self{ordinal:0}} }
impl Backend for Bridge {
    fn mode(&self)->&str { "trusted_local_bridge" }
    fn dispatch(&mut self,request:&[u8])->Result<Reply> {
        self.ordinal+=1;
        if self.ordinal>13 {return Err("call limit".into());}
        let row=json!({"op":"generate","ordinal":self.ordinal,"model_digest":DIGEST,
            "request_utf8":std::str::from_utf8(request)?});
        let mut stdout=std::io::stdout().lock();
        writeln!(stdout,"{row}")?;stdout.flush()?;drop(stdout);
        let mut line=String::new();
        let read=std::io::stdin().lock().take(1_048_577).read_line(&mut line)?;
        if read==0 || line.len()>1_048_576 {return Err("missing or oversized bridge reply".into());}
        let reply:Value=serde_json::from_str(&line)?;
        if reply["ordinal"]!=self.ordinal || reply["ok"]!=true {return Err("bridge refused or uncertain delivery".into());}
        let input=reply["input_tokens"].as_u64().ok_or("missing measured input usage")?;
        let output=reply["output_tokens"].as_u64().ok_or("missing measured output usage")?;
        if input==0 || input>2048 || output==0 || output>96 {return Err("observed usage outside envelope".into());}
        Ok(Reply{text:reply["text"].as_str().ok_or("missing text")?.to_owned(),input,output})
    }
}
