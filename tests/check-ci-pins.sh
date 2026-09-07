#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
fixture_root="$(mktemp -d)"
fixture_repo="$fixture_root/repo"
trap 'rm -rf "$fixture_root"' EXIT

mkdir -p "$fixture_repo/scripts" "$fixture_repo/.github/workflows"
cp "$root/scripts/check-ci-pins.sh" "$fixture_repo/scripts/check-ci-pins.sh"
git -C "$fixture_repo" init -q
git -C "$fixture_repo" config user.name "CI Pin Test"
git -C "$fixture_repo" config user.email "ci-pin@example.invalid"

write_valid_workflow() {
  cat > "$fixture_repo/.github/workflows/ci.yml" <<'YAML'
name: CI
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567 # v1.2.3
      - uses: ./local-action
      - uses: docker://example.invalid/tool@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
YAML
}

check_passes() {
  local mode="$1"
  if ! output=$(cd "$fixture_repo" && bash scripts/check-ci-pins.sh "$mode" 2>&1); then
    printf 'expected %s check to pass:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi
}

check_fails() {
  local mode="$1"
  local expected="$2"
  if output=$(cd "$fixture_repo" && bash scripts/check-ci-pins.sh "$mode" 2>&1); then
    printf 'expected %s check to fail:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi
  grep -Fq "$expected" <<<"$output"
  grep -Fq 'mutable CI reference(s) found' <<<"$output"
}

# A missing authoritative tree is a closed failure, never a clean result.
if output=$(cd "$fixture_repo" && bash scripts/check-ci-pins.sh tracked 2>&1); then
  printf 'expected missing HEAD check to fail:\n%s\n' "$output" >&2
  exit 1
fi
grep -Fq 'unable to enumerate authoritative workflows' <<<"$output"

write_valid_workflow
git -C "$fixture_repo" add -A
git -C "$fixture_repo" commit -qm "test: valid immutable references"
check_passes tracked
check_passes staged

# Worktree bytes do not override the authoritative HEAD or index.
sed -i.bak 's/@0123456789abcdef0123456789abcdef01234567/@v1/' \
  "$fixture_repo/.github/workflows/ci.yml"
check_passes tracked
check_passes staged
git -C "$fixture_repo" add .github/workflows/ci.yml
check_fails staged 'actions/checkout@v1'

git -C "$fixture_repo" commit -qm "test: mutable tag"
check_fails tracked 'actions/checkout@v1'

# Short commit hashes and mutable container tags are rejected too.
write_valid_workflow
sed -i.bak 's/@0123456789abcdef0123456789abcdef01234567/@0123456789ab/' \
  "$fixture_repo/.github/workflows/ci.yml"
git -C "$fixture_repo" add .github/workflows/ci.yml
check_fails staged 'actions/checkout@0123456789ab'

write_valid_workflow
sed -i.bak 's|docker://example.invalid/tool@sha256:[0-9a-f]*|docker://example.invalid/tool:latest|' \
  "$fixture_repo/.github/workflows/ci.yml"
git -C "$fixture_repo" add .github/workflows/ci.yml
check_fails staged 'docker://example.invalid/tool:latest'

echo '[ci-pins-test] SHA, digest, local-action, and Git-authority behavior verified'
