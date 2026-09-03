# Retrieval parity

`why-golden` checks open-why's top-1 result against a golden set you provide: for each
query, the top-1 memory some reference engine (e.g. cogitod's production `mem_search`)
returned against your own corpus. The memory id survives the mirror verbatim, so parity
is an exact-id comparison, not a fuzzy title match. This is inherently private — it's
only meaningful against your own live corpus — so no fixture ships in this repo; point
`--fixture` at your own file in the same shape:

```json
{
  "description": "Golden retrieval parity set",
  "scope": "1",
  "captured_at": "2026-08-30T06:44:00Z",
  "queries": [
    {
      "query": "example search query",
      "types": ["fact"],
      "expected": { "id": "<uuid>", "title": "example title", "type": "fact" }
    }
  ]
}
```

```bash
OPEN_WHY_EMBED_MODEL_PATH=/path/to/all-MiniLM-L6-v2 \
  cargo run --release --bin why-golden -- --fixture /path/to/your-golden-queries.json
```

Both engines are run against the same corpus (open-why's store is a mirror of
cogitod's durable memories) and the same local embedder. The reference answer is
captured, not computed live, so the check is a deterministic regression gate.

The lexical arm is a native SQLite FTS5 external-content table
(`decisions_fts`, columns `scope/title/content/tags`, ranked by
`bm25(decisions_fts, 0, 10, 5, 1)`) — the same engine cogitod's
`MemoryRepository.lexicalSearchIds` calls, including its narrow-then-broad
AND→OR heuristic and its `toSearchTerms` tokenization (`[a-z0-9_]+`, FTS5
stopwords).

Known gap (2026-09-03): 5/8 golden exact, up from 4/8 after porting cogitod's
post-fusion **relevance gate** (`MemoryRelevanceGate`: similarity floor +
`RAG_UTILITY_THRESHOLD` lexical utility — see `src/relevance.rs`). The gate
fixed the one failure that was a true admission problem (a weak-but-nonzero
semantic-arm competitor scoring below `SIMILARITY_FLOOR`, crowding out the
right answer). The remaining 3 misses are **ranking-order** mismatches, not
admission problems: the expected record is present and gate-admitted, just
outscored by 1-2 other admitted candidates in the RRF/hybrid fusion — so the
relevance gate, which only filters and never reorders, can't reach them.
Closing those needs a closer comparison of open-why's hybrid rerank
(`rank_by` in `src/db.rs`) against cogitod's exact `searchMemoriesHybrid`
fusion, tracked as a separate work item.
