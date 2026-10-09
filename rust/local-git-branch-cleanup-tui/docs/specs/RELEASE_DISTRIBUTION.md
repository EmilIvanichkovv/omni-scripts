# Cross-Platform Release and Distribution Specification

Status: Proposed implementation specification. Target project: `rust/local-git-branch-cleanup-tui`.
Repository: `EmilIvanichkovv/omni-scripts`. Source snapshot reviewed: `main` @ `0466bfb`,
2026-10-09.

This document supersedes the earlier draft "Cross-Platform GitHub Release Specification". It keeps
that draft's packaging decisions (`dist`, five targets, shell and PowerShell installers, checksums,
version consistency) and replaces its release trigger: **every merged PR that changes the
application publishes a new GitHub Release automatically.** The manual "tag the merged commit" flow
from the earlier draft is removed.

## 1. Goals

1. **One-line install** on Linux, macOS and Windows, with no Rust, Cargo, Nix or repository clone:

   ```bash
   # Linux / macOS
   curl --proto '=https' --tlsv1.2 -LsSf \
     https://github.com/EmilIvanichkovv/omni-scripts/releases/latest/download/local-git-branch-cleanup-tui-installer.sh | sh
   ```

   ```powershell
   # Windows (PowerShell)
   powershell -ExecutionPolicy Bypass -c "irm https://github.com/EmilIvanichkovv/omni-scripts/releases/latest/download/local-git-branch-cleanup-tui-installer.ps1 | iex"
   ```

2. **Release on every merge.** Each PR merged to `main` that touches the application with at least
   one releasable commit (section 6.2) produces a new version, a git tag, a changelog entry and a
   GitHub Release with prebuilt binaries, with no human step after the merge.
3. **Manual download stays first-class.** Every release carries plain archives plus SHA-256
   checksums, so users who refuse `curl | sh` can download, verify and run the binary themselves.

### Non-goals (initial implementation)

crates.io publishing, Homebrew tap, Scoop, WinGet, Chocolatey, APT/RPM repositories, Snap, Flatpak,
native Windows ARM64, self-updater, macOS notarization, Windows code signing, a shorter binary name.
All of these can be added later without changing the architecture (see section 14).

## 2. Research summary and tool choice

| Concern                         | Options considered                                                       | Choice                                    | Why                                                                                                                                                                                                                                                                              |
| ------------------------------- | ------------------------------------------------------------------------ | ----------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Build, package, installers      | `dist` (axodotdev), hand-written matrix + scripts, `cargo-binstall` only | **`dist`**                                | Generates the GitHub Actions release workflow, per-target archives, `installer.sh`/`installer.ps1`, checksums and a manifest from one config. Actively maintained (v0.33.0, 2026-09-10). This is how `uv`, `ruff` and many Rust CLIs ship their one-line installers.             |
| Version bump + changelog        | release-plz, release-please, semantic-release, git-cliff + small script  | **git-cliff + `scripts/release/` script** | release-plz and release-please work through a "release PR" that someone has to merge, which means two merges per change instead of a release per merge. git-cliff computes the next semver from conventional commits and renders the changelog, without opinions about the flow. |
| Pushing the bump commit and tag | `GITHUB_TOKEN`, personal access token, GitHub App installation token     | **GitHub App token**                      | Tags pushed with `GITHUB_TOKEN` do **not** trigger other workflows (GitHub docs), so `dist`'s tag-triggered release would never run. A GitHub App token is scoped to this repo, short-lived, and not tied to a person's account.                                                 |
| Linux libc                      | glibc (`-gnu`), musl (`-musl`)                                           | **musl**                                  | Statically linked; runs on any distribution regardless of glibc version, including Alpine.                                                                                                                                                                                       |

Rejected alternative for the token: switch `dist` to `dispatch-releases = true` and have the
auto-release job start it with `gh workflow run` (allowed with `GITHUB_TOKEN`, because
`workflow_dispatch` is an exception to the no-trigger rule). This avoids a secret but turns off
tag-push releases and is less common, so `dist`'s own docs and examples cover it less. It is the
fallback if creating a GitHub App is not possible (see section 6.4).

## 3. Current state (facts the implementer must account for)

- Rust workspace at `rust/`; release target is the `local-git-branch-cleanup-tui` package. Binary
  name: `local-git-branch-cleanup-tui` (`.exe` on Windows). `omni-lib` has no binaries and `dist`
  ignores it automatically.
- **Version drift:** the workspace says `0.1.0` (`rust/Cargo.toml`, inherited via
  `version.workspace = true`), `nix/pkgs/local-git-branch-cleanup/tui.nix` hard-codes `0.2.0`, the
  root `README.md` says "v0.2.0", and `tests/integration_test.rs` (`test_cli_version`) asserts
  `0.1.0`.
- **CI exists (merged in `8986e3f`), Linux only and Nix-based:**
  - `.github/workflows/ci.yml` runs on `pull_request`, `push: main` and `workflow_dispatch`. Jobs:
    `lint` (`pre-commit run --all-files`: cargo fmt, clippy `-D warnings`, prettier, markdownlint,
    yaml/toml checks), `test` (`cargo test --workspace --all-features` + bats), and `build`
    (`nix build` of both packages). Every job runs inside `nix develop` with the Cachix cache
    `git-branch-manager`, so CI uses the toolchain pinned in `flake.lock`.
  - `.github/workflows/audit.yml` runs `cargo audit` on Cargo changes and weekly.
  - `.github/dependabot.yml` updates `github-actions` (prefix `ci`) and `cargo` (prefix `build`)
    weekly. Actions are pinned by major tag (`actions/checkout@v7`), not by SHA.
  - Gaps for this project: nothing runs on macOS, Windows or Linux ARM, and nothing builds release
    artifacts.
- **No `LICENSE` file**, although Cargo metadata declares `MIT`.
- No Cargo `repository` field.
- The flake declares `systems = [ "x86_64-linux" ]` only. Nix stays a developer and Nix-user channel
  and is not part of this pipeline.
- PRs are merged with **rebase-merge only** (squash and merge commits are disabled). Every commit on
  a PR lands on `main` unchanged, so every commit message feeds the version calculation.
- `main` has no branch protection.
- History mixes gitmoji (`✨(tui): ...`) and conventional commits. Conventional commits are the
  convention from now on. Pre-baseline history is irrelevant, because version calculation only looks
  at commits after the last release tag.
- Runtime dependencies: `git` on `PATH` (required); `gh` only for `--github`; `BITBUCKET_TOKEN` only
  for `--bitbucket`. SQLite is bundled (`rusqlite` `bundled` feature), and the cache dir comes from
  `dirs`, so no system libraries are needed.
- The CLI already has `--version`, `--help`, `--cli` and `--dry-run`, which is enough for
  non-interactive smoke tests.

## 4. Platform contract

| OS      | Arch          | Target triple                | Native CI test runner      |
| ------- | ------------- | ---------------------------- | -------------------------- |
| Linux   | x86_64        | `x86_64-unknown-linux-musl`  | `ubuntu-latest`            |
| Linux   | aarch64       | `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm`         |
| macOS   | Intel         | `x86_64-apple-darwin`        | `macos-latest` via Rosetta |
| macOS   | Apple Silicon | `aarch64-apple-darwin`       | `macos-latest`             |
| Windows | x86_64        | `x86_64-pc-windows-msvc`     | `windows-latest`           |

Windows ARM64 is out of scope. On Windows ARM, the x64 build runs under emulation, but documentation
must not claim native support. If a musl target cannot build (most likely culprit: the bundled
SQLite C build when cross-compiling), fall back to the `-gnu` triple for that target, document the
minimum glibc, and do not substitute silently.

## 5. Architecture

```
PR opened ──► ci.yml (existing, Nix, ubuntu)
              ├─ lint: pre-commit (fmt, clippy, prettier, ...) + commit-message lint  [new step]
              ├─ test: cargo test + bats
              ├─ build: nix build
              └─ test-native (ubuntu-arm, macos, windows): cargo test   [new job]
             release.yml (dist, pr-run-mode = plan): dist plan          [new]

PR rebase-merged to main
      │
      ▼
auto-release.yml  (push: main, path-filtered, serialized)
  1. skip if HEAD is a release commit
  2. compute next version (git-cliff, commits since last tag, app paths only)
  3. bump Cargo.toml + Cargo.lock, prepend CHANGELOG.md
  4. commit "chore(release): local-git-branch-cleanup-tui v X.Y.Z"
  5. tag local-git-branch-cleanup-tui-vX.Y.Z
  6. push commit + tag with GitHub App token
      │
      ▼  (tag push triggers, because it was not pushed by GITHUB_TOKEN)
release.yml  (generated by dist)
  plan ─► build 5 targets ─► host (upload assets) ─► announce (GitHub Release)
      │
      ▼
smoke-test.yml (post-announce): run the real one-line installers on
  ubuntu, ubuntu-arm, macos, windows → --version, --help, --cli --dry-run in a temp repo
```

Invariant at every step: `git tag version == Cargo.toml version == --version output`.

## 6. Release-on-merge design

### 6.1 Trigger and scope

`.github/workflows/auto-release.yml`:

```yaml
on:
  push:
    branches: [main]
    paths:
      - "rust/local-git-branch-cleanup-tui/**"
      - "rust/Cargo.toml"
      - "rust/Cargo.lock"
      - "rust/dist-workspace.toml" # or wherever dist init puts config
  workflow_dispatch: # manual trigger, see section 6.6
    inputs:
      bump:
        type: choice
        options: [auto, patch, minor, major]
        default: auto
      prerelease:
        description: "Prerelease label (e.g. rc, beta). Empty = stable release."
        type: string
        default: ""
      dry-run:
        type: boolean
        default: false
concurrency:
  group: auto-release
  cancel-in-progress: false # never cancel a half-done release; queue instead
```

- Merges that only touch other scripts, Nix or repo-level docs do **not** release. Docs under
  `rust/local-git-branch-cleanup-tui/docs/` do match the filter; exclude them with a
  `!rust/local-git-branch-cleanup-tui/docs/**` path entry so a spec edit does not ship a binary.
- Dependabot merges touching `Cargo.lock` **do** release (as a patch). This is intentional, because
  the shipped binary changes.
- **Loop guard:** the job exits early when the head commit message starts with
  `chore(release): local-git-branch-cleanup-tui`. Do **not** use `[skip ci]` in the release commit:
  GitHub applies skip directives to push events, which risks suppressing the tag-triggered release
  workflow.
- When several commits arrive in one push (rebase-merge of a multi-commit PR), they produce **one**
  release covering all of them.

### 6.2 Version calculation

Run git-cliff (pinned version) scoped to the app:

```bash
git cliff --include-path 'rust/local-git-branch-cleanup-tui/**' \
          --include-path 'rust/Cargo.lock' \
          --tag-pattern '^local-git-branch-cleanup-tui-v[0-9]+\.[0-9]+\.[0-9]+$' \
          --bumped-version
```

Rules (configured in `cliff.toml` `[bump]`):

| Commits since last tag                             | Bump while `0.x` | Bump once `>= 1.0` |
| -------------------------------------------------- | ---------------- | ------------------ |
| any `feat!:`, `fix!:` or `BREAKING CHANGE:` footer | minor            | major              |
| any `feat:`                                        | minor            | minor              |
| any `fix`, `perf`, `refactor`, `build`, `revert`   | patch            | patch              |
| only `docs`, `test`, `chore`, `ci`, `style`        | **no release**   | **no release**     |

- **Releasable types** are `feat`, `fix`, `perf`, `refactor`, `build` and `revert`, plus any commit
  marked breaking. If the commits since the last stable tag contain none of these, the job logs "no
  releasable changes" and exits successfully without a commit, tag or release. Those commits are not
  lost: they are included in the next release's changelog range (the git-cliff template may hide
  `test`/`chore`/`ci`/`style` entries from the rendered notes).
- git-cliff's auto mode only bumps on `feat`/`fix`, so for `perf`, `refactor`, `build` and `revert`
  the script falls back to `git cliff --bump patch`.
- Dependabot `build(deps): ...` merges therefore ship a patch, because the binary changes.
  Dependabot `ci(deps): ...` merges do not release.
- `just release patch` (section 6.6) can still force a release when only non-releasable commits
  landed.
- **Bootstrap / manual override:** if no tag `local-git-branch-cleanup-tui-v<Cargo version>` exists
  yet, release the Cargo version **as is** and skip calculation. This makes the very first release
  `0.2.0` (set by hand in the implementation PR), and lets a maintainer force a version such as
  `1.0.0` by editing `Cargo.toml` in a PR.
- The tag pattern only matches **stable** tags, so prerelease tags (section 6.6) never become the
  base for the next calculation.
- Keep the logic in `scripts/release/next-version.sh` so it can be run and tested locally
  (`--dry-run` prints the computed version and changelog without writing).

### 6.3 Bump, commit, tag, push

The script, then the workflow:

1. Set `version = "X.Y.Z"` in `rust/local-git-branch-cleanup-tui/Cargo.toml` (`cargo set-version`
   from `cargo-edit`, or a strict `sed` on the `[package]` table), then run `cargo check` so
   `Cargo.lock` picks up the new version.
2. Prepend the rendered section to `rust/local-git-branch-cleanup-tui/CHANGELOG.md` (git-cliff
   `--prepend`).
3. Commit as the GitHub App's bot identity: `chore(release): local-git-branch-cleanup-tui vX.Y.Z`.
4. Annotated tag `local-git-branch-cleanup-tui-vX.Y.Z` on that commit. `dist` reads
   `PACKAGE-vVERSION` as a release of only that package.
5. `git push --atomic origin HEAD:main <tag>`. If the push is rejected because `main` moved, fetch,
   recompute from step 1 and retry (max 3). The concurrency group makes this rare.

The job checks out `main` with `fetch-depth: 0` and `fetch-tags: true` (it needs history to find the
previous tag), using the App token, so the push is made as the App.

### 6.4 Credentials

- Create a GitHub App ("omni-scripts-release") installed only on this repo with **Contents: read &
  write**. Store `RELEASE_APP_ID` and `RELEASE_APP_PRIVATE_KEY` as repo secrets; mint the token with
  `actions/create-github-app-token`.
- Only `auto-release.yml` gets the App token. `release.yml` keeps `dist`'s generated, minimal
  `GITHUB_TOKEN` permissions.
- If branch protection is added to `main` later, add the App to the bypass list. Otherwise the
  release commit push will be rejected.
- Fallback (no App possible): `dist-workspace.toml` `dispatch-releases = true`; the auto-release job
  pushes commit and tag with `GITHUB_TOKEN` (`contents: write`, `actions: write`) and then runs
  `gh workflow run release.yml -f tag=local-git-branch-cleanup-tui-vX.Y.Z`. Verify against the
  pinned `dist` version before choosing this.

### 6.5 Commit message discipline

Because rebase-merge puts every PR commit on `main`, release quality depends on commit messages:

- A new step in the existing `ci.yml` `lint` job (PRs only) lints every commit in the PR range
  (`base..head`) against
  `^(feat|fix|perf|refactor|build|ci|docs|test|chore|style|revert)(\([a-z0-9-]+\))?!?: .+`. Fail
  with a message naming the offending commit.
- Add the same check as a `commit-msg` hook through `git-hooks.nix` so it fails locally first.
- Dependabot is configured with `prefix: build` / `prefix: ci` and `include: scope`, producing
  `build(deps): ...` and `ci(deps): ...`, which pass.

### 6.6 Manual releases and experiments (`just` recipes)

Automatic release-on-merge is the default path, but a maintainer must be able to cut or rehearse a
release by hand, especially while experimenting with the pipeline itself. Three levels, from no side
effects to a real release:

| Recipe                              | Where it runs             | Side effects                                                                   | Use it for                                                                                 |
| ----------------------------------- | ------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| `just release-plan [bump]`          | Local                     | None                                                                           | See the next version, the changelog section and `dist plan` output                         |
| `just release-build`                | Local                     | Writes archives to `rust/target/distrib/` only                                 | Build and inspect the host-platform archive (`dist build`); run the binary from it         |
| `just release-pre [label] [ref]`    | GitHub Actions (dispatch) | Publishes a **prerelease** `X.Y.Z-<label>.N` from any branch; `main` untouched | Try the full pipeline (builds, installers, smoke tests) on a feature branch before merging |
| `just release-pre-delete <version>` | Local (`gh`)              | Deletes that prerelease and its tag                                            | Clean up after an experiment                                                               |
| `just release [bump]`               | GitHub Actions (dispatch) | Real stable release from `main`, same as a merge would produce                 | Release after merges the path filter skipped, or force a `minor`/`major` bump              |
| `just release-status`               | Local (`gh`)              | None                                                                           | Show the latest `auto-release` and `release` runs and the latest tags                      |

Recipe sketch (`Justfile`):

```just
app := "local-git-branch-cleanup-tui"

# Show next version, changelog section and dist plan (no side effects)
release-plan bump="auto":
    scripts/release/next-version.sh --dry-run --bump {{bump}}
    cd rust && dist plan

# Build the host-platform release archive locally into rust/target/distrib/
release-build:
    cd rust && dist build --artifacts=local

# Publish a prerelease from a branch (default: current branch), e.g. `just release-pre rc`
release-pre label="rc" ref=`git branch --show-current`:
    gh workflow run auto-release.yml --ref {{ref}} -f prerelease={{label}}
    @echo "Follow with: just release-status"

# Delete a prerelease and its tag, e.g. `just release-pre-delete 0.3.0-rc.1`
release-pre-delete version:
    @echo "{{version}}" | grep -q -- '-' || (echo "refusing: {{version}} is not a prerelease" && exit 1)
    gh release delete "{{app}}-v{{version}}" --cleanup-tag --yes

# Cut a stable release from main now (bump: auto|patch|minor|major)
release bump="auto":
    gh workflow run auto-release.yml --ref main -f bump={{bump}}

release-status:
    gh run list --workflow auto-release.yml --limit 3
    gh run list --workflow release.yml --limit 3
    git ls-remote --tags origin '{{app}}-v*' | tail -5
```

Workflow behavior for `workflow_dispatch`:

- **`bump`** overrides the calculated bump (`auto` = section 6.2 rules). The bootstrap rule still
  wins if the Cargo version has no tag yet.
- **`dry-run: true`** runs the calculation and prints the version and changelog, then stops before
  committing. This is the CI equivalent of `just release-plan`.
- **Stable releases (`prerelease` empty) are refused unless `github.ref == refs/heads/main`.** A
  stable release from a feature branch would put a tag on a commit `main` never sees.
- **Prereleases** (`prerelease` set):
  - Version = next calculated version + `-<label>.N`, where `N` is one more than the highest
    existing `<app>-vX.Y.Z-<label>.*` tag (first one is `.1`).
  - The bump commit (Cargo version + changelog) is created on a **detached** commit on top of the
    chosen ref. Only the tag is pushed (`git push origin <tag>`). Neither `main` nor the branch is
    updated, so experiments leave no trace in history.
  - `dist` sees the semver prerelease suffix and publishes a GitHub **prerelease**. GitHub never
    marks prereleases as "latest", so the public one-liners (`/releases/latest/...`) are not
    affected.
  - Install a prerelease with the tag-pinned installer URL:
    `curl -LsSf https://github.com/EmilIvanichkovv/omni-scripts/releases/download/local-git-branch-cleanup-tui-v0.3.0-rc.1/local-git-branch-cleanup-tui-installer.sh | sh`
    (PowerShell: same path with `installer.ps1`).
  - Smoke tests run for prereleases too, but a failure only fails the run; it does not open an
    issue.
  - The immutability rule (section 9) applies to stable releases only. Prereleases may be deleted
    with `just release-pre-delete`.
- Manual dispatch shares the `auto-release` concurrency group, so it queues behind an in-flight
  merge release instead of racing it.
- `just release-build` needs `dist` locally. Add the pinned `dist` (and `git-cliff`) to the Nix dev
  shell (`nix/shells/default.nix`) so the recipes work inside `nix develop` like the existing ones.
  `dist build` builds only the host target; cross targets are covered by `release-pre`.

## 7. dist configuration

Run `dist init` from `rust/` with the pinned version (`0.33.0` at time of writing; pin whatever is
current at implementation time) and accept its generated syntax. The **effective** configuration
must be:

```toml
# rust/dist-workspace.toml (shape only; let dist init write the real file)
[dist]
cargo-dist-version = "0.33.0"
ci = "github"
installers = ["shell", "powershell"]
targets = [
  "x86_64-unknown-linux-musl",
  "aarch64-unknown-linux-musl",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc",
]
checksum = "sha256"
pr-run-mode = "plan"
install-path = ["$XDG_BIN_HOME/", "$XDG_DATA_HOME/../bin", "~/.local/bin"]
install-updater = false
include = ["README.md", "LICENSE"]          # resolved relative to the package
post-announce-jobs = ["./smoke-test"]
# Optional, after baseline works:
# github-attestations = true
# github-build-setup = "..."                # only if musl cross needs extra toolchain setup
# github-custom-runners.aarch64-unknown-linux-musl = "ubuntu-24.04-arm"
```

Notes:

- **`install-path`**: `dist` defaults to `$CARGO_HOME/bin`, which makes no sense for users who do
  not have Rust. The XDG cascade above is what `uv` uses: `~/.local/bin` on Linux and macOS,
  `%USERPROFILE%\.local\bin` on Windows. The installers add it to `PATH` and print a hint to restart
  the shell.
- **`include`**: ship `README.md` and `LICENSE` inside each archive. Nothing else (no source, no
  `target/`, no test binaries).
- `dist plan` fails CI if `release.yml` drifts from what the config would generate. Never hand-edit
  `release.yml`; change config and rerun `dist init` / `dist generate`.
- **Dependabot conflict:** the existing `github-actions` Dependabot entry will open PRs that bump
  action versions inside the generated `release.yml`, which then fails `dist plan`. Exclude the file
  from Dependabot (`exclude-paths: [".github/workflows/release.yml"]` on the `github-actions` entry;
  verify the option is supported when implementing). Upgrade `dist` itself instead to get newer
  actions. Use `allow-dirty = ["ci"]` only if excluding the file is impossible, because it stops
  `dist` from regenerating the workflow.
- Archive names stay `dist` defaults, e.g.
  `local-git-branch-cleanup-tui-x86_64-unknown-linux-musl.tar.xz`,
  `local-git-branch-cleanup-tui-x86_64-pc-windows-msvc.zip`, each with a `.sha256` file, plus
  `local-git-branch-cleanup-tui-installer.sh`, `local-git-branch-cleanup-tui-installer.ps1`,
  `sha256.sum` and `dist-manifest.json`.
- Release notes: `dist` takes the body from the matching `CHANGELOG.md` section, so the auto-release
  job's changelog entry becomes the GitHub Release text. Append a fixed footer (requirements: Git
  required; `gh` only for `--github`; `BITBUCKET_TOKEN` only for `--bitbucket`) through the
  git-cliff template.

### 7.1 The `releases/latest` caveat (monorepo)

The one-liner uses `/releases/latest/download/...`. GitHub's "latest" is the most recent
non-prerelease release **in the whole repository**. While this TUI is the only thing released here,
that works. When a second tool starts publishing releases, either:

- publish the other tool's releases with `make_latest: false`, or
- change the documented one-liner to a version-pinned URL that the release workflow rewrites in the
  README, or
- move the tool to its own repository.

Record this as a constraint in the README's maintainer section.

## 8. CI: extend the existing `ci.yml`

`ci.yml` already covers formatting, clippy, tests and the Nix build on Linux. Keep it as it is and
add only what release-on-merge needs:

1. **`test-native` job (new)**: matrix `ubuntu-24.04-arm`, `macos-latest`, `windows-latest`. Runs
   `cargo test --workspace --all-features` (configure `git` `user.name`/`user.email` first; the
   tests create repos).
   - These runners **cannot use the Nix dev shell**: the flake only declares `x86_64-linux`, and Nix
     does not run natively on Windows. Use `dtolnay/rust-toolchain` with the same Rust version as
     `flake.lock` (read it from `nix eval` in the existing jobs, or pin it in a
     `rust-toolchain.toml` that the flake also reads, so the two cannot drift) plus
     `Swatinem/rust-cache`.
   - Optional later: add `aarch64-linux` and `aarch64-darwin` to the flake `systems` and run those
     two through Nix like the other jobs. Windows stays on the plain toolchain regardless.
   - Same timeouts, `permissions: contents: read` and major-tag action pinning as the existing jobs.
2. **Commit-message lint step (new)** in the `lint` job, PR events only (section 6.5).
3. **`dist plan`** comes from the generated `release.yml` (`pr-run-mode = "plan"`), not from
   `ci.yml`.
4. Make `lint`, `test`, `build` and `test-native` required checks once branch protection is turned
   on.

- Windows will likely expose path, line-ending or shell assumptions in tests. Fix the code or tests;
  do not mark tests `#[cfg(unix)]` to get a green matrix unless the behavior really is Unix-only.
- `audit.yml` stays unchanged. A failing audit does not block the auto-release (it is a separate
  workflow); fix forward.
- The release commit pushed by the GitHub App to `main` triggers `ci.yml` (`push: main`) like any
  other commit. That is fine and adds a post-release check of the bumped tree.

## 9. Smoke tests (`.github/workflows/smoke-test.yml`)

Invoked by `dist` as a `post-announce-jobs` reusable workflow (receives the `plan` JSON). Matrix:
`ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest` (arm64, plus the x86_64 binary via
`arch -x86_64`), `windows-latest`.

Per runner:

1. Run the **documented one-liner** (pinned to this release's tag URL rather than `latest`, to avoid
   racing a newer release).
2. `local-git-branch-cleanup-tui --version` must equal the tag version.
3. `--help` must succeed.
4. In a temp dir: `git init`, commit, create branch `smoke`, run `--cli --dry-run`, assert exit 0
   and that `smoke` still exists.
5. Manual-download path on one runner: fetch an archive and its `.sha256`, verify, extract, run
   `--version`.

On failure, the workflow opens a GitHub issue titled `Release vX.Y.Z smoke test failed (<os>)`.
Releases are **immutable**: never replace assets of a published version. Fix forward with the next
merge, which ships `X.Y.Z+1`.

## 10. Repository changes

| Path                                                          | Change                                                                                              |
| ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `rust/local-git-branch-cleanup-tui/Cargo.toml`                | `version = "0.2.0"` (explicit, not workspace-inherited); add `repository`, `homepage`, `readme`     |
| `rust/local-git-branch-cleanup-tui/tests/integration_test.rs` | `test_cli_version` asserts `env!("CARGO_PKG_VERSION")` instead of `"0.1.0"`                         |
| `nix/pkgs/local-git-branch-cleanup/tui.nix`                   | Replace hard-coded `version = "0.2.0"` with `craneLib.crateNameFromCargoToml` so Nix follows Cargo  |
| `LICENSE`                                                     | Add MIT license text (repo root)                                                                    |
| `rust/dist-workspace.toml`                                    | Generated by `dist init` (section 7)                                                                |
| `.github/workflows/release.yml`                               | Generated by `dist`; never hand-edited                                                              |
| `.github/workflows/ci.yml`                                    | Existing; add `test-native` job and commit-lint step (section 8)                                    |
| `.github/dependabot.yml`                                      | Exclude generated `release.yml` from `github-actions` updates (section 7)                           |
| `.github/workflows/auto-release.yml`                          | New (section 6)                                                                                     |
| `.github/workflows/smoke-test.yml`                            | New (section 9)                                                                                     |
| `cliff.toml`                                                  | git-cliff config: conventional parsers, `[bump]` rules, changelog template with requirements footer |
| `scripts/release/next-version.sh`                             | Version calculation + bump + changelog; `--dry-run` mode                                            |
| `rust/local-git-branch-cleanup-tui/CHANGELOG.md`              | New, seeded with a hand-written `0.2.0` entry                                                       |
| `rust/local-git-branch-cleanup-tui/README.md`                 | Installation section reordered (section 11)                                                         |
| `README.md` (root)                                            | Drop "v0.2.0"; say "Prebuilt releases for Linux, macOS and Windows" and show the one-liners         |
| `nix/shells/pre-commit.nix`                                   | Add commit-msg conventional-commit hook                                                             |
| `Justfile`                                                    | Release recipes from section 6.6                                                                    |
| `nix/shells/default.nix`                                      | Add pinned `dist` and `git-cliff` to the dev shell                                                  |

Must not be committed: `target/`, archives, binaries, App private key, any token.

## 11. User documentation

`rust/local-git-branch-cleanup-tui/README.md`, `## Installation` becomes:

1. **Install with one command** (recommended): the two one-liners from section 1, the install
   location (`~/.local/bin`), and how to uninstall (delete the binary).
2. **Download manually:** link to Releases, a table of the five archives, checksum verification
   (`sha256sum -c`, `shasum -a 256`, `Get-FileHash -Algorithm SHA256`).
3. **Requirements:** Git on `PATH`. Optional: `gh` for `--github`, `BITBUCKET_TOKEN` for
   `--bitbucket`.
4. **Known limitations:** unsigned binaries. On macOS, binaries installed with the installer or
   `curl` carry no quarantine flag, but a browser download may need `xattr -d com.apple.quarantine`.
   On Windows, SmartScreen may warn about a browser-downloaded `.exe`.
5. **Build from source (Cargo)** and **Nix**: existing instructions, unchanged, moved below.

Add a short **Maintainers: how releases work** section: merge → automatic release; how to force a
version (edit `Cargo.toml` in the PR, or `just release major`); how to experiment with
`just release-plan` / `just release-pre`; commits must be conventional; never re-upload assets of a
stable release.

## 12. Implementation phases

Each phase is one PR, merged in order. Nothing releases until phase D merges.

**Phase A: metadata hygiene.** Explicit `0.2.0` version, `repository` metadata, `LICENSE`, version
test via `CARGO_PKG_VERSION`, Nix version from Cargo, root README de-versioned, seed `CHANGELOG.md`.
Exit: `cargo test` green; `--version` prints `0.2.0`; `nix build` still works.

**Phase B: cross-platform confidence.** Extend the existing `ci.yml` with the `test-native` matrix
and the commit-msg lint step, and add the local commit-msg hook (section 8). Fix whatever
macOS/Windows/ARM test runs uncover. Exit: all four test runners green on the PR.

**Phase C: dist packaging (no publishing yet).** `dist init` with section 7 config, minus
`post-announce-jobs`. Run `dist plan` and confirm all five targets plus both installers are listed.
Temporarily set `pr-run-mode = "upload"` on the PR to build every target in CI and download the
artifacts; check musl builds and archive contents. Revert to `plan` before merge. Exit: every target
builds; archives contain only the binary, `README.md` and `LICENSE`.

**Phase D: auto-release + smoke tests.** Create the GitHub App and secrets (manual, repo owner). Add
`cliff.toml`, `scripts/release/next-version.sh`, `auto-release.yml` (push and dispatch triggers),
`smoke-test.yml`, the section 6.6 `just` recipes, and `dist`/`git-cliff` in the dev shell; wire
`post-announce-jobs`. Before merging, rehearse on the PR branch: `just release-plan`, then
`just release-pre rc` and check that the prerelease has all assets, the tag-pinned installers work,
and smoke tests pass; delete it with `just release-pre-delete`. Exit: merging this PR triggers the
bootstrap path → tag `local-git-branch-cleanup-tui-v0.2.0` → GitHub Release `0.2.0` with all assets
→ smoke tests green on all runners.

**Phase E: docs.** README installation rewrite (section 11) and maintainer section. Merging it
should **not** release, because the docs paths are excluded. That doubles as a test of the path
filter.

**Phase F: verify release-on-merge.** Merge a trivial `fix:` PR and observe `0.2.1` released
end-to-end; then a `feat:` PR and observe `0.3.0`; then a `test:`-only PR touching the app and
observe that no release is made. Run both one-liners on a clean machine or VM per OS.

## 13. Acceptance criteria

- [ ] `Cargo.toml`, `Cargo.lock`, `tui.nix` (derived), `--version` and the latest tag agree.
- [ ] No test hard-codes a version.
- [ ] Existing `ci.yml` jobs still pass; new `test-native` runs tests on Linux ARM, macOS and
      Windows; commit lint runs on PRs.
- [ ] Dependabot does not modify the generated `release.yml`.
- [ ] `dist plan` runs on every PR; a PR cannot publish a release.
- [ ] Merging an app-affecting PR produces, without human action: version bump commit, changelog
      entry, tag `local-git-branch-cleanup-tui-vX.Y.Z`, and a GitHub Release.
- [ ] Merging a docs-only or non-app PR does not release.
- [ ] Merging a PR whose commits are only `docs`/`test`/`chore`/`ci`/`style` does not release, even
      if it touches app source.
- [ ] Release commit does not trigger another release (no loop).
- [ ] `just release-plan` prints the next version and changelog with no side effects.
- [ ] `just release-pre` publishes a GitHub prerelease from a non-`main` branch without changing any
      branch, and `/releases/latest` still points at the last stable release.
- [ ] `just release-pre-delete` removes a prerelease and its tag; it refuses stable versions.
- [ ] `just release` produces a stable release from `main`; dispatching a stable release from
      another branch is refused.
- [ ] Release contains 5 archives, per-archive `.sha256`, `sha256.sum`, `installer.sh`,
      `installer.ps1`, `dist-manifest.json`.
- [ ] Shell one-liner installs a working binary on Linux x64, Linux ARM and macOS (both arches).
- [ ] PowerShell one-liner installs a working binary on Windows x64.
- [ ] Smoke tests pass for every target; failures open an issue.
- [ ] Binary installs to `~/.local/bin` (or XDG equivalent), not `~/.cargo/bin`.
- [ ] README makes the one-liners the primary install path; Cargo and Nix instructions remain.
- [ ] No binaries, archives or credentials in git history.

## 14. Follow-ups (not blocking)

1. `github-attestations = true` (build provenance), with `gh attestation verify` documented.
2. Homebrew tap via `dist`'s `homebrew` installer (needs a `homebrew-tap` repo + token).
3. WinGet / Scoop manifests.
4. macOS signing + notarization; Windows Authenticode signing (`dist` has `ssldotcom` support).
5. `install-updater = true` for a `local-git-branch-cleanup-tui-update` command.
6. `cargo binstall` metadata so `cargo binstall local-git-branch-cleanup-tui` pulls these archives.
7. Tool rename and repo re-layout (planned). See section 16.
8. Windows ARM64 (`aarch64-pc-windows-msvc`) via a custom build job, once `dist` or GitHub runners
   make it straightforward.
9. Decide the monorepo `latest` strategy (section 7.1) before a second tool ships releases.

## 15. Open questions for the maintainer

1. Is a GitHub App acceptable, or should the `dispatch-releases` fallback (no extra secret) be used?
2. ~~Release on `docs`/`test`/`chore`-only merges?~~ **Decided 2026-10-09: no.** Only releasable
   commit types ship a version (section 6.2).
3. ~~Shorter binary name?~~ **Decided 2026-10-09:** keep `local-git-branch-cleanup-tui` for now. The
   tool and the repo layout will be renamed later (section 16).

## 16. Designing for the upcoming rename

The tool name and repository layout will change soon. Until then everything ships as
`local-git-branch-cleanup-tui`. The pipeline should make that rename a small, mechanical change:

- **One source for the name and path.** Workflows define the package name and its directory once
  (workflow-level `env: APP: local-git-branch-cleanup-tui`,
  `APP_DIR: rust/local-git-branch-cleanup-tui`), `scripts/release/next-version.sh` takes them as
  arguments or env vars, and the `Justfile` uses the `app :=` variable. Do not hard-code the name in
  scripts. The exceptions are `on.push.paths`, which cannot read variables, and the generated
  `release.yml`.
- **What a rename touches:** Cargo package/binary name, `dist` config (then `dist init` to
  regenerate `release.yml`), the `APP`/`APP_DIR` values, the `paths` filters, `cliff.toml` tag
  pattern, the README one-liners, and the Nix package.
- **Tag continuity.** New tags will be `<new-name>-vX.Y.Z`. Before the first release under the new
  name, create a tag `<new-name>-v<last version>` on the commit of the last old-name release so
  version calculation continues from it instead of hitting the bootstrap rule. Keep the old tags and
  releases.
- **Old install URL.** `/releases/latest/download/local-git-branch-cleanup-tui-installer.sh` will
  404 once the latest release has the new name. Either upload a final old-name installer that
  installs the new tool, or document the new one-liner and accept the break. Decide this when the
  rename happens.
- **Repo move.** If the tool moves to its own repository, GitHub redirects the old repo's
  `releases/...` URLs only if the repository itself is renamed or transferred. New-repo releases
  start from the same version (carry the changelog over).

## References

- dist book: config reference and workspace tag formats,
  <https://axodotdev.github.io/cargo-dist/book/reference/config.html>,
  <https://axodotdev.github.io/cargo-dist/book/workspaces/workspace-guide.html>
- dist releases (v0.33.0, 2026-09-10): <https://github.com/axodotdev/cargo-dist/releases>
- GitHub: `GITHUB_TOKEN` events do not trigger new workflow runs (exceptions: `workflow_dispatch`,
  `repository_dispatch`): <https://docs.github.com/en/actions/concepts/security/github_token>
- release-plz release-on-merge flow (evaluated, not chosen):
  <https://dev.to/marcoieni/release-plz-release-rust-packages-from-ci-1e49>
- release-plz + cargo-dist integration notes: <https://blog.orhun.dev/automated-rust-releases/>
- git-cliff: <https://github.com/orhun/git-cliff>
