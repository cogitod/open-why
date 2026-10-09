# Public-beta readiness

Assessment: 2026-10-09. Baseline: `ac2f19b`. Prerelease: `0.1.0-beta.1`.
Runtime fixes are reviewed and merged at `ffb32f6` (PR #45). The signed
`v0.1.0-beta.1` tag points to that revision. README illustrations landed separately
in PR #46. See [verification evidence](VERIFICATION_REPORT.md).

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
| Installation | Source and published archive tested | Recommended lexical install omits ONNX/tokenizers; packaged source and extracted binary pass the demo and maintenance journey; platform results in report |
| Retrieval | Behavioral evidence available | Committed synthetic fixture; lexical and real local-model runs; immutable model revision and verified SHA-256 inputs; no broad accuracy claim |
| Data maintenance | Tested | Online snapshot, schema/SQLite/digest verification, restore-to-new-path, preserved identity/evidence; live WAL/daemon and refusal tests |
| Versioning and artifacts | Published prerelease verified | Signed `v0.1.0-beta.1`; workflow 37972316016 passed; all assets passed checksum/provenance verification, all nine downloaded anonymously, and the public macOS archive passed the maintenance journey |
| CI | Reviewed runtime revision passed | Main `ffb32f6` passed run 37909312971; README PR #46 passed run 37971256136; release-candidate validation is separate |
| Contributions | Ready for outside review | Existing guide/templates/hooks retained; actual feature-matrix commands, scoped issue drafts, separate security/conduct contacts |
| Client acceptance | Bounded Codex check passed | Codex CLI 0.162.0 on macOS ARM64 retrieved correct evidence through three real MCP calls; independent human first use, Claude Code and broader client journeys remain unverified |

**Verdict: GO for supervised beta evaluation using the published prerelease;
independent first use remains an open acceptance gate.** No unresolved critical isolation
defect was observed in the executed tests. Passing tests is not a guarantee of
bug-free behavior or a substitute for independent first use.

PR #44 established the beta foundation; PR #45 fixed the reproduced follow-up
regressions and validated a vendor-neutral embedding example. Both were merged
through the required checks. The maintainer authorized the signed version tag
and release publication. The actual Codex client result and its approval-policy
limitation are recorded in [integration notes](docs/integrations.md#beta-client-verification-scope).
Linux ARM64 local packaged-artifact validation also passed; that Ubuntu 24.04
binary requires glibc 2.39. Published targets are Apple Silicon and Ubuntu 24.04
x86-64; the Linux archive also requires glibc 2.39. See the [release](https://github.com/cogitod/open-why/releases/tag/v0.1.0-beta.1).
