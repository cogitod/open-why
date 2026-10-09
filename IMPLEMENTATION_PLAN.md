# Public-beta implementation plan

Assessment date: 2026-10-09. Base beta work merged as `1f97e04` (PR #44).
The post-merge regression follow-up landed in PR #45 at `ffb32f6`. The signed
`v0.1.0-beta.1` tag identifies that reviewed runtime revision.
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
  manually dispatched provenance workflow. Signed beta published with verified
  hosted provenance, anonymous downloads and a public macOS archive smoke test.
- **Post-merge audit:** initial merged commit passed hosted CI run 37906471276.
  Reproduced and fixed false-positive model diagnostics, stale remote decision-file
  reads and rejection of beta versions in integration manifests. Added a tested
  vendor-neutral host example. Historical schema migration and narrow public API
  compatibility wrappers remain intentional data/caller protections.
- **P2:** accurate purpose/client limitations, contributor commands/tasks, private
  conduct email distinct from security reporting, scoped leak-hook exceptions.

## Remaining acceptance and follow-up work

| Priority | Work | Acceptance | Dependency / owner |
|---|---|---|---|
| P1 | Independent first use | Outside developer records version/checksum, OS and actual MCP client/version; completes discover → install → evidence → evaluate → backup/restore → tested contribution | Actual Codex 0.162.0 evidence read passed; an independent participant is still needed |
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
