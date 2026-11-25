# Changelog

All notable changes to `local-git-branch-cleanup-tui` are recorded here. Starting after 0.2.0, each
release adds its entry automatically from conventional commit messages.

## 0.11.0 - 25-11-2026

### Features

- **tui:** Fetch and prune the remote when the TUI starts

### Bug fixes

- **tui:** Report a startup fetch error when r is pressed during it

## 0.10.0 - 25-11-2026

### Features

- **tui:** Make the layout responsive to the terminal size
- **tui:** Give the legend its own box below the list

### Bug fixes

- **tui:** Address review findings in the responsive layout
- **tui:** Address the second review of the responsive layout
- **tui:** Address the final review of the responsive layout

## 0.9.1 - 23-11-2026

### Bug fixes

- **deps:** Bump url to 2.5.9 for RUSTSEC-2026-0592

## 0.9.0 - 20-11-2026

### Features

- **tui:** Keep the cursor, selection and worktree state across reloads
- **tui:** Reload the branch list in the background with r
- **tui:** Reload automatically when branches change elsewhere
- **tui:** Check for branch changes when the terminal regains focus
- **tui:** Fetch the remote and re-fetch PR data with R
- **tui:** Check for branch changes every 10 s, set with --auto-refresh-interval
- **tui:** Run R's full sync on a timer with --auto-sync
- **tui:** Sync every 5 minutes by default
- **tui:** Show a failed automatic fetch once, not every sync

### Bug fixes

- **tui:** Keep the selection on the same branches when the list changes
- **tui:** Keep --trunk and the sort order when the branch list reloads
- **tui:** Keep the user's GIT_SSH program when R fetches
- **tui:** Read git fetch's stderr while it runs
- **tui:** Only a full sync clears a shown fetch error
- **tui:** Do not repeat failed PR lookups on every automatic sync
- **tui:** Stop retrying a failed reload every interval
- **tui:** Keep known PR info when a sync's PR lookup fails
- **tui:** Drop PR cache entries of branches deleted during a sync
- **tui:** Keep the branch view's cursor across reloads in the worktree view

### Performance

- **tui:** Leave ahead/behind counts out of the refs fingerprint

## 0.8.0 - 19-11-2026

### Features

- **tui:** Copy the branch name under the cursor with y
- **tui:** Show new files as N and group changes in the worktree details
- **tui:** Make the worktree view read-only until x turns removal on

### Bug fixes

- **tui:** Keep the selection when toggling force mode
- **tui:** Give the worktree table's branch column the spare width

## 0.7.0 - 19-11-2026

### Features

- **tui:** Add a worktree view behind --worktrees

### Bug fixes

- **tui:** Use ⑂ as the protected-wt icon

## 0.6.0 - 19-11-2026

### Features

- **tui:** Protect branches checked out in other worktrees

### Bug fixes

- **tui:** Use ⑂ as the protected-wt icon

## 0.5.0 - 19-11-2026

### Features

- **tui:** Drop a branch's cached PR entry when it is deleted

## 0.4.2 - 19-11-2026

### Bug fixes

- **deps:** Bump rustls, rustls-webpki and webpki-root-certs past security advisories
- **deps:** Move rusqlite to 0.43 for the bundled SQLite fix

## 0.4.1 - 13-10-2026

### Bug fixes

- **tui:** Check for updates through the releases feed
- **tui:** Find a GitHub token for update from GH_TOKEN, GITHUB_TOKEN or gh
- **tui:** Do not cache an update check cancelled at exit
- **tui:** Say when GitHub's rate limit resets after a failed update

## 0.4.0 - 13-10-2026

### Features

- **tui:** Show a notice when a newer release is available

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
