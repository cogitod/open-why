# Public-beta readiness

Assessment: 2026-10-09. Baseline: `ac2f19b`. Candidate: `0.1.0-beta.1`.
Runtime and release tooling validated through `273ecb2`; subsequent assessment
edits do not constitute a published release. See [verification evidence](VERIFICATION_REPORT.md).

Ready means an outside developer can understand the purpose, install an
identifiable version, retrieve useful evidence, maintain data safely, and
contribute a tested change using public material alone. This is weaker than the
stable-release contract in STABILITY.md.

| Area | Status | Evidence / remaining gate |
|---|---|---|
| OSS independence | Ready | Apache-2.0; standalone Rust library, CLI and MCP; no private service required |
| Purpose | Ready | Recorded rationale and inspectable evidence, without claims of verified truth |
| PR governance | Existing controls confirmed | Read-only GitHub inspection: PR required, strict current-branch checks, admin enforcement, resolved conversations, squash-only; zero required human approvals disclosed |
| Store isolation | Implemented and tested | Six process regressions cover wrong identities, suffix collisions, concurrent stores, stale/redirected endpoints, replaced files and daemon death; baseline defects reproduced before fixing |
| Repository scope | Implemented and tested | Remote URL basename collision reproduced and fixed; separate hashed caches, verified origins and explicit failed-refresh errors |
| Installation | Tested locally | Recommended lexical install omits ONNX/tokenizers; packaged source and extracted binary pass the demo and maintenance journey; platform results in report |
| Retrieval | Behavioral evidence available | Committed synthetic fixture; lexical and real local-model runs; immutable model revision and verified SHA-256 inputs; no broad accuracy claim |
| Data maintenance | Tested | Online snapshot, schema/SQLite/digest verification, restore-to-new-path, preserved identity/evidence; live WAL/daemon and refusal tests |
| Versioning and artifacts | Prepared, unpublished | `0.1.0-beta.1`, changelog, locked source archive, host binary, SBOM, checksums and local build record; remote attestation/download gates remain |
| CI | Merged beta revision passed | Run 37906471276 passes at main 1f97e04; final PR run 37904010530 also passed; follow-up revisions require their own checks |
| Contributions | Ready for outside review | Existing guide/templates/hooks retained; actual feature-matrix commands, scoped issue drafts, separate security/conduct contacts |
| Independent adoption | Unverified | Real CLI/process harness is tested; no claim of actual Codex/Claude application-version acceptance or an independent human completing first use |

**Verdict: base beta work merged; regression follow-up ready for PR review; NO-GO for public
release yet.** No unresolved critical isolation defect was observed in the
executed tests. Public release still requires landing the regression follow-up, hosted checks on
the release revision, validated/attested published artifacts, and recorded external-client acceptance.
[PR #44](https://github.com/cogitod/open-why/pull/44) was squash-merged with
maintainer authorization after all required checks passed. No remote settings, issues, tags or
releases were created. Linux ARM64 packaged-artifact validation also passed;
that Ubuntu 24.04 binary requires glibc 2.39 (see the report).
