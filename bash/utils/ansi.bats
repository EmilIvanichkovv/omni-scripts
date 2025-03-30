#!/usr/bin/env bats

load './ansi.sh'

@test "ansi.sh is loaded" {
  [ -n "$RESET" ]
  [ -n "$BOLD" ]
  [ -n "$DIM" ]
  [ -n "$DIM" ]
  [ -n "$GREEN" ]
  [ -n "$YELLOW" ]
  [ -n "$RED" ]
  [ -n "$BLUE" ]
  [ -n "$UNDERLINE" ]
  [ -n "$H_LINE" ]
  [ -n "$V_LINE" ]
  [ -n "$TL_CORNER" ]
  [ -n "$TR_CORNER" ]
  [ -n "$BL_CORNER" ]
  [ -n "$BR_CORNER" ]
  [ -n "$T_MID" ]
  [ -n "$B_MID" ]
  [ -n "$L_MID" ]
  [ -n "$R_MID" ]
  [ -n "$CROSS" ]
  [ -n "$(type -t print_line)" ]
  [ -n "$(type -t print_boxed_new_line)" ]
  [ -n "$(type -t print_boxed_text)" ]
  [ -n "$(type -t print_boxed_centered_text)" ]
}

@test "ansi.sh variables are set" {
  [ "$RESET" == "\e[0m" ]
  [ "$BOLD" == "\e[1m" ]
  [ "$DIM" == "\e[2m" ]
  [ "$GREEN" == "\e[32m" ]
  [ "$YELLOW" == "\e[33m" ]
  [ "$RED" == "\e[31m" ]
  [ "$BLUE" == "\e[34m" ]
  [ "$UNDERLINE" == "\e[4m" ]
}

@test "ansi.sh box characters are set" {
  [ "$H_LINE" == "─" ]
  [ "$V_LINE" == "│" ]
  [ "$TL_CORNER" == "┌" ]
  [ "$TR_CORNER" == "┐" ]
  [ "$BL_CORNER" == "└" ]
  [ "$BR_CORNER" == "┘" ]
  [ "$T_MID" == "┬" ]
  [ "$B_MID" == "┴" ]
  [ "$L_MID" == "├" ]
  [ "$R_MID" == "┤" ]
  [ "$CROSS" == "┼" ]
}

@test "print_line function works" {
  run print_line "test" 10 "test"
  [ "$status" -eq 0 ]

  result=$(print_line "test" 10 "test")
  [ "$result" == "test──────────test" ]
}

@test "print_boxed_new_line function works" {
  run print_boxed_new_line 10
  [ "$status" -eq 0 ]

  result=$(print_boxed_new_line 10)
  [ "$result" == "│          │" ]
}

@test "print_boxed_text function works" {
  run print_boxed_text "test" 10
  [ "$status" -eq 0 ]

  result=$(print_boxed_text "test" 10)
  [ "$result" == "│test      │" ]
}

@test "print_boxed_centered_text function works" {
  run print_boxed_centered_text "test" 10
  [ "$status" -eq 0 ]

  result=$(print_boxed_centered_text "test" 10)
  [ "$result" == "│  test  │" ]
}
