# Design

## Purpose

Code history records what changed, who changed it, and when. The rationale often
lives elsewhere and becomes difficult to recover. open-why stores that rationale as
structured records, links records to Git commits, and retrieves them with stable
identities and source metadata.

The project is deliberately narrow. It is a decision-recall store, not a general
memory platform.

## Storage and identity

All records and links live in one local SQLite database. A record contains its text,
kind, scope, author, source, importance, temporal fields, and retrieval metadata.
Supported kinds include `decision`, `fact`, `reference`, `pattern`, `doc`, `project`,
and `observation`.

Records are retired through supersession rather than deletion. `superseded_by`,
`valid_from`, and `valid_until` preserve point-in-time identity while normal search
returns current records. Historical search and exact history reads can still reach
retired records.

`content_digest` and `source_identity` make capture and import idempotent for the
identities their write paths define. Git links are stored separately, so one record
can carry multiple commit references.

## Retrieval

Search combines two ranked candidate sets:

- A semantic arm uses local `all-MiniLM-L6-v2` embeddings or a configured compatible
  endpoint.
- A lexical arm uses SQLite FTS5 BM25 with weighted title and content columns.

Reciprocal-rank fusion combines the arms. Importance, effectiveness, recency, and
explicit feedback contribute to ranking. The constants and relevance checks have
regression coverage and should not change without representative evidence.

Without an available embedder, records remain eligible through lexical search.
`why feedback <id> --helpful` raises a record's effectiveness by 0.05;
`--not-helpful` lowers it by 0.03. Values are clamped to `[0.01, 1.0]` from an
ungraded default of 0.5.

## Git linkage

Repository indexing imports commit messages and recognized decision documents.
`mem-ref:` trailers and the explicit `why link` command bind commits to stored
records.

Library consumers can call `Store::get_current_evidence` with a stable record ID.
The method follows supersession to the active record and returns that record's Git
references plus the traversed chain.

MCP consumers can perform the inverse lookup with `open-why_commit_links`. An exact
stored commit hash and explicit scope return bounded, directly linked historical
record IDs. The IDs are not rewritten to their current successors.

## Exact MCP reads

The MCP server publishes three read contracts in deterministic order:

1. `open-why.current-rationale/v1`
2. `open-why.rationale-history/v1`
3. `open-why.commit-links/v1`

`open-why_get` resolves one exact ID in one explicit scope at server time. A
successful response contains the complete current record, its Git references, and
the supersession chain used for resolution.

`open-why_history` pages the exact forward chain rooted at a supplied ID. Records
stay in predecessor-to-successor order and each one carries its own Git references.
The cursor is the inclusive first record of the next page. One page uses one SQLite
read snapshot. History v1 validates each record's timestamp syntax and positive
interval, but it does not certify continuity or non-overlap between adjacent records.

`open-why_commit_links` performs an exact, case-sensitive lookup on stored commit
hash bytes within one scope. Results are directly linked record IDs and commit
subjects ordered by record ID. The inclusive cursor must identify an authorized row
for the same scope and hash. One page uses one read snapshot; separate pages observe
fresh database state.

Exact reads are bounded and return typed errors for invalid input, unavailable data,
invalid chains or cursors, and responses that exceed their limits.

## Interfaces

The `why` binary provides bare questions plus `init`, `capture`, `search`, `get`,
`link`, `import`, `fetch-model`, `feedback`, and `serve` commands. The crate also
exposes the storage and retrieval types as a Rust library. `why serve` presents the
same store through MCP over standard input and output.

See [retrieval parity](retrieval-parity.md) for the optional local harness used to
compare ranking behavior against a caller-supplied fixture.
