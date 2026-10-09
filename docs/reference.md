# CLI, MCP tools, and configuration

Install and connect a client with the [quickstart](../README.md). This page lists
the supported commands, MCP tools, and environment settings. For errors and
recovery, see [maintenance](maintenance.md#check-and-recover-your-setup).

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

## CLI commands

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
why fetch-model                                            # requires local-embeddings build
why serve                                                  # MCP over standard input/output
```

Run `why --help` or `why <command> --help` for all arguments.

## Configuration

| Variable | Effect |
| --- | --- |
| `OPEN_WHY_DB=/path/to/open-why.db` | Use a specific SQLite database. |
| `OPEN_WHY_STORE_INSTANCE_ID=my-host:primary` | Bind a new or migrating database to a provider-minted identity. |
| `OPEN_WHY_EMBED_MODEL_PATH=/path/to/all-MiniLM-L6-v2` | Use a local embedding model. |
| `OPEN_WHY_AUTO_FETCH=1` | Download the local model on first use if the cache is empty. |
| `OPEN_WHY_EMBED_URL=https://example.invalid/embeddings` | Use an OpenAI-compatible embedding endpoint. |
| `OPEN_WHY_EMBED_MODEL=model-name` | Choose the remote model; the default is `text-embedding-3-small`. |
| `OPEN_WHY_ALLOW_REMOTE_CLONE=1` | Opt into a full managed Git clone; default is off because Git history can contain secrets. |
| `OPEN_WHY_EMBED_API_KEY=...` | Send a bearer token to the remote embedding endpoint. |
| `OPEN_WHY_DEBUG_RANK=1` | Print ranking diagnostics to standard error. |
| `ORT_LIB_LOCATION=/path/to/onnxruntime` | Build against an installed ONNX Runtime. |

On Unix, a database path must not contain symbolic-link directory components or
name a symbolic-link file. Resolve trusted path aliases before the first launch,
then keep the same concrete path in the client configuration.

Source builds enable `local-embeddings` by default. In that build, an explicitly
configured model or a previously fetched default model enables local inference;
otherwise search remains lexical. `why fetch-model` stores the pinned,
digest-verified `Xenova/all-MiniLM-L6-v2` inputs under `~/.cache/open-why/models/`.

A `--no-default-features` build does not load cached models. Explicit local-model
settings, auto-fetch and `why fetch-model` fail visibly on that build; remote
embeddings remain available when configured. See [model evaluation](public-evaluation.md)
for download verification and backend-change limitations.

## Indexing and refresh

Indexing reads committed Git history and recognized decision Markdown. It runs
automatically only when the scope is empty; call `open-why_index` after new
commits or when records were captured before the first ask.

Use local repositories by default. Remote URL indexing requires explicit
`OPEN_WHY_ALLOW_REMOTE_CLONE=1`; it retains the full cloned Git data, not only
accepted rationale. Credential detection does not sanitize that checkout.
See [data ingestion policy](../SECURITY.md#data-ingestion-policy-after-beta1).

When explicitly enabled, remote Git URLs use separate caches keyed by the full URL and verify the cached
origin before refresh. Different owners or hosts with the same repository name
remain separate. Older basename caches are retained but no longer reused;
reindex remote URLs after upgrading from a pre-beta build. Existing records in
old scopes remain unchanged. A failed remote refresh returns an error;
local-path indexing remains available offline. MCP indexing uses local absolute
paths; remote URL indexing is a CLI operation.
