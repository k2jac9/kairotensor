//! Frozen synthetic corpus and independent answer labels for the token-cap pilot.
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub const FAMILIES: [&str; 5] = ["unit", "integration", "package", "migration", "compatibility"];
pub const CAPS: [u64; 4] = [32, 48, 64, 96];

// Freeze the exact system message from the historical *successful* experiment.
// Future changes to an example's current prompt cannot change this pilot.
fn system_message() -> &'static str {
    static MESSAGE: OnceLock<String> = OnceLock::new();
    MESSAGE.get_or_init(|| {
        let archive: Value = serde_json::from_str(include_str!(
            "../../docs/evaluations/scoped-reuse-qwen-2026-11-13.json"
        )).expect("valid pinned evaluation archive");
        let first = &archive["events"][0];
        assert_eq!(first["task"], "ci-judgment");
        let request: Value = serde_json::from_str(first["request_utf8"].as_str().unwrap()).unwrap();
        assert_eq!(request["messages"][0]["role"], "system");
        request["messages"][0]["content"].as_str().unwrap().to_owned()
    })
}

#[derive(Clone, Copy)]
pub struct Case {
    pub id: &'static str,
    pub family: &'static str,
    pub split: &'static str,
    release: &'static str,
    run: &'static str,
    passed: u32,
    total: u32,
    expected_tests: &'static str,
    expected_status: &'static str,
    order: usize,
}

// Case IDs, split and reference answers are harness metadata and stay out of prompts.
const fn case(
    id: &'static str, family: &'static str, split: &'static str, release: &'static str,
    run: &'static str, passed: u32, total: u32, expected_tests: &'static str,
    expected_status: &'static str, order: usize,
) -> Case {
    Case { id, family, split, release, run, passed, total, expected_tests, expected_status, order }
}

const CASES: [Case; 10] = [
    case("unit-green", "unit", "calibration", "U9", "unit/81", 9, 9, "9/9", "pass", 0),
    case("unit-red", "unit", "calibration", "unit-runner-linux-arm64-release-2026.11.09+build.371",
         "unit/82", 22, 24, "22/24", "fail", 1),
    case("integration-green", "integration", "calibration",
         "integration-bundle-pg17-linux-aarch64-2026.11.10+run.204",
         "integration/204", 36, 36, "36/36", "pass", 2),
    case("integration-red", "integration", "calibration", "INT-14", "integration/205",
         30, 31, "30/31", "fail", 3),
    case("package-green", "package", "heldout", "PKG-72", "package/52", 6, 6, "6/6", "pass", 1),
    case("package-red", "package", "heldout", "package-registry-x86_64-release-2026.11.08+dist.490",
         "package/53", 17, 18, "17/18", "fail", 0),
    case("migration-green", "migration", "heldout",
         "ledger-migration-v004-to-v005-postgresql-2026.11.12+schema.142",
         "migration/310", 48, 48, "48/48", "pass", 3),
    case("migration-red", "migration", "heldout", "MIG-5", "migration/311", 13, 16, "13/16", "fail", 2),
    case("compatibility-green", "compatibility", "heldout",
         "sdk-compatibility-matrix-rust-1.99-python-3.11-linux-arm64+bundle.850",
         "compat/70", 52, 52, "52/52", "pass", 0),
    case("compatibility-red", "compatibility", "heldout",
         "client-api-v3.8.2", "compat/71", 7, 10, "7/10", "fail", 2),
];

pub fn family(name: &str) -> Option<Vec<Case>> {
    if !FAMILIES.contains(&name) { return None; }
    Some(CASES.iter().copied().filter(|c| c.family == name).collect())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Verdict {
    release: String,
    tests: String,
    status: String,
}

pub struct Score {
    pub valid: bool,
    pub correct: bool,
    pub reason: Option<String>,
}

impl Case {
    pub fn report(&self) -> Value {
        json!({
            "version": self.run, "release": self.release,
            "test_summary": format!("{}/{}", self.passed, self.total),
            "failing_cases": self.total - self.passed
        })
    }

    pub fn messages(&self) -> Value {
        let user = json!({"ci_report": self.report()}).to_string();
        json!([
            {"role":"system","content":system_message()},
            {"role":"user","content":user}
        ])
    }

    pub fn cap_order(&self) -> [u64; 4] {
        // A Williams square rotates first cap and immediate predecessor.
        // Each calibration case uses one order; the six held-out cases repeat two.
        let [a, b, c, d] = CAPS;
        [[a, b, d, c], [b, c, a, d], [c, d, b, a], [d, a, c, b]][self.order]
    }

    pub fn score(&self, output: &str) -> Score {
        let text = output.trim();
        let body = match text.strip_prefix("```json\n") {
            Some(inner) => inner.strip_suffix("```").unwrap_or(text).trim(),
            None => text,
        };
        let Ok(v) = serde_json::from_str::<Verdict>(body) else {
            return Score { valid: false, correct: false, reason: Some("invalid verdict schema".into()) };
        };
        if !matches!(v.status.as_str(), "pass" | "fail") {
            return Score { valid: false, correct: false, reason: Some("unknown verdict status".into()) };
        }
        let correct = v.release == self.release && v.tests == self.expected_tests
            && v.status == self.expected_status;
        Score { valid: true, correct, reason: (!correct).then(|| "verdict disagrees with reference answer".into()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn corpus_and_labels_are_consistent_and_absent_from_model_metadata() {
        let mut ids = BTreeSet::new();
        assert!(family("unknown").is_none());
        for (i, name) in FAMILIES.iter().enumerate() {
            let cases = family(name).unwrap();
            assert_eq!(cases.len(), 2);
            for c in cases {
                assert!(ids.insert(c.id));
                assert_eq!(c.split, if i < 2 { "calibration" } else { "heldout" });
                assert_eq!(c.expected_tests, c.report()["test_summary"].as_str().unwrap());
                assert_eq!(c.expected_status, if c.passed == c.total { "pass" } else { "fail" });
                let mut caps = c.cap_order(); caps.sort();
                assert_eq!(caps, CAPS);
                let messages = c.messages();
                let user: Value = serde_json::from_str(messages[1]["content"].as_str().unwrap()).unwrap();
                assert_eq!(user, json!({"ci_report":c.report()}));
                let expected = json!({"release":c.release,"tests":c.expected_tests,"status":c.expected_status});
                assert!(c.score(&expected.to_string()).correct);
            }
        }
        assert_eq!(ids.len(), 10);
    }

    #[test]
    fn truncated_malformed_and_incorrect_answers_are_scored_separately() {
        let c = family("unit").unwrap()[0];
        for text in ["{", r#"{"release":"U9","tests":"9/9","status":"pass","status":"fail"}"#,
                     r#"{"release":"U9","tests":"9/9","status":"pass","extra":"ok"}"#] {
            let s = c.score(text);
            assert!(!s.valid && !s.correct);
        }
        let wrong = c.score(r#"{"release":"U9","tests":"9/9","status":"fail"}"#);
        assert!(wrong.valid && !wrong.correct);
        assert!(c.score("```json\n{\"release\":\"U9\",\"tests\":\"9/9\",\"status\":\"pass\"}\n```").correct);
    }
}
