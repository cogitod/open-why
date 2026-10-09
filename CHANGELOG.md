# Changelog

Every release entry records user-visible changes, upgrade implications and known
limits. Unreleased entries are not evidence that artifacts are publicly available.

## 0.1.0-beta.1 — unreleased candidate

- Fix daemon identity bypass and database endpoint collisions. New endpoints
  append `.sock` to the whole filename. Stop old daemons before upgrading;
  stale sockets now fail closed. Relative paths remain supported, while parent
  traversal, symlink aliases and hard-linked MCP stores are rejected.
- Add backup, verify-backup and restore-to-new-path commands. They preserve store
  identity and reject missing, incompatible or corrupt sources and overwrites.
- Keep local embeddings enabled in default Cargo builds; offer
  `--no-default-features` to omit ONNX/tokenizers for lexical installs.
- Pin model inputs to an immutable revision and verify SHA-256 before use.
  Nonmatching older/custom model files now fail explicitly.
- Add a synthetic public evaluation and explicit real-inference test command.
- Add macOS CI and candidate artifact validation/provenance workflow; these must
  execute remotely before claiming continuous support or published provenance.
- Keep schema family/version and named MCP contracts unchanged. Rust MSRV 1.88.

Known limits: the default ONNX build fails against Debian 12 ARM64’s older C++
runtime; the recommended lexical build avoids it. No stable API guarantee, no Windows/Android support, no automatic
contradiction resolution, no claim of broad retrieval accuracy or real-client
acceptance from the protocol test harness. Public artifact download and independent
user acceptance remain unverified until a release is authorized and tested.
