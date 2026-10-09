#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
fixture_root="$(mktemp -d)"
fixture_repo="$fixture_root/repo"
trap 'rm -rf "$fixture_root"' EXIT

mkdir -p "$fixture_repo/hooks"
cp "$root/hooks/check-leaks.sh" "$fixture_repo/hooks/check-leaks.sh"
touch "$fixture_repo/hooks/leak-allowlist.txt"
printf 'safe\n' > "$fixture_repo/secret.txt"
printf 'safe\n' > "$fixture_repo/records.txt"
printf 'safe\n' > "$fixture_repo/provenance.txt"
git -C "$fixture_repo" init -q
git -C "$fixture_repo" config user.name "Leak Scanner Test"
git -C "$fixture_repo" config user.email "leak-scanner@example.invalid"
git -C "$fixture_repo" add .
git -C "$fixture_repo" commit -qm base

# Stage three leak classes, then hide each behind safe unstaged worktree bytes.
printf 'AKIA%s\n' 'ABCDEFGHIJKLMNOP' > "$fixture_repo/secret.txt"
{
  printf '%s-%s-%s-%s-%s\n' 11111111 1111 1111 1111 111111111111
  printf '%s-%s-%s-%s-%s\n' 22222222 2222 2222 2222 222222222222
  printf '%s-%s-%s-%s-%s\n' 33333333 3333 3333 3333 333333333333
} > "$fixture_repo/records.txt"
printf '/%s/private/project\n' 'Users' > "$fixture_repo/provenance.txt"
git -C "$fixture_repo" add secret.txt records.txt provenance.txt
printf 'safe\n' > "$fixture_repo/secret.txt"
printf 'safe\n' > "$fixture_repo/records.txt"
printf 'safe\n' > "$fixture_repo/provenance.txt"

if output=$(cd "$fixture_repo" && bash hooks/check-leaks.sh staged 2>&1); then
  echo "expected staged index leaks to fail the scanner" >&2
  exit 1
fi
grep -q 'possible secret in secret.txt' <<<"$output"
if grep -qF "$(printf 'AKIA%s' 'ABCDEFGHIJKLMNOP')" <<<"$output"; then
  echo 'scanner exposed the detected secret in its output' >&2
  exit 1
fi
grep -q 'records.txt contains 3 UUID-like identifiers' <<<"$output"
grep -q 'private implementation provenance in provenance.txt' <<<"$output"

# Stage the safe bytes, then put the leaks only in the worktree. The exact staged
# scan must pass without false positives from those unstaged changes.
git -C "$fixture_repo" add secret.txt records.txt provenance.txt
printf 'AKIA%s\n' 'ABCDEFGHIJKLMNOP' > "$fixture_repo/secret.txt"
printf '%s-%s-%s-%s-%s\n' 11111111 1111 1111 1111 111111111111 > "$fixture_repo/records.txt"
printf '/%s/private/project\n' 'Users' > "$fixture_repo/provenance.txt"
(
  cd "$fixture_repo"
  bash hooks/check-leaks.sh staged
)

echo "[leak-check-test] exact staged index authority verified"

# A reviewed public contact is allowed only as the exact conduct-policy line.
contact='foundry@cogi''to.cv'
printf 'privately to [%s](mailto:%s).\n' "$contact" "$contact" > "$fixture_repo/CODE_OF_CONDUCT.md"
git -C "$fixture_repo" add CODE_OF_CONDUCT.md
(cd "$fixture_repo" && bash hooks/check-leaks.sh staged)
printf 'privately to [%s](mailto:%s). private downstream context\n' "$contact" "$contact" > "$fixture_repo/CODE_OF_CONDUCT.md"
git -C "$fixture_repo" add CODE_OF_CONDUCT.md
if (cd "$fixture_repo" && bash hooks/check-leaks.sh staged >/dev/null 2>&1); then
  echo 'expected extra contact-line provenance to be rejected' >&2
  exit 1
fi
printf 'privately to [%s](mailto:%s).\n' "$contact" "$contact" > "$fixture_repo/CODE_OF_CONDUCT.md"
git -C "$fixture_repo" add CODE_OF_CONDUCT.md
publisher='cogi''tod/open-why'
printf '  --repo %s \\\n  --signer-workflow %s/.github/workflows/release-candidate.yml\n' "$publisher" "$publisher" > "$fixture_repo/RELEASE.md"
git -C "$fixture_repo" add RELEASE.md
(cd "$fixture_repo" && bash hooks/check-leaks.sh staged)
printf '  --repo %s private downstream context\n' "$publisher" >> "$fixture_repo/RELEASE.md"
git -C "$fixture_repo" add RELEASE.md
if (cd "$fixture_repo" && bash hooks/check-leaks.sh staged >/dev/null 2>&1); then
  echo 'expected extra release-line provenance to be rejected' >&2
  exit 1
fi
echo '[leak-check-test] narrow public-contact and release-publisher exceptions verified'
