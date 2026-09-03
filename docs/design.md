# Design

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
  decision. Library consumers can resolve any stable record ID through
  `Store::get_current_evidence`: it follows supersession to the active record and
  returns that record's Git references plus the explicit chain it traversed.
- **Temporal identity** — `superseded_by`, `valid_from`, `valid_until`. Search
  hydrates the *current* version; history stays reachable. `--historical` (search,
  get, and MCP) reaches past supersession and walks the chain, so "what changed and
  why" is answerable.
- **Hybrid recall** — reciprocal-rank fusion of a **semantic arm** (local on-device
  `all-MiniLM-L6-v2` embeddings) and a **lexical arm** (FTS5-style BM25 with
  title/content column weights), weighted by importance and effectiveness and
  decayed by Ebbinghaus recency — with spaced-repetition stability, so a memory
  that keeps being the right answer stays findable.
- **Capture provenance** — `content_digest` + `source_identity` make capture
  idempotent and de-duplicated by content.
- **The learning loop** — `why feedback <id> --helpful` / `--not-helpful` folds a
  verdict into the record's effectiveness (a 0.05 raise / 0.03 drop on the
  ungraded 0.5 prior, clamped to `[0.01, 1.0]`), so recall quality improves from
  usage rather than from hand-tuning.

Records carry a kind — `decision`, `fact`, `reference`, `pattern`, `doc`,
`project`, `observation` — so recall can be scoped by type as well as by query.

## Status

**Pre-1.0.** The git-mining path (`why init`, `why capture`, `why search`, `why
serve`) is fully standalone and works against any git repo today. The `why import`
path is how this store gets bulk-loaded from an external system (currently:
cogitod's durable memories) — see [retrieval-parity](retrieval-parity.md) for what
that does and doesn't require.

## Full CLI surface

`why` is the only binary: `ask` (bare), `init`, `capture`, `search`, `get`, `link`,
`import`, `fetch-model`, `feedback`, `serve`.

```bash
why search "sqlite" --historical      # include superseded decisions
why get <id> --historical             # walk the supersession chain
why link <commit> <decision>
why import --file decisions.json
```
