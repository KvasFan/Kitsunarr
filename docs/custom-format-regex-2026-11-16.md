# Invalid negated custom-format regexes — 16 November 2026

Custom formats with an invalid release-title or release-group regex used to evaluate as
satisfied when the specification was negated. Compilation reported that the pattern would
never match, but represented both invalid and blank regexes as the same absent pattern;
negation then turned the failed match into success. This could grant a positive score,
apply a negative score, or include the format in a renamed filename.

Invalid regexes now compile into the existing unevaluable-specification state, which is
always unsatisfied, including when negated. The warning remains visible. Valid and blank
patterns retain their existing semantics. Grouping also remains unchanged: an invalid
required member blocks its group, while an optional invalid member can coexist with a
valid alternative that matches.

Regression coverage includes:

- Both regex specification types, with every negate/required combination.
- Required versus optional invalid members alongside valid alternatives.
- Blank regex semantics.
- Release decisions with positive and negative format scores, including the profile's
  minimum score rejection.
- Manual and completed-download movie imports, filename previews, real renames and
  subtitle handling with invalid negated formats stored through the normal database API.
  These use malformed character classes and trailing escapes, which the existing minimal
  save-time syntax check accepts; no database validation is bypassed.

The two focused matching regressions failed before the implementation change and passed
afterwards. All filesystem tests use temporary synthetic media and local database fixtures.
No live indexer searches or downloads are needed for this correction.

Final verification:

| Check | Result |
|---|---|
| `cargo test --workspace` | 844 passed, 0 failed, 12 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo check -p kitsunarr-web --target wasm32-unknown-unknown` | Passed |
| `cargo clippy -p kitsunarr-web --target wasm32-unknown-unknown -- -D warnings` | Passed |
| `cargo fmt --all -- --check`, `git diff --check` | Passed |

Local test output is under `.dev/verification/custom-format-regex-*`, excluded from Git;
the full passing run is `custom-format-regex-tests-final.log`. Browser and container checks
were not rerun for this shared evaluator correction. The existing local instance has not
been redeployed in this follow-up.
