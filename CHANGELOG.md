# Changelog

Every release entry records user-visible changes, upgrade implications and known
limits. Unreleased entries are not evidence that artifacts are publicly available.

## 0.1.0-beta.2 — 2026-10-09

- Reject recognizable credentials at library, CLI, and MCP ingestion boundaries
  before writes or embedding calls. Entire batches fail atomically; records are
  not silently redacted. This does not classify arbitrary confidential prose or
  scrub existing databases, backups, or Git caches.
- Remote URL indexing now requires `OPEN_WHY_ALLOW_REMOTE_CLONE=1`. Prefer local
  repositories: a full Git checkout can retain secrets outside the rationale store.
- Bound MCP frames and request IDs, redact leak-scanner findings, protect Git
  caches with owner-only permissions, and reject credential-bearing clone URLs.
  Required CI now includes checksum-pinned Gitleaks history scanning.

- Consolidate onboarding and reference documentation, with a single prioritized
  contributor roadmap.

Upgrade: back up and verify the store, stop existing daemons, install beta.2 and
reconnect clients. Store schema, named contracts and Rust MSRV 1.88 are unchanged.
Previously accepted credential-shaped inputs now fail explicitly. Old stores,
backups and clones are not scrubbed; inspect and rotate exposed credentials
separately. Confidential prose and unrecognized secrets can still be stored.

## 0.1.0-beta.1 — 2026-10-09

- Follow-up review: doctor now checks all pinned model inputs/digests and refuses
  auto-fetch on lexical builds. Remote reindexing advances its managed checkout
  to fetched HEAD before reading decision files and refuses conflicting edits.
- Integration manifests accept semantic prerelease/build versions; the library
  example declares the beta minimum and demonstrates external event rationale
  through public APIs, including replay, supersession and scope refusal.

- Fix daemon identity bypass and database endpoint collisions. New endpoints
  append `.sock` to the whole filename. Stop old daemons before upgrading;
  stale sockets now fail closed. Relative paths remain supported, while parent
  traversal, symlink aliases and hard-linked MCP stores are rejected.
- Isolate remote Git caches by full URL identity and validate cached origin.
  Old basename caches are retained but no longer reused; reindex a URL into its
  new scope. Failed refresh now returns an error instead of silently using stale evidence.
- Add backup, verify-backup and restore-to-new-path commands. They preserve store
  identity and reject missing, incompatible or corrupt sources and overwrites.
- Keep local embeddings enabled in default Cargo builds; offer
  `--no-default-features` to omit ONNX/tokenizers for lexical installs.
- Pin model inputs to an immutable revision and verify SHA-256 before use.
  Nonmatching older/custom model files now fail explicitly.
- Add a synthetic public evaluation and explicit real-inference test command.
- Add macOS CI and hosted artifact validation/provenance. Published lexical
  archives cover Apple Silicon and Ubuntu 24.04 x86-64 (glibc 2.39 required).
- Keep schema family/version and named MCP contracts unchanged. Rust MSRV 1.88.

Known limits: the default ONNX build fails against Debian 12 ARM64’s older C++
runtime; the recommended lexical build avoids it. No stable API guarantee, no Windows/Android support, no automatic
contradiction resolution or broad retrieval-accuracy claim. Codex CLI 0.162.0 on
macOS ARM64 passed a bounded evidence-read check; independent human first use,
Claude Code, client-driven supersession and reconnect remain unverified.
