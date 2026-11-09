# Performance measurements

These are real measurements, with the method and machine given so they can be reproduced. Nothing on this page is estimated.

## Large-library queries

- **Benchmark:** `crates/kitsunarr-db/tests/perf.rs` (ignored by default).
- **Command:** `cargo test -p kitsunarr-db --release --test perf -- --ignored --nocapture`
- **Data:** 1,000 series (750 TV, 250 anime), 2,000 seasons, 25,000 episodes, 12,000 episode files, 1,000 alternate titles and 5,000 history rows in a file-backed SQLite database (WAL).
- **Method:** each query runs 15 times and the median is reported. The queries are the ones the web interface issues: pages of 60 items, with aggregates (episode counts, size on disk, next airing) computed per page.

Measured on 2026-11-09 with Rust 1.99.1, release profile and SQLite 3.x (bundled), on a 16-thread x86_64 Linux workstation. Seeding took 482 ms.

| Query | Median |
|---|---:|
| Series page 1, sorted by title | 1.42 ms |
| Series page 10, sorted by title | 6.43 ms |
| Series page 1, sorted by size on disk | 10.06 ms |
| Series page 1, sorted by next airing | 3.36 ms |
| Anime only | 0.72 ms |
| Title search (including alternate titles) | 1.77 ms |
| Dashboard statistics | 2.19 ms |
| Calendar, 15 days | 2.28 ms |
| Series detail | 0.07 ms |
| Episodes of one series | 0.13 ms |
| History, page 1 | 0.24 ms |

The browser never loads the whole library. Libraries are paged on the server (60 items per page), searches are debounced (300 ms), and long episode lists are virtualised: only visible rows plus 8 above and below are rendered.

## WebAssembly bundle

See [technical decisions §10](architecture/technical-decisions.md#10-measured-webassembly-bundle). The WASM file is 2.29 MB raw, 793 KB gzip and 606 KB brotli. CI reports the current sizes in its job summary.

## Not measured yet

- Browser rendering time at 1,000+ titles (first contentful paint, interaction latency).
- Throughput of migrations from large real-world Sonarr databases.
- The cost of indexer and download-client polling, which is not implemented yet.
