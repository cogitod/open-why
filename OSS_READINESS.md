# Public-beta readiness

Baseline: `ac2f19bab912871f1857f1e230db7b912e371ba8`. Assessment date: 2026-10-09.
This milestone precedes the stronger guarantees in STABILITY.md.

Ready means an outside developer can understand the purpose, install an
identifiable version, retrieve evidence, maintain data, and contribute a tested
change using public material alone.

| Area | Evidence / remaining gate |
|---|---|
| OSS independence | Apache-2.0, standalone library/CLI/MCP; retained |
| PR governance | Existing required PR/check configuration; no remote changes in this work |
| Isolation | Baseline regressions reproduced identity bypass and cross-store read; correction under test |
| Installation | Default ONNX build dependency being evaluated; clean install pending |
| Retrieval | Public reproducible fixture and real model execution pending |
| Maintenance | Existing online snapshot library tested; user workflow pending |
| Distribution | No published beta; local release preparation pending |
| Contributions | Existing guides/hooks retained; exact build matrix to be aligned |
| Adoption | Automated protocol harness is not independent human/client acceptance |

See IMPLEMENTATION_PLAN.md and the final verification report for exact gates.
No public-beta release is authorized by this assessment.
