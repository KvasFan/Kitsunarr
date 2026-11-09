//! Loader for Sonarr's test corpus (`tests/fixtures/parser/sonarr/*.json`, produced by
//! `tests/fixtures/generate/sonarr_test_cases.py`).
//!
//! Each test file maps every upstream test method to a check. A method present in the corpus
//! but not handled here fails the test, so upstream additions cannot be skipped silently.

#![allow(dead_code)]

use std::collections::BTreeMap;

use serde_json::Value;

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

    /// Runs `check` on every case. `check` returns `None` for methods it does not handle and
    /// `Some(Err(message))` for a failing case.
    pub fn run(&self, check: impl Fn(&str, &[Value]) -> Option<Result<(), String>>) {
        let mut failures = Vec::new();
        let mut total = 0;
        for (method, cases) in &self.methods {
            for args in cases {
                total += 1;
                match check(method, args) {
                    None => failures.push(format!("{method}: not handled by the Kitsunarr test")),
                    Some(Err(msg)) => failures.push(format!("{method}{}: {msg}", show(args))),
                    Some(Ok(())) => {}
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{}: {} of {total} upstream cases failed:\n{}",
            self.name,
            failures.len(),
            failures.join("\n")
        );
        eprintln!("{}: all {total} upstream cases pass", self.name);
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
