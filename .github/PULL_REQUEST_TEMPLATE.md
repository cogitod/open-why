## What changed

## Why

## Linked issue
Fixes #

## Testing
- [ ] `cargo fmt --check`
- [ ] `cargo clippy --release --all-targets --locked -- -D warnings`
- [ ] `cargo test --locked`
- [ ] dependency policy and immutable CI-reference checks pass
- [ ] `bash hooks/check-leaks.sh staged`
- [ ] If ranking changed (`src/db.rs`, `src/relevance.rs`): ran `why-golden` and noted the pass count below

## Promotion
- [ ] This scoped topic branch started from the current `origin/main`
- [ ] I reviewed the complete final diff at the latest PR head
- [ ] `leak-check` and `build-and-test` are green
- [ ] All review conversations are resolved

## Notes
Note behavior changes, ranking-constant changes, and follow-up work that is outside this PR.
