# openwhy

**Ask any repository why a decision was made — with the evidence.**

`git blame` tells you *who* changed a line and *when*. `openwhy` tells you *why* —
the commit, the ADR, the author, and the rationale — as a cited answer.

## Where openwhy sits

The open-source AI-agent stack is filling in every layer except the memory:

| Layer | Project | Job |
| --- | --- | --- |
| Agent | [OpenHands](https://github.com/OpenHands/openhands) | do the work |
| Harness | [opencode](https://github.com/anomalyco/opencode) | run the agent |
| TUI | [opentui](https://github.com/anomalyco/opentui) | see the agent |
| Runtime | [herdr](https://herdr.dev) | where the agent lives |
| Connectors | [open-connector](https://github.com/oomol-lab/open-connector) | reach the apps |
| **Memory** | **openwhy** | **remember why** ← the missing layer |

They all make agents *do*. None of them makes an agent (or you) *remember why*.

## Not another memory layer

Agent memory is the most crowded corner of the open stack — Mem0 (64k★),
Zep Graphiti (30k★), Letta (24k★), Hindsight (21k★) all remember **what**
happened. openwhy is deliberately narrow: it remembers **why**, and cites the
evidence (the commit, the ADR, the decision record) so you can trust the answer.

> Mem0 and Graphiti remember *what*. openwhy remembers *why* — with the proof.

## Quick start

```bash
# answer "why" on any repo — no setup needed
openwhy why "why is the sandbox separate?" --repo https://github.com/anomalyco/opencode

# index the current repo explicitly
openwhy init

# ask a question of the current repo
openwhy why "why do we use SQLite instead of Postgres?"
```

## What you get

Every answer is evidence-bound, not a guess:

```
- Use SQLite for the local-first record
  2025-03-11 · adrian · commit 8f2c41a
  zero-config, single file, survives a laptop
```

## How it works

1. `init` walks git history + ADRs + design docs and extracts decisions.
2. `why` ranks them (git-aware lexical scoring) and cites the source.

On the roadmap: `openwhy serve` (an MCP server so Claude Code / Codex / opencode
can ask *"why is this here?"* before editing) and on-device embeddings.

## License

Apache-2.0.
