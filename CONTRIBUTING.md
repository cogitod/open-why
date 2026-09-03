# Contributing to open-why

Issues and PRs welcome. This is a small, single-maintainer project — keep changes
scoped and cite what you're changing and why.

## Local setup

```bash
git clone https://github.com/cogitod/open-why.git
cd open-why
cargo build --release
```

See [README.md#building-without-network-access](README.md#building-without-network-access)
if `cargo build` can't reach `cdn.pyke.io` from your network.

Enable the pre-commit leak-check hook once per clone:

```bash
git config core.hooksPath hooks
```

It scans staged files for secrets (keys, tokens) and for unreviewed exports of real
internal records — see `hooks/check-leaks.sh`. Reviewed, intentional exceptions are
listed in `hooks/leak-allowlist.txt`.

## Before opening a PR

Run exactly what CI runs, locally, first:

```bash
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
cargo test
```

If your change touches ranking (`src/db.rs`, `src/relevance.rs`) and you have your own
golden fixture (see [README.md#retrieval-parity](README.md#retrieval-parity) — none
ships in this repo, it's only meaningful against your own corpus), also run the
golden-parity harness and note the pass count in your PR if it changed:

```bash
cargo build --release --bin why-golden && ./target/release/why-golden --fixture /path/to/your-golden-queries.json
```

## Opening a PR

- Link an issue if one exists.
- Describe what changed and why — the "why" matters more here than most projects.
- Note any behavior change, even a small one (this project ports calibrated
  constants from another system; don't retune ranking numbers without saying so).
- `main` requires the CI checks (`leak-check`, `build-and-test`) to pass before merge.

## Reporting a security issue

Don't open a public issue — see [SECURITY.md](SECURITY.md).
