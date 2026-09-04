# Retrieval parity

`why-golden` checks open-why's top result against a golden set you provide. Each
query records the stable ID returned by a trusted reference run, so parity is an
exact-ID comparison rather than a fuzzy title match.

Golden sets often reflect private source material. No real corpus fixture ships in
this repository. Keep fixtures outside the checkout, sanitize descriptions, and
never commit exported records merely to reproduce a ranking result.

```json
{
  "description": "Sanitized retrieval parity set",
  "scope": "example-project",
  "captured_at": "2026-01-01T00:00:00Z",
  "queries": [
    {
      "query": "example search query",
      "types": ["fact"],
      "expected": { "id": "example-record-id", "title": "example title", "type": "fact" }
    }
  ]
}
```

```bash
OPEN_WHY_EMBED_MODEL_PATH=/path/to/all-MiniLM-L6-v2 \
  cargo run --release --bin why-golden -- --fixture /path/to/golden-queries.json
```

Load the same corpus used to produce the captured expectations and configure the
same embedder. The reference answers are not computed live, making the harness a
deterministic regression gate.

The lexical arm uses a native SQLite FTS5 external-content table
(`decisions_fts`, columns `scope/title/content/tags`) ranked by
`bm25(decisions_fts, 0, 10, 5, 1)`. It applies a narrow-then-broad AND-to-OR
heuristic and tokenizes lowercase ASCII words and underscores after removing FTS5
stopwords.

Ranking changes should report:

- exact top-result matches;
- whether each miss was absent or merely ordered below another admitted result;
- the fixture date and embedder identity;
- confirmation that the fixture was kept out of the public repository.

Do not retune constants to a tiny fixture. A change belongs in the engine only
when a representative corpus and replay evidence show a general improvement.
