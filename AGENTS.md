# Working on open-why

open-why is a public Rust library, CLI, and MCP server for recorded decision
rationale. Read [CONTRIBUTING.md](CONTRIBUTING.md) before changing the repository.

## Landing changes

- Create a scoped topic branch from current `origin/main`. Never commit or push
  directly to `main`, including as an administrator.
- Open a PR with the concrete problem, resulting behavior, and validation.
- Review the complete final diff at the latest PR head. Resolve conversations.
- Both required checks, `leak-check` and `build-and-test`, must pass with the
  branch up to date before a squash merge. Do not bypass the protection.
- One maintainer currently operates this repository; zero required approvals
  does not imply independent review. Keep the review evidence honest.

## Scope and validation

- Keep changes focused. Do not include private paths, credentials, exported
  records, or unrelated project context in this public repository.
- Preserve explicit scope, store identity, evidence, and versioned contracts.
  Do not infer missing rationale or quietly change ranking behavior.
- Keep Rust files under `src/` and `tests/` at or below 999 lines.
- Run `cargo fmt --check`, `cargo build --release --locked`,
  `cargo clippy --release --all-targets --locked -- -D warnings`, and
  `cargo test --locked` for Rust changes. CI also verifies Rust 1.88.
- Run affected shell regression tests for hook or CI changes. After staging,
  run `bash hooks/pre-commit` to check the actual staged files.
- Follow [STABILITY.md](STABILITY.md) before making any stable-release claim.
  Passing ordinary CI alone is insufficient.
