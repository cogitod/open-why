# open-why roadmap

open-why has one job: answer why a decision exists and return the evidence that
makes the answer checkable. It is a standalone Rust library, CLI, and MCP server;
it does not require or assume any particular agent host, continuity service, or
private data source.

## Product boundary

open-why owns:

- decision, fact, reference, pattern, document, project, and observation records;
- source, author, date, and Git evidence;
- temporal supersession and historical chains;
- hybrid lexical and semantic retrieval;
- retrieval feedback and ranking quality.

open-why does not own:

- mutable work or task lifecycle state;
- sessions, agent presence, messages, or coordination;
- terminals, worktrees, process supervision, or deployment;
- organization policy, tenancy, access control, or orchestration;
- provider-native transcripts.

Consumers compose those concerns around the library through stable record IDs.
They must not copy open-why's storage, ranking, supersession, or evidence logic.

## Stable contracts

1. **Decision linkage.** A `mem-ref:` Git trailer creates a bidirectional link:
   commit to rationale and rationale to commits.
2. **Temporal identity.** A superseding record retires its predecessor without
   deleting history. Current reads resolve to an active record or fail closed.
3. **Evidence-bound recall.** Answers include record identity and available
   source, author, date, and Git proof.
4. **Hybrid retrieval.** Lexical and semantic candidates are fused with stable,
   regression-tested weights and a relevance gate.
5. **Capture provenance.** `content_digest` and `source_identity` make repeated
   capture idempotent.
6. **Feedback.** Helpful and unhelpful verdicts adjust effectiveness within a
   bounded range.
7. **Integration boundary.** Third-party tools use versioned MCP or Rust library
   contracts with explicit scope and store identity. open-why never loads vendor
   plugins or adopts vendor task, session, or orchestration concepts.

## Release path

- Keep the public crate and binary self-contained and local-first.
- Maintain sanitized retrieval fixtures that reveal no private corpus content.
- Publish ranking changes only with regression evidence.
- Add library operations only when they strengthen the focused rationale contract.
- Keep downstream integration guidance generic and client-neutral.
- Keep the integration manifest, examples, and executable conformance checks in
  lockstep with the versioned contracts they declare.

## Non-goals

open-why will not become a general memory platform, task tracker, agent mesh, host
runtime, or compatibility facade. A useful downstream feature is not automatically
an open-why feature.

## Positioning

> Other memory systems remember what. open-why remembers why, with the proof.
