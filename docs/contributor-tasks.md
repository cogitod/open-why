# Prioritized public issue drafts

These are issue-ready scopes kept locally until issue creation is authorized.
No internal tools, accounts, private corpus or company context are required.

| Priority / title | Acceptance criteria | Starting point |
|---|---|---|
| P1: Independently verify first use with a released candidate | Record OS, exact binary checksum, client/version, commands, retrieved evidence and unknown-answer behavior; report every failed step | README, examples/quickstart.sh |
| P1: Verify published downloads and provenance | Install public source/binary by immutable version; verify checksums and attestation against reviewed SHA; repeat backup/restore | RELEASE.md |
| P2, first contribution: Add an independently authored retrieval case | Synthetic input and explicit expected evidence/absence; explain failure before any ranking change; no private corpus | tests/public_evaluation.rs |
| P2, first contribution: Improve a concrete diagnostic | Reproduce a confusing setup error, add one process regression, document expected recovery | tests/onboarding.rs |
| P2: Test additional MCP clients | Actual client version and transport, not just generated-config parsing; capture/index/search/get/history, reconnect and missing evidence | docs/integrations.md |
| P2: Exercise disk-full snapshot failure | Isolated filesystem fault injection; original unchanged; partial new destination clearly rejected or removable; never use a personal store | tests/maintenance.rs, src/db/backup.rs |

Use the existing contributor workflow and test commands in CONTRIBUTING.md. A
ranking change needs independently justified cases beyond this tiny fixture.
