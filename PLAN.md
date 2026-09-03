# open-why — distillation plan

How open-why is extracted from cogitod. This doc pins the *what* and the *why-not*,
so the build never drifts into "another agent memory layer."

## The wedge

open-why's defensible ground is **decision-recall with evidence**: answer *"why was
this decision made?"* from git history + decision records, with the proof attached.
It is **not** a general memory store.

## Competitive landscape (measured 2026-08-29)

| Space | State | Names |
| --- | --- | --- |
| Agent memory | **crowded, funded** | Mem0 64k★, Graphiti 30k★, OpenViking 34k★, TencentDB-Agent-Memory 25k★, Letta 24k★, Hindsight 21k★ |
| ADR tooling | mature, **writes** ADRs only | adr-tools 5.6k★, log4brains 1.6k★ |
| **"why / decision-recall"** | **empty** | WhyTracker 1★, wd2t 0★ — the pain is named, nobody owns it |

`git blame` says *who* and *when*; nothing open says *why* with evidence. That is
the entire opportunity. Memory is commodity; the "why" recall is not.

## What we distill — the "why" core

Four sub-contracts, each traceable to cogitod source:

**A. Decision linkage** — the "why" primitive. From `052_memory_git_refs.sql`
(ADR-004): every `mem-ref:` trailer in a commit message creates a bidirectional
`memory_git_refs(memory_id, commit_hash, commit_subject)` row. commit → memory
(*why was this commit made*) and memory → commits (*which commits realized this
decision*). Powers `link_git` and `ask_why`.

**B. Decision temporal identity.** From `039_temporal_memory.sql` + the 2d recency
half-life in `MemorySearchUtils.ts`: decisions are point-in-time and are
**superseded, never deleted** (`superseded_by`, `valid_from`/`valid_until`). A newer
decision on the same question retires the older one; the search always hydrates the
*current* version.

**C. Recall ranking.** From `MemorySearchUtils.ts` + shared `ranking.ts`:
two-step (search IDs → hydrate current), hybrid rerank
`0.65·similarity + 0.25·importance + 0.10·effectiveness` × Ebbinghaus recency decay
(7d half-life; **2d for `decision`**), non-durable blocklist (`event` firehose
excluded). Reproduce the calibrated constants; do not "improve" them without the
golden set.

**D. Capture provenance.** From `092_memory_capture_provenance.sql`:
`content_digest` + `source_identity` → idempotent capture, dedup by content.

## What we deliberately do NOT distill (commodity / orchestration)

| Skip | Why |
| --- | --- |
| embeddings engine (`p-embeddings` + vec0) | commodity; leave a pluggable embedder *interface* instead |
| entity identities / aliases / full contradiction ledger (`091`) | defer; MVP needs only supersede + `valid_until` |
| wikilinks / wiki docs (`119`, `128`) | defer |
| tenant isolation / ACL / PostgreSQL managed path (`record-tools.ts`) | cogitod-only |
| work_items, sessions, fleet, lanes, landing, verification, conductor, hooks, OPC | orchestration = the moat; stays in cogitod |

**OPC registration.** open-why is deliberately never registered with cogito-opc's registry — the
table above already draws this boundary (OPC = orchestration = the moat). Registration would be a
dependency, not a distillation. This is a standing design choice, not an oversight to close.

Revisited 2026-09-03: after the relevance-gate port (README "Known gap") closed golden
parity from 4/8 to 5/8, the contradiction ledger was investigated as the likely fix for
the remaining 3 (ranking-order, not admission, misses — cogitod's `demotionFactor`
suppresses contested/superseded candidates before the final sort). Full cross-repo
porting shape was scoped but declined: it's exactly the ledger this table already
defers, and 3 golden queries don't justify reopening that scope. Revisit only if this
table's scope is deliberately reopened for other reasons.

## The seam

open-why exposes a minimal verb set that cogitod (and any other agent) can call:

`capture` · `search` · `get` · `link_git(commit, memory)` · `ask_why(question, repo)`

Form is an open decision (MCP-host is the lean; see Phases).

## Phases

**P0 — done.** open-why v1: git-mining "why" CLI (`init`, `why`) + MCP server
(`open-why_ask`, `open-why_index`). Self-contained, no cogitod dependency.

**P1 — open-why owns the "why" core.** Port A–D into a Rust SQLite store
(`rusqlite`, bundled): `memories` + `valid_from/valid_until` + `memory_git_refs` +
`memory_capture_provenance`. Port the recall ranking + temporal active-window filter.
Wire `open-why_capture/search/get/link/ask` into the MCP server. Lexical-first;
embeddings deferred. **Gate:** `cargo build --release` clean; dogfood `ask_why`
against a live corpus.

**P2 — cogitod consumes open-why.** Lock the seam form (lean: **MCP-host** — cogitod
already is the hub). Migrate durable knowledge memories
(`decision`/`fact`/`reference`/`pattern`/`doc`/`project`/`observation`) into open-why's
store, preserving `git_refs` + temporal windows. Route cogitod's knowledge-recall
reads through open-why; retire the private knowledge-memory engine; keep operational
state. Verify golden-set parity before cutover.

**P3 — release.** open-why ships as the open "why" layer (CLI + MCP + lib crate);
cogitod stays the closed orchestration on top. Public release is pure-git + its own
self-contained store — never a read into private Breathe data.

## Risks

- **Port fidelity** — the rerank carries hard-won calibration (`RAG_UTILITY_THRESHOLD
  0.3825`, floor 0.34, 2d decision half-life). Reproduce, validate against the same
  fixtures, then move.
- **Two-step read contract** (`search IDs → hydrate current`) must be preserved or
  superseded rows ghost the search.
- **Embeddings are 65% of the rank weight in cogitod** — lexical-first open-why ranks
  *differently* until P2 adds vectors. Accepted for v1; call it out in the README.

## Positioning line

> Mem0 and Graphiti remember *what*. open-why remembers *why* — with the proof.
