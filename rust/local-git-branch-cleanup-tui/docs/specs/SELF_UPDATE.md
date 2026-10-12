# Self-Update Specification

Status: Accepted (decisions in section 9). Target project: `rust/local-git-branch-cleanup-tui`.
Repository: `EmilIvanichkovv/omni-scripts`. Source snapshot reviewed: `main` @ `8f14b6b`,
2026-10-12. Builds on [Release & Distribution](RELEASE_DISTRIBUTION.md), which ships the binaries,
installers and install receipts this feature relies on.

## 1. Goals

1. **`local-git-branch-cleanup-tui update`** installs the newest release in place, on Linux, macOS
   and Windows, for anyone who installed with the one-line installer.
2. **`local-git-branch-cleanup-tui update --check`** only reports whether a newer release exists.
3. **A quiet "update available" notice** in normal use, so people learn about new versions without
   running a command. It never blocks, never slows startup and can be turned off.
4. **Correct behaviour for every install channel.** Copies installed with Cargo, Nix or a manual
   download are never overwritten; they get the right upgrade instructions instead.

Non-goals: downgrades, release channels beyond stable (see section 11), auto-installing without the
user asking, and package-manager installs (Homebrew, winget), which do not exist yet.

## 2. Current state

- **Upgrading today:** run the one-line installer again. Verified on 2026-10-12: a second run on the
  same version is safe. It downloads and replaces the binary (identical checksum), finds the
  `env.sh` and shell rc lines already in place and adds nothing, and rewrites the install receipt. A
  second run with a newer release would upgrade in place the same way.
- **Install receipt:** every installer run writes
  `~/.config/local-git-branch-cleanup-tui/local-git-branch-cleanup-tui-receipt.json`
  (`%LOCALAPPDATA%\local-git-branch-cleanup-tui\` on Windows), for example:

  ```json
  {
    "binaries": ["local-git-branch-cleanup-tui"],
    "install_prefix": "/home/user/.local/bin",
    "install_layout": "flat",
    "modify_path": true,
    "provider": { "source": "cargo-dist", "version": "0.33.0" },
    "source": {
      "app_name": "local-git-branch-cleanup-tui",
      "name": "omni-scripts",
      "owner": "EmilIvanichkovv",
      "release_type": "github"
    },
    "version": "0.2.0"
  }
  ```

  Copies installed with `cargo install`, `nix profile install` or a manual download have no receipt.

- **Relevant dependencies already in the binary:** `tokio`, and `reqwest` 0.12 with rustls and the
  `ring` crypto provider.
- **CLI:** `clap` derive, flags only, no subcommands. Running the binary with no arguments opens the
  TUI.

## 3. Research

### 3.1 Options

| Option                                                      | How it works                                                                                                                    | Pros                                                                                                         | Cons                                                                                                                         |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| **A. dist's standalone updater** (`install-updater = true`) | dist ships a second binary, `local-git-branch-cleanup-tui-update`, built from axoupdater.                                       | No code in the app. One config line.                                                                         | A second command with a long name. Cannot power an in-app notice or `--check`. Doubles the installed files.                  |
| **B. axoupdater as a library** (recommended)                | The app calls the `axoupdater` crate: reads the receipt, finds the newest release, downloads and runs that release's installer. | Built and maintained by the dist authors for exactly these receipts. Handles the monorepo and Windows cases. | Pulls in `reqwest` 0.13; needs our own `reqwest` moved to 0.13 to avoid a second HTTP stack (section 3.3).                   |
| **C. Hand-written updater**                                 | Our own code: GitHub API request, receipt parsing, installer download and run, Windows self-replace.                            | No new dependencies.                                                                                         | Reimplements receipt handling, release lookup and Windows file locking that axoupdater already gets right. More to test.     |
| D. `self_update` crate                                      | Downloads the archive and swaps the binary itself.                                                                              | Popular.                                                                                                     | Ignores dist receipts and installers: install directory, PATH setup and the receipt can drift from what the installer wrote. |

### 3.2 What axoupdater does (version 0.10.2, released 2026-08-12, MIT/Apache-2.0)

Read from the crate source and checked with a prototype build against the real 0.2.0 release:

- **Receipt.** `load_receipt()` reads the receipt above. Without one it returns an error ("Unable to
  load receipt"), which is how we detect a non-installer copy.
  `check_receipt_is_for_this_executable()` compares the running binary's directory with the
  receipt's `install_prefix`, so a Cargo or Nix copy is not mistaken for the installer copy when
  both exist.
- **Finding the newest release in this monorepo.** It first asks GitHub for `releases/latest`. If
  that release has no `local-git-branch-cleanup-tui-installer*` asset (for example, when another
  tool in the repo published last), it lists all releases and keeps those whose tag parses as
  `local-git-branch-cleanup-tui-vX.Y.Z`. So it does not depend on this tool being the repo's latest
  release.
- **Prereleases** are skipped by default.
- **Updating** downloads the new release's `installer.sh` (`installer.ps1` on Windows) and runs it,
  forcing the install directory from the receipt and keeping `modify_path` as it was. On Windows it
  first renames the running `.exe` to `.previous.exe` (Windows cannot overwrite a running
  executable), restores it if the installer fails, and deletes it afterwards.
- **GitHub API:** unauthenticated by default (60 requests per hour per IP address). A token can be
  passed with `set_github_token()`.
- **Testing:** `LOCAL_GIT_BRANCH_CLEANUP_TUI_INSTALLER_GITHUB_BASE_URL` points it at another API
  server, so tests can use a local mock instead of GitHub.

Prototype results on Linux, with a probe calling `load_receipt`, `is_update_needed` and
`query_new_version`:

| Situation                                         | Result                                                      |
| ------------------------------------------------- | ----------------------------------------------------------- |
| Copy installed by the installer (receipt matches) | receipt for this binary: yes; newest 0.2.0; no update       |
| Same binary copied elsewhere (like Cargo/Nix)     | receipt for this binary: **no**; newest 0.2.0               |
| No receipt at all                                 | error "Unable to load receipt" (handled as "not installer") |

### 3.3 Binary size

Measured with the `dist` profile on Linux x86_64:

| Build                                                            | Size    | Change      |
| ---------------------------------------------------------------- | ------- | ----------- |
| Today (`reqwest` 0.12, `ring`)                                   | 9.1 MB  | -           |
| + axoupdater, our `reqwest` left on 0.12 (two HTTP stacks)       | 13.7 MB | **+4.6 MB** |
| Our `reqwest` on 0.13 (default `aws-lc-rs` provider), no updater | 11.4 MB | +2.3 MB     |
| Our `reqwest` on 0.13 (`aws-lc-rs`) + axoupdater                 | 11.9 MB | +2.8 MB     |

axoupdater itself costs about 0.5 MB. Most of the growth comes from running two `reqwest` versions,
or from `reqwest` 0.13's default crypto provider (`aws-lc-rs`), which is bigger than `ring` and
needs a C toolchain and CMake to build, a risk for the musl and cross builds. Hence the plan: move
to `reqwest` 0.13 **with the `ring` provider** first (section 7, phase 1). Target: the binary grows
by at most 1 MB.

## 4. User experience

### 4.1 `update`

```text
$ local-git-branch-cleanup-tui update
Checking for updates... 0.2.0 -> 0.3.0
<installer output>
Updated local-git-branch-cleanup-tui to 0.3.0.

$ local-git-branch-cleanup-tui update
local-git-branch-cleanup-tui 0.3.0 is the latest version.
```

Depending on how the running copy was installed:

| Install channel                         | Detected by                                         | `update` does                                                                                                    |
| --------------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| One-line installer                      | receipt exists and matches the running binary       | installs the newest release in place                                                                             |
| Installer copy exists, but not this one | receipt exists, `install_prefix` differs            | refuses: "This copy was not installed by the installer (it is in `<dir>`). The installer copy is in `<prefix>`." |
| Nix                                     | no matching receipt, binary path under `/nix/store` | prints the newest version and `nix profile upgrade ...` / "update your flake input"                              |
| Cargo                                   | no matching receipt, binary under `~/.cargo/bin`    | prints the newest version and the `cargo install` command from the README                                        |
| Manual download or anything else        | no matching receipt                                 | prints the newest version, the release page link and the one-line installer command                              |

Flags:

- `--check`: report only, change nothing. Exit code 0 when up to date, **10** when an update is
  available, 1 on error, so scripts can use it.
- `--yes` is not needed: running `update` is the confirmation.

### 4.2 The "update available" notice

- At most **once every 24 hours**, the app checks for a newer release **in the background** while it
  runs. The result is cached; every later run in that window reads the cache and makes no request.
- The request has a **2 second timeout**. Failures (offline, rate limit, proxy) are silent, and the
  next check happens after the next 24 hours.
- **TUI:** one line in the status bar, for example
  `Update available: 0.3.0 (run: local-git-branch-cleanup-tui update)`.
- **CLI mode (`--cli`):** the same line on stderr after the command finishes, never mixed into
  stdout.
- **Never checks when:** `LOCAL_GIT_BRANCH_CLEANUP_TUI_NO_UPDATE_CHECK=1`, `--no-update-check`, the
  `CI` environment variable is set, or stdout is not a terminal.
- **Cache:** next to the existing PR cache (`dirs::cache_dir()/local-git-branch-cleanup-tui/`), as a
  small JSON file: `{ "checked_at": ..., "latest": "0.3.0" }`. It never stores anything about the
  user's repositories.

### 4.3 What is sent

One HTTPS request to `api.github.com` per 24 hours (and on every `update` or `update --check`),
containing the app version in the `User-Agent` header. No identifiers, no repository data. Users can
set `LOCAL_GIT_BRANCH_CLEANUP_TUI_GITHUB_TOKEN` to authenticate if they hit the rate limit behind a
shared IP address. Document this in the README.

## 5. Design

### 5.1 CLI shape

Add an optional subcommand to the existing `Args`; running with no subcommand stays the TUI or
`--cli` as today:

```rust
#[derive(Parser, Debug)]
#[command(args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Do not check for new versions
    #[arg(long, global = true)]
    no_update_check: bool,
    // ... existing flags unchanged
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Update to the newest release
    Update {
        /// Only report whether an update is available
        #[arg(long)]
        check: bool,
    },
}
```

`update` runs before the "are we in a git repository" check, so it works from any directory.

### 5.2 Module

A new `src/update.rs`, independent of the TUI:

- `enum InstallChannel { Installer, OtherInstallerCopy { prefix }, Nix, Cargo, Unknown }`, from the
  receipt check plus the binary path (section 4.1).
- `async fn latest_version() -> Result<Version>`: axoupdater `query_new_version`, or the GitHub
  releases lookup for channels without a receipt (axoupdater needs a receipt for its query, so for
  other channels use `set_release_source` with the repository coordinates).
- `async fn run_update(check_only: bool) -> Result<ExitCode>`.
- `fn cached_notice() -> Option<String>` and `spawn_background_check()` for section 4.2.

The background check runs as a `tokio::spawn` task with a timeout; the TUI polls its result each
frame, so the UI never waits on the network.

### 5.3 Rename compatibility

The planned rename (Release & Distribution, section 16) changes the app name, which axoupdater uses
to find the receipt and match release tags. A copy of the old name cannot find releases with the new
name. Before the rename, decide one of:

1. Release a last old-name version whose `update` knows the new name: it installs the new name's
   installer and tells the user the command changed.
2. Accept a one-time manual reinstall and announce it in the release notes.

Option 1 is better for users; it is a small, temporary branch in `update.rs`.

## 6. Testing

- **Unit tests (`update.rs`):** channel detection from fake receipts and binary paths; cache read,
  write and expiry; disabled conditions (env variable, flag, `CI`, non-terminal).
- **Integration tests:** run axoupdater against a local mock GitHub API (via
  `LOCAL_GIT_BRANCH_CLEANUP_TUI_INSTALLER_GITHUB_BASE_URL`) serving a fake newer release, and check
  `update --check` exit codes 0 and 10 and the messages. No real network in tests.
- **Smoke tests (`smoke-test.yml`):** after installing the new release, run `update --check` and
  expect "latest version" (exit 0). Once two releases exist, add a job that installs the previous
  release with its tag-pinned installer, runs `update`, and checks `--version` shows the new one; on
  Windows this exercises the `.previous.exe` rename.
- **Manual check before merging the implementation:** Linux, macOS (Apple Silicon) and Windows, each
  updating a real older install with the one-line installer.

## 7. Implementation phases

Each phase is its own PR with atomic commits.

1. **`reqwest` 0.13 with `ring`.** Move the workspace `reqwest` to 0.13 with
   `default-features = false` and `rustls-no-provider` (or the equivalent `ring` feature), install
   the `ring` provider at startup, keep native root certificates and proxy support. No behaviour
   change; CI must build all five dist targets (temporarily set `pr-run-mode = "upload"`, as in
   Release & Distribution phase C). Commit type `build(deps)`: per the release rules this does not
   release by itself.
2. **`update` and `update --check`.** `update.rs`, the subcommand, channel detection, unit and
   integration tests, README section "Updating". Commit type `feat`: releases a minor version.
3. **Background notice.** Cache, background task, TUI status line, CLI stderr line, opt-outs, README
   privacy note. Commit type `feat`.
4. **Smoke tests.** The `update --check` step right away; the "update from the previous release" job
   once two releases exist with the command.

## 8. Acceptance criteria

1. With the one-line installer copy of an older version, `update` installs the newest release in
   place on Linux, macOS and Windows; `--version` shows it afterwards; the shell setup is unchanged.
2. `update --check` exits 0 when up to date and 10 when an update is available, and changes nothing.
3. Cargo, Nix and manual copies are never modified; `update` prints the right instructions for each.
4. The background check makes at most one request per 24 hours, never delays startup or the UI,
   prints nothing on failure, and is off with the env variable, the flag, `CI` or a non-terminal
   stdout.
5. The dist binary grows by at most 1 MB.
6. All five dist targets still build.

## 9. Decisions log

1. ~~Notice on by default?~~ **Decided 2026-10-12: yes**, with the 24-hour cache and the opt-outs in
   section 4.2, like `gh` and `npm`.
2. ~~Command form?~~ **Decided 2026-10-12: an `update` subcommand**, with `update --check` to only
   report (section 5.1), as `rustup update` and `uv self update`.
3. ~~Exit code for "update available"?~~ **Decided 2026-10-12: 10** (section 4.1).

## 10. Risks

| Risk                                                                          | Mitigation                                                                                                                      |
| ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `reqwest` 0.13 migration breaks GitHub/Bitbucket PR fetching                  | Phase 1 is separate with no behaviour change; existing tests plus a manual run with `--github` and `--bitbucket`.               |
| `ring` provider not supported the way we need in `reqwest` 0.13               | Fall back to `aws-lc-rs` only if the five targets build, and accept the size growth in the acceptance criteria.                 |
| GitHub API rate limit (60 per hour per IP) in offices sharing an IP           | 24-hour cache; silent failure; optional token variable.                                                                         |
| A broken release is installed by `update`                                     | Smoke tests run on every release before people update; the user can reinstall a specific version with its tag-pinned installer. |
| macOS users with a broken `~/.profile` symlink see installer errors on update | Same as the first install: harmless, documented in the README. axoupdater keeps `modify_path` from the receipt.                 |
| The rename strands old installs                                               | Section 5.3.                                                                                                                    |

## 11. Follow-ups (not in this spec)

1. `update --version X.Y.Z` to install a specific version (axoupdater supports it).
2. `update --prerelease` to opt into release candidates.
3. Show what changed: print the new release's changelog entry after updating.
4. Homebrew and winget packages, whose update commands replace this one for those users.
