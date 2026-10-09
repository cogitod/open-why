#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT
# Git exports these while running hooks; fixtures must use their own index.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
mkdir -p "$fixture_root/repo"
cp -R "$root/hooks" "$root/scripts" "$fixture_root/repo/"
cd "$fixture_root/repo"
git init -q --initial-branch=main
git config user.name 'Branch Guard Test'
git config user.email 'branch-test@example.invalid'
git config commit.gpgsign false
git config core.hooksPath hooks

reject_commit() {
  local expected="$1"
  if output="$(git commit --allow-empty -m 'test: branch guard' 2>&1)"; then
    printf 'expected commit to fail:\n%s\n' "$output" >&2
    exit 1
  fi
  if [[ "$output" != *"$expected"* ]]; then
    printf 'wrong rejection:\n%s\n' "$output" >&2
    exit 1
  fi
}

# The actual installed hook rejects both unborn and existing main branches.
reject_commit 'commits on main are blocked'
git switch -qc fix/example
printf 'Synthetic fixture\n' > fixture.txt
git add fixture.txt
git commit -qm 'test: topic branch accepted'
original_head="$(git rev-parse HEAD)"
git switch -qc main
reject_commit 'commits on main are blocked'
test "$(git rev-parse HEAD)" = "$original_head"

git switch --detach -q
reject_commit 'create a topic branch before committing'
test "$(git rev-parse HEAD)" = "$original_head"

cd "$fixture_root"
if bash "$root/scripts/check-branch.sh" > outside.log 2>&1; then
  echo 'expected guard outside a Git repository to fail' >&2
  exit 1
fi
echo '[branch-test] installed hook accepts topic branches and rejects main/detached commits'
