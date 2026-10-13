# Changelog

All notable changes to `local-git-branch-cleanup-tui` are recorded here. Starting after 0.2.0, each
release adds its entry automatically from conventional commit messages.

## 0.3.0 - 13-10-2026

### Features

- **tui:** Add an update command that installs the newest release

## 0.2.1 - 12-10-2026

### Bug fixes

- **tui:** Honour XDG_CACHE_HOME for the PR cache on macOS and Windows

### Build and dependencies

- **deps:** Move reqwest to 0.13 with the ring crypto provider

## 0.2.0

First version published as prebuilt binaries for Linux, macOS and Windows.

### Features

- Interactive TUI with keyboard navigation, checkbox selection, status filters and a details pane.
- Branch classification: merged, pr-merged, pr-diverged, gone, unmerged, local, protected and
  current.
- Safe delete by default (`git branch -d`); unmerged, local and pr-diverged branches need `--force`.
- Sorting by status, name, activity or creation date.
- Search by name or author (`@author:name`, `@author:me`) with autocomplete.
- Pull request status from GitHub (`--github`, uses the `gh` CLI) or Bitbucket Data Center
  (`--bitbucket`, uses `BITBUCKET_TOKEN`), cached in SQLite for one hour.
- CLI mode (`--cli`) and dry run (`--dry-run`).
