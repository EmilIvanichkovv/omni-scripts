#!/usr/bin/env bash

source "${BASH_SOURCE%/*}/utils/path.sh"
source "${BASH_SOURCE%/*}/utils/user-interaction.sh"
source "${BASH_SOURCE%/*}/utils/ansi.sh"

WIDTH=120

# Parse flags
CHRONOLOGICAL=false
for arg in "$@"; do
  if [ "$arg" == "--chronological" ]; then
    CHRONOLOGICAL=true
  fi
done

# Initialize an array to keep track of deleted branches
DELETED_BRANCHES=()

# Initilize an array to keep track of branches that do not have a remote counterpart
LOCAL_BRANCHES=()

# Get all local Git branches that either:
# - Do not have an upstream (remote tracking) branch, or
# - Have an upstream branch that no longer exists (e.g. deleted on the remote)
get_branches_with_no_remote_counterpart() {
  for branch in $(git for-each-ref --format='%(refname:short)' refs/heads/); do
    _upstream=$(git rev-parse --abbrev-ref --symbolic-full-name "$branch@{u}" 2>/dev/null)
    if [ $? -ne 0 ]; then
      last_epoch=$(git log -1 --format="%ct" "$branch")
      LOCAL_BRANCHES+=("$branch|$last_epoch")
    fi
  done
}

print_header () {
  HEADER="${BOLD}🧹 Local Git Branch Cleanup:x${RESET}"

  print_line "$TL_CORNER" $WIDTH "$TL_CORNER"
  print_boxed_centered_text "$HEADER" $WIDTH
  print_line "$L_MID" $WIDTH "$R_MID"
}

list_local_branches () {
  print_boxed_text " Scanning local git branches..." $WIDTH
  print_boxed_new_line $WIDTH

  get_branches_with_no_remote_counterpart

  if [ ${#LOCAL_BRANCHES[@]} -eq 0 ]; then
    print_boxed_text " No local branches without a remote counterpart." $WIDTH
    print_line "$BL_CORNER" $WIDTH "$BR_CORNER"
    exit 0
  fi

  if $CHRONOLOGICAL; then
    IFS=$'\n' LOCAL_BRANCHES=($(for item in "${LOCAL_BRANCHES[@]}"; do echo "$item"; done | sort -t '|' -k2 -n))
  fi

  print_boxed_text " Local branches without a remote counterpart:" $WIDTH
  print_boxed_new_line $WIDTH

  for entry in "${LOCAL_BRANCHES[@]}"; do
    branch="${entry%%|*}"
    last_epoch="${entry##*|}"
    last_update=$(date -d "@$last_epoch" "+%Y-%m-%d %H:%M")
    print_boxed_text " ${BOLD} • $branch${RESET} [Last update: ${UNDERLINE}$last_update${RESET}"] $WIDTH
  done
}


print_confirmation() {
  print_line "$L_MID" $WIDTH "$R_MID"
  print_boxed_text " These branches are not present on the remote." $WIDTH
  print_boxed_text " ${RED}${BOLD}Do you want to delete them locally?${RESET}" $WIDTH
  print_boxed_new_line $WIDTH
  print_boxed_text "  [y] Yes, delete them" $WIDTH
  print_boxed_text "  [n] No, do not delete them" $WIDTH
  print_line "$BL_CORNER" $WIDTH "$BR_CORNER"

  # Ask the user if they want to delete the local branches without a remote counterpart
  if ! confirm " Your choice: "; then
    print_line "$TL_CORNER" $WIDTH "$TR_CORNER"
    print_boxed_text " Operation cancelled. No branches were deleted." $WIDTH
    print_line "$BL_CORNER" $WIDTH "$BR_CORNER"

    exit 0
  fi
}

list_deleted_branches() {
  # Delete the local branches without a remote counterpart
  for entry in "${LOCAL_BRANCHES[@]}"; do
    branch="${entry%%|*}"
    git branch -D "$branch" 1>/dev/null
    DELETED_BRANCHES+=("$branch")
  done

  # Print the deleted branches
  print_line "$TL_CORNER" $WIDTH "$TR_CORNER"
  print_boxed_text " Deleting selected branches...  " $WIDTH
  print_line "$L_MID" $WIDTH "$R_MID"

  for branch in "${DELETED_BRANCHES[@]}"; do
    print_boxed_text "  ✓ Deleted: ${BOLD}$branch${RESET}" $WIDTH
  done
  print_line "$L_MID" $WIDTH "$R_MID"
  print_boxed_text " ${GREEN}Cleanup complete!${RESET}" $WIDTH
  print_line "$BL_CORNER" $WIDTH "$BR_CORNER"
}

# Main function
main() {
  print_header
  list_local_branches
  print_confirmation
  list_deleted_branches
}

main
