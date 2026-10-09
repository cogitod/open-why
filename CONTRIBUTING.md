# Contributing to open-why

Issues and focused PRs are welcome. Useful first contributions include a
reproducible bug, an onboarding improvement, or a regression test for a reported
problem. For a new capability or contract change, open an issue describing the
user's problem before investing in a large implementation.

## Local setup

Install Git, Rust 1.88 or newer, and your platform's native build tools. CI checks
Linux with both Rust 1.88 and current stable; other platforms are not currently
covered by the [stability contract](STABILITY.md).

Fork the repository on GitHub if you do not have write access, then clone your
fork and add this repository as `upstream`. Maintainers can clone directly:

```bash
git clone https://github.com/cogitod/open-why.git
cd open-why
git switch -c fix/describe-your-change
cargo build --release --locked
```

Run the development binary with `cargo run --locked --bin why -- --help`. To
install your checkout, use `cargo install --locked --path . --bin why`.

The build fetches a prebuilt ONNX Runtime archive from `cdn.pyke.io` through the
`ort` crate, even for lexical search. If that download is unavailable, point
`ORT_LIB_LOCATION` at a compatible pre-installed ONNX Runtime:

```bash
ORT_LIB_LOCATION=/path/to/onnxruntime cargo build --release --locked
```

### Enable local checks

Git does not install repository hooks automatically. If this clone has no
existing hook setup, enable the included hook without extra tooling:

```bash
git config --local core.hooksPath hooks
```

If you already use Lefthook, run `lefthook install` instead. If you have other
hooks configured, integrate `bash hooks/pre-commit` into them rather than replacing
them. Both supplied setups reject commits on `main` or a detached HEAD and scan
the staged files for secrets, private provenance, oversized Rust files, and
mutable GitHub Actions references. GitHub branch protection enforces the remote
PR policy even when local hooks are absent.

## Find your way around

| Area | Start here |
| --- | --- |
| First-use setup and diagnostics | `src/bin/onboarding/`, `tests/onboarding.rs`, `examples/quickstart.sh` |
| CLI commands | `src/bin/why.rs` |
| MCP tools and contracts | `src/mcp/`, [integration standard](docs/integrations.md) |
| Storage and retrieval | `src/db.rs`, `src/relevance.rs`, [design](docs/design.md) |
| Contributor checks | `hooks/`, `scripts/`, `.github/workflows/ci.yml` |

## Validate your change

For Rust changes, run:

```bash
cargo fmt --check
cargo build --release --locked
cargo clippy --release --all-targets --locked -- -D warnings
cargo test --locked
```

For onboarding changes, `cargo test --locked --test onboarding` exercises setup,
diagnostics, generated configuration, and the walkthrough with isolated stores.
For hook or workflow changes, also run the affected shell tests:

```bash
bash tests/check-branch.sh
bash tests/check-leaks.sh
bash tests/check-ci-pins.sh
bash tests/check-rust-loc.sh
```

After staging your intended changes with `git add`, run `bash hooks/pre-commit`.
The scanners' `staged` mode reads the Git index; `tracked` mode reads the committed
`HEAD`, not uncommitted edits. Rust files in `src/` and `tests/` have a 999-line
limit. Reviewed synthetic UUID fixtures may be listed in
`hooks/leak-allowlist.txt`; secrets and private provenance may not be allowlisted.

CI also checks dependency advisories, yanked packages, licenses, and sources
against the committed lockfile using cargo-deny 0.20.2. To reproduce that check:

```bash
cargo install cargo-deny --version 0.20.2 --locked
cargo deny --all-features check advisories licenses sources
```

Scope any proposed dependency-policy exception to an exact advisory or package
version, with a reason, removal date, and public review issue in `deny.toml`.
An expired or unreviewed exception blocks a stable-release claim.

If changing ranking, add representative regression evidence. You can also use
the [retrieval parity harness](docs/retrieval-parity.md) against your own corpus;
there is no universal golden fixture bundled with the repository.

## Open and merge a PR

1. Start a scoped topic branch from current `origin/main` (or `upstream/main`
   when working from a fork). Never commit or push directly to `main`.
2. Describe the problem, resulting behavior, and validation. Link an issue when
   relevant. Keep private repository content and personal paths out of examples.
3. Review the complete final diff at the latest PR head. Resolve every review
   conversation and keep the branch up to date with `main`.
4. Wait for both required checks, `leak-check` and `build-and-test`, to pass.
   Maintainers squash merge using a descriptive PR title, which becomes the
   commit title with its PR number. Merged branches are automatically deleted.

These requirements apply to administrators too. Force pushes and branch deletion
are disabled for `main`. Squash merging is the only enabled merge method, keeping
one traceable commit per PR without rewriting previous history.

This is a single-maintainer project. The required approval count is zero because
the maintainer cannot approve their own PR; this does not claim independent
review. Contributors can open PRs without access to repository secrets.

## Releases and security

Passing ordinary CI does not establish a stable release. Follow the
[end-user stability contract](STABILITY.md) at the exact release revision and
report any unmet gate. Stable release notes must state supported operating
systems, MSRV, contract versions, migration range, and the evidence run.

Report vulnerabilities privately through [SECURITY.md](SECURITY.md).
