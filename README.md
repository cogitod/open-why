# open-why

![open-why — decision memory for coding agents, with evidence you can inspect. An archival decision card links to its source documents.](docs/assets/open-why-hero.png)

[![CI](https://github.com/cogitod/open-why/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/cogitod/open-why/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/github/license/cogitod/open-why)](LICENSE)

**Decision memory for coding agents, with evidence you can inspect.**

Ask why a repository uses SQLite, why a module is separate, or what replaced an
old decision. open-why retrieves recorded rationale from Git history and decision
documents, with source links and supersession history. When the reason was never
recorded, the answer should be **unknown**.

Use it through a local MCP server, the `why` CLI, or a Rust library. Records live
in SQLite on your machine. Lexical retrieval needs no model or API key. Your
connected AI client can receive retrieved records; optional remote embeddings
also send text to the configured provider.

[Install](#install-from-source) · [Connect your agent](#set-up-your-coding-agent) ·
[Try an example](#try-an-isolated-example) · [Contribute](CONTRIBUTING.md)

An open-source project by [Cogito](https://cogito.cv). Apache-2.0.

## Install from source

You need Git, Rust 1.88 or newer, and your platform's native build tools.
Install the tagged `0.1.0-beta.1` lexical build:

```bash
cargo install --locked --git https://github.com/cogitod/open-why \
  --tag v0.1.0-beta.1 --bin why --no-default-features
why --version
```

This uses the tag's committed lockfile. The signed tag identifies reviewed commit
`ffb32f6a9a9187df69ee812ea9a38a1014e4aea6`; use `--rev` with that full commit
in place of `--tag` to pin the source identity directly. See
[release instructions](RELEASE.md) for archive verification and upgrades.
Prebuilt lexical archives are available in the
[beta release](https://github.com/cogitod/open-why/releases/tag/v0.1.0-beta.1) for
Apple Silicon and Ubuntu 24.04 x86-64 (**glibc 2.39 required**). Verify their
checksums and provenance using the release instructions before extraction.

To remove the executable, run `cargo uninstall open-why`; your data and model
cache remain on disk.

CI exercises Linux and macOS, including the pinned local-embedding evaluation.
Passing CI does not establish the stronger stable-release guarantees. See
[readiness](OSS_READINESS.md) and [stability expectations](STABILITY.md).

Ensure Cargo's binary directory (normally `~/.cargo/bin`) is on your `PATH`.
The recommended lexical install uses `--no-default-features`, so it does not
build or download ONNX Runtime or tokenizers. Remote embeddings remain available
when explicitly configured. Cargo's default `local-embeddings` feature preserves
existing builds: omit that flag to include on-device inference, then run
`why fetch-model`. Model inputs are pinned and digest-verified; see the
[public evaluation](docs/public-evaluation.md). Local model settings on a lexical
build fail explicitly. A cached model is not loaded by that build.

See [backup and recovery](docs/maintenance.md) before using a store for important
records. A beta is not the stronger stability guarantee described in STABILITY.md.

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

![Three principles: preserve recorded rationale, link retrieved records to evidence, and say unknown when the reason was never recorded.](docs/assets/open-why-principles.png)

Indexing reads commit messages and recognized decision Markdown files from Git,
not uncommitted changes or all conversations. Automatic indexing happens only
when the scope contains no records. After new commits—or if you captured a
record before the first ask—call `open-why_index` with the same absolute
repository path. Retrieval does not automatically refresh a populated scope.
Remote Git URLs use separate caches keyed by the full URL and verify the cached
origin before refresh. Different owners or hosts with the same repository name
remain separate. Older basename caches are not reused; reindex remote URLs after
upgrading to this candidate. Existing records in old scopes remain unchanged.
A failed remote refresh returns an error; local-path indexing remains available
offline.

### Try an isolated example

With `why` installed, clone the repository to get the walkthrough:

```bash
git clone --branch v0.1.0-beta.1 --depth 1 https://github.com/cogitod/open-why.git
cd open-why
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

Add the Git dependency to your application's `Cargo.toml`. Use a `rev` when you
need to pin an integration to a reviewed commit; commit your application's
lockfile. The example also uses `anyhow` for error handling.

```toml
[dependencies]
open-why = { git = "https://github.com/cogitod/open-why" }
anyhow = "1"
```

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

See [library inspection, backup, and scoped evidence](docs/integrations.md#library-store-operations)
for integration details and [exact read contracts](docs/integrations.md#exact-read-contracts)
for MCP read semantics.

## Third-party integrations

Developer tools integrate through the MCP stdio server or the Rust library.
`open-why.integration/v1` provides a machine-readable compatibility manifest and
an executable conformance check without loading third-party code into the server.
See [the integration standard](docs/integrations.md) and its
[runnable host adapter](docs/integrations.md#embed-in-a-host-or-action-recorder).
External rationale does not need a Git repository or a private service.

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

## Contributing

Start with a reproducible bug report, a clearer example, or a focused fix. The
[contributor guide](CONTRIBUTING.md) covers local setup, the source map, checks,
and the required PR workflow. All changes to `main` go through a pull request
with passing CI and resolved conversations; merges are squashed with the PR title
and number. This is a single-maintainer project, so independent approval is not
currently required.

## Project information

- [Design and behavior](docs/design.md)
- [End-user stability contract](STABILITY.md)
- [Retrieval parity harness](docs/retrieval-parity.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Apache-2.0 license](LICENSE)
