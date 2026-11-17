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

See [technical decisions §10](architecture/technical-decisions.md#10-measured-webassembly-bundle). The WASM file is 2.40 MB raw, 828 KB gzip and 629 KB brotli. CI reports the current sizes in its job summary.

## Release parsing

`cargo test -p kitsunarr-parser --release --test perf -- --ignored --nocapture` parses every title of Sonarr's parser test corpus (1,629 titles, five rounds) after a warm-up. Measured on 2026-11-09:

| | |
|---|---:|
| Average per title | 206 µs |
| Slowest title (long anime batch name) | 1.6 ms |

A search with 500 results therefore spends about 0.1 s parsing release names.

## Browser at 1,000 series

- **Benchmark:** `tests/e2e/tests/perf.rs` (ignored by default; needs chromedriver and Chromium).
- **Command:** `KITSUNARR_BIN=target/release/kitsunarr cargo test -p kitsunarr-e2e --test perf -- --ignored --nocapture`, after `dx build -p kitsunarr-web --release --profile web-release` and `cargo build --release -p kitsunarr-server`.
- **Data:** 1,000 series (750 TV, 250 anime), 2,000 seasons, 25,000 episodes and 12,000 episode files in the real server's database, no artwork.
- **Method:** headless Chromium signs in, then each step runs five times and the median is reported. Times are from navigation start (cold page loads; the browser keeps its HTTP cache between runs) or from the action until the condition shows on screen, checked every animation frame.

Measured on 2026-11-17 with Chromium 157.0.8103.78, an AMD Ryzen 7 7800X3D (16 threads), Linux, release server build and release WebAssembly bundle, server and browser on the same machine:

| Step | Median |
|---|---:|
| Series library: first contentful paint | 48 ms |
| Series library: first 60 posters shown | 56 ms |
| Title search typed in the header to its one result (300 ms debounce included) | 307 ms |
| Clearing the search back to 60 posters (debounce included) | 313 ms |
| Next page of 60 | 8 ms |
| A series page (cold load) until its episodes show | 58 ms |

With a debug server build the same steps took 128–160, 168–196, 324–337, 326–344, 33–46 and 176–208 ms over three runs. Searching costs little beyond the debounce; the browser never holds more than one page of 60 titles.

## Not measured yet

- Throughput of migrations from large real-world Sonarr databases.
- The cost of indexer and download-client polling, which is not implemented yet.
