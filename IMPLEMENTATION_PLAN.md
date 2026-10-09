# Public-beta implementation plan

Assessment date: 2026-10-09. Local topic branch: `feat/trustworthy-public-beta`.
See OSS_READINESS.md and VERIFICATION_REPORT.md for evidence rather than assuming
that a checked box establishes independent adoption.

## Implemented

- **P0:** reproduced both daemon defects; added canonical full-filename endpoints,
  preconnection identity checks, physical-store handshake and per-request file
  checks. Six process regressions cover direct/shared transport and failure paths.
  A separate real-Git regression covers full-URL cache identity and scope isolation.
- **P1 installation:** optional ONNX/tokenizers with compatible Cargo defaults;
  recommended lexical install; measured dependencies/size; fresh-source and
  packaged-artifact installation; macOS CI with the existing required check gate.
- **P1 retrieval:** committed synthetic fixture; pinned model revision/digests;
  actual local inference and vector assertions; explicit opt-in tests run by CI.
- **P1 maintenance:** backup/verify/restore commands, read-only nonmigrating source,
  online WAL snapshot, no-overwrite destination, evidence-preserving round trip.
- **P1 distribution:** beta version/lockfile, changelog and upgrade policy, clean
  source packaging, extracted-binary journey, SBOM/checksums/build record and
  manually dispatched provenance workflow. Nothing published.
- **P2:** accurate purpose/client limitations, contributor commands/tasks, private
  conduct email distinct from security reporting, scoped leak-hook exceptions.

## Remaining gates, in dependency order

| Priority | Work | Acceptance | Dependency / owner |
|---|---|---|---|
| P0 release gate | Review and run hosted checks on the exact proposed commit | Both required checks pass, macOS and real model stages execute; isolation regressions pass; conversations resolved | PR #44 authorized and open; hosted CI and maintainer review |
| P1 | Verify native distribution targets | Linux/macOS candidate archives are installed and exercised on their declared architecture; no unsupported platform claims | Local macOS/Ubuntu ARM64 archives pass; hosted x86-64/provenance pending |
| P1 | Independent first use | Outside developer records version/checksum, OS and actual MCP client/version; completes discover → install → evidence → evaluate → backup/restore → tested contribution | Reviewed candidate and an independent participant |
| P1 | Authorize version tag and candidate workflow | Tag/version match reviewed main history; final assets pass validation and GitHub provenance verification | Explicit maintainer authorization; no local substitute for hosted attestations |
| P1 | Publish and verify downloads | Immutable public install and checksum/provenance checks succeed from public URLs | Explicit release authorization; all previous gates |
| P2 | Create scoped public issues | Publish the acceptance criteria in docs/contributor-tasks.md as focused issues | Authorization to create externally visible issues |
| P2 | Broaden held-out evaluation | Independently authored cases and real user traces are sanitized/consented; failures recorded before changing ranking | Actual adoption; no manufactured scores |

## Deliberately deferred

- Fixing upstream ONNX's Debian 12 ARM64 C++ ABI requirement: lexical installation
  works without that dependency; default-model builds need a compatible native
  runtime. The limitation and newer-environment validation are recorded.
- Automatic contradiction adjudication, remote MCP transports, Windows/Android,
  orchestration, daemon supervision and in-place destructive restore: outside
  this beta's scope. Existing Unix stdio and daemon transports remain the focus.
- Stable 1.0 guarantees, hardware/power-loss fault testing, signed local builds
  and bit-for-bit binary reproducibility: do not claim them from ordinary CI.
