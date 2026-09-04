#!/usr/bin/env bash
# Leak scanner for open-why. Self-contained: no network, no dependencies beyond git/grep.
# Catches three different things:
#   1. Classic secrets: API keys, tokens, private key blocks.
#   2. Record-export density: a file carrying several real-looking UUIDs, which no generic
#      secret scanner flags
#      because a UUID and a plain-English title aren't "secrets" by any entropy heuristic.
#   3. Private implementation provenance: downstream product names and local user paths do
#      not belong in this standalone public repository.
#
# Usage:
#   hooks/check-leaks.sh staged    # currently staged files (used by hooks/pre-commit)
#   hooks/check-leaks.sh tracked   # every tracked file (used by CI)
#
# A path in hooks/leak-allowlist.txt is exempt from the UUID-density check (not from the
# secret-pattern check; a real secret is never safe to allowlist and must be revoked or removed).
set -euo pipefail

mode="${1:-staged}"
root="$(git rev-parse --show-toplevel)"

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
  path_exists "hooks/leak-allowlist.txt" || return 1
  read_path "hooks/leak-allowlist.txt" | grep -qxF "$1"
}

path_exists() {
  case "$mode" in
    staged) git -C "$root" cat-file -e ":$1" 2>/dev/null ;;
    tracked) [[ -f "$root/$1" ]] ;;
  esac
}

read_path() {
  case "$mode" in
    staged) git -C "$root" show ":$1" ;;
    tracked) command cat -- "$root/$1" ;;
  esac
}

problems=0

# 1. Secret-pattern scan. This applies to every file, allowlist or not.
secret_patterns=(
  '-----BEGIN (RSA|OPENSSH|EC|DSA|PGP) PRIVATE KEY-----'
  'AKIA[0-9A-Z]{16}'                            # AWS access key id
  'gh[pousr]_[A-Za-z0-9]{36,}'                  # GitHub token (pat/oauth/user/server/refresh)
  'xox[baprs]-[A-Za-z0-9-]{10,}'                # Slack token
  'sk-ant-[A-Za-z0-9_-]{20,}'                   # Anthropic key
  'sk-[A-Za-z0-9]{20,}'                         # OpenAI-style key
)

for f in "${files[@]:-}"; do
  [[ -z "$f" ]] && continue
  path_exists "$f" || continue
  for pat in "${secret_patterns[@]}"; do
    if hits=$(read_path "$f" | grep -EnI "$pat" 2>/dev/null); then
      echo "[leak-check] possible secret in $f:"
      echo "$hits" | sed 's/^/    /'
      problems=$((problems + 1))
    fi
  done
done

# 2. Record-export density heuristic. This is skipped for allowlisted paths.
uuid_re='[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}'
for f in "${files[@]:-}"; do
  [[ -z "$f" ]] && continue
  path_exists "$f" || continue
  is_allowlisted "$f" && continue
  count=$( (read_path "$f" | grep -EoI "$uuid_re" 2>/dev/null || true) | wc -l | tr -d ' ')
  if [[ "$count" -ge 3 ]]; then
    echo "[leak-check] $f contains $count UUID-like identifiers; looks like an unreviewed export"
    echo "  of real records. If this is reviewed, sanitized fixture"
    echo "  data, add its path to hooks/leak-allowlist.txt with a note on why, then re-commit."
    problems=$((problems + 1))
  fi
done

# 3. Standalone-source check. Public repository ownership URLs and copyright attribution are
# intentionally allowed in the small metadata set below; implementation and product surfaces
# must remain generic. Split marker literals keep this checker from flagging itself.
provenance_patterns=(
  'cogi''to'
  'Breathe''MCP'
  'Or''ca'
  'Her''dr'
  '/Users/''[A-Za-z0-9._-]+'
  '/home/''[A-Za-z0-9._-]+'
)

for f in "${files[@]:-}"; do
  [[ -z "$f" ]] && continue
  path_exists "$f" || continue
  for pat in "${provenance_patterns[@]}"; do
    hits=$(read_path "$f" | grep -EnIi "$pat" 2>/dev/null || true)
    # These exact strings are public publisher identity, not private implementation provenance.
    hits=$(printf '%s\n' "$hits" \
      | grep -vE 'github\.com/cogitod/open-why|github/license/cogitod/open-why|Copyright 2026 Cogito Agency' \
      || true)
    if [[ -n "$hits" ]]; then
      echo "[leak-check] private implementation provenance in $f:"
      echo "$hits" | sed 's/^/    /'
      problems=$((problems + 1))
    fi
  done
done

if [[ "$problems" -gt 0 ]]; then
  echo "[leak-check] $problems potential issue(s) found (mode=$mode)."
  exit 1
fi
echo "[leak-check] clean ($mode)"
