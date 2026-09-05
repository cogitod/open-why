# Integration standard

`open-why.integration/v1` gives third-party developer tools one vendor-neutral
way to declare how they consume open-why. It is a compatibility profile, not an
in-process plugin ABI.

Third-party code does not run inside `why`. Agent products and IDEs use the MCP
stdio server. Rust hosts use the crate. External systems that produce rationale
call the versioned import contract through one of those two interfaces.

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
Examples cover [MCP stdio](../examples/integrations/mcp-stdio.json) and the
[Rust library](../examples/integrations/rust-library.json).

Validate a declaration without executing it:

```bash
cargo run --bin why-integration-check -- examples/integrations/mcp-stdio.json
```

For MCP integrations, build `why` and probe the declared command. The probe uses
an isolated temporary database, performs initialization and tool discovery, and
checks the declared protocol version, contracts, and capabilities:

```bash
cargo build --bin why
cargo run --bin why-integration-check -- \
  examples/integrations/mcp-stdio.json --probe
```

Manifest files are untrusted input. Probing launches the declared command and
must be an explicit human or CI action; open-why never discovers or executes
manifests automatically.

## Compatibility

Additive capabilities and new versioned contracts do not invalidate an existing
v1 manifest. Removing a capability, changing scope or store-identity semantics,
or changing a named contract requires a new contract version. The MCP protocol
revision is explicit because protocol negotiation and open-why application
contracts evolve independently.
