#!/usr/bin/env bash
# Enforce the repository-wide size ceiling for handwritten Rust sources.
#
# No generated-file exclusions are defined. A generated marker does not exempt
# an ordinary tracked Rust file from the limit.
set -euo pipefail

readonly limit=999
mode="${1:-}"
root="$(git rev-parse --show-toplevel)"

case "$mode" in
  staged|tracked) ;;
  *)
    echo "usage: scripts/check-rust-loc.sh {staged|tracked}" >&2
    exit 2
    ;;
esac

read_blob() {
  local path="$1"
  case "$mode" in
    staged) git -C "$root" show ":$path" ;;
    tracked) git -C "$root" show "HEAD:$path" ;;
  esac
}

list_paths() {
  case "$mode" in
    staged) git -C "$root" ls-files -z --cached -- src tests ;;
    tracked) git -C "$root" ls-tree -r -z --name-only HEAD -- src tests ;;
  esac
}

path_list="$(mktemp "${TMPDIR:-/tmp}/open-why-rust-loc.XXXXXX")" || {
  printf '[rust-loc] failed: unable to create temporary path list (mode=%s)\n' "$mode" >&2
  exit 1
}
cleanup() {
  rm -f -- "$path_list"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

if ! list_paths > "$path_list" 2>/dev/null; then
  printf '[rust-loc] failed: unable to enumerate authoritative paths (mode=%s)\n' "$mode" >&2
  exit 1
fi

checked=0
problems=0
while IFS= read -r -d '' path; do
  case "$path" in
    src/*.rs|tests/*.rs) ;;
    *) continue ;;
  esac

  lines="$(read_blob "$path" | awk 'END { print NR + 0 }')"
  checked=$((checked + 1))
  if (( lines > limit )); then
    printf '[rust-loc] %s: %s physical lines (maximum %s)\n' "$path" "$lines" "$limit" >&2
    problems=$((problems + 1))
  fi
done < "$path_list"

if (( problems > 0 )); then
  printf '[rust-loc] failed: %s tracked Rust file(s) exceed %s physical lines (mode=%s)\n' \
    "$problems" "$limit" "$mode" >&2
  exit 1
fi

printf '[rust-loc] clean: %s tracked Rust file(s), maximum %s physical lines (mode=%s)\n' \
  "$checked" "$limit" "$mode"
