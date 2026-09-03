#!/usr/bin/env bash
# Leak scanner for open-why. Self-contained: no network, no dependencies beyond git/grep.
# Catches two different things:
#   1. Classic secrets — API keys, tokens, private key blocks.
#   2. Internal-data density — a file carrying several real-looking UUIDs (the shape of an
#      unreviewed export from cogitod's internal store), which no generic secret scanner flags
#      because a UUID and a plain-English title aren't "secrets" by any entropy heuristic.
#
# Usage:
#   hooks/check-leaks.sh staged    # currently staged files (used by hooks/pre-commit)
#   hooks/check-leaks.sh tracked   # every tracked file (used by CI)
#
# A path in hooks/leak-allowlist.txt is exempt from the UUID-density check (not from the
# secret-pattern check — a real secret is never fine to allowlist, it must be revoked/removed).
set -euo pipefail

mode="${1:-staged}"
root="$(git rev-parse --show-toplevel)"
allowlist="$root/hooks/leak-allowlist.txt"

case "$mode" in
  staged) list_cmd=(git diff --cached --name-only --diff-filter=ACM) ;;
  tracked) list_cmd=(git ls-files) ;;
  *)
    echo "usage: check-leaks.sh [staged|tracked]" >&2
    exit 2
    ;;
esac

# bash 3.2 (macOS default) has no `mapfile`/`readarray`, so build the array by hand.
files=()
while IFS= read -r line; do
  [[ -n "$line" ]] && files+=("$line")
done < <("${list_cmd[@]}")

is_allowlisted() {
  [[ -f "$allowlist" ]] || return 1
  grep -qxF "$1" "$allowlist"
}

problems=0

# 1. Secret-pattern scan — applies to every file, allowlist or not.
secret_patterns=(
  '-----BEGIN (RSA|OPENSSH|EC|DSA|PGP) PRIVATE KEY-----'
  'AKIA[0-9A-Z]{16}'                            # AWS access key id
  'gh[pousr]_[A-Za-z0-9]{36,}'                  # GitHub token (pat/oauth/user/server/refresh)
  'xox[baprs]-[A-Za-z0-9-]{10,}'                # Slack token
  'sk-ant-[A-Za-z0-9_-]{20,}'                   # Anthropic key
  'sk-[A-Za-z0-9]{20,}'                         # OpenAI-style key
)

for f in "${files[@]:-}"; do
  [[ -z "$f" || ! -f "$root/$f" ]] && continue
  for pat in "${secret_patterns[@]}"; do
    if hits=$(grep -EnI "$pat" "$root/$f" 2>/dev/null); then
      echo "[leak-check] possible secret in $f:"
      echo "$hits" | sed 's/^/    /'
      problems=$((problems + 1))
    fi
  done
done

# 2. Internal-data density heuristic — skipped for allowlisted paths.
uuid_re='[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}'
for f in "${files[@]:-}"; do
  [[ -z "$f" || ! -f "$root/$f" ]] && continue
  is_allowlisted "$f" && continue
  count=$( (grep -EoI "$uuid_re" "$root/$f" 2>/dev/null || true) | wc -l | tr -d ' ')
  if [[ "$count" -ge 3 ]]; then
    echo "[leak-check] $f contains $count UUID-like identifiers — looks like an unreviewed export"
    echo "  of real records (e.g. cogitod memory ids). If this is reviewed, sanitized fixture"
    echo "  data, add its path to hooks/leak-allowlist.txt with a note on why, then re-commit."
    problems=$((problems + 1))
  fi
done

if [[ "$problems" -gt 0 ]]; then
  echo "[leak-check] $problems potential issue(s) found (mode=$mode)."
  exit 1
fi
echo "[leak-check] clean ($mode)"
