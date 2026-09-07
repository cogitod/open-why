# End-user stability contract

This document defines what open-why means by a **stable release**. The project is
currently pre-1.0; this contract is a release gate, not a claim that the current
version is stable. A guarantee below is current only when its evidence is both in
the repository and run continuously on every proposed release. Everything listed
as an unmet gate is a target, not supported behavior.

open-why's stable product boundary remains narrow: it answers why a decision
exists and returns evidence that makes the answer checkable. Task management,
sessions, orchestration, process supervision, provider-specific behavior, and
vendor data models are not part of this contract.

## Platforms and toolchains

The only continuously verified end-user platform is Linux as provided by
GitHub's `ubuntu-24.04` runner. CI builds and tests the crate, CLI, and MCP stdio
server with Rust 1.88 (the MSRV declared in `Cargo.toml`) and the current stable
Rust toolchain.

The implementation contains Unix-specific path protections for Linux, macOS, and
Android, but macOS and Android are not continuously tested and therefore are not
currently supported platforms under this contract. Windows and other Unix
targets are also unsupported. Adding a platform to the supported set requires a
CI job that runs the full applicable evidence matrix on it.

## Compatibility

Until 1.0, every release must describe user-visible changes. The following rules
apply to a future stable release line:

- **Rust library:** Cargo SemVer applies to the public API exported by the crate.
  Patch and minor releases in a stable major version do not remove public items,
  change their types or documented meaning, or raise the MSRV. A breaking change
  requires a new major version. Internal modules and SQLite tables are not APIs.
- **CLI:** documented command names, flags, exit success/failure meaning, and
  machine-readable output remain compatible within a stable major version.
  Additive commands and optional flags are allowed. Human-readable prose is not
  a parsing contract unless explicitly documented as one.
- **MCP:** a name ending in `/vN` identifies an immutable application contract.
  Existing required fields, types, error codes, scope rules, and semantics do not
  change incompatibly. Additive optional fields are allowed only when old clients
  can ignore them. An incompatible change gets a new `/vN+1` name; protocol
  negotiation remains independent of application-contract versions.
- **Integration manifests:** `open-why.integration/v1` and its JSON Schema remain
  backward compatible as described in `docs/integrations.md`. Removing a
  capability or changing scope, identity, or a named contract requires a new
  manifest version. Manifests never authorize plugin loading or execution.
- **SQLite:** the store is private persistent state, not a public table-level API.
  Consumers must use the library, CLI, or MCP contracts. A compatible build opens
  the current schema and only explicitly recognized older schemas; migrations are
  transactional and recorded in a checksummed, append-only ledger. A newer,
  partial, structurally changed, or checksum-mismatched schema fails closed rather
  than being guessed at or downgraded. Stable releases must preserve an automated
  migration path from every store version supported by that stable major line.

## Durability and recovery

Completed capture, import, supersession, link, and feedback mutations are SQLite
transactions: success means the transaction committed; an error means callers
must not assume any part succeeded. Replaying the same capture identity or sealed
external record is idempotent. Reusing an identity with a changed immutable
payload fails with an identity-conflict error. Supersession retires a predecessor
without deleting it, preserves its evidence, rejects invalid chains, and resolves
current reads through a bounded chain or fails closed.

Opening a recognized legacy store performs its schema and identity migration in
one transaction. A failed migration must leave no partially accepted schema.
Opening with a different store identity, or reading sealed evidence from another
store, scope, record, or digest, fails before the requested mutation.

Automated child-process fault injection verifies two process-abort boundaries:
a completed capture and its sealed evidence survive reopening, and an interrupted
multi-column transaction leaves no partial mutation. The test uses an explicit
parent/child handshake and signal termination without timing sleeps or network
access. This is process-abort evidence only; open-why does not yet have power-loss
or filesystem-loss fault injection. Likewise, open-why currently detects several
corrupt or incompatible schema states and refuses them, but does not promise
repair of arbitrary database corruption.

Users remain responsible for retaining backups. `Store::backup_to` creates a
consistent SQLite snapshot while the source remains open, including committed
WAL state; copying only the main file while a writer is active is not a supported
backup method. Restore means opening that snapshot through `Store`, which
preserves and verifies the database's bound store identity and sealed evidence
identity. The destination must be new, and an unsuccessful operation removes the
newly created destination. Automated tests exercise the online round trip,
existing-file refusal, cleanup, symlink boundaries, and private Unix filesystem
state on Linux and macOS-compatible code paths. External filesystem or media
loss, backup retention, and arbitrary database repair remain outside the proven
guarantee.

## Privacy and network access

Records and retrieval state live in the configured local SQLite file. The MCP
server communicates over standard input/output and does not discover or execute
integration manifests. No record contents are sent over the network in the
default lexical path.

Network access may occur only in these explicit circumstances:

- dependency or ONNX Runtime downloads performed by Cargo during build/install;
- `why fetch-model`, or automatic model download when `OPEN_WHY_AUTO_FETCH=1`;
- embedding text sent to the operator-configured `OPEN_WHY_EMBED_URL` (with the
  configured model and optional bearer token); or
- Git clone or fetch when the CLI is explicitly given an HTTP(S), SSH, or
  `git@` repository URL. MCP repository arguments remain absolute local paths.

If a local model is already installed, local embedding does not send record text
to a service. Leak checks prevent known secret and private-provenance patterns
from entering tracked repository content; they do not inspect an end user's
database or prove that arbitrary input is non-sensitive.

## Safe and visible failure

Malformed protocol JSON produces a protocol error. Invalid, unknown, or
oversized MCP arguments, imports, manifests, pages, and responses produce bounded
typed errors rather than truncated success. Invalid history, cursors, temporal
values, cycles, and overlong supersession chains fail closed without returning an
unverified current record.

An unavailable remote network or embedding model may reduce search to lexical
retrieval where the documented operation supports best-effort embedding; an
explicit model fetch or required initialization fails visibly. It must never
fabricate semantic results. Identity mismatch, newer or incompatible schemas,
indeterminate live-WAL inspection, and detected corruption refuse the operation.
Errors are written to the interface's error channel and must not be reported as
success. Backend diagnostics exposed over MCP are bounded and redacted.

Multi-row and multi-relation mutations are atomic at their documented transaction
boundary. After an ambiguous external interruption, callers may retry only
operations documented as idempotent and must inspect state before attempting a
different mutation. Stronger crash-interruption evidence remains an unmet gate.

## Dependency integrity and release provenance

`Cargo.lock` is committed and is the only dependency resolution accepted by CI.
Metadata, release builds, lints, and tests fail rather than update it. Every pull
request and every commit to `main` checks that resolution against the current
RustSec advisory database and registry yank state. The same release-blocking job
allows only the licenses and registry sources declared in `deny.toml`; an
unavailable database, index, or policy tool is a failure, not a skipped result.

All external GitHub Actions are pinned to full commit SHAs. Container actions, if
introduced, must use an image digest. A repository-owned test reads the exact
committed or staged workflow blobs and rejects mutable tags, branches, short
SHAs, and container tags. Dependabot may propose pin updates, but a human review
and the complete evidence suite are required before they land.

There are no dependency-policy exceptions. A future exception must identify the
exact advisory or package version, explain why open-why is unaffected, link a
public review issue, and include a removal date. It must be removed or renewed by
review before that date. An expired, broad, unexplained, or unused exception
blocks a stable release; an exception is never evidence that the underlying
dependency is safe.

No release artifacts are published today. A future artifact is eligible for a
stable release only when an automated release workflow, at the exact tagged
revision, builds it from the committed lockfile and produces all of the
following without modifying the artifact afterward:

- a SHA-256 digest manifest covering every distributed file;
- a dependency SBOM for the artifact;
- a GitHub artifact attestation binding the artifact digest to this repository,
  workflow, commit SHA, tag event, and hosted build environment; and
- a clean verification step that downloads the final artifact and verifies its
  digest and attestation against `https://github.com/cogitod/open-why` before
  publication.

The release notes must link the workflow evidence and give users the digest and
attestation verification command. Signing or attestation proves origin and build
inputs, not correctness; all other stability evidence remains independently
required. These requirements define the future release gate and do not add an
artifact workflow or publish anything now.

## Evidence required for a stable release

A stable-version claim is allowed only when every required row below passes at
the exact release revision and all unmet gates have been converted into automated
checks or explicitly removed from the promised support set.

| Evidence | Required stable-release result | Current continuous evidence |
| --- | --- | --- |
| Formatting | `cargo fmt --check` | Yes, current stable and Rust 1.88 on Ubuntu |
| Release build | `cargo build --release` | Yes, both toolchains on Ubuntu |
| Lints | `cargo clippy --release --all-targets -- -D warnings` | Yes, both toolchains on Ubuntu |
| Tests | `cargo test` | Yes, both toolchains on Ubuntu |
| MSRV/current Rust | declared MSRV and current stable both pass | Yes, Rust 1.88 and current stable |
| Supported OS | full matrix passes on every claimed OS | Linux only; macOS/Android/Windows are unmet |
| MCP contracts | schemas, bounds, typed errors, exact reads, and stdio smoke tests pass | Yes on Ubuntu |
| Integration conformance | schema/examples and MCP probe pass automatically | Yes, exercised by `cargo test` on Ubuntu |
| Store compatibility | fresh, recognized legacy, newer, partial, corrupt, identity, and migration cases pass | Yes for covered fixtures on Ubuntu |
| Durability | idempotency, atomicity, supersession, migration, restart, and interruption tests pass | Logical cases and deterministic process-abort recovery pass on Ubuntu; power/filesystem-loss injection is unmet |
| Backup/restore | documented consistent backup and restore round-trip pass | Yes on Ubuntu; library snapshot includes committed WAL state and preserves store and sealed evidence identity |
| Privacy/leaks | tracked and staged-authority leak tests pass | Yes on Ubuntu |
| Repository hardening | Rust size control and its tests pass | Yes on Ubuntu |
| Dependency/supply chain | locked dependency audit and artifact provenance policy pass | Yes on Ubuntu: locked resolution, RustSec/yank, license/source policy, and immutable workflow-reference checks are release-blocking; future artifacts must satisfy the provenance policy above |
| Release artifacts | packaged crate/binaries install and pass smoke tests on every supported OS | Unmet |

Passing only the existing CI is necessary but not sufficient to call a release
stable. The release notes must name the supported OS set, MSRV, stable contract
versions, store migration range, and the evidence run. A failed, skipped, or
unavailable required check blocks the stable claim; it cannot be replaced by a
manual assertion.
