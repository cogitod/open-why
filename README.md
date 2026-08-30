# open-why

**The thin "why" layer for AI agents.**

`git blame` tells you *who* changed a line and *when*. `open-why` tells you *why* —
the commit, the ADR, the author, the rationale — as a cited answer.

open-why is a small, self-contained Rust crate: one local SQLite store, a retrieval
engine (local embeddings + BM25 + recency), and an MCP server. It does one thing,
and it does not grow.

## The idea

Agents are good at *doing*. They are bad at *remembering why*.

Every time an agent (or a team) makes a consequential choice, the reasoning behind
it lives in a PR comment, a chat thread, or someone's head — and evaporates. Six
months later you are staring at a line of code asking "why is this here?" and the
only honest answer left is `git blame`, which tells you *who* and *when*, never
*why*.

open-why exists to make "why" a first-class, machine-retrievable thing. It captures
the decision, binds it to the commits that realized it, and recalls it on demand —
with the evidence attached, so the answer can be checked rather than trusted.

## The thought behind the design

A few convictions shape every part of open-why:

- **Narrow, not general.** open-why is not a memory platform. It does decision
  recall. Doing one thing well is the feature.
- **Evidence-bound, not a guess.** An answer without proof is an opinion. open-why
  returns the source — the commit, the record, the author, the date — alongside
  every result.
- **Superseded, never deleted.** Decisions are point-in-time. A newer decision on
  the same question retires the older one; the history stays, the *current* answer
  is always what you get. The past is never lost, and never mistaken for the
  present.
- **Local and customer-owned.** One SQLite file on your machine. No service, no
  cloud, no account. Your reasons are yours.
- **Calibrated, not tuned.** The ranking is ported from a production retrieval
  engine and kept to its measured constants. It is reproduced, not improvised.

## What it does

Four primitives, each small and composable:

- **Decision linkage** — a commit's `mem-ref:` trailers bind it to the decision it
  realizes. Ask which decisions a commit realizes, or which commits realized a
  decision.
- **Temporal identity** — `superseded_by`, `valid_from`, `valid_until`. Search
  hydrates the *current* version; history stays reachable.
- **Hybrid recall** — reciprocal-rank fusion of a **semantic arm** (local on-device
  `all-MiniLM-L6-v2` embeddings) and a **lexical arm** (FTS5-style BM25 with
  title/content column weights), weighted by importance and effectiveness and
  decayed by Ebbinghaus recency — with spaced-repetition stability, so a memory
  that keeps being the right answer stays findable.
- **Capture provenance** — `content_digest` + `source_identity` make capture
  idempotent and de-duplicated by content.

Records carry a kind — `decision`, `fact`, `reference`, `pattern`, `doc`,
`project`, `observation` — so recall can be scoped by type as well as by query.

## Quick start

```bash
# ask "why" — one word, no prefix. Bare `why "..."` asks; no subcommand needed.
why "why is the sandbox separate?" --repo https://github.com/anomalyco/opencode

# ask a question of the current repo
why "why do we use SQLite instead of Postgres?"

# index a repo explicitly
why init

# the rest of the surface
why capture --title "Use SQLite" --content "..." --kind decision
why search "sqlite" --types decision,fact
why get <id>
why link <commit> <decision>
why import --file decisions.json
why serve          # MCP stdio
```

`why` is the only binary. The full verb set is `ask` (bare), `init`, `capture`,
`search`, `get`, `link`, `import`, `fetch-model`, and `serve`.

Every answer is evidence-bound:

```
- Use SQLite for the local-first record
  2025-03-11 · adrian · commit 8f2c41a
  zero-config, single file, survives a laptop
```

## Use as a library

`open-why` is a lib + bin crate. Embed the store directly:

```rust
use open_why::Store;

let store = Store::open_default()?;                 // wires an embedder from the env
let hits = store.search("why sqlite", &["my-project"], &[], 10)?;
for h in hits {
    println!("{} — {}", h.subject, h.date);
}
```

Semantic recall is on when an embedder is configured; off, search is lexical-first.

No embedder configured and want zero-config local embeddings? Fetch the model once,
then open-why finds it automatically:

```bash
why fetch-model    # downloads Xenova/all-MiniLM-L6-v2 into ~/.cache/open-why/models
```

| Env | Effect |
| --- | --- |
| `OPEN_WHY_EMBED_MODEL_PATH=/path/to/all-MiniLM-L6-v2` | local on-device embedder |
| `OPEN_WHY_AUTO_FETCH=1` | download the model on first use if missing |
| `OPEN_WHY_EMBED_URL` (+ `OPEN_WHY_EMBED_MODEL`, `OPEN_WHY_EMBED_API_KEY`) | OpenAI-compatible remote |
| *(none of the above)* | local model from the cache if fetched, else lexical-first |

The public surface is `Store`, `Decision`, `Record`, `ExternalDecision`, and the
`Embedder` trait (`LocalEmbedder`, `HttpEmbedder`).

## Use as an MCP server

`why serve` speaks stdio MCP and exposes `open-why_ask`, `open-why_index`,
`open-why_capture`, `open-why_search`, `open-why_get`, `open-why_import`, and
`open-why_link`.

opencode (`~/.config/opencode/opencode.jsonc`):

```jsonc
"open-why": {
  "type": "local",
  "command": ["/path/to/open-why/target/release/why", "serve"],
  "enabled": true
}
```

Claude Code / Codex (`.mcp.json` / `claude mcp add`):

```json
{ "mcpServers": { "open-why": { "command": "/path/to/open-why/target/release/why", "args": ["serve"] } } }
```

## Build

```bash
cargo build --release
```

Requires Rust 1.88+. The first build downloads the onnxruntime runtime for your
platform (via `ort`'s `download-binaries`). The embedding model is fetched with
`why fetch-model` (or loaded from `OPEN_WHY_EMBED_MODEL_PATH` at runtime), not
vendored.

## License

Apache-2.0.
