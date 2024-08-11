#!/usr/bin/env bash

source "${BASH_SOURCE%/*}/utils/path.sh"
source "${BASH_SOURCE%/*}/utils/user-interaction.sh"

# Initialize an array to keep track of deleted branches
DELETED_BRANCHES=()

# Initilize an array to keep track of branches that do not have a remote counterpart
LOCAL_BRANCHES=()

# Get all local branches
ALL_BRANCHES=$(git branch --format='%(refname:short)')

for branch in $ALL_BRANCHES; do
  # Check if the branch has a remote counterpart
  upstream=$(git rev-parse --symbolic-full-name "$branch"@{u} 2>/dev/null)
  if [ -z "$upstream" ]; then
    # If the branch does not have a remote counterpart, add it to the LOCAL_BRANCHES array
    LOCAL_BRANCHES+=("$branch")
  fi
done

# If there are no local branches without a remote counterpart, exit
if [ ${#LOCAL_BRANCHES[@]} -eq 0 ]; then
  echo "No local branches without a remote counterpart."
  exit 0
fi

# Print the local branches without a remote counterpart
echo "Local branches without a remote counterpart:"
for branch in "${LOCAL_BRANCHES[@]}"; do
  echo "  $branch"
done

# Ask the user if they want to delete the local branches without a remote counterpart
if ! confirm "Are you sure you want to delete these branches?"; then
  echo "Aborting."
  exit 0
fi

# Delete the local branches without a remote counterpart
for branch in "${LOCAL_BRANCHES[@]}"; do
  git branch -D "$branch"
  DELETED_BRANCHES+=("$branch")
done

# Print the deleted branches
echo "Deleted branches:"
for branch in "${DELETED_BRANCHES[@]}"; do
  echo "  $branch"
done
