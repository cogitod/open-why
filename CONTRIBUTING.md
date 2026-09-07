# Contributing to open-why

Issues and PRs are welcome. This is a small, single-maintainer project. Keep changes
scoped and cite what you're changing and why.

## Local setup

The minimum supported Rust version is 1.88, declared by `package.rust-version`
in `Cargo.toml`. CI validates both that exact floor and the current stable
toolchain.

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

Enable the pre-commit checks once per clone with Lefthook:

```bash
lefthook install
```

The checks scan exact staged blobs for leaks and enforce a 999-line maximum for
tracked Rust files under `src/` and `tests/`. There are no generated-file
exclusions. Reviewed synthetic UUID fixtures may be listed in
`hooks/leak-allowlist.txt`; secrets and provenance leaks may not be allowlisted.
They also reject external GitHub Actions that are not pinned to a full commit
SHA (and container actions that are not pinned by digest).

## Before opening a PR

Run the main CI checks locally first:

```bash
cargo fmt --check
cargo metadata --locked --format-version 1 --no-deps > /dev/null
cargo check --locked
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --release --all-targets --locked -- -D warnings
cargo test --locked
cargo deny --all-features check advisories licenses sources
bash scripts/check-ci-pins.sh tracked
bash scripts/check-rust-loc.sh tracked
```

The dependency command requires cargo-deny 0.20.2. CI runs that pinned version
against the committed lockfile and fails on known
vulnerabilities, yanked packages, unapproved licenses, and unapproved dependency
sources. Supply-chain exceptions are not accepted silently: scope an exception
to the exact advisory or package version, give the reason and removal date in
`deny.toml`, and link the public review issue. An expired or unreviewed exception
blocks a stable release even if ordinary CI passes.

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

## Stable releases

Do not describe a version as stable merely because the normal required checks pass.
The [end-user stability contract](STABILITY.md) is the release gate: every required
evidence row must pass at the exact release revision, and every guarantee must be
backed by continuous automation on each claimed platform. An unmet, skipped, or
manual-only gate must remain documented as unmet and blocks a stable-version claim.
Stable release notes must record the supported operating systems, MSRV, contract
versions, store migration range, and evidence run.

## Reporting a security issue

Do not open a public issue. See [SECURITY.md](SECURITY.md).
