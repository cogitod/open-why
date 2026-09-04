# Contributing to open-why

Issues and PRs are welcome. This is a small, single-maintainer project. Keep changes
scoped and cite what you're changing and why.

## Local setup

```bash
git clone https://github.com/cogitod/open-why.git
cd open-why
cargo build --release
```

`cargo build` fetches a prebuilt ONNX Runtime archive from `cdn.pyke.io` (via the
`ort` crate) unless told not to. If your network can't reach it, point
`ORT_LIB_LOCATION` at a pre-installed ONNX Runtime instead. No `Cargo.toml` change is
needed:

```bash
ORT_LIB_LOCATION=/path/to/onnxruntime cargo build --release
```

Enable the pre-commit leak-check hook once per clone:

```bash
git config core.hooksPath hooks
```

It scans staged files for secrets, unreviewed exports of real records, private
filesystem paths, and references to downstream/private implementations. See
`hooks/check-leaks.sh`. Reviewed synthetic UUID fixtures may be listed in
`hooks/leak-allowlist.txt`; secrets and provenance leaks may not be allowlisted.

## Before opening a PR

Run exactly what CI runs, locally, first:

```bash
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
cargo test
```

If your change touches ranking (`src/db.rs`, `src/relevance.rs`) and you have your own
golden fixture (see [docs/retrieval-parity.md](docs/retrieval-parity.md); none
ships in this repo, it's only meaningful against your own corpus), also run the
golden-parity harness and note the pass count in your PR if it changed:

```bash
cargo build --release --bin why-golden && ./target/release/why-golden --fixture /path/to/your-golden-queries.json
```

## Opening a PR

- Do not commit or push directly to `main`.
- Fetch `origin/main`, then create a scoped topic branch from its current tip.
- Link an issue if one exists.
- Describe what changed and why. The rationale is part of the review evidence.
- Note any behavior change, even a small one. Do not retune ranking constants
  without representative regression evidence.
- At the latest PR head, review the complete final diff against current `main`.
- Resolve every review conversation and wait for both required checks,
  `leak-check` and `build-and-test`, to pass before merge.

The branch policy requires zero approving reviews because open-why currently has one
maintainer, who cannot approve their own PR. This avoids an impossible approval gate; it
does not claim independent review.

## Reporting a security issue

Do not open a public issue. See [SECURITY.md](SECURITY.md).
