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

2. **Release on every merge.** Each PR merged to `main` that touches the application produces a new
   version, a git tag, a changelog entry and a GitHub Release with prebuilt binaries, with no human
   step after the merge.
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
- **No CI at all:** there is no `.github/workflows/` directory. Dependabot is active (open PRs #37,
  #38, #39).
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
PR opened ──► ci.yml
              ├─ quality   (ubuntu): fmt, clippy, commit-message lint
              ├─ test      (ubuntu, macos, windows): cargo test
              └─ release.yml (pr-run-mode = plan): dist plan

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
          --tag-pattern '^local-git-branch-cleanup-tui-v' \
          --bumped-version
```

Rules (configured in `cliff.toml` `[bump]`):

| Commits since last tag                             | Bump while `0.x` | Bump once `>= 1.0` |
| -------------------------------------------------- | ---------------- | ------------------ |
| any `feat!:`, `fix!:` or `BREAKING CHANGE:` footer | minor            | major              |
| any `feat:`                                        | minor            | minor              |
| anything else (`fix`, `perf`, `refactor`, `build`) | patch            | patch              |

- "Anything else" must still bump a patch. git-cliff's auto mode returns the current version when
  there is no `feat`/`fix`, so the script falls back to `git cliff --bump patch`. Every
  app-affecting merge ships.
- **Bootstrap / manual override:** if no tag `local-git-branch-cleanup-tui-v<Cargo version>` exists
  yet, release the Cargo version **as is** and skip calculation. This makes the very first release
  `0.2.0` (set by hand in the implementation PR), and lets a maintainer force a version such as
  `1.0.0` by editing `Cargo.toml` in a PR.
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
  `actions/create-github-app-token` (pinned by SHA).
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

- `ci.yml` `quality` job lints every commit in the PR range (`base..head`) against
  `^(feat|fix|perf|refactor|build|ci|docs|test|chore|style|revert)(\([a-z0-9-]+\))?!?: .+`. Fail
  with a message naming the offending commit.
- Add the same check as a `commit-msg` hook through `git-hooks.nix` so it fails locally first.
- Dependabot already uses `build(deps): ...`, which passes.

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
  `release.yml`; change config and rerun `dist init` / `dist generate`. Do not set `allow-dirty`.
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

## 8. CI (`.github/workflows/ci.yml`)

No CI exists today; this adds it. Runs on `pull_request` and `push: main`.

| Job       | Runners                                                               | Steps                                                                                                                 |
| --------- | --------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `quality` | `ubuntu-latest`                                                       | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, commit-msg lint |
| `test`    | `ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest`, `windows-latest` | `cargo test --workspace --all-features` (needs `git` configured with `user.name`/`user.email` for test repos)         |
| `plan`    | (from `dist`'s `release.yml`, `pr-run-mode = plan`)                   | `dist plan`                                                                                                           |

- CI uses `dtolnay/rust-toolchain` + `Swatinem/rust-cache`, **not** Nix. Nix only targets
  `x86_64-linux`, and the release binaries are built without Nix anyway. A separate optional
  `nix build` job can stay Linux-only.
- Pin third-party actions by commit SHA. Workflow-level `permissions: contents: read`.
- Windows will likely expose path, line-ending or shell assumptions in tests. Fix the code or tests;
  do not mark tests `#[cfg(unix)]` to get a green matrix unless the behavior really is Unix-only.

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
| `.github/workflows/ci.yml`                                    | New (section 8)                                                                                     |
| `.github/workflows/auto-release.yml`                          | New (section 6)                                                                                     |
| `.github/workflows/smoke-test.yml`                            | New (section 9)                                                                                     |
| `cliff.toml`                                                  | git-cliff config: conventional parsers, `[bump]` rules, changelog template with requirements footer |
| `scripts/release/next-version.sh`                             | Version calculation + bump + changelog; `--dry-run` mode                                            |
| `rust/local-git-branch-cleanup-tui/CHANGELOG.md`              | New, seeded with a hand-written `0.2.0` entry                                                       |
| `rust/local-git-branch-cleanup-tui/README.md`                 | Installation section reordered (section 11)                                                         |
| `README.md` (root)                                            | Drop "v0.2.0"; say "Prebuilt releases for Linux, macOS and Windows" and show the one-liners         |
| `nix/shells/pre-commit.nix`                                   | Add commit-msg conventional-commit hook                                                             |
| `Justfile`                                                    | `release-dry-run` recipe → `scripts/release/next-version.sh --dry-run` and `dist plan`              |

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
version (edit `Cargo.toml` in the PR); commits must be conventional; never re-upload assets.

## 12. Implementation phases

Each phase is one PR, merged in order. Nothing releases until phase D merges.

**Phase A: metadata hygiene.** Explicit `0.2.0` version, `repository` metadata, `LICENSE`, version
test via `CARGO_PKG_VERSION`, Nix version from Cargo, root README de-versioned, seed `CHANGELOG.md`.
Exit: `cargo test` green; `--version` prints `0.2.0`; `nix build` still works.

**Phase B: CI and cross-platform confidence.** Add `ci.yml` (section 8) and the commit-msg lint and
hook. Fix whatever macOS/Windows/ARM test runs uncover. Exit: all four test runners green on the PR.

**Phase C: dist packaging (no publishing yet).** `dist init` with section 7 config, minus
`post-announce-jobs`. Run `dist plan` and confirm all five targets plus both installers are listed.
Temporarily set `pr-run-mode = "upload"` on the PR to build every target in CI and download the
artifacts; check musl builds and archive contents. Revert to `plan` before merge. Exit: every target
builds; archives contain only the binary, `README.md` and `LICENSE`.

**Phase D: auto-release + smoke tests.** Create the GitHub App and secrets (manual, repo owner). Add
`cliff.toml`, `scripts/release/next-version.sh`, `auto-release.yml`, `smoke-test.yml`, wire
`post-announce-jobs`. Run `next-version.sh --dry-run` locally against a scratch tag to prove the
bump rules. Exit: merging this PR triggers the bootstrap path → tag
`local-git-branch-cleanup-tui-v0.2.0` → GitHub Release `0.2.0` with all assets → smoke tests green
on all runners.

**Phase E: docs.** README installation rewrite (section 11) and maintainer section. Merging it
should **not** release, because the docs paths are excluded. That doubles as a test of the path
filter.

**Phase F: verify release-on-merge.** Merge a trivial `fix:` PR and observe `0.2.1` released
end-to-end; then a `feat:` PR and observe `0.3.0`. Run both one-liners on a clean machine or VM per
OS.

## 13. Acceptance criteria

- [ ] `Cargo.toml`, `Cargo.lock`, `tui.nix` (derived), `--version` and the latest tag agree.
- [ ] No test hard-codes a version.
- [ ] `ci.yml` runs fmt, clippy, commit lint, and tests on Linux x64, Linux ARM, macOS and Windows.
- [ ] `dist plan` runs on every PR; a PR cannot publish a release.
- [ ] Merging an app-affecting PR produces, without human action: version bump commit, changelog
      entry, tag `local-git-branch-cleanup-tui-vX.Y.Z`, and a GitHub Release.
- [ ] Merging a docs-only or non-app PR does not release.
- [ ] Release commit does not trigger another release (no loop).
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
7. A short binary alias (e.g. `lgbc`).
8. Windows ARM64 (`aarch64-pc-windows-msvc`) via a custom build job, once `dist` or GitHub runners
   make it straightforward.
9. Decide the monorepo `latest` strategy (section 7.1) before a second tool ships releases.

## 15. Open questions for the maintainer

1. Is a GitHub App acceptable, or should the `dispatch-releases` fallback (no extra secret) be used?
2. Should a `docs:`/`test:`/`chore:`-only merge that touches the app's source paths still ship a
   patch release, or be skipped? The spec currently says **ship** (every merge releases).
3. Keep the long binary name for the one-liners, or add a short alias in Phase A?

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
