#!/usr/bin/env bats

load './path.sh'

@test "GIT_REPO_ROOT is correct" {
  [ "$GIT_REPO_ROOT" == "$(git rev-parse --show-toplevel)" ]
}

@test "ROOT is correct" {
  [ "$ROOT" == "$GIT_REPO_ROOT" ]
  [ "$ROOT" == "$(git rev-parse --show-toplevel)" ]
}
