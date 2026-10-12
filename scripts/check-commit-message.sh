#!/usr/bin/env bash
# Check that a commit message subject follows Conventional Commits. Release versions are
# computed from these subjects, so CI rejects PR commits that do not match.
#
# Usage: check-commit-message.sh [--local] <message-file>
#   --local  also accept subjects git writes during local work (merge, fixup!, squash!, amend!).
#            The commit-msg hook passes it; CI does not, so such commits cannot be merged.
set -euo pipefail

local_mode=false
if [[ "${1:-}" == "--local" ]]; then
    local_mode=true
    shift
fi

# First line that is not a git comment.
subject=$(grep -v '^#' "$1" | head -n 1 || true)

types='feat|fix|perf|refactor|build|ci|docs|test|chore|style|revert'
pattern="^($types)(\([a-z0-9-]+\))?!?: [^ ].*"

if [[ "$subject" =~ $pattern ]]; then
    exit 0
fi
if $local_mode && [[ "$subject" =~ ^(Merge\ |fixup!\ |squash!\ |amend!\ ) ]]; then
    exit 0
fi

cat >&2 <<MSG
Commit subject does not follow Conventional Commits:

  $subject

Expected: <type>(<optional-scope>)<optional !>: <description>
Types:    ${types//|/, }
Example:  fix(tui): Keep search cursor on char boundaries
MSG
exit 1
