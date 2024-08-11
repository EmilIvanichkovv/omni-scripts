#!/usr/bin/env bats

load './user-interaction.sh'

@test "'confirm' returns true when user inputs 'y'" {
  run confirm "Do you confirm?" <<< "y"
  [ "$status" -eq 0 ]
}

@test "'confirm' returns true when user inputs 'yes'" {
  run confirm "Do you confirm?" <<< "yes"
  [ "$status" -eq 0 ]
}

@test "'confirm' returns true when user inputs 'Yes'" {
  run confirm "Do you confirm?" <<< "Yes"
  [ "$status" -eq 0 ]
}

@test "'confirm' returns true when user inputs 'YES'" {
  run confirm "Do you confirm?" <<< "YES"
  [ "$status" -eq 0 ]
}
@test "'confirm' returns false when user inputs 'n'" {
  run confirm "Do you confirm?" <<< "n"
  [ "$status" -eq 1 ]
}

@test "'confirm' returns false when user inputs 'no'" {
  run confirm "Do you confirm?" <<< "no"
  [ "$status" -eq 1 ]
}

@test "'confirm' returns false when user inputs 'No'" {
  run confirm "Do you confirm?" <<< "No"
  [ "$status" -eq 1 ]
}

@test "'confirm' returns false when user inputs 'NO'" {
  run confirm "Do you confirm?" <<< "NO"
  [ "$status" -eq 1 ]
}

@test "'confirm' returns false when user inputs 'foo'" {
  run confirm "Do you confirm?" <<< "foo"
  [ "$status" -eq 1 ]
}
