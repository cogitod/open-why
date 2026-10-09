#!/usr/bin/env bash
# Local convenience guard; GitHub branch protection is the authoritative gate.
set -euo pipefail

if ! branch="$(git symbolic-ref --quiet --short HEAD)"; then
  echo '[branch] create a topic branch before committing (git switch -c fix/description).' >&2
  exit 1
fi

if [[ "$branch" == main ]]; then
  echo '[branch] commits on main are blocked; use git switch -c fix/description and open a PR.' >&2
  exit 1
fi
