//! Isolated calibration pilot. No network access; LIVE transport belongs to the reviewed supervisor.
#[path = "controlled_correction_pilot/adapter.rs"] mod adapter;
use adapter::{Backend, Reply};
use kairotensor::{Current, Disposition, ModelAction, Store};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json,Value};
use std::{fs,io::Write,path::{Path,PathBuf},time::Instant};
type Result<T> = std::result::Result<T,Box<dyn std::error::Error>>;
const ACTOR:&str="release-planner";
const SCOPE:&str="R42";
const CONTRACT:&str="ci-report-extractor/2+strict-verdict-validator/1+ollama-pilot/1";
const SLOTS:[&str;13]=["bootstrap/report-a","bootstrap/report-b","bootstrap/plan",
    "report_correction/repair/report","report_correction/repair/plan",
    "report_correction/fresh/report","report_correction/fresh/plan",
    "unrelated_policy_change/fresh/report","unrelated_policy_change/fresh/plan",
    "unrelated_policy_change/repair/plan","alternative_support/fresh/report",
    "alternative_support/fresh/plan","alternative_support/repair/plan"];
const BEFORE: &[u8] = br#"{"version":"policy/v1","release":"R42","target":"staging","approval":"S42-A"}"#;
const AFTER: &[u8] = br#"{"version":"policy/v2","release":"R42","target":"canary","approval":"C42-B"}"#;
const GREEN: &[u8] = br#"{"version":"run/17","release":"R42","test_summary":"12/12","failing_cases":0}"#;
const RED: &[u8] = br#"{"version":"run/18","release":"R42","test_summary":"11/12","failing_cases":1}"#;
// The same balanced examples are shown for every run and release.
const REPORT_PROMPT: &str = r#"Extract the release test verdict. Even ONE failing case makes the entire verdict "fail".
CI example A: {"release":"EX1","test_summary":"7/8","failing_cases":1} -> {"release":"EX1","tests":"7/8","status":"fail"}
CI example B: {"release":"EX2","test_summary":"8/8","failing_cases":0} -> {"release":"EX2","tests":"8/8","status":"pass"}
Read the user's ci_report. Copy release and test_summary. If failing_cases is positive use "fail"; only zero uses "pass". Return only JSON with keys release, tests, status."#;
const PLAN_PROMPT: &str = r#"Create a JSON release plan. Copy release, target and approval from requirement. Copy tests from test_certificate. The certificate decides the action: {"status":"pass"} means {"action":"deploy"}; {"status":"fail"} means {"action":"hold"} even with an approval code. Return exactly five string fields: release, target, approval, tests, action. No explanation."#;

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Verdict { release: String, tests: String, status: String }

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plan { release: String, target: String, approval: String, tests: String, action: String }

fn parse_answer<T: DeserializeOwned>(text: &str) -> Result<T> {
    let text = text.trim();
    let body = if let Some(inner) = text.strip_prefix("```json\n") {
        inner.strip_suffix("```").ok_or("unclosed JSON fence")?.trim()
    } else { text };
    Ok(serde_json::from_str(body)?)
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name].as_str().ok_or_else(|| format!("missing source field {name}").into())
}

fn validate_verdict(text: &str, report: &Value) -> Result<Verdict> {
    let actual: Verdict = parse_answer(text)?;
    let failed = report["failing_cases"].as_u64().ok_or("missing problem count")?;
    let status = if failed == 0 { "pass" } else { "fail" };
    if actual.release != field(report, "release")? || actual.tests != field(report, "test_summary")?
        || actual.status != status {
        return Err("test verdict disagrees with captured evidence".into());
    }
    Ok(actual)
}

fn validate_plan(text: &str, requirement: &Value, verdict: &Verdict) -> Result<Plan> {
    let actual: Plan = parse_answer(text)?;
    let action = if verdict.status == "pass" { "deploy" } else { "hold" };
    if actual.release != field(requirement, "release")? || actual.release != verdict.release
        || actual.target != field(requirement, "target")? || actual.approval != field(requirement, "approval")?
        || actual.tests != verdict.tests || actual.action != action {
        return Err("release plan failed the checklist".into());
    }
    Ok(actual)
}

fn write_new(path:&Path,value:&Value)->Result<()> {
    let mut f=fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer(&mut f,value)?;f.write_all(b"\n")?;f.sync_all()?;
    fs::File::open(path.parent().ok_or("missing parent")?)?.sync_all()?;Ok(())
}
struct Journal<'a> { out:&'a Path, next:usize, dispatches:u64, reuse_hits:u64, backend:&'a mut dyn Backend, pairs:std::collections::BTreeMap<String,Vec<u8>> }
impl<'a> Journal<'a> {
    fn new(out:&'a Path,backend:&'a mut dyn Backend)->Self {Self{out,next:0,dispatches:0,reuse_hits:0,backend,pairs:std::collections::BTreeMap::new()}}
    fn event(&mut self,value:Value)->Result<()> {self.next+=1;write_new(&self.out.join(format!("event-{:03}.json",self.next)),&value)}
    fn deliver<T>(&mut self,store:&mut Store,attempt:i64,request:&[u8],context:Value,
        check:impl Fn(&str)->Result<T>,validator:&str,reuse:bool)->Result<(T,String,i64,bool)> {
        if let Some((case,_))=context["route"].as_str().and_then(|r|r.rsplit_once('/')) {
            let key=format!("{case}/{}",context["task"].as_str().ok_or("missing task")?);
            if let Some(previous)=self.pairs.get(&key) {
                if previous!=request {return Err("paired request mismatch before dispatch".into());}
            }else{self.pairs.insert(key,request.to_vec());}
        }
        self.event(json!({"kind":"prepared","attempt":attempt,"context":context,
            "request_utf8":std::str::from_utf8(request)?}))?;
        let action=if reuse {store.resolve_model_attempt(attempt,request)?} else {
            store.record_dispatch(attempt,request)?;ModelAction::Dispatch
        };
        match action {
            ModelAction::Reused{origin_attempt,output}=>{
                let text=String::from_utf8(output)?;let value=check(&text)?;
                self.reuse_hits+=1;
                self.event(json!({"kind":"reused","attempt":attempt,"origin":origin_attempt,
                    "context":context,"output":text,"new_usage":0}))?;
                Ok((value,text,origin_attempt,true))
            },
            ModelAction::Dispatch=>{
                if self.dispatches>=13{return Err("global call ceiling".into());}
                self.dispatches+=1;
                self.event(json!({"kind":"dispatch","ordinal":self.dispatches,"attempt":attempt,"context":context}))?;
                let start=Instant::now();
                let Reply{text,input,output}=match self.backend.dispatch(request) {
                    Ok(reply)=>reply,
                    Err(error)=>{self.event(json!({"kind":"uncertain","attempt":attempt,
                        "context":context,"error":error.to_string()}))?;return Err(error);}
                };
                let usage=i64::try_from(input.checked_add(output).ok_or("usage overflow")?)?;
                self.event(json!({"kind":"received","attempt":attempt,"context":context,"output":text,
                    "input_units":input,"output_units":output,"roundtrip_us":start.elapsed().as_micros()}))?;
                let value=match check(&text) {
                    Ok(v)=>v,
                    Err(error)=>{
                        let disposition=store.reject_invalid(attempt,text.as_bytes(),usage,validator,&error.to_string())?;
                        self.event(json!({"kind":"invalid_output","attempt":attempt,"context":context,
                            "usage":usage,"store_disposition":format!("{disposition:?}")}))?;
                        return Err(error);
                    }
                };
                let disposition=store.settle(attempt,text.as_bytes(),usage)?;
                self.event(json!({"kind":"settled","attempt":attempt,"context":context,
                    "usage":usage,"store_disposition":format!("{disposition:?}")}))?;
                if disposition!=Disposition::Accepted{return Err("noncurrent publication".into());}
                Ok((value,text,attempt,false))
            }
        }
    }
}
#[derive(Clone,Copy)] struct Ids{policy:i64,a:i64,b:i64,red:i64,after:i64}
fn setup(path:&Path)->Result<(Store,Ids)> {
    let mut s=Store::open(path)?;
    let ids=s.observe_and_admit_many(ACTOR,SCOPE,&[("policy/v1",BEFORE),("run/17-a",GREEN),("run/17-b",GREEN)])?;
    let red=s.observe("run/18",RED)?;let after=s.observe("policy/v2",AFTER)?;
    s.publish_support(ACTOR,SCOPE,"alternative-support",&[vec![ids[1]],vec![ids[2]]])?;
    Ok((s,Ids{policy:ids[0],a:ids[1],b:ids[2],red,after}))
}
struct Certificate { value:Verdict,raw:String,attempt:i64,origin:i64,request:Vec<u8>,report:i64,branch:String,hit:bool }
fn certificate(s:&mut Store,j:&mut Journal<'_>,ids:Ids,branch:&str,context:&str,reuse:bool)->Result<Option<Certificate>> {
    let candidates=if branch=="report-b" {vec![ids.b]}else{vec![ids.a,ids.red]};
    let (attempt,captured)=if reuse {s.begin_model_attempt(ACTOR,SCOPE,branch,&candidates,CONTRACT)?}
        else {s.begin_from_candidates(ACTOR,SCOPE,branch,&candidates)?};
    if captured.is_empty(){j.event(json!({"kind":"unavailable_evidence","context":context,"attempt":attempt}))?;return Ok(None);}
    let [(report,payload)]=captured.as_slice() else{return Err("ambiguous report capture".into())};
    let value:Value=serde_json::from_slice(payload)?;
    let request=adapter::freeze(REPORT_PROMPT,json!({"ci_report":value}))?;
    let (verdict,raw,origin,hit)=j.deliver(s,attempt,&request,json!({"route":context,"task":branch}),
        |text|validate_verdict(text,&value),"strict-verdict-validator/1",reuse)?;
    Ok(Some(Certificate{value:verdict,raw,attempt,origin,request,report:*report,branch:branch.to_owned(),hit}))
}
fn plan(s:&mut Store,j:&mut Journal<'_>,policy:i64,cert:&Certificate,context:&str)->Result<(Plan,Vec<u8>)> {
    let required=[policy,cert.report];
    let (attempt,captured)=s.begin_from_candidates(ACTOR,SCOPE,"release-plan",&required)?;
    if captured.iter().map(|(id,_)|*id).ne(required) || s.read_current(ACTOR,SCOPE,&cert.branch)?!=Current::Ready(cert.raw.as_bytes().to_vec()) {
        return Err("planner capture or certificate not current".into());
    }
    let requirement:Value=serde_json::from_slice(&captured[0].1)?;
    let report:Value=serde_json::from_slice(&captured[1].1)?;
    let request=adapter::freeze(PLAN_PROMPT,json!({"requirement":requirement,
        "test_certificate":cert.value,"report_version":report["version"]}))?;
    let (answer,_,_,_)=j.deliver(s,attempt,&request,json!({"route":context,"task":"release-plan","certificate_attempt":cert.attempt}),
        |text|validate_plan(text,&requirement,&cert.value),"scoped-release-plan-checklist/1",false)?;
    Ok((answer,request))
}
fn route(s:&mut Store,j:&mut Journal<'_>,ids:Ids,case:&str,reuse:bool)->Result<Value> {
    let start=Instant::now();let offset=j.dispatches;
    let context=format!("{case}/{}",if reuse{"repair"}else{"fresh"});
    // Selection is based only on actual admitted source capture, never a label/output.
    let (_,primary)=s.begin_from_candidates(ACTOR,SCOPE,"selection-primary",&[ids.a,ids.red])?;
    let branch=if primary.is_empty(){"report-b"}else{"report-a"};
    let (_,policies)=s.begin_from_candidates(ACTOR,SCOPE,"selection-policy",&[ids.policy,ids.after])?;
    let [(policy,_)]=policies.as_slice() else{return Err("expected one current policy".into())};
    let Some(cert)=certificate(s,j,ids,branch,&context,reuse)? else {
        if matches!(s.read_current(ACTOR,SCOPE,"release-plan")?,Current::Ready(_)){return Err("withdrawn planner still readable".into());}
        return Ok(json!({"outcome":"unavailable_evidence","dispatches":0,"plan":null,"requests":[]}));
    };
    let (answer,request)=plan(s,j,*policy,&cert,&context)?;
    Ok(json!({"outcome":"accepted","dispatches":j.dispatches-offset,"report_branch":branch,
        "origin":cert.origin,"reused":cert.hit,"plan":answer,"route_us":start.elapsed().as_micros(),
        "requests":[std::str::from_utf8(&cert.request)?,std::str::from_utf8(&request)?]}))
}
fn mutate(s:&mut Store,ids:Ids,case:&str)->Result<bool> {
    match case {
        "report_correction"=>{s.set_admitted(ACTOR,SCOPE,ids.a,false)?;s.set_admitted(ACTOR,SCOPE,ids.b,false)?;s.set_admitted(ACTOR,SCOPE,ids.red,true)?;},
        "unrelated_policy_change"=>{s.set_admitted(ACTOR,SCOPE,ids.policy,false)?;s.set_admitted(ACTOR,SCOPE,ids.after,true)?;},
        "withdrawal"=>{s.set_admitted(ACTOR,SCOPE,ids.a,false)?;s.set_admitted(ACTOR,SCOPE,ids.b,false)?;},
        "alternative_support"=>{s.set_admitted(ACTOR,SCOPE,ids.a,false)?;},
        _=>return Err("unknown scenario".into())
    }
    s.revise_support(ACTOR,SCOPE,"alternative-support")?;
    let (_,active)=s.begin_from_candidates(ACTOR,SCOPE,"support-oracle",&[ids.a,ids.b])?;
    let oracle=!active.is_empty();
    let expected=if oracle{b"supported".to_vec()}else{b"unsupported".to_vec()};
    if s.read_current(ACTOR,SCOPE,"alternative-support")?!=Current::Ready(expected){return Err("typed support differs from admission oracle".into());}
    if matches!(s.read_current(ACTOR,SCOPE,"release-plan")?,Current::Ready(_)){return Err("old planner unexpectedly current".into());}
    Ok(oracle)
}
fn snapshot(source:&Path,target:&Path)->Result<()> {
    // Store uses rollback journaling. Only copy a closed, complete database.
    for suffix in ["-wal","-journal"] {
        if PathBuf::from(format!("{}{suffix}",source.display())).exists(){return Err("uncheckpointed Store".into());}
    }
    let check=rusqlite::Connection::open_with_flags(source,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity:String=check.query_row("PRAGMA integrity_check",[],|r|r.get(0))?;
    if integrity!="ok"{return Err("checkpoint integrity failure".into());}drop(check);
    let mut input=fs::File::open(source)?;let mut output=fs::OpenOptions::new().create_new(true).write(true).open(target)?;
    std::io::copy(&mut input,&mut output)?;output.sync_all()?;Ok(())
}
fn run_pilot(out:&Path,backend:&mut dyn Backend)->Result<Value> {
    fs::create_dir(out)?;
    let mode=backend.mode().to_owned();
    let result=run_owned(out,backend);
    let mut report=match &result {Ok(value)=>value.clone(),Err(error)=>json!({"complete":false,"error":error.to_string(),"mode":mode})};
    let mut dispatched=0usize;
    let mut completed=std::collections::BTreeSet::new();
    for entry in fs::read_dir(out)? {
        let path=entry?.path();
        if !path.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with("event-")){continue;}
        let value:Value=serde_json::from_slice(&fs::read(path)?)?;
        if value["kind"]=="dispatch"{dispatched+=1;}
        if value["kind"]=="pair_complete" {completed.insert(value["result"]["scenario"].as_str().ok_or("missing scenario")?.to_owned());}
    }
    report["unexecuted_slots"]=json!(&SLOTS[dispatched.min(SLOTS.len())..]);
    report["unexecuted_scenarios"]=json!(["report_correction","unrelated_policy_change","withdrawal","alternative_support"]
        .into_iter().filter(|name|!completed.contains(*name)).collect::<Vec<_>>());
    report["rust_dispatch_intents"]=json!(dispatched);
    write_new(&out.join("result.json"),&report)?;
    result
}
fn run_owned(out:&Path,backend:&mut dyn Backend)->Result<Value> {
    write_new(&out.join("manifest.json"),&json!({"schema":"controlled-correction-pilot/1","mode":backend.mode(),
        "model":adapter::MODEL,"model_digest":adapter::DIGEST,"max_calls":13,"planned_slots":SLOTS,
        "dataset":"existing scoped_model_reuse fixtures",
        "accounting_units":if backend.mode()=="offline_fixture"{"synthetic fixture units"}else{"provider reported tokens"}}))?;
    let mut j=Journal::new(out,backend);
    let base=out.join("bootstrap.sqlite");let (mut original,ids)=setup(&base)?;
    let a=certificate(&mut original,&mut j,ids,"report-a","bootstrap",true)?.ok_or("bootstrap A absent")?;
    let b=certificate(&mut original,&mut j,ids,"report-b","bootstrap",true)?.ok_or("bootstrap B absent")?;
    plan(&mut original,&mut j,ids.policy,&a,"bootstrap")?;
    drop(original);
    let bootstrap=json!({"a_origin":a.origin,"b_origin":b.origin,"dispatches":j.dispatches});
    let mut scenarios=Vec::new();
    for (case,repair_first) in [("report_correction",true),("unrelated_policy_change",false),("withdrawal",true),("alternative_support",false)] {
        let repair_path=out.join(format!("{case}-repair.sqlite"));snapshot(&base,&repair_path)?;
        let mut repair=Store::open(&repair_path)?;
        let (mut fresh,fresh_ids)=setup(&out.join(format!("{case}-fresh.sqlite")))?;
        let support=mutate(&mut repair,ids,case)?;
        if mutate(&mut fresh,fresh_ids,case)?!=support{return Err("different final support views".into());}
        let (r,f)=if repair_first {
            let r=route(&mut repair,&mut j,ids,case,true)?;let f=route(&mut fresh,&mut j,fresh_ids,case,false)?;(r,f)
        }else{
            let f=route(&mut fresh,&mut j,fresh_ids,case,false)?;let r=route(&mut repair,&mut j,ids,case,true)?;(r,f)
        };
        if r["requests"]!=f["requests"] || r["plan"]!=f["plan"] || r["outcome"]!=f["outcome"] {
            return Err("paired requests or checked outputs differ".into());
        }
        let row=json!({"scenario":case,"typed_support":support,"repair":r,"fresh":f,"requests_equal":true});
        j.event(json!({"kind":"pair_complete","result":row}))?;scenarios.push(row);
    }
    Ok(json!({"complete":true,"mode":j.backend.mode(),"dispatches":j.dispatches,"reuse_hits":j.reuse_hits,
        "bootstrap":bootstrap,"scenarios":scenarios}))
}
fn main()->Result<()> {
    let mut args=std::env::args().skip(1);
    let mode=args.next().ok_or("use --fixture or --bridge, followed by a new output directory")?;
    let out=PathBuf::from(args.next().ok_or("missing output directory")?);
    if args.next().is_some(){return Err("unexpected argument".into());}
    let mut backend:Box<dyn Backend>=match mode.as_str(){"--fixture"=>Box::new(adapter::Fixture),"--bridge"=>Box::new(adapter::Bridge::new()),_=>return Err("invalid mode".into())};
    run_pilot(&out,backend.as_mut()).map(|_|())
}
#[cfg(test)] #[path = "controlled_correction_pilot/tests.rs"] mod tests;
