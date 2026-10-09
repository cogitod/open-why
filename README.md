# open-why

**Keep the reasons within reach.**

An open-source project by [Cogito](https://cogito.cv). Apache-2.0.

Code records what changed. The reasons are often scattered across commits and
decision documents. open-why makes that recorded reasoning available to you and
your AI tools, with sources you can inspect before deciding what still applies.

open-why is a Rust library and local MCP server that lets an LLM ask why code
decisions were made. It indexes Git history and decision documents, stores
rationale in SQLite, and returns scoped records with source metadata. A CLI is
included as a convenience for setup and inspection.

## Why we build this

Our promise is **Make intelligence compound.** We build tools that help
people carry what they learn into what they do next. open-why expresses that idea
by preserving the reasons behind decisions and making them available for reuse.

The returned records are evidence to examine. They do not establish that a past
decision was correct, and missing rationale is not permission to invent an
explanation. You can use open-why independently of our other products.

## Install from source

You need Git, Rust 1.88 or newer, and the native build tools for your operating
system. This is a pre-1.0 project. Linux is the only platform continuously
verified by CI; macOS is not yet a supported platform under the
[stability contract](STABILITY.md).

```bash
git clone https://github.com/cogitod/open-why.git
cd open-why
cargo install --locked --path . --bin why
why --version
```

Ensure Cargo's binary directory (normally `~/.cargo/bin`) is on your `PATH`.
Building downloads ONNX Runtime even when you intend to use lexical search;
see [build troubleshooting](CONTRIBUTING.md#local-setup) for
`ORT_LIB_LOCATION` when the download is unavailable. Normal lexical retrieval
requires no embedding model or API key.

## Set up your coding agent

Choose one database and reuse its configuration across clients. Setup creates a
new store with a unique, persisted identity, or reads the identity of an existing
compatible store. It prints configuration without editing your client settings,
loading an embedding model, or making network requests.

For Codex:

```bash
why setup --db "$HOME/.cache/open-why/open-why.db" --client codex
```

Merge the printed TOML into `~/.codex/config.toml`. For Claude Code, run the same
command with `--client claude-code` and merge its JSON into your project's
`.mcp.json`. Keep machine-specific paths out of version control. Update an
existing `open-why` entry instead of adding a duplicate. Other clients can use
`--client generic` for a command/args/env JSON entry.

The snippets use an absolute executable path and set `OPEN_WHY_DB` and
`OPEN_WHY_STORE_INSTANCE_ID`. Retain both values when copying the configuration.
If you already supply an identity through the environment, setup honors it and
refuses a mismatch. Never assign a new identity to an existing database.

Reconnect your client and check that `open-why` and its tools appear in MCP
status. See the official [Codex configuration guide](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
and [Claude Code configuration guide](https://code.claude.com/docs/en/mcp).
A generated configuration alone does not establish a successful connection.

## Get your first answer with evidence

Paste this into your coding agent, replacing the path with an absolute Git
working-tree path:

```text
Use open-why_ask to answer: why does this repository use SQLite?
Repository: /absolute/path/to/repository
Then call open-why_get with a returned ID and the same scope. Cite the recorded
reason, record ID, and available source or commit evidence. If the records do
not establish the reason, say unknown. Do not edit the repository.
```

`open-why_ask` returns scoped previews. `open-why_get` retrieves the complete
current record, its Git links, and the supersession chain. These are recorded
reasons to inspect, not proof that a decision was correct.

Indexing reads commit messages and recognized decision Markdown files from Git,
not uncommitted changes or all conversations. Automatic indexing happens only
when the scope contains no records. After new commits—or if you captured a
record before the first ask—call `open-why_index` with the same absolute
repository path. Retrieval does not automatically refresh a populated scope.

### Try an isolated example

From the cloned open-why checkout, with `why` installed:

```bash
bash examples/quickstart.sh "$PWD/../open-why-demo" codex
```

Use `claude-code` or `generic` for another configuration format. The destination
must not already exist, and its parent must exist. The script creates a synthetic
Git repository, a dedicated database, and a configuration snippet, then prints
the exact prompts to use. It does not change your client settings or your normal
store. Use the snippet temporarily in place of an existing open-why entry and
restore your usual configuration afterward.

The example records a SQLite choice for offline use on one laptop with one
writer and no database service. A successful first answer cites that reason and
the original commit. The script also supplies a missing-evidence question and a
capture/supersession exercise: resolving the original captured ID returns the
replacement, while history retains both records. These are synthetic assertions,
not measured database performance claims.

After disconnecting the demo in your client, delete only the directory you
created. Its repository, configuration, and database are all contained there.

## Check and recover your setup

```bash
why doctor --db "$HOME/.cache/open-why/open-why.db" --repo /absolute/path/to/repository
```

Omit `--db` to use `OPEN_WHY_DB` or the normal default path. Omit `--repo` to
check only the store and embedding configuration. Doctor is read-only: it does
not create a database, write SQLite sidecars, load models, download files, or
contact services. Exit status 1 means a check failed or could not be verified.
Remote connectivity, model inference, client connectivity, and index freshness
are not verified by this command.

| Symptom | Recovery |
| --- | --- |
| `why` is not found | Add Cargo's binary directory to `PATH`; run `why --version`. |
| No MCP tools appear | Check your client's MCP status, the snippet's absolute executable path, and its stderr; reconnect after updating configuration. |
| `identity_mismatch` | Restore the identity from the original configuration, or choose a new database path for an independent store. |
| Database path rejected | Use an absolute path without symlink components; resolve trusted directory aliases first. |
| Empty or incompatible existing file | Choose a new path, use the compatible build, or restore a verified backup. Setup does not overwrite or repair existing files. |
| `migration_required` | Preserve a consistent backup, then use the documented Rust Store open API with the original identity to migrate a recognized legacy store. Setup deliberately refuses migration. See [durability and recovery](STABILITY.md#durability-and-recovery). |
| `live_wal_indeterminate` | Read-only inspection cannot verify this store. Close its writers and inspect a safely checkpointed snapshot, or use a new path. Do not delete WAL/SHM files. |
| Local model missing or startup fails loading it | Restore the configured model files or unset the explicit model path. A cached model is also loaded automatically when present; move an unusable model cache aside to use lexical retrieval. |
| Empty results or missing new decisions | Use the exact same absolute repository path as the scope, explicitly index it, and check that the reason was actually recorded. |
| Remote embeddings configured unexpectedly | Inspect `OPEN_WHY_EMBED_URL` in the server environment. Removing it restores local/default selection; a connected cloud-model client still receives records it retrieves. |

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

Create a consistent backup while the source store remains open, then restore by
opening the snapshot as a normal bound store:

```rust
use open_why::Store;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let source = Store::open(Path::new("/path/to/open-why.db"))?;
    source.backup_to(Path::new("/path/to/new-backup.db"))?;

    let restored = Store::open(Path::new("/path/to/new-backup.db"))?;
    assert_eq!(source.store_identity()?, restored.store_identity()?);
    Ok(())
}
```

`Store::backup_to` uses SQLite's online-backup mechanism, so the snapshot
includes committed WAL state. The destination must not already exist; on Unix,
new directories are private and the database is created with mode `0600`.
Failure removes the newly created destination rather than leaving a partial
database that appears restorable. Restore means opening the snapshot through
`Store`; retaining and protecting backup files against external filesystem or
media loss remains the operator's responsibility.

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

## Third-party integrations

Developer tools integrate through the MCP stdio server or the Rust library.
`open-why.integration/v1` provides a machine-readable compatibility manifest and
an executable conformance check without loading third-party code into the server.
See [the integration standard](docs/integrations.md).

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
- [End-user stability contract](STABILITY.md)
- [Retrieval parity harness](docs/retrieval-parity.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Apache-2.0 license](LICENSE)
