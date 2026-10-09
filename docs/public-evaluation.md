# Reproducible public evaluation

The seven records in `tests/fixtures/public-beta/evidence.json` are synthetic,
public-domain test data authored for this project. They contain no user records
and need no external corpus. They exercise behavior, not statistical quality.

```bash
cargo test --locked --no-default-features --test public_evaluation -- --nocapture
bash scripts/evaluate-local.sh
```

The second command downloads the model if needed, then explicitly runs the
otherwise ignored inference and local retrieval tests. Download failure, missing
files, wrong digests, missing vectors, or failed inference fail the command.
Ordinary `cargo test` reports model-dependent tests as ignored, never as passed.
CI explicitly runs them. The additional ignored cosine table is a manual debug
utility, not part of acceptance.

Model: Apache-2.0 `Xenova/all-MiniLM-L6-v2`, immutable revision
`751bff37182d3f1213fa05d7196b954e230abad9` on Hugging Face. Every downloaded and
loaded input is checked against these SHA-256 digests:

| File | SHA-256 |
|---|---|
| tokenizer.json | da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0 |
| config.json | 7135149f7cffa1a573466c6e4d8423ed73b62fd2332c575bf738a0d033f70df7 |
| onnx/model_quantized.onnx | afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1 |

The same verification applies to `OPEN_WHY_EMBED_MODEL_PATH`: this local backend
supports this pinned model, not arbitrary ONNX models. Old cache files with other
bytes fail explicitly; move them aside and run `why fetch-model` to refresh them.
The ONNX Runtime build dependency comes from the locked `ort` crate and is
separate from these model files.

Assertions cover the expected SQLite evidence first, repeat-query ordering,
current versus historical cache decisions, expired evidence, missing exact
records, both sides of an unresolved disagreement, empty scope, foreign scope,
and a second database. The real model run also checks vector dimensions and
semantic ranking participation. Lexical nonsense queries return no hits.

Semantic similarity can retrieve candidates even when no record answers the
question. Neither similarity nor a sealed digest establishes that a statement is
true or that a retrieved candidate supports the requested explanation. The engine
does not resolve contradictions automatically. Explicit supersession records a
replacement; age alone does not invalidate an otherwise current record. The
fixture uses old validity dates so these temporal assertions do not depend on a
narrow clock window; ranking contains time decay, so numeric scores are not a
portable snapshot contract.

This small suite is not a recall benchmark or evidence of real-client answer
quality. Do not tune ranking to maximize its apparent score. Add held-out,
independently authored examples when investigating retrieval changes. Existing
`why-golden` remains available for corpus-specific evaluation; it is not required
for this public suite.
