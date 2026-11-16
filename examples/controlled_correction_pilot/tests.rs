use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self { Self(std::env::temp_dir().join(format!("kt-pilot-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)))) }
}
impl Drop for Scratch { fn drop(&mut self) { let _=fs::remove_dir_all(&self.0); } }

#[test]
fn actual_store_pairs_recompute_and_refuse_withdrawn_origins() {
    let scratch=Scratch::new();
    let result=run_pilot(&scratch.0,&mut adapter::Fixture).expect("pilot must execute the Store scenarios");
    assert_eq!(result["dispatches"],13);
    assert_eq!(result["reuse_hits"],2);
    assert_eq!(result["scenarios"].as_array().unwrap().len(),4);
    let cases=result["scenarios"].as_array().unwrap();
    assert_eq!(cases[0]["repair"]["plan"]["action"],"hold");
    assert_eq!(cases[1]["repair"]["plan"]["target"],"canary");
    assert_eq!(cases[2]["repair"]["outcome"],"unavailable_evidence");
    assert_eq!(cases[2]["fresh"]["dispatches"],0);
    assert_eq!(cases[3]["repair"]["report_branch"],"report-b");
    assert_eq!(cases[3]["repair"]["origin"],result["bootstrap"]["b_origin"]);
    assert_ne!(cases[3]["repair"]["origin"],result["bootstrap"]["a_origin"]);
    for (case,support) in cases.iter().zip([false,true,false,true]) {
        assert_eq!(case["typed_support"],support);
        assert_eq!(case["requests_equal"],true);
        assert_eq!(case["repair"]["requests"],case["fresh"]["requests"]);
        assert_eq!(case["repair"]["plan"],case["fresh"]["plan"]);
    }
    assert!(run_pilot(&scratch.0,&mut adapter::Fixture).is_err(),"existing evidence cannot be overwritten");
}

#[test]
fn paired_mismatch_is_refused_before_second_dispatch() {
    let scratch=Scratch::new();fs::create_dir(&scratch.0).unwrap();
    let mut fixture=adapter::Fixture;
    let mut journal=Journal::new(&scratch.0,&mut fixture);
    let mut store=Store::in_memory().unwrap();
    let (a,_)=store.begin_from_candidates(ACTOR,SCOPE,"x",&[]).unwrap();
    let green:Value=serde_json::from_slice(GREEN).unwrap();
    let red:Value=serde_json::from_slice(RED).unwrap();
    let first=adapter::freeze(REPORT_PROMPT,json!({"ci_report":green})).unwrap();
    let second=adapter::freeze(REPORT_PROMPT,json!({"ci_report":red})).unwrap();
    journal.deliver(&mut store,a,&first,json!({"route":"paired/repair","task":"report-a"}),
        |text|validate_verdict(text,&green),"v",false).unwrap();
    let (b,_)=store.begin_from_candidates(ACTOR,SCOPE,"y",&[]).unwrap();
    assert!(journal.deliver(&mut store,b,&second,json!({"route":"paired/fresh","task":"report-a"}),
        |text|validate_verdict(text,&red),"v",false).is_err());
    assert_eq!(journal.dispatches,1,"second mismatched model request must never be sent");
}

struct InvalidFirst;
impl Backend for InvalidFirst {
    fn mode(&self)->&str {"offline_test_invalid"}
    fn dispatch(&mut self,_:&[u8])->Result<Reply> {
        Ok(Reply{text:r#"{"release":"R42","tests":"12/12","status":"fail"}"#.to_owned(),input:7,output:5})
    }
}
#[test]
fn invalid_bootstrap_is_charged_and_every_remaining_slot_is_unexecuted() {
    let scratch=Scratch::new();
    assert!(run_pilot(&scratch.0,&mut InvalidFirst).is_err());
    let report:Value=serde_json::from_slice(&fs::read(scratch.0.join("result.json")).unwrap()).unwrap();
    assert_eq!(report["unexecuted_slots"].as_array().map(Vec::len),Some(12));
    assert_eq!(report["unexecuted_scenarios"].as_array().map(Vec::len),Some(4));
    let mut store=Store::open(scratch.0.join("bootstrap.sqlite")).unwrap();
    assert_eq!(store.charged_usage().unwrap(),14); // 12 reply units plus 2 typed support checks.
    assert_eq!(store.read_current(ACTOR,SCOPE,"report-a").unwrap(),Current::Empty);
}
