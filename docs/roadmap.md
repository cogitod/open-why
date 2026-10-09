# Roadmap and contributor tasks

open-why records rationale, preserves evidence and history, and retrieves it
through a standalone Rust library, CLI, and local MCP server. Its scope includes
decisions, facts, references, observations and their provenance. Hosts own work
items, sessions, coordination, raw transcripts, authentication and deployment.
See [design](design.md) and [integration contracts](integrations.md) for behavior.

## Completed beta foundation

The [published beta](https://github.com/cogitod/open-why/releases/tag/v0.1.0-beta.1)
includes store/daemon isolation fixes, optional local embeddings, a public
retrieval fixture, pinned model inputs, backup/restore, and verified versioned
artifacts. Follow-up fixes cover diagnostics, remote refresh and beta integration
manifests. [Readiness](readiness.md) summarizes the current assessment;
[verification](verification.md) preserves commands, results and failures.

## Contributor tasks

These scopes need no private tools, data or organization context. Check existing
[issues](https://github.com/cogitod/open-why/issues) before starting and follow
[CONTRIBUTING.md](../CONTRIBUTING.md). Priorities reflect remaining evidence and
adoption gaps, not promises of delivery dates.

| Priority / task | Acceptance | Starting point |
| --- | --- | --- |
| P1: Independent first use | An outside developer records version/checksum, OS, actual MCP client/version, and every step from install through retrieval, evaluation, backup/restore and a tested contribution; failed steps remain visible | [README](../README.md), `examples/quickstart.sh` |
| P2: Small Python or TypeScript MCP example | Use the public stdio contracts against a disposable store; demonstrate explicit scope, stable identity, evidence reads and clean process shutdown; no new server protocol | [Integrations](integrations.md) |
| P2: Broader actual-client acceptance | Record client/version and transport; exercise capture/index/search/get/history, missing evidence, supersession and reconnect | [Client verification scope](integrations.md#beta-client-verification-scope) |
| P2, first contribution: Independent retrieval case | Synthetic input and expected evidence/absence; explain a reproduced failure before changing ranking; keep held-out cases separate | `tests/public_evaluation.rs`, [evaluation guide](public-evaluation.md) |
| P2, first contribution: Clearer diagnostic | Reproduce confusing setup behavior; document recovery and add an appropriate process regression if behavior changes | `tests/onboarding.rs`, [maintenance](maintenance.md) |
| P2: Disk-full snapshot failure | Isolated fault injection; original data unchanged; partial destination rejected or removable; no personal stores | `tests/maintenance.rs`, `src/db/backup.rs` |

## Boundaries and deferred work

- Keep core storage, ranking, supersession and evidence logic in the library;
  consumers use public APIs and stable record IDs rather than copying internals.
- Preserve recognized schema migrations and narrow compatibility APIs for
  existing stores and consumers. Their continued use is intentional.
- Default ONNX builds require a compatible native C++ runtime; Debian 12 ARM64
  users can use lexical installation. Replacing that upstream runtime is deferred.
- Automatic contradiction adjudication, remote MCP transport, Windows/Android,
  orchestration, daemon supervision and destructive in-place restore are outside
  this beta's scope.
- Stable API guarantees, power/filesystem-loss testing and bit-for-bit binary
  reproducibility remain future work. The [stability policy](../STABILITY.md)
  defines the stronger release gate.
