#!/usr/bin/env bash
# Create only synthetic demo data in a new directory, separate from the normal store.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo 'usage: bash examples/quickstart.sh <new-directory> <codex|claude-code|generic>' >&2
  exit 2
fi
case "$2" in
  codex) config_name=mcp.toml ;;
  claude-code|generic) config_name=mcp.json ;;
  *) echo 'client must be codex, claude-code, or generic' >&2; exit 2 ;;
esac

why_bin="${OPEN_WHY_BIN:-why}"
command -v "$why_bin" >/dev/null || {
  echo 'Install why first, or set OPEN_WHY_BIN to its absolute executable path.' >&2
  exit 1
}
command -v git >/dev/null || { echo 'Git is required.' >&2; exit 1; }
destination="$1"
if [[ -e "$destination" || -L "$destination" ]]; then
  echo 'Destination already exists. Choose a new directory; no files were changed.' >&2
  exit 1
fi
# Resolve trusted parent aliases before handing a path to the store's symlink protections.
parent="$(cd -- "$(dirname -- "$destination")" && pwd -P)"
demo_dir="$parent/$(basename -- "$destination")"
umask 077
mkdir -- "$demo_dir"
repo="$demo_dir/repository"
mkdir -- "$repo"
# A caller may launch this from a Git hook or another worktree. Do not inherit
# Git's alternate index/worktree locations into the synthetic repository.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
git -C "$repo" -c init.defaultBranch=main init --quiet --template=
cat > "$repo/README.md" <<'EOF'
# Offline notebook example

This synthetic application uses SQLite because it runs on one laptop, must work
offline, and should require no database service. It has one writer. PostgreSQL
was considered but would add an unnecessary service to this deployment.

Reconsider the choice if the application needs writes from multiple machines.
No encryption algorithm has been selected or documented.
EOF
git -C "$repo" add -- README.md
GIT_AUTHOR_NAME='Example Developer' GIT_AUTHOR_EMAIL='developer@example.invalid' \
GIT_COMMITTER_NAME='Example Developer' GIT_COMMITTER_EMAIL='developer@example.invalid' \
git -C "$repo" -c user.name='Example Developer' -c user.email='developer@example.invalid' \
  -c commit.gpgSign=false -c core.hooksPath=/dev/null commit --quiet \
  -m 'Choose SQLite for the offline notebook' \
  -m 'SQLite supports offline use on one laptop with one writer and no database service. PostgreSQL adds an unnecessary service. Reconsider when multiple machines need to write.'

env -u OPEN_WHY_STORE_INSTANCE_ID "$why_bin" setup \
  --db "$demo_dir/open-why.db" --client "$2" > "$demo_dir/$config_name"

cat <<EOF
Demo created: $demo_dir
Configuration snippet: $demo_dir/$config_name

Merge the snippet into your client's configuration, reconnect, and check its MCP status.
Keep this demo configuration separate from an existing open-why connection.

Paste into a fresh client conversation:

Use open-why_ask to answer: why did we choose SQLite for the offline notebook?
Repository: $repo
Then call open-why_get with a returned ID and the same scope. Cite the record ID,
the recorded reason, and the commit evidence. Do not edit this repository.

Expected: offline use on one laptop, one writer, and no database service; a
record ID and the original commit should support the explanation.

Next, ask which encryption algorithm was chosen. The records do not establish
one; the agent should say unknown rather than invent an explanation.

Optional supersession exercise (write only to this demo scope):
Scope: $repo
Use open-why_capture to save a decision titled "Demo storage choice" recording
the SQLite rationale above. Save its returned ID. For this synthetic exercise,
the requirements now include concurrent writes from multiple machines. Capture
a replacement decision titled "Demo storage choice" choosing PostgreSQL for
that requirement, setting supersedes to the first captured ID. Call open-why_get
on the first ID: it should resolve to the replacement. Call open-why_history
from the first ID to inspect both records. These are demo assertions, not test results.

Cleanup: remove the demo entry from your client, stop its connection, then delete
only this demo directory: $demo_dir
EOF
