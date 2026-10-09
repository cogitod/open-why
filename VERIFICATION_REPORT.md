# Public-beta verification report

Date: 2026-10-09. Baseline: `ac2f19b`. Runtime changes through `273ecb2`.
Local branch: `feat/trustworthy-public-beta`. Version: `0.1.0-beta.1`.
No PR, issue, tag, release, remote setting change or hosted artifact was created.

## Recommendation

**NO-GO for public release; GO for PR review and supervised beta evaluation.**
The reproduced store-isolation defects are fixed, and local product/artifact
journeys are exercised. Remaining publication gates are exact-commit hosted CI,
artifact attestations/public download verification, and independent real-client
acceptance. A protocol harness is not evidence that a named vendor client/version
or an independent developer completed the journey.

## Implemented and verified

- Daemon reuse validates identity before discovery; full database filenames have
  distinct endpoints. A bounded handshake binds protocol version, canonical path,
  store identity, device and inode. Every request checks physical-file continuity.
  Existing regular/stale endpoints are never unlinked automatically. Six process
  regressions include concurrent stores, wrong identities, endpoint redirection,
  replaced files, alias rejection, write isolation and disconnected proxies.
- Remote Git cache collisions were separately reproduced: two owners' `project.git`
  URLs produced one scope. Full URL hashes and cached-origin verification now
  keep them separate. Failed fetches fail explicitly. The real-Git regression
  uses local URL rewriting, no network or private repositories.
- `--no-default-features` removes ONNX/tokenizers; default Cargo behavior retains
  local embeddings. Recommended first installation is lexical. Resolved Cargo
  package counts: 106 lexical versus 217 default (includes build/dev/platform
  resolution, not only linked runtime dependencies). Measured macOS release
  binaries were approximately 6.65 MB lexical and 36.99 MB default, unstripped.
- Seven synthetic evidence records exercise relevant/missing/scoped evidence,
  supersession, expiration, unresolved disagreement and repeat queries. Actual
  pinned-model runs verify every record's 384-dimensional vector and semantic
  ranking participation. No statistical retrieval score is claimed.
- Backup/verify/restore work through the CLI. Live WAL/daemon snapshots preserve
  store and evidence identities. Existing destinations, wrong identities, absent
  sources, corrupt files and modified sealed content are rejected.
- Candidate preparation installs a locked source archive, extracts the binary
  archive, and tests those bytes through setup, index, capture, search, MCP,
  backup, verification, restore and exact read. Assets include dependency license
  texts, a Cargo SBOM, SHA-256 manifest and an unsigned source/toolchain record.
- Existing contribution infrastructure was retained. CI now adds macOS, lexical
  tests and explicit model execution without changing required check names.
  Action pins/workflow syntax and narrow public-contact leak-scan exceptions are
  tested. Conduct reports use the owner-confirmed email in CODE_OF_CONDUCT.md;
  security reports retain the separate private GitHub channel.

## Environments and results

macOS 26.5.2, ARM64; stable Rust 1.98.0 (`88d9e12ae`), Cargo 1.98.0; MSRV
Rust 1.88.0 (`6b00bc388`). Python 3.9.6 for candidate packaging; actionlint 1.7.12;
cargo-deny 0.20.2. Git and native build tools were installed. Fresh source/archive
and isolated homes/stores were used; local Cargo dependency caches were reused.

Linux ran in local ARM64 containers, not GitHub-hosted x86-64 runners:

- Debian 12, official Rust 1.88.0 image pinned to
  `sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0`.
- Ubuntu 24.04 pinned to
  `sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55`,
  Rust copied from that Rust image, GCC 13.3.0. Build prerequisites installed
  with apt: build-essential, ca-certificates, git, pkg-config, libssl-dev, python3.
  Apt repositories are not an immutable toolchain snapshot; no hermetic-build claim.

| Environment / command | Passed | Failed | Ignored | Result |
|---|---:|---:|---:|---|
| macOS stable, `cargo test --locked` | 170 | 0 | 3 | Pass |
| macOS stable, `cargo test --locked --no-default-features` | 170 | 0 | 0 | Pass |
| macOS Rust 1.88, default suite | 170 | 0 | 3 | Pass |
| macOS Rust 1.88, lexical suite | 170 | 0 | 0 | Pass |
| Debian 12 ARM64 Rust 1.88, lexical suite | 171 | 0 | 0 | Pass; install, index, search, backup, restore and restored search also passed |
| Ubuntu 24.04 ARM64 Rust 1.88, default suite | 171 | 0 | 3 | Pass; install and demo passed |
| Explicit model-vector + retrieval cases, macOS | 2 | 0 | 0 | Real inference passed |
| Explicit model-vector + retrieval cases, Ubuntu | 2 | 0 | 0 | Real inference passed |

Counts include doctests; Linux includes one additional platform-specific library
test. Formatting, release builds and Clippy passed on macOS stable/MSRV. Both
feature configurations passed Clippy. Dependency advisories/licenses/sources,
actionlint, hook/CI-pin/line-limit regressions and warning-level ShellCheck passed.
These are local results, not hosted CI badges.

Model tests are explicitly invoked separately from ordinary `cargo test`.
The default suite's ignored tests are two model-dependent acceptance cases and
one manual cosine diagnostic. Only the first two are subsequently executed;
the manual diagnostic is not counted as passed. Lexical builds do not compile
unsupported local-model tests and advertise no local-model capability.

## Reproduction commands

Run from a reviewed checkout with Cargo on PATH:

```bash
cargo fmt --check
cargo build --release --locked
cargo clippy --release --all-targets --locked -- -D warnings
cargo clippy --release --all-targets --locked --no-default-features -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
cargo test --locked --test daemon_isolation
cargo test --locked --no-default-features --test repository_isolation
cargo test --locked --test maintenance
cargo test --locked --no-default-features --test public_evaluation -- --nocapture
bash scripts/evaluate-local.sh
rustup run 1.88.0 cargo build --release --locked
rustup run 1.88.0 cargo clippy --release --all-targets --locked -- -D warnings
rustup run 1.88.0 cargo test --locked
rustup run 1.88.0 cargo test --locked --no-default-features
cargo deny --all-features check advisories licenses sources
actionlint .github/workflows/ci.yml .github/workflows/release-candidate.yml
bash tests/check-branch.sh
bash tests/check-leaks.sh
bash tests/check-ci-pins.sh
bash tests/check-rust-loc.sh
shellcheck -S warning hooks/check-leaks.sh tests/check-leaks.sh scripts/evaluate-local.sh
bash hooks/check-leaks.sh tracked
```

The MSRV run used `CARGO_TARGET_DIR=/tmp/open-why-onboarding-msrv`. The macOS
model run additionally used
`OPEN_WHY_EMBED_MODEL_PATH=/private/tmp/open-why-beta-model`; the model files
were downloaded from the immutable URL and matched the digests documented in
`docs/public-evaluation.md`. The Ubuntu model script downloaded and verified
its own cache. No API key or private corpus was used.

The clean-container sequence copied the public source into `/work`, initialized
and committed a synthetic local Git checkout for repository-index tests, and ran:

```bash
cargo test --locked --no-default-features
cargo install --locked --path . --bin why --no-default-features --root /installation
OPEN_WHY_BIN=/installation/bin/why bash examples/quickstart.sh /demo generic
export OPEN_WHY_DB=/demo/open-why.db
/installation/bin/why init /demo/repository
/installation/bin/why search SQLite --scope /demo/repository
/installation/bin/why backup --to /snapshot.db
/installation/bin/why verify-backup /snapshot.db
/installation/bin/why restore /snapshot.db --to /restored.db
OPEN_WHY_DB=/restored.db /installation/bin/why search SQLite --scope /demo/repository
```

Ubuntu default-feature validation used `cargo test --locked`,
`bash scripts/evaluate-local.sh`, and the same install/demo commands with
`--no-default-features` omitted. Containers used separate Cargo caches;
source checkout and test stores were recreated. Container commits are synthetic
fixture commits, not claimed to be reviewed upstream history.

## External journey evidence

The candidate script contains the exact per-command arguments and checks. Its
reported source commit, host and toolchain are in each candidate's build JSON.

| Step | Command / evidence | Expected and observed behavior |
|---|---|---|
| Discover / understand | README, CHANGELOG, limitations and public fixture reviewed | Purpose is recorded evidence; absent rationale is not invented; independent comprehension untested |
| Install identifiable source | `python3 scripts/prepare-release.py /private/tmp/open-why-beta-candidate-final` | Clean source `.crate` extracted; `cargo install --locked --path <extracted> --bin why --no-default-features` succeeds |
| Version | Extracted archive's `why --version` | `why 0.1.0-beta.1` |
| Initialize | `OPEN_WHY_BIN=<archive>/why bash examples/quickstart.sh <new-directory> generic`; `why init <demo>/repository` | Private store/config and synthetic Git repository created; rationale indexed |
| Store | `why capture --id release-smoke --title 'Release smoke evidence' --content 'Synthetic artifact validation'` | Record captured in isolated store |
| Retrieve | `why search SQLite --scope <demo>/repository`; MCP initialize | SQLite evidence present; application version matches artifact |
| Evaluate | `cargo test --test public_evaluation`; explicit local-model script | Expected IDs/current state/absence/scopes checked; vectors and semantic arm actually execute |
| Backup | `why backup --to <candidate>/snapshot.db` | New consistent snapshot; source remains available |
| Verify | `why verify-backup <candidate>/snapshot.db` | Schema, integrity, foreign keys and sealed evidence pass |
| Restore | `why restore <candidate>/snapshot.db --to <candidate>/restored.db`; `why get release-smoke` against restored store | Same store identity and synthetic record content preserved |
| Contribute | Topic commits, staged hooks, shell regressions and Rust matrix | Checks execute locally; public issue/PR creation not authorized |
| Pass CI | Local CI-equivalent commands and actionlint | Local checks pass; hosted workflow execution remains pending |
| Install public versioned release | RELEASE.md commands and local archive | Local immutable candidate works; no public beta tag/download exists yet |

## Final local candidate

Prepared and validated from clean commit
`03d5a1167ed3b24875b9b51eb5ee36d48927429d` on macOS ARM64. This final
report entry is a later documentation-only update. Candidate assets are local
under `/private/tmp/open-why-beta-candidate-final/assets`; they are not public
release downloads.

| Asset | SHA-256 |
|---|---|
| open-why-0.1.0-beta.1.crate | 57bd165aaffb8abc8d913fca3a1d19304070c2a7307ee57722446a88245174a7 |
| open-why-0.1.0-beta.1-aarch64-apple-darwin-lexical.tar.gz | 37738f4d7ccd5dad7fb9abccfcaf9f17b2f50c2ca4badcd67b3167adbe1cee8f |
| build-aarch64-apple-darwin.json | d592d4447a57f2824c1d923a2defd6db1ff8685662d5478916c6a4da42f0d98e |
| sbom-aarch64-apple-darwin.cdx.json | f1c9b508fed19abb9da8c4e188f980bc3661917be43a563c7bce28f67e7ad24b |

`shasum -a 256 -c SHA256SUMS-aarch64-apple-darwin` returned OK for all
four assets. The binary archive contains `why`, dependency license texts,
LICENSE and README. `otool -L` reported only system `libiconv` and `libSystem`
dynamic dependencies. The extracted binary completed the full scripted journey;
no real user store was opened. Public provenance remains unverified until the
hosted attestation workflow executes on an authorized tag.

## Failures observed and handled

1. Baseline daemon tests failed: wrong identity accepted and a second database
   returned the first database's marker. Fixed and covered by six regressions.
2. Remote Git caches shared a basename scope across different URLs. A failing
   real-process test preceded the URL-identity fix; it now expects both scopes
   and refuses a deliberately altered cached origin without changing the store.
3. A fresh container exposed a conformance test's assumption that Cargo had
   created `./target`. The fixture now creates its own directory, including with
   external `CARGO_TARGET_DIR`. A startup rejection also raced a test's stdin
   write on Linux; BrokenPipe is accepted only while the child's failure status
   remains checked. Successful startup must still return MCP server information.
4. Debian 12 ARM64 default ONNX build fails to link `_M_replace_cold` against its
   older C++ runtime. This remains a documented limitation, not a passing build.
   Lexical installation avoids ONNX. Ubuntu 24.04 with GCC 13 links successfully.
5. Ubuntu ARM64 ONNX emits one nonfatal pre-main CPU-vendor warning. Initial tests
   failed empty-stderr/diagnostic-length assertions. Tests now recognize exactly
   that prefix once, only on the affected feature/platform; guard tests ensure
   application/unknown/repeated output is not hidden. The executable still emits
   the warning, and real inference is separately exercised.
6. Hook checks initially rejected the explicitly authorized public conduct email
   and release publisher arguments. Exact path/full-line exceptions, including
   negative tests for appended private text, resolve those false positives.
7. An initial container source copy lacked `.git`, so repository-index testing
   failed. Subsequent clean runs create an actual synthetic Git checkout; this
   setup error was not counted as a passing product test.

## Not fully verified, blocked, or deferred

- **Implemented but not hosted:** macOS/Linux GitHub workflow matrix, tag/main
  ancestry gate, artifact attestation and upload. Static validation/local runs
  cannot certify remote runner permissions or hosted provenance.
- **Authorization gate:** no publishing PRs, issues, tags, releases or remote
  setting changes. Actual public immutable-download verification cannot run
  until publication is authorized. Existing protections were read, not changed.
- **Independent acceptance:** no actual Codex/Claude application version or
  outside human has been certified. The custom process harness tests MCP
  `2024-11-05` and generated config shapes only.
- **Deferred with reason:** Debian 12 ARM64 upstream ONNX ABI compatibility;
  a lexical build is available. Windows/Android/HTTP transports, in-place
  destructive restore, power-loss guarantees, network filesystems and broad
  retrieval benchmarks are outside this milestone's validated scope.
- **No mock evidence:** checksum verification detects corruption; it is not an
  attestation. Local build records are unsigned. Candidate repeat installation
  is tested; bit-for-bit binary reproducibility is not promised.
