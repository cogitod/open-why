# open-why: decision memory for AI agents

**An open-source MCP server, Rust library, and CLI for understanding why decisions were made.**

open-why retrieves recorded rationale from Git history, architecture decision
records (ADRs), and imported documents or events. Results carry source evidence
and decision history so coding agents and teammates can inspect the reason. If
no supporting rationale was recorded, the answer should be **unknown**.

![open-why — decision memory for coding agents, with evidence you can inspect. An archival decision card links to its source documents.](docs/assets/open-why-hero.png)

[![CI](https://github.com/cogitod/open-why/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/cogitod/open-why/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/github/license/cogitod/open-why)](LICENSE)

[Install](#install-from-source) · [Connect an agent](#set-up-your-coding-agent) ·
[Try the demo](#try-an-isolated-example) · [Integrate](#build-on-open-why) ·
[Contribute](CONTRIBUTING.md)

An open-source project by [Cogito](https://cogito.cv). Apache-2.0.

## What it helps you answer

- **Agent continuity:** Why did a previous agent choose this approach?
- **Developer onboarding:** Why does this repository use SQLite or separate a module?
- **Decision history:** What replaced an earlier decision, and what evidence was recorded?

Records live in local SQLite. Search works without a model or API key; optional
embeddings add semantic retrieval. Imports can preserve explicit rationale from
other tools without a Git repository. open-why stores recorded explanations; it
does not reconstruct unrecorded reasoning or establish that a decision was correct.

**Status: public beta, `0.1.0-beta.1`.** Installation, store isolation, retrieval,
and backup/restore have automated verification. An actual Codex CLI session has
passed a bounded evidence-read check. Independent human first use and broader
client coverage remain open; see [readiness and limitations](docs/readiness.md).

Security hardening added after this tag is described in [SECURITY.md](SECURITY.md).
A follow-up release is pending. On `main`, recognizable credentials are rejected
before ingestion or embedding; ordinary private rationale remains local unless
a client or remote embedding provider is explicitly connected. See the
[data policy and limits](SECURITY.md#data-ingestion-policy-after-beta1).

## Install from source

You need Git, Rust 1.88 or newer, and your platform's native build tools:

```bash
cargo install --locked --git https://github.com/cogitod/open-why \
  --tag v0.1.0-beta.1 --bin why --no-default-features
why --version
```

Add Cargo's binary directory (normally `~/.cargo/bin`) to `PATH`. This installs
the versioned lexical build without ONNX Runtime or model downloads.

[Prebuilt binaries](https://github.com/cogitod/open-why/releases/tag/v0.1.0-beta.1)
are available for Apple Silicon and Ubuntu 24.04 x86-64 (**glibc 2.39 required**).
Follow [release verification and upgrades](RELEASE.md) before using an archive.
Linux and macOS are tested; Windows is unsupported.

For local embeddings, omit `--no-default-features` and run `why fetch-model`.
See [configuration](docs/reference.md#configuration) for model selection and
[build requirements](CONTRIBUTING.md#local-setup) for native runtime limitations.
A connected AI client receives the records it requests; configured remote
embedding providers also receive text. Lexical retrieval itself needs no service.

## Set up your coding agent

Generate a configuration for one database and reuse it across your clients:

```bash
why setup --db "$HOME/.cache/open-why/open-why.db" --client codex
```

Merge the printed TOML into `~/.codex/config.toml`. For Claude Code, use
`--client claude-code` and merge the printed JSON into the project's `.mcp.json`.
Other MCP clients can use `--client generic` for command/args/env JSON.

Setup prints configuration without changing client settings. It creates a new
store identity or reads the existing one. Keep both `OPEN_WHY_DB` and
`OPEN_WHY_STORE_INSTANCE_ID`, use concrete paths without symlink components,
and replace an existing server entry instead of adding a duplicate.
Never assign a new identity to an existing database.

Reconnect the client and check that open-why's tools appear. Generated settings
alone do not prove connectivity; [tested client scope and tool approval](docs/integrations.md#beta-client-verification-scope)
describe what has actually been exercised.

## Get your first answer with evidence

Ask your connected agent, using an absolute Git working-tree path:

```text
Use open-why_ask to answer: why does this repository use SQLite?
Repository: /absolute/path/to/repository
Then call open-why_get with a returned ID and the same scope. Cite the recorded
reason, record ID, and available source or commit evidence. If the records do
not establish the reason, say unknown. Do not edit the repository.
```

`open-why_ask` returns scoped previews; `open-why_get` returns the complete current
record and its evidence. Indexing reads committed history and recognized decision
Markdown, not all conversations or uncommitted files. An empty scope is indexed
on first ask. After new commits, call `open-why_index` explicitly to refresh it.

![Three principles: preserve recorded rationale, link retrieved records to evidence, and say unknown when the reason was never recorded.](docs/assets/open-why-principles.png)

## Try an isolated example

With `why` installed:

```bash
git clone --branch v0.1.0-beta.1 --depth 1 https://github.com/cogitod/open-why.git
cd open-why
bash examples/quickstart.sh "$PWD/../open-why-demo" codex
```

The destination must be new and its parent must exist. The script creates a
synthetic repository, separate store, client snippet, and prompts for evidence
retrieval, missing evidence, and supersession. It leaves normal stores and client
settings untouched. Use `claude-code` or `generic` for another snippet format.
After testing, disconnect the demo, restore your usual client configuration,
and remove only the demo directory you created.

## Build on open-why

| Your application | Integration route |
| --- | --- |
| Rust host | Embed the library and call its public typed APIs. |
| Agent, IDE, Python, or TypeScript backend | Launch the local MCP stdio server with an MCP client. |
| Research tool or event recorder | Import explicit decisions with stable IDs, scope, author, date, and source references. |

Start with the [integration guide](docs/integrations.md), its pinned Cargo example,
and the [runnable host adapter](examples/embedded_adapter.rs). The
`open-why.integration/v1` manifest and conformance checker describe and check
compatibility. Integrations use explicit scope and store identity; SQLite tables
are private implementation details.

There is no dynamic plugin loader or built-in HTTP service. A remote application
supplies its own backend and authentication. Scope is a retrieval boundary, not
multi-user authorization. Task orchestration and raw activity logs stay in the host.

## Maintain your store

```bash
why doctor --db "$HOME/.cache/open-why/open-why.db" --repo /absolute/path/to/repository
```

Doctor is read-only and exits nonzero when a check fails or cannot be verified.
Follow [backup, restore, and troubleshooting](docs/maintenance.md) before keeping
important records or upgrading. Backup and restore require new destination paths.
To uninstall the executable, run `cargo uninstall open-why`; data and models remain.

## Documentation and contributions

| Need | Guide |
| --- | --- |
| Commands, MCP tools, environment settings | [Reference](docs/reference.md) |
| Embedding open-why or connecting another stack | [Integration contracts](docs/integrations.md) |
| Data maintenance and diagnostics | [Backup and recovery](docs/maintenance.md) |
| How records, evidence, and ranking work | [Design](docs/design.md) |
| Reproduce retrieval checks | [Public evaluation](docs/public-evaluation.md) |
| Release identity, compatibility, and upgrades | [Releases](RELEASE.md) · [Stability policy](STABILITY.md) |
| What to work on next | [Roadmap and contributor tasks](docs/roadmap.md) |

Report a reproducible bug or propose a focused improvement. The
[contributor guide](CONTRIBUTING.md) covers setup and required checks. Changes go
through PRs with passing CI; this single-maintainer project does not claim
independent human review. Report vulnerabilities through the
[private security channel](SECURITY.md).
