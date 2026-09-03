# Security Policy

## Supported versions

open-why is pre-1.0. Only `main` / the latest tagged release is supported — there
are no maintained older branches.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for a security report. Instead, use
GitHub's private vulnerability reporting:

**[github.com/cogitod/open-why/security/advisories/new](https://github.com/cogitod/open-why/security/advisories/new)**

Include what you found, how to reproduce it, and its impact if you can. This is a
single-maintainer project — expect an initial response within a few days, not
hours.

## What's in scope

open-why is a local-first tool: one SQLite file on your machine, an optional MCP
stdio server, and a CLI. Things worth reporting:

- File permission issues on the SQLite store or cache directory (`~/.cache/open-why`).
- `why serve` (the MCP server) trusting or mishandling its stdio input in an unsafe
  way — it's a local process talking to a local caller, but a parsing bug is still
  a bug.
- Supply-chain concerns in the embedding model / onnxruntime fetch path
  (`why fetch-model`, the `ort` crate's `download-binaries`).
- `why import` accepting external JSON — a payload that causes something worse than
  a bad row in the store (e.g. path traversal, resource exhaustion).

Ranking-quality issues (bad search results) and normal bugs are **not** security
reports — file those as a regular issue.
