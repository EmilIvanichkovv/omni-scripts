#!/usr/bin/env bash

source "${BASH_SOURCE%/*}/utils/path.sh"
source "${BASH_SOURCE%/*}/utils/user-interaction.sh"
source "${BASH_SOURCE%/*}/utils/ansi.sh"

WIDTH=120

# Initialize an array to keep track of deleted branches
DELETED_BRANCHES=()

# Initilize an array to keep track of branches that do not have a remote counterpart
LOCAL_BRANCHES=()

# Get all local Git branches that either:
# - Do not have an upstream (remote tracking) branch, or
# - Have an upstream branch that no longer exists (e.g. deleted on the remote)
get_branches_with_no_remote_counterpart() {
  for branch in $(git for-each-ref --format='%(refname:short)' refs/heads/); do
    # Check if the branch has a remote counterpart
    _upstream=$(git rev-parse --abbrev-ref --symbolic-full-name "$branch@{u}" 2>/dev/null)
    if [ $? -ne 0 ]; then
      # If the branch does not have a remote counterpart, add it to the LOCAL_BRANCHES array
      LOCAL_BRANCHES+=("$branch")
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

  # If there are no local branches without a remote counterpart, exit
  if [ ${#LOCAL_BRANCHES[@]} -eq 0 ]; then
    print_boxed_text " No local branches without a remote counterpart." $WIDTH
    print_line "$BL_CORNER" $WIDTH "$BR_CORNER"

    exit 0
  fi

  # Print the local branches without a remote counterpart
  print_boxed_text " Local branches without a remote counterpart:" $WIDTH
  print_boxed_new_line $WIDTH

  for branch in "${LOCAL_BRANCHES[@]}"; do
    last_update=$(git log -1 --format="%cr" "$branch")
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
  for branch in "${LOCAL_BRANCHES[@]}"; do
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
