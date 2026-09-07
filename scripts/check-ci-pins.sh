#!/usr/bin/env bash
# Require immutable references for every external dependency executed by CI.
set -euo pipefail

mode="${1:-}"
root="$(git rev-parse --show-toplevel)"

case "$mode" in
  staged|tracked) ;;
  *)
    echo "usage: scripts/check-ci-pins.sh {staged|tracked}" >&2
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
    staged) git -C "$root" ls-files -z --cached -- .github/workflows ;;
    tracked) git -C "$root" ls-tree -r -z --name-only HEAD -- .github/workflows ;;
  esac
}

path_list="$(mktemp "${TMPDIR:-/tmp}/open-why-ci-pins.XXXXXX")" || {
  printf '[ci-pins] failed: unable to create temporary path list (mode=%s)\n' "$mode" >&2
  exit 1
}
cleanup() {
  rm -f -- "$path_list"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

if ! list_paths > "$path_list" 2>/dev/null; then
  printf '[ci-pins] failed: unable to enumerate authoritative workflows (mode=%s)\n' "$mode" >&2
  exit 1
fi

checked=0
problems=0
while IFS= read -r -d '' path; do
  case "$path" in
    .github/workflows/*.yml|.github/workflows/*.yaml) ;;
    *) continue ;;
  esac

  checked=$((checked + 1))
  line_number=0
  while IFS= read -r line || [[ -n "$line" ]]; do
    line_number=$((line_number + 1))
    line="${line%$'\r'}"
    if [[ "$line" =~ ^[[:space:]]*(-[[:space:]]*)?uses:[[:space:]]*(.+)$ ]]; then
      value="${BASH_REMATCH[2]}"
      value="$(printf '%s' "$value" | sed -E 's/[[:space:]]+#.*$//; s/^[[:space:]]+//; s/[[:space:]]+$//')"
      if [[ "$value" == \"*\" || "$value" == \'*\' ]]; then
        value="${value:1:${#value}-2}"
      fi

      if [[ "$value" == ./* ]]; then
        continue
      fi
      if [[ "$value" =~ ^docker://[^@]+@sha256:[0-9a-fA-F]{64}$ ]]; then
        continue
      fi
      if [[ "$value" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[A-Za-z0-9_./-]+)?@[0-9a-fA-F]{40}$ ]]; then
        continue
      fi

      printf '[ci-pins] %s:%s: external uses must end in a full commit SHA (or container digest): %s\n' \
        "$path" "$line_number" "$value" >&2
      problems=$((problems + 1))
    fi
  done < <(read_blob "$path")
done < "$path_list"

if (( problems > 0 )); then
  printf '[ci-pins] failed: %s mutable CI reference(s) found (mode=%s)\n' \
    "$problems" "$mode" >&2
  exit 1
fi

printf '[ci-pins] clean: %s workflow file(s) use immutable external references (mode=%s)\n' \
  "$checked" "$mode"
