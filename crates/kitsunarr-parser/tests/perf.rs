//! Parser throughput over every title in Sonarr's test corpus (ignored by default).
//! Run: `cargo test -p kitsunarr-parser --release --test perf -- --ignored --nocapture`

use std::time::Instant;

#[test]
#[ignore = "benchmark"]
fn parse_title_throughput() {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/parser/sonarr"
    );
    let mut titles = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        for method in doc["methods"].as_object().unwrap().values() {
            for case in method["cases"].as_array().unwrap() {
                if let Some(t) = case["args"].get(0).and_then(|v| v.as_str()) {
                    titles.push(t.to_string());
                }
            }
        }
    }
    // Warm up (pattern compilation).
    for t in &titles {
        let _ = kitsunarr_parser::parse_title(t);
    }
    let rounds = 5;
    let start = Instant::now();
    let mut parsed = 0usize;
    for _ in 0..rounds {
        for t in &titles {
            parsed += usize::from(kitsunarr_parser::parse_title(t).is_some());
        }
    }
    let elapsed = start.elapsed();
    let per = elapsed / (rounds * titles.len()) as u32;
    eprintln!(
        "{} titles x {rounds}: {elapsed:?} total, {per:?} per title, {} parsed per round",
        titles.len(),
        parsed / rounds
    );
    // Slowest titles.
    let mut timings: Vec<(std::time::Duration, &String)> = titles
        .iter()
        .map(|t| {
            let s = Instant::now();
            let _ = kitsunarr_parser::parse_title(t);
            (s.elapsed(), t)
        })
        .collect();
    timings.sort_by_key(|t| std::cmp::Reverse(t.0));
    for (d, t) in timings.iter().take(5) {
        eprintln!("  {d:?}  {t}");
    }
}
