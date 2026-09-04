# open-why

open-why is a Rust library and local MCP server that lets an LLM ask why code
decisions were made. It indexes Git history and decision documents, stores
rationale in SQLite, and returns scoped records with source metadata. A CLI is
included as a convenience for setup and inspection.

## What the LLM gets

You ask your LLM:

> Why does this repository use SQLite?

The LLM calls `open-why_ask` with the question and an absolute repository path.
On first use, open-why indexes that repository. The result is structured for a
follow-up exact read:

```json
{
  "status": "ok",
  "scope": "/path/to/repository",
  "results": [
    {
      "id": "8f2c41ab0123456789abcdef0123456789abcdef",
      "kind": "commit",
      "title": "Use SQLite for local storage",
      "preview": "Keep setup local and store rationale in one portable database.",
      "preview_truncated": false,
      "source": "commit",
      "author": "Developer",
      "date": "2026-08-12T10:00:00Z"
    }
  ]
}
```

The LLM can pass the returned ID and scope to `open-why_get` for the complete
current record, its Git links, and the supersession chain used to resolve it.

## Install and connect

open-why requires Rust 1.88 or newer.

```bash
git clone https://github.com/cogitod/open-why.git
cd open-why
cargo install --path .
```

Configure your MCP client to run the installed binary:

```json
{
  "command": "/path/to/why",
  "args": ["serve"],
  "env": {
    "OPEN_WHY_STORE_INSTANCE_ID": "your-client:open-why:replace-with-unique-id"
  }
}
```

Choose the store identity once, make it unique to this database, and keep it
stable in the client configuration. The first launch binds the new database to
that identity. Later launches verify the same identity before opening it.

Then ask the LLM a question about the repository. The MCP call requires an
absolute path:

```text
Use open-why to answer: why is the sandbox separate?
Repository: /path/to/repository
```

Storage stays in a local SQLite file. Search is lexical-first unless a local or
remote embedder is configured.

## MCP tools

| Tool | Purpose |
| --- | --- |
| `open-why_ask` | Index if needed, then return scoped rationale previews for a question. |
| `open-why_index` | Index one explicitly identified Git repository. |
| `open-why_capture` | Store a bounded rationale in an explicit scope. |
| `open-why_import` | Import bounded records into an explicit scope. |
| `open-why_search` | Search one scope and return stable-ID previews. |
| `open-why_get` | Resolve an exact stable ID to complete current rationale and evidence. |
| `open-why_history` | Page one exact supersession chain with record-local Git evidence. |
| `open-why_commit_links` | Find direct rationale links for one exact commit hash and scope. |
| `open-why_link` | Link a commit to a record in an explicit scope. |
| `open-why_feedback` | Record whether a retrieved record was helpful. |

Record reads and mutations require an explicit `scope`. Asking and indexing require
an explicit absolute repository path. Tool schemas reject unknown fields and bound
input and response sizes.

## Exact read contracts

### Current rationale

`open-why_get` implements `open-why.current-rationale/v1`. Given an exact record
ID and scope, it follows the supersession chain at the server's current time. It
returns the complete current record, that record's Git references, and the IDs it
traversed. Unavailable records and invalid chains return typed errors.

### Rationale history

`open-why_history` implements `open-why.rationale-history/v1`. It pages one exact
forward chain in predecessor-to-successor order. Each item contains a complete
historical record and that record's Git references. A cursor names the inclusive
first record of the next page. Each page uses one coherent, current database
snapshot.

The contract validates each record's temporal fields. It does not claim that
adjacent intervals are contiguous or non-overlapping.

### Commit links

`open-why_commit_links` implements `open-why.commit-links/v1`. Given an explicit
scope and exact, case-sensitive stored commit hash, it returns directly linked
historical record IDs and commit subjects in ascending record-ID order. It does not
return rationale bodies or rewrite IDs to current successors.

Its cursor is the inclusive first record of the next page. Each page is a fresh,
coherent snapshot. Pass a returned ID to `open-why_get` to resolve current rationale.

## CLI convenience

The same store is available from a terminal for setup and inspection:

```bash
why "why is the sandbox separate?"                         # index if needed, then ask
why init /path/to/repository                               # index explicitly
why capture --title "Use SQLite" --content "..."          # capture in global scope
why search "sqlite" --scope /path/to/repository           # search one scope
why search "sqlite" --types decision,fact --historical    # include retired records
why get <record-id>                                        # resolve to the current record
why get <record-id> --historical                           # print the forward chain
why link <commit-hash> <record-id> --subject "Commit title"
why feedback <record-id> --helpful
why import --file decisions.json
why fetch-model                                            # cache the local embedder
why serve                                                  # MCP over standard input/output
```

Run `why --help` or `why <command> --help` for all arguments.

## Rust library

```rust
use open_why::Store;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let store = Store::open_with_store_instance_id(
        Path::new("/path/to/open-why.db"),
        "my-app:open-why:replace-with-unique-id",
    )?;
    let hits = store.search("why sqlite", &["my-project"], &[], 10)?;
    for hit in hits {
        println!("{}: {}", hit.subject, hit.date);
    }
    Ok(())
}
```

Mint one stable, unique identity for each database. New databases use
`Store::open_with_store_instance_id`; `Store::open` reopens an already-bound
database with lexical search. `Store::open_default` uses the configured embedder
and default database path, and reads the required first-binding identity from
`OPEN_WHY_STORE_INSTANCE_ID`.

Library hosts can inspect a database before opening it:

```rust
use open_why::{inspect_store, Store, StoreCompatibility};
use std::path::Path;

let path = Path::new("/path/to/open-why.db");
match inspect_store(path)? {
    StoreCompatibility::Compatible { identity } => {
        println!("store {}", identity.store_instance_id);
    }
    StoreCompatibility::MigrationRequired { .. } => {
        let store = Store::open_with_store_instance_id(path, "my-host:primary")?;
        println!("store {}", store.store_identity()?.store_instance_id);
    }
    state => println!("store is not ready: {state:?}"),
}
# Ok::<(), anyhow::Error>(())
```

`inspect_store` is read-only: it does not create a path, migrate a schema, or
write SQLite sidecars. A live or indeterminate WAL state fails closed instead of
reporting a potentially stale main-file view. Initial binding requires a
provider-minted identity of 1 to 128 ASCII letters, digits, `.`, `_`, `:`, or `-`;
a later explicit mismatch fails with a typed identity error.

`Store::get_current_evidence_in_scope` resolves Current at the Store clock in one
snapshot and returns `open-why.scoped-current-evidence/v1`, including a verified
sealed evidence identity. Git links, supersession state, feedback, and retrieval
counters do not change that identity. `Store::import_external` and its compatibility
alias `Store::import_external_sealed` accept exact replays, report only newly created
records, and reject a changed immutable envelope with `RecordIdentityConflict`
before record or relation effects. `open-why_import` exposes the same result as
`open-why.rationale-import/v1`. Existing MCP Current v1 outcomes remain unchanged.
Canonical temporal values use ASCII `YYYY-MM-DDTHH:MM:SS[.digits]Z`. Their shared
128-byte limit is measured over UTF-8 at runtime and generated from the same public
constant in MCP catalog schemas.

`Store::link_git_in_scope` accepts the sealed `EvidenceIdentity` returned by the
scoped Current read. It verifies the store, scope, record, and immutable digest in
one immediate transaction before creating a Git link. Its versioned result reports
`created`, `exact_replay`, or a fixed typed error without exposing record authority.
The existing `Store::link_git` method is retained only as a trusted, unscoped
compatibility API. New scoped integrations should not call it. The MCP server keeps
the existing `open-why_link` schema and success payload, but delegates its write to
the scoped method.

## Configuration

| Variable | Effect |
| --- | --- |
| `OPEN_WHY_DB=/path/to/open-why.db` | Use a specific SQLite database. |
| `OPEN_WHY_STORE_INSTANCE_ID=my-host:primary` | Bind a new or migrating database to a provider-minted identity. |
| `OPEN_WHY_EMBED_MODEL_PATH=/path/to/all-MiniLM-L6-v2` | Use a local embedding model. |
| `OPEN_WHY_AUTO_FETCH=1` | Download the local model on first use if the cache is empty. |
| `OPEN_WHY_EMBED_URL=https://example.invalid/embeddings` | Use an OpenAI-compatible embedding endpoint. |
| `OPEN_WHY_EMBED_MODEL=model-name` | Choose the remote model; the default is `text-embedding-3-small`. |
| `OPEN_WHY_EMBED_API_KEY=...` | Send a bearer token to the remote embedding endpoint. |
| `OPEN_WHY_DEBUG_RANK=1` | Print ranking diagnostics to standard error. |
| `ORT_LIB_LOCATION=/path/to/onnxruntime` | Build against an installed ONNX Runtime. |

On Unix, a database path must not contain symbolic-link directory components or
name a symbolic-link file. Resolve trusted path aliases before the first launch,
then keep the same concrete path in the client configuration.

Without embedding configuration, open-why uses a previously fetched local model if
present. Otherwise, search remains lexical-first. `why fetch-model` stores
`Xenova/all-MiniLM-L6-v2` under `~/.cache/open-why/models/`.

## Project information

- [Design and behavior](docs/design.md)
- [Retrieval parity harness](docs/retrieval-parity.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Apache-2.0 license](LICENSE)
