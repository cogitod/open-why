#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
fixture_root="$(mktemp -d)"
fixture_repo="$fixture_root/repo"
trap 'rm -rf "$fixture_root"' EXIT

mkdir -p "$fixture_repo/scripts" "$fixture_repo/src" "$fixture_repo/tests"
cp "$root/scripts/check-rust-loc.sh" "$fixture_repo/scripts/check-rust-loc.sh"
git -C "$fixture_repo" init -q
git -C "$fixture_repo" config user.name "Rust LOC Test"
git -C "$fixture_repo" config user.email "rust-loc@example.invalid"

write_lines() {
  local path="$1"
  local count="$2"
  local suffix="${3:-newline}"
  local index
  : > "$path"
  for ((index = 1; index <= count; index += 1)); do
    if [[ "$suffix" == "none" && "$index" -eq "$count" ]]; then
      printf '// line %s' "$index" >> "$path"
    else
      printf '// line %s\n' "$index" >> "$path"
    fi
  done
}

check_passes() {
  local mode="$1"
  if ! output=$(cd "$fixture_repo" && bash scripts/check-rust-loc.sh "$mode" 2>&1); then
    printf 'expected %s check to pass:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi
}

check_fails() {
  local mode="$1"
  if output=$(cd "$fixture_repo" && bash scripts/check-rust-loc.sh "$mode" 2>&1); then
    printf 'expected %s check to fail:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi
  grep -q '1000 physical lines (maximum 999)\|1001 physical lines (maximum 999)' <<<"$output"
}

check_enumeration_fails() {
  local mode="$1"
  local index_path="${2:-}"
  local expected="[rust-loc] failed: unable to enumerate authoritative paths (mode=$mode)"

  if [[ -n "$index_path" ]]; then
    if output=$(cd "$fixture_repo" && GIT_INDEX_FILE="$index_path" bash scripts/check-rust-loc.sh "$mode" 2>&1); then
      printf 'expected %s path enumeration to fail:\n%s\n' "$mode" "$output" >&2
      exit 1
    fi
  elif output=$(cd "$fixture_repo" && bash scripts/check-rust-loc.sh "$mode" 2>&1); then
    printf 'expected %s path enumeration to fail:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi

  if [[ "$output" != "$expected" ]]; then
    printf 'unexpected %s path-enumeration diagnostic:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi
  if grep -Fq '[rust-loc] clean:' <<<"$output"; then
    printf 'failed %s path enumeration printed a clean result:\n%s\n' "$mode" "$output" >&2
    exit 1
  fi
}

commit_all() {
  git -C "$fixture_repo" add -A
  git -C "$fixture_repo" commit -qm "$1"
}

# Authoritative enumeration failures are closed failures, never clean results.
check_enumeration_fails tracked
malformed_index="$fixture_root/malformed-index"
printf 'invalid index\n' > "$malformed_index"
check_enumeration_fails staged "$malformed_index"

# Exact boundaries and files without a final newline use physical-line counting.
write_lines "$fixture_repo/src/boundary-998.rs" 998
write_lines "$fixture_repo/src/boundary-999.rs" 999
write_lines "$fixture_repo/tests/no-final-newline-999.rs" 999 none
commit_all "test: valid boundaries"
check_passes tracked
check_passes staged

write_lines "$fixture_repo/src/boundary-1000.rs" 1000
git -C "$fixture_repo" add src/boundary-1000.rs
check_fails staged
git -C "$fixture_repo" reset -q HEAD -- src/boundary-1000.rs
rm "$fixture_repo/src/boundary-1000.rs"

write_lines "$fixture_repo/tests/no-final-newline-1000.rs" 1000 none
git -C "$fixture_repo" add tests/no-final-newline-1000.rs
check_fails staged
git -C "$fixture_repo" reset -q HEAD -- tests/no-final-newline-1000.rs
rm "$fixture_repo/tests/no-final-newline-1000.rs"

# New breaches fail, growth within the ceiling passes, and committed breaches
# remain failures when modified to equal, larger, or still-too-large sizes.
write_lines "$fixture_repo/src/added.rs" 1000
git -C "$fixture_repo" add src/added.rs
check_fails staged
git -C "$fixture_repo" reset -q HEAD -- src/added.rs
rm "$fixture_repo/src/added.rs"

write_lines "$fixture_repo/src/boundary-998.rs" 999
git -C "$fixture_repo" add src/boundary-998.rs
check_passes staged
git -C "$fixture_repo" reset -q --hard HEAD

write_lines "$fixture_repo/src/legacy.rs" 1000
commit_all "test: establish oversized fixture"
printf '// changed but equal\n' > "$fixture_repo/src/legacy.rs.tmp"
tail -n +2 "$fixture_repo/src/legacy.rs" >> "$fixture_repo/src/legacy.rs.tmp"
mv "$fixture_repo/src/legacy.rs.tmp" "$fixture_repo/src/legacy.rs"
git -C "$fixture_repo" add src/legacy.rs
check_fails staged

write_lines "$fixture_repo/src/legacy.rs" 1001
git -C "$fixture_repo" add src/legacy.rs
check_fails staged

write_lines "$fixture_repo/src/legacy.rs" 999
git -C "$fixture_repo" add src/legacy.rs
check_passes staged
git -C "$fixture_repo" reset -q --hard HEAD

# Rename, copy, and delete are interpreted from the complete index state.
git -C "$fixture_repo" mv src/legacy.rs "src/renamed file.rs"
check_fails staged
git -C "$fixture_repo" reset -q --hard HEAD

cp "$fixture_repo/src/legacy.rs" "$fixture_repo/src/copied.rs"
git -C "$fixture_repo" add src/copied.rs
git -C "$fixture_repo" diff --cached --name-status -C --find-copies-harder | grep -q '^C'
check_fails staged
git -C "$fixture_repo" reset -q --hard HEAD

git -C "$fixture_repo" rm -q src/legacy.rs
check_passes staged
commit_all "test: remove oversized fixture"

# NUL-delimited enumeration supports whitespace, tabs, and newlines in paths.
weird_path=$'src/odd name\tand\nnewline.rs'
write_lines "$fixture_repo/$weird_path" 1000
git -C "$fixture_repo" add "$weird_path"
check_fails staged
git -C "$fixture_repo" reset -q HEAD -- "$weird_path"
rm "$fixture_repo/$weird_path"

# The index and HEAD are authoritative even when worktree bytes disagree.
write_lines "$fixture_repo/src/index-authority.rs" 1000
git -C "$fixture_repo" add src/index-authority.rs
write_lines "$fixture_repo/src/index-authority.rs" 1
check_fails staged
git -C "$fixture_repo" reset -q HEAD -- src/index-authority.rs
rm "$fixture_repo/src/index-authority.rs"

write_lines "$fixture_repo/src/head-authority.rs" 999
commit_all "test: tracked authority"
write_lines "$fixture_repo/src/head-authority.rs" 1000
check_passes tracked
git -C "$fixture_repo" checkout -q -- src/head-authority.rs

write_lines "$fixture_repo/src/tracked-breach.rs" 1000
commit_all "test: tracked breach"
write_lines "$fixture_repo/src/tracked-breach.rs" 1
check_fails tracked

# Marker text alone never creates an undocumented generated-file exclusion.
write_lines "$fixture_repo/src/generated.rs" 1000
printf '// @generated: do not edit\n' > "$fixture_repo/src/generated.tmp"
tail -n +2 "$fixture_repo/src/generated.rs" >> "$fixture_repo/src/generated.tmp"
mv "$fixture_repo/src/generated.tmp" "$fixture_repo/src/generated.rs"
git -C "$fixture_repo" add src/generated.rs
check_fails staged

echo "[rust-loc-test] boundaries, Git states, path safety, and marker behavior verified"
