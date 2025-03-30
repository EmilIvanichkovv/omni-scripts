#!/usr/bin/env bash

# ANSI styles
export RESET="\e[0m"
export BOLD="\e[1m"
export DIM="\e[2m"
export GREEN="\e[32m"
export YELLOW="\e[33m"
export RED="\e[31m"
export BLUE="\e[34m"
export UNDERLINE="\e[4m"

# Box characters
export H_LINE="─"
export V_LINE="│"
export TL_CORNER="┌"
export TR_CORNER="┐"
export BL_CORNER="└"
export BR_CORNER="┘"
export T_MID="┬"
export B_MID="┴"
export L_MID="├"
export R_MID="┤"
export CROSS="┼"

# Function to draw a horizontal line
print_line() {
  printf "$1"
  printf '%.0s'"$H_LINE" $(seq 1 $2)
  printf "$3\n"
}

print_boxed_new_line() {
  local width="$1"

  # Print the top line of the box
  printf "%s" "$V_LINE"
  printf "%*s" $width ""
  printf "%s\n" "$V_LINE"
}

print_boxed_text() {
  local text="$1"
  local width="$2"

  # Strip ANSI escape sequences to calculate the visible length of the text
  local stripped_text=$(echo -e "$text" | sed -r 's/\x1B\[[0-9;]*[mK]//g')
  local visible_length=${#stripped_text}

  # Calculate padding
  local padding=$(( (width - visible_length)  )) # No need to subtract for V_LINE

  # Print the text with V_LINE at the beginning and end
  printf "%s" "$V_LINE"
  printf "%b" "$text"
  printf "%*s" $padding ""
  printf "%s\n" "$V_LINE"
}

print_boxed_centered_text() {
  local text="$1"
  local width="$2"

  # Strip ANSI escape sequences to calculate the visible length of the text
  local stripped_text=$(echo -e "$text" | sed -r 's/\x1B\[[0-9;]*[mK]//g')
  local visible_length=${#stripped_text}

  # Calculate padding
  local padding=$(( (width - visible_length - 1) / 2 )) # Subtract 1 for V_LINE

  # Print the text with V_LINE at the beginning and end
  printf "%s" "$V_LINE"
  printf "%*s" $padding ""
  printf "%b" "$text"
  printf "%*s" $padding ""
  printf "%s\n" "$V_LINE"
}

indent() {
  sed -u 's/^/  /'
}
