# open-why

[![CI](https://github.com/cogitod/open-why/actions/workflows/ci.yml/badge.svg)](https://github.com/cogitod/open-why/actions/workflows/ci.yml)
[![license](https://img.shields.io/github/license/cogitod/open-why?labelColor=333333&color=666666)](LICENSE)

**the "why" layer for AI agents.**

`git blame` tells you who changed a line and when. `open-why` tells you *why* —
the commit, the decision, the author, the evidence — as a cited answer.

- **evidence-bound** — every answer carries its source: the commit, the record,
  the author, the date. Nothing returned without proof.
- **superseded, never deleted** — a newer decision retires the old one; history
  stays reachable, `--historical` walks the chain.
- **hybrid recall** — local on-device embeddings + BM25 lexical search, fused
  and decayed by recency, so the right answer stays findable.
- **local and yours** — one SQLite file, no service, no cloud, no account.
- **one small binary** — a lib + bin Rust crate. It does one thing and doesn't grow.

> Mem0 / Graphiti remember **what**. open-why remembers **why**.

---

## quick start

```bash
git clone https://github.com/cogitod/open-why
cd open-why
cargo build --release
```

then, from any git repo:

```bash
why "why is the sandbox separate?"    # ask — no subcommand needed
why init                               # index this repo explicitly
why capture --title "Use SQLite" --content "..." --kind decision
why search "sqlite" --types decision,fact
why get <id>
why feedback <id> --helpful            # teach it what was useful
why serve                              # MCP stdio
```

every answer is evidence-bound:

```
- Use SQLite for the local-first record
  2025-03-11 · developer · commit 8f2c41a
  zero-config, single file, survives a laptop
```

`why fetch-model` downloads a local embedder (`Xenova/all-MiniLM-L6-v2`) for
zero-config semantic recall; without it, search is lexical-first. See
[configuration](#configuration) to point at a remote embedder instead.

## use as a library

```rust
use open_why::Store;

let store = Store::open_default()?;
let hits = store.search("why sqlite", &["my-project"], &[], 10)?;
for h in hits {
    println!("{} — {}", h.subject, h.date);
}
```

## use as an MCP server

`why serve` speaks stdio MCP: `open-why_ask`, `open-why_index`,
`open-why_capture`, `open-why_search`, `open-why_get`, `open-why_import`,
`open-why_link`, `open-why_feedback`.

```jsonc
// opencode: ~/.config/opencode/opencode.jsonc
"open-why": { "type": "local", "command": ["/path/to/open-why/target/release/why", "serve"], "enabled": true }
```

```json
// Claude Code / Codex: .mcp.json
{ "mcpServers": { "open-why": { "command": "/path/to/open-why/target/release/why", "args": ["serve"] } } }
```

## configuration

| Env | Effect |
| --- | --- |
| `OPEN_WHY_EMBED_MODEL_PATH=/path/to/all-MiniLM-L6-v2` | local on-device embedder |
| `OPEN_WHY_AUTO_FETCH=1` | download the model on first use if missing |
| `OPEN_WHY_EMBED_URL` (+ `OPEN_WHY_EMBED_MODEL`, `OPEN_WHY_EMBED_API_KEY`) | OpenAI-compatible remote embedder |
| *(none of the above)* | local model from cache if fetched, else lexical-first |
| `ORT_LIB_LOCATION=/path/to/onnxruntime` | build offline, against a pre-installed ONNX runtime |

## docs

[what it does & why](docs/design.md) · [retrieval-parity harness](docs/retrieval-parity.md) · [contributing](CONTRIBUTING.md)

## license

[Apache-2.0](LICENSE).
