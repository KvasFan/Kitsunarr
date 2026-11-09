//! Loader for Sonarr's test corpus (`tests/fixtures/parser/sonarr/*.json`, produced by
//! `tests/fixtures/generate/sonarr_test_cases.py`).
//!
//! Each test file maps every upstream test method to a check. A method present in the corpus
//! but not handled here fails the test, so upstream additions cannot be skipped silently.

#![allow(dead_code)]

use std::collections::BTreeMap;

use serde_json::Value;

/// Result of one upstream case.
pub enum Outcome {
    Pass,
    Fail(String),
    /// Not checkable yet: the assertion needs a part of the parser that is not ported yet. Listed
    /// in the output and counted, but not a failure.
    Pending(&'static str),
}

impl From<Result<(), String>> for Outcome {
    fn from(r: Result<(), String>) -> Self {
        match r {
            Ok(()) => Outcome::Pass,
            Err(e) => Outcome::Fail(e),
        }
    }
}

pub struct Fixture {
    pub name: &'static str,
    methods: BTreeMap<String, Vec<Vec<Value>>>,
}

impl Fixture {
    pub fn load(name: &'static str) -> Self {
        let path = format!(
            "{}/../../tests/fixtures/parser/sonarr/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let methods = doc["methods"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, m)| {
                let cases = m["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["args"].as_array().unwrap().clone())
                    .collect();
                (k.clone(), cases)
            })
            .collect();
        Self { name, methods }
    }

    /// Runs `check` on every case. `check` returns `None` for methods it does not handle.
    pub fn run(&self, check: impl Fn(&str, &[Value]) -> Option<Outcome>) {
        let mut failures = Vec::new();
        let mut pending = std::collections::BTreeMap::<String, (usize, &str)>::new();
        let mut total = 0;
        for (method, cases) in &self.methods {
            for args in cases {
                total += 1;
                match check(method, args) {
                    None => failures.push(format!("{method}: not handled by the Kitsunarr test")),
                    Some(Outcome::Fail(msg)) => {
                        failures.push(format!("{method}{}: {msg}", show(args)));
                    }
                    Some(Outcome::Pending(why)) => {
                        pending.entry(method.clone()).or_insert((0, why)).0 += 1
                    }
                    Some(Outcome::Pass) => {}
                }
            }
        }
        let pending_count: usize = pending.values().map(|(n, _)| n).sum();
        for (method, (n, why)) in &pending {
            eprintln!("{}: {n} case(s) of {method} pending: {why}", self.name);
        }
        assert!(
            failures.is_empty(),
            "{}: {} of {total} upstream cases failed:\n{}",
            self.name,
            failures.len(),
            failures.join("\n")
        );
        eprintln!(
            "{}: {} of {total} upstream cases pass, {pending_count} pending",
            self.name,
            total - pending_count
        );
    }
}

fn show(args: &[Value]) -> String {
    format!(
        "({})",
        args.iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub fn s(v: &Value) -> &str {
    v.as_str()
        .unwrap_or_else(|| panic!("expected string, got {v}"))
}

pub fn b(v: &Value) -> bool {
    v.as_bool()
        .unwrap_or_else(|| panic!("expected bool, got {v}"))
}

pub fn i(v: &Value) -> i64 {
    v.as_i64()
        .unwrap_or_else(|| panic!("expected integer, got {v}"))
}

/// `{"enum": "Quality.HDTV720p"}` → `"HDTV720p"`.
pub fn enum_name<'a>(v: &'a Value, prefix: &str) -> &'a str {
    let full = v["enum"]
        .as_str()
        .unwrap_or_else(|| panic!("expected enum, got {v}"));
    full.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('.'))
        .unwrap_or_else(|| panic!("expected {prefix}.*, got {full}"))
}

pub fn expect_eq<T: PartialEq + std::fmt::Debug>(
    what: &str,
    got: T,
    want: T,
) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}
