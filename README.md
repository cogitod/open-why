# open-why

**Ask any repository why a decision was made — with the evidence.**

`git blame` tells you *who* changed a line and *when*. `open-why` tells you *why* —
the commit, the ADR, the author, and the rationale — as a cited answer.

## Where open-why sits

The open-source AI-agent stack is filling in every layer except the memory:

| Layer | Project | Job |
| --- | --- | --- |
| Agent | [OpenHands](https://github.com/OpenHands/openhands) | do the work |
| Harness | [opencode](https://github.com/anomalyco/opencode) | run the agent |
| TUI | [opentui](https://github.com/anomalyco/opentui) | see the agent |
| Runtime | [herdr](https://herdr.dev) | where the agent lives |
| Connectors | [open-connector](https://github.com/oomol-lab/open-connector) | reach the apps |
| **Memory** | **open-why** | **remember why** ← the missing layer |

They all make agents *do*. None of them makes an agent (or you) *remember why*.

## Not another memory layer

Agent memory is the most crowded corner of the open stack — Mem0 (64k★),
Zep Graphiti (30k★), Letta (24k★), Hindsight (21k★) all remember **what**
happened. open-why is deliberately narrow: it remembers **why**, and cites the
evidence (the commit, the ADR, the decision record) so you can trust the answer.

> Mem0 and Graphiti remember *what*. open-why remembers *why* — with the proof.

## Quick start

```bash
# answer "why" on any repo — no setup needed
open-why why "why is the sandbox separate?" --repo https://github.com/anomalyco/opencode

# index the current repo explicitly
open-why init

# ask a question of the current repo
open-why why "why do we use SQLite instead of Postgres?"
```

## What you get

Every answer is evidence-bound, not a guess:

```
- Use SQLite for the local-first record
  2025-03-11 · adrian · commit 8f2c41a
  zero-config, single file, survives a laptop
```

## How it works

1. `init` walks git history + ADRs + design docs and extracts decisions into a
   local SQLite store (`~/.cache/open-why/open-why.db`).
2. `why` hybrid-ranks them (lexical + importance + recency decay) and cites the
   source.
3. `capture`, `search`, `get`, and `link` record and recall decisions beyond git.

On the roadmap: on-device embeddings (semantic ranking).

## Use as an MCP server

`open-why serve` speaks stdio MCP and exposes `open-why_ask`, `open-why_index`,
`open-why_capture`, `open-why_search`, `open-why_get`, and `open-why_link`.

opencode (`~/.config/opencode/opencode.jsonc`):

```jsonc
"open-why": {
  "type": "local",
  "command": ["/path/to/open-why/target/release/open-why", "serve"],
  "enabled": true
}
```

Claude Code / Codex (`.mcp.json` / `claude mcp add`):

```json
{ "mcpServers": { "open-why": { "command": "/path/to/open-why/target/release/open-why", "args": ["serve"] } } }
```

## License

Apache-2.0.
