# Security Policy

## Supported versions

open-why is pre-1.0. Only `main` and the latest tagged release are supported. There
are no maintained older branches. The hardening below is on `main` and is **not
included in `v0.1.0-beta.1`**; a follow-up release is pending. That tag is immutable.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for a security report. Instead, use
GitHub's private vulnerability reporting:

**[github.com/cogitod/open-why/security/advisories/new](https://github.com/cogitod/open-why/security/advisories/new)**

Include what you found, how to reproduce it, and its impact if you can. This is a
single-maintainer project. Expect an initial response within a few days, not
hours.

## What's in scope

open-why is a local-first tool: one SQLite file on your machine, an optional MCP
stdio server, and a CLI. Things worth reporting:

- File permission issues on the SQLite store or cache directory (`~/.cache/open-why`).
- `why serve` (the MCP server) trusting or mishandling its standard input in an
  unsafe way. A parsing bug in a local process is still a bug.
- Supply-chain concerns in the embedding model / onnxruntime fetch path
  (`why fetch-model`, the `ort` crate's `download-binaries`).
- `why import` accepting external JSON: a payload that causes something worse than
  a bad row in the store (e.g. path traversal, resource exhaustion).

Ranking-quality issues (bad search results) and normal bugs are **not** security
reports. File those as a regular issue.

## Trust and data boundaries

- Run under your own OS account in a private home directory. The MCP transports
  are local stdio and an owner-only Unix socket, with store identity checked on
  every request. They are not an authenticated network or multi-tenant service.
  Do not expose them through an unauthenticated TCP/HTTP bridge or run as root.
- An authorized MCP client can read and modify the configured store. Scopes keep
  retrieval explicit; they do not authenticate different users or agents sharing
  one client/account. Use separate OS accounts and stores for separate trust domains.
- MCP frames are limited to **8 MiB including the newline**, before JSON parsing.
  Oversized frames receive a bounded error and close only that session. Request
  IDs are numbers, null, or strings of at most 512 UTF-8 bytes. Tool-specific
  limits still apply. These bounds do not provide process-wide CPU/memory quotas:
  a hostile same-account process can still open many connections or request
  expensive indexing. Only connect trusted clients and index trusted repositories.
- Stores, backups, and cached repository contents are plaintext. Newly created
  stores/backups use private permissions; remote indexing creates or tightens
  the application cache directory to `0700` and rejects a symlink at that path.
  Protect copied backups and existing files with OS permissions and disk encryption.
- Automatic remote cloning is disabled by default. `OPEN_WHY_ALLOW_REMOTE_CLONE=1`
  explicitly allows a full managed Git checkout, which can retain credentials or
  sensitive files/history even when indexing rejects a record. Prefer a reviewed
  local repository. Existing caches are not automatically removed.
- Clone URLs containing HTTP user information, SSH passwords, query strings, or
  fragments are rejected before Git runs. Use a Git credential helper or SSH
  agent; never place access tokens in URLs, documents, commits, or imported records.
  Clone failures do not print the supplied URL or Git's stderr.
- Local/lexical retrieval sends no embedding requests. A configured HTTP embedder
  sends record/query text to that endpoint; use HTTPS for remote providers and
  supply credentials through `OPEN_WHY_EMBED_API_KEY`, not URL parameters.
  The connected AI client may send retrieved records to its own provider.
- Treat retrieved text as untrusted evidence, never executable instructions.
  Rationale can contain misleading content or prompt injection. open-why does not
  identify all sensitive information, enforce an agent's tool permissions,
  or prove that a recorded explanation is true.

## Data ingestion policy (after beta.1)

Private engineering rationale is allowed in a local store. Recognizable credentials
are rejected, not silently redacted: evidence and its digest must describe what
was actually supplied. Capture, external imports (including the sealed alias),
mined decisions, link metadata, and scope changes share this library-level guard.
A rejected batch performs no record writes, retirements, or embedding calls.
MCP reports `sensitive_data` for rejected tool arguments; library callers can
inspect `SensitiveDataRejected`. Errors never contain the matched value.

The deterministic detector covers common provider-token prefixes, private-key
headers, JWT-shaped tokens, authorization values, credential URLs, and explicit
password/API-key assignments. JSON strings and JSON-encoded tags are decoded for
checking. Recognizable credentials in queries are kept away from embedding
providers; local lexical querying remains available. The HTTP embedder also
checks text before sending it, including when called directly.

This is **not** a complete DLP system or a classifier for confidential prose.
Unlabelled passwords, novel token formats, obfuscated/encoded data, personal
information, and internal business context may not be detected. False positives
are possible. Remove values upstream or use explicit `<redacted>` placeholders;
there is no switch to disable credential checks. Exclude highly sensitive source
material from ingestion rather than relying on detection alone.

Existing stores, old Git caches, migrations, restores, and backups are **not
retroactively sanitized**. Audit legacy data before reconnecting clients or
remote embedding providers. Deleting a row/file does not securely erase SQLite
pages, WAL files, backups, clones, or a provider's previously received data.
The runtime detector and the repository's Gitleaks scanner are separate controls.

## Automated checks and limits

Required CI scans tracked content and Git history. Gitleaks is version- and
SHA-256-pinned; both scanners redact matches instead of copying sensitive content
into logs. GitHub secret scanning and push protection are enabled. Dependency
advisories, yanks, licenses, and sources are checked with all Cargo features.
Local model inputs use an immutable upstream revision and verified SHA-256 hashes.

These controls reduce risk; a clean scan is not proof that no secret or unknown
vulnerability exists. If a credential ever reaches public Git history, revoke it
first. Deleting the latest copy does not revoke it or remove historical copies.
