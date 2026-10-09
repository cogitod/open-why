# Public-beta verification report

Date: 2026-10-09. Baseline: `ac2f19b`. Reviewed runtime:
`ffb32f6a9a9187df69ee812ea9a38a1014e4aea6`, signed tag `v0.1.0-beta.1`.
[PR #44](https://github.com/cogitod/open-why/pull/44) established the beta;
[PR #45](https://github.com/cogitod/open-why/pull/45) fixed follow-up regressions;
[PR #46](https://github.com/cogitod/open-why/pull/46) added README illustrations.
All merged through required checks. The tag intentionally freezes PR #45's
validated runtime; later documentation changes do not change its bytes.

## Recommendation

**GO for supervised beta evaluation using the published prerelease; independent
human first use remains unverified.** The reproduced isolation defects are fixed.
The tagged source has passed hosted Linux/macOS checks and Rust 1.88 checks. Actual Codex CLI 0.162.0
completed the bounded evidence-read check described below. Independent human
first use and broader client acceptance remain unverified. Earlier sections
retain the environment and outcomes of the initial beta work; later dated
sections record follow-up checks and failures rather than treating them as
results from the same revision.

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

## Hosted PR verification

[CI run 37902743944](https://github.com/cogitod/open-why/actions/runs/37902743944)
passed `leak-check`, `macos-test` and required `build-and-test` for PR #44 head
`495acafde1ac183c2ce3f658ce518c74fbc5c84e` against main `ac2f19b`. GitHub
checks the PR merge checkout; later documentation commits require their own run.
No check was bypassed. The reviewed diff and passing automation are maintainer
verification, not an independent human review.

| Hosted configuration | Passed | Failed | Ignored |
|---|---:|---:|---:|
| macOS 14.8.9 ARM64, Rust 1.99 default | 170 | 0 | 3 |
| macOS 14.8.9 ARM64, Rust 1.99 lexical | 170 | 0 | 0 |
| Ubuntu 24.04 x86-64, stable default | 171 | 0 | 3 |
| Ubuntu 24.04 x86-64, stable lexical | 171 | 0 | 0 |
| Ubuntu 24.04 x86-64, explicit model cases | 2 | 0 | 0 |
| Ubuntu 24.04 x86-64, Rust 1.88 default | 171 | 0 | 3 |
| Ubuntu 24.04 x86-64, Rust 1.88 lexical | 171 | 0 | 0 |

Formatting, release builds, strict Clippy, source-package generation, dependency
audit and repository-policy regressions passed in that run. The explicit model
stage passed both previously ignored acceptance cases; the manual debug utility
remained ignored. This is source-tree CI, not hosted release artifact acceptance.

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
| Contribute | Topic commits, staged hooks, shell regressions and Rust matrix | Local checks pass; PR #44 opened with authorization; issues remain drafts |
| Pass CI | Local checks and hosted run 37902743944 | Required checks and macOS pass at PR head 495acaf; later revisions require their own run |
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

## Additional Linux packaged-artifact validation

A clean local clone at `495acafde1ac183c2ce3f658ce518c74fbc5c84e` was copied
into the Ubuntu 24.04 ARM64 container described above. Rust/Cargo 1.88.0 ran:

```bash
python3 scripts/prepare-release.py /output/candidate
cd /output/candidate/assets
sha256sum -c SHA256SUMS-aarch64-unknown-linux-gnu
ldd ../unpacked-binary/why
readelf --version-info ../unpacked-binary/why
```

The packaged source installed, and the extracted Linux binary passed every
scripted journey assertion. All four checksum entries matched. The binary links
system libc, libm and libgcc_s; its symbol requirements include `GLIBC_2.39`.
This archive is validated on Ubuntu 24.04 ARM64, not older glibc systems. The
Debian lexical source-build result does not certify this prebuilt archive there.

| Asset | SHA-256 |
|---|---|
| open-why-0.1.0-beta.1.crate | 5fdbf721820343be8cb153069a4c6b575e5d7139fedae5ff06b92f43b6e5a60a |
| open-why-0.1.0-beta.1-aarch64-unknown-linux-gnu-lexical.tar.gz | 54d74bb0e0ba26f277b3bf12086212131d42be3143def6ef1a4187a8ac54ab2c |
| build-aarch64-unknown-linux-gnu.json | 794d4e199298464dc590921aa74b26cdd5e0fc8a9a6ffcdffe8d87ebbaab8c23 |
| sbom-aarch64-unknown-linux-gnu.cdx.json | f1c9b508fed19abb9da8c4e188f980bc3661917be43a563c7bce28f67e7ad24b |

The source hash differs from the earlier macOS candidate because it includes the
later verification-report commit. These are separate local validation candidates,
not a release bundle; final platform builders must use one identical revision.
The hosted Linux runner uses x86-64, which this ARM64 container does not certify.

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

- **Implemented but not hosted:** release tag/main ancestry gate, artifact
  attestation and upload. The ordinary macOS/Linux CI matrix has executed; this
  does not certify release-workflow permissions or hosted provenance.
- **Authorization gate:** PR #44 and hosted CI are authorized. Tags, releases,
  public issues and remote setting changes remain outside this continuation.
  Public immutable-download verification awaits an authorized release. Existing
  protections were read, not changed.
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

## Post-merge regression and adapter audit

The final PR head `ae6f37d` passed [run 37904010530](https://github.com/cogitod/open-why/actions/runs/37904010530).
The squash-merged main revision `1f97e04` also passed [run 37906471276](https://github.com/cogitod/open-why/actions/runs/37906471276).
Passing those checks did not mean the software was bug-free. A follow-up audit
reproduced three gaps before applying fixes:

1. Doctor could approve automatic local-model fetching on a lexical build, or
   approve incomplete/corrupt local model inputs that normal startup rejects.
   This was a diagnostic inconsistency introduced by the beta feature/pin policy.
   Doctor now enforces feature availability, all required files and verified
   digests without downloading, loading a model or modifying the store.
2. Remote reindexing fetched commits but left HEAD unchanged when reading decision
   files. This predates the beta cache-isolation fix. The cache now follows fetched
   remote HEAD with a non-forced detached checkout. Tests cover subsequent edits,
   a changed default branch, conflicting local cache edits and failed fetch;
   refused refreshes preserve both cached edits and the evidence database.
3. The integration validator accepted only numeric versions, so the beta version
   introduced by PR #44 could not be declared in a manifest. Rust validation now
   uses the semver parser; JSON schema patterns and the example support prerelease
   and build versions. Tests compare accepted and rejected cases across both forms.

The new `examples/embedded_adapter.rs` is executed by a normal integration test
with both feature configurations. It uses public library APIs, synthetic external
rationale and no Git repository, model, credentials or private service. Stable IDs,
exact replay, supersession, evidence identity, missing/foreign-scope refusal,
lexical retrieval and verified backup all execute. It is a reference pattern, not
third-party product certification.

Local follow-up validation (macOS ARM64, Rust 1.98.0):

| Check | Result |
|---|---|
| `cargo test --locked` | 176 passed, 0 failed, 3 ignored |
| `cargo test --locked --no-default-features` | 175 passed, 0 failed, 0 ignored |
| Explicit pinned-model vector and retrieval tests | 2 passed, 0 failed; manual diagnostic remains ignored |
| Format, release build and strict Clippy for both feature sets | Passed |
| Dependency advisories, licenses and sources after adding semver | Passed |
| Documented standalone adapter example | Passed; second use of existing directory refused |
| Doctor against an intact pinned model and the example store | Digests verified; no model load or service request |

The first diagnostic-test attempt used an unbound store without supplying its
required identity; the test setup was corrected before reproducing the actual
product failures. No failed setup or skipped test is counted as product success.
The remaining release gates remain in IMPLEMENTATION_PLAN.md. Schema migration
and narrow compatibility APIs are retained deliberately; deleting them would
break existing data or consumers, not improve beta readiness.

## Signed beta candidate and actual Codex client (2026-10-09)

PR #45 merged the diagnostic, remote-refresh and integration-manifest fixes as
`ffb32f6a9a9187df69ee812ea9a38a1014e4aea6`. Its main CI passed
[run 37909312971](https://github.com/cogitod/open-why/actions/runs/37909312971).
The maintainer authorized release preparation, and `v0.1.0-beta.1` was signed
with the existing SSH agent key. Local signature verification passed before the
tag was pushed. Hosted candidate validation is
[run 37972316016](https://github.com/cogitod/open-why/actions/runs/37972316016).
The publication and final-byte verification results follow below.

Actual client: `codex-cli 0.162.0`, macOS ARM64, lexical `why` installed with the
locked Cargo dependency graph from exact revision `ffb32f6`. The public
`examples/quickstart.sh <new-demo-directory> codex` created a new synthetic Git
repository and store. The generated command, args, database path and persisted
identity were passed as per-run Codex overrides. User configuration was not
changed. The run used `codex exec --ignore-user-config --ephemeral --sandbox
read-only --disable multi_agent --disable plugins --disable shell_tool --json`;
only `open-why_ask` and `open-why_get` were enabled and approved for this demo.

- Initial attempt: MCP startup succeeded, but calls were refused because tool
  approval was required and the non-interactive run could not prompt. Exit code
  zero alone did **not** count as acceptance.
- Authorized retry: three MCP calls completed without errors (`ask`, `get`,
  `ask`). The final SQLite rationale matched the synthetic source, and the cited
  record/commit matched the demo repository's Git HEAD.
- Missing evidence: the encryption query returned the SQLite record, which did
  not establish an encryption choice. The client reported unknown. This does
  not mean lexical retrieval always returns an empty list for unsupported queries.
- Limits: client-driven supersession/reconnection, Claude Code, and independent
  human first use were not exercised. This is one bounded interoperability check,
  not a general client certification or retrieval-accuracy score.

PR #46 adds generated README illustrations with descriptive alt text and saved
prompts. They illustrate the product principles and do not represent actual UI
screenshots or additional correctness evidence.

## Published beta artifacts (2026-10-09)

[Release v0.1.0-beta.1](https://github.com/cogitod/open-why/releases/tag/v0.1.0-beta.1)
is a prerelease at `ffb32f6a9a9187df69ee812ea9a38a1014e4aea6`. Hosted
[run 37972316016](https://github.com/cogitod/open-why/actions/runs/37972316016)
passed all five jobs: leak check, Linux build/test (stable and Rust 1.88),
macOS tests, Ubuntu 24.04 candidate and macOS 14 candidate. The explicit pinned
model evaluation ran; ordinary ignored model tests alone were not counted.

Both candidate jobs ran `python3 scripts/prepare-release.py <new-output-dir>`
with Rust 1.88.0. Each installed the packaged source and exercised the extracted
lexical archive: version, demo, index, capture, search, MCP initialize, backup,
verification, restore and exact read. These are native x86-64 Linux and ARM64
macOS builds. All ten candidate files (including the duplicate source archive)
passed SHA-256 and GitHub attestation verification. The exact verification
command in [RELEASE.md](RELEASE.md#authorized-publication-process) was run for
each asset, enforcing the repository, release workflow, full source revision,
tag ref and GitHub-hosted runner identity.

The source archives were byte-identical, SHA-256
`8ad94fc274178cdbb66370db615c569f3fbdff45b87541d27055adc01b100040`.
One source archive and eight platform-specific assets were published unchanged.
All nine assets downloaded through public URLs using unauthenticated
`curl --fail --location`; both checksum manifests passed. An initial request
while `gh release create` was still uploading received 404 from its temporary
draft URL. After publication completed, the final URLs succeeded; that initial
request is not counted as a successful download.

On macOS 26.5.2 ARM64, installation from the public source tag succeeded:

```bash
cargo install --locked --git https://github.com/cogitod/open-why \
  --tag v0.1.0-beta.1 --bin why --no-default-features --root <new-install-dir>
<new-install-dir>/bin/why --version
```

Cargo recorded source `ffb32f6`; output was `why 0.1.0-beta.1`. Dependencies were
cached, so this does not claim an empty-cache or offline build. Independently,
the anonymously downloaded Apple Silicon archive was extracted into a new
directory. With `OPEN_WHY_BIN` pointing to that executable, the following
commands passed against a disposable fixture and its generated identity:

```bash
why --version
bash examples/quickstart.sh <new-demo-dir> generic
# Apply OPEN_WHY_DB and OPEN_WHY_STORE_INSTANCE_ID from the generated snippet.
why init <new-demo-dir>/repository
why search SQLite --scope <new-demo-dir>/repository
why capture --id public-release-smoke --title 'Public release smoke' \
  --content 'Synthetic public download verification'
why backup --to <new-snapshot-path>
why verify-backup <new-snapshot-path>
why restore <new-snapshot-path> --to <new-restored-path>
# Point OPEN_WHY_DB at the restored path, preserving the generated identity.
why get public-release-smoke
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize"}' | why serve
```

Assertions checked SQLite retrieval, exact restored content and MCP server
version. No existing store or user client configuration was modified.

Linux runtime inspection (`ldd why`, `readelf --version-info why`,
`readelf -d why`) identified glibc 2.39, libm and libgcc_s dependencies. The
archive failed to start on Ubuntu 22.04 x86-64 with `GLIBC_2.39 not found`; this
is an unsupported-host result, not a pass. Ubuntu 24.04 candidate execution
passed in the release workflow. The runtime requirement is explicit in release
notes, README and RELEASE.md; older GNU/Linux users should build from source.

Remaining acceptance: an independent developer's complete first-use and
contribution journey; Claude Code and broader real-client coverage. No general
bug-free, broad retrieval-accuracy, or stable-API claim follows from this release.
