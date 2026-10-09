# Integration standard

`open-why.integration/v1` gives third-party developer tools one vendor-neutral
way to declare how they consume open-why. It is a compatibility profile, not an
in-process plugin ABI.

Third-party code does not run inside `why`. Agent products and IDEs use the MCP
stdio server. Rust hosts use the crate. External systems that produce rationale
call the versioned import contract through one of those two interfaces.

## Embed in a host or action recorder

No Git repository or vendor account is required for externally recorded rationale.
An adapter can keep its raw action log in its own system and import only explicit
human/agent decisions, facts or observations with stable IDs, source references,
author, date and scope. An action alone does not establish why it happened;
missing rationale must stay missing. Use explicit supersession for recorded
corrections, and let open-why resolve current evidence and history.

A runnable library example exercises import, exact replay, supersession, scoped
current evidence, missing evidence, lexical retrieval and verified backup:

```bash
cargo run --locked --no-default-features --example embedded_adapter -- /absolute/new-adapter-demo
cargo test --locked --no-default-features --test embedded_adapter
```

The [example](../examples/embedded_adapter.rs) refuses an existing demo directory
and needs no model, credentials or private service. Its regression runs in normal
CI with both Cargo feature sets. This is a generic integration pattern, not a
claim that any particular third-party product has integrated or been certified.
Choose MCP for a process boundary or the library for a Rust host; neither requires
running vendor code inside open-why or accessing its SQLite tables directly.

## Required invariants

- Every operation supplies an explicit repository or scope.
- Every installation mints and persists its own bounded store identity through
  `OPEN_WHY_STORE_INSTANCE_ID`.
- Integrations depend on versioned open-why contracts, not SQLite tables or
  internal Rust modules.
- Stable record IDs and sealed evidence are preserved during import.
- A manifest declares only capabilities the integration actually uses.
- Vendor task, session, orchestration, and messaging concepts stay outside the
  open-why data model.

## Manifest

The canonical schema is
[`spec/open-why.integration-v1.schema.json`](../spec/open-why.integration-v1.schema.json).
Version fields accept semantic versions, including `0.1.0-beta.1`.
The manifest validates declarations; a minimum-version string does not itself
pin Cargo resolution or prove downstream compatibility. Pin the reviewed Git
revision and run the host's own tests.

Examples cover [MCP stdio](../examples/integrations/mcp-stdio.json) and the
[Rust library](../examples/integrations/rust-library.json).

Validate a declaration without executing it:

```bash
cargo run --locked --bin why-integration-check -- examples/integrations/mcp-stdio.json
```

For MCP integrations, build `why` and probe the declared command. The probe uses
an isolated temporary database, performs initialization and tool discovery, and
checks the declared protocol version, contracts, and capabilities:

```bash
cargo build --locked --bin why
cargo run --locked --bin why-integration-check -- \
  examples/integrations/mcp-stdio.json --probe
```

Manifest files are untrusted input. Probing launches the declared command and
must be an explicit human or CI action; open-why never discovers or executes
manifests automatically.

### Conformance output

`why-integration-check` writes one `open-why.integration-conformance/v1` JSON
object to standard output. A successful validation (and probe, when requested)
exits 0:

```json
{"contract":"open-why.integration-conformance/v1","status":"ok","integration_id":"dev.example.coding-agent","integration_version":"1.0.0","mode":"mcp-stdio","probed":false}
```

Validation and probe errors exit nonzero and use the same envelope:

```json
{"contract":"open-why.integration-conformance/v1","status":"error","message":"parse manifest: expected value at line 1 column 1"}
```

Consumers may display `message` for diagnostics, but must not parse or depend on
its human-readable text; that text is not a stable interface.

## Compatibility

Additive capabilities and new versioned contracts do not invalidate an existing
v1 manifest. Removing a capability, changing scope or store-identity semantics,
or changing a named contract requires a new contract version. The MCP protocol
revision is explicit because protocol negotiation and open-why application
contracts evolve independently.

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

## Library store operations

Library hosts can inspect a database before opening it:

```rust
use open_why::{inspect_store, Store, StoreCompatibility};
use std::path::Path;

fn main() -> anyhow::Result<()> {
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
    Ok(())
}
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


## Beta client verification scope

The repository's process harness runs MCP protocol `2024-11-05` against the real
CLI, including direct stdio and shared Unix-daemon paths. Onboarding tests parse
the generated Codex TOML, Claude Code JSON and generic command/args/env formats
and launch those commands. They do not launch the actual vendor clients.

No particular Codex or Claude Code application version is certified by this beta
work. Independent acceptance must record the actual client version, platform,
release checksum, setup, first evidence read, missing evidence, supersession and
reconnection results before adding it to a tested-client list. A configuration
snippet is not evidence of end-to-end client interoperability.

Only local stdio and the local Unix daemon are provided; there is no HTTP/SSE
server, multi-user authentication service or Windows transport. Scopes are
explicit retrieval boundaries, not an authorization system for untrusted OS
users. Keep a store and its socket private to the account running the client.
