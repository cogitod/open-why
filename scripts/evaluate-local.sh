#!/usr/bin/env bash
# Explicit opt-in; missing models or failed inference fail this command and CI.
set -euo pipefail
cargo run --locked --bin why -- fetch-model
model_dir="${OPEN_WHY_EMBED_MODEL_PATH:-$HOME/.cache/open-why/models/Xenova/all-MiniLM-L6-v2}"
OPEN_WHY_EMBED_MODEL_PATH="$model_dir" cargo test --locked --lib local_embedder_produces_normalized_384d_vectors -- --ignored --nocapture
OPEN_WHY_EMBED_MODEL_PATH="$model_dir" cargo test --locked --test public_evaluation public_local_embedding_evaluation -- --ignored --nocapture
