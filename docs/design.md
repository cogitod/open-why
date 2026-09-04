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

Each database also has a bounded, provider-minted `store_instance_id`. Initial
binding requires that explicit identity inside the immediate migration transaction;
later explicit mismatches fail with a typed error. A copied database keeps its
identity, while independently created databases receive different identities.
The schema identity combines a family, version, build-known SHA-256 shape digest,
and an append-only migration ledger whose checksums cover immutable executable SQL
payloads. Schema changes, metadata, ledger entries, record-digest backfill, and
`user_version` commit in one transaction. Only enumerated legacy shapes migrate.

`inspect_store` checks this identity through a read-only SQLite connection. It does
not create or migrate a database, create journal sidecars, or repair drift. A live
or indeterminate WAL state also fails closed so inspection cannot silently ignore
committed WAL content. Newer, partial, checksum-mismatched, corrupt, and
shape-drifted stores fail closed.

`open-why.record-digest/v1` seals the immutable record envelope with a versioned,
length-prefixed SHA-256 encoding. It covers scope, record ID, rationale fields,
observation time, tags, fact key, declared validity, and commit SHA. It excludes Git links,
supersession state, embeddings, ranking projections, feedback, and retrieval
counters. `Store::import_external` and `Store::import_external_sealed` provide
strict replay for library hosts and the CLI. An exact replay creates zero records;
changing a sealed field for the same store, scope, and record ID returns
`identity_conflict` before any record or relation effect. Database guards also
reject updates, deletion, and replacement of sealed records. The MCP import surface
publishes these success and conflict outcomes as `open-why.rationale-import/v1`.

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

Untrusted library hosts use `Store::get_current_evidence_in_scope`. It applies exact
scope authority during traversal, samples the Store production clock for each call,
and returns `open-why.scoped-current-evidence/v1`. That contract has its own typed
error enum and carries the current record's verified
`open-why.evidence-identity/v1` identity from the same read snapshot. Missing and
wrong-scope roots are indistinguishable; an unavailable foreign successor returns a
metadata-free broken-chain result. The MCP `open-why.current-rationale/v1` contract
and its outcome enum remain unchanged.

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
