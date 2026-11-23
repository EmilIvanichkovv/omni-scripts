# Git Manager migration specification

## Purpose and scope

Make Git Manager the repository's primary product: a terminal application for managing local Git
branches and worktrees. Rename the Rust package and executable from `local-git-branch-cleanup-tui`
to `git-manager` without changing its branch safety rules, CLI options, supported platforms, or
integrations. The Bash implementation remains a legacy tool, not a second product. Repository
administration, including renaming the GitHub repository, is a separate post-merge action.

## Target layout

The root contains the Cargo package, lockfile, `src/`, `tests/`, product README and changelog.
Product documentation lives under `docs/`, release and development automation under `scripts/`, and
Nix and GitHub configuration remain in their current top-level directories. Keep the existing Bash
script and helpers together, so its established paths and Nix package continue working. Remove
`omni-lib`: it is a placeholder and is not a dependency of the application.

## Product and interface

- The package, executable, CLI help, distribution assets and primary Nix output are `git-manager`.
  The Nix default resolves to the same package; retain the old TUI Nix output as an alias.
- Maintain the current CLI options and behavior. Describe existing branch, worktree and PR features
  accurately; the new name does not promise functionality that has not been implemented.
- Promote the product README and changelog to the root. Update active guides and preserve historical
  design specifications as dated snapshots; their original names and paths may no longer apply.
- Keep the Bash cleanup script and its Nix output as legacy functionality with existing behavior.

## Existing installations and local state

Existing released executables look for tags prefixed `local-git-branch-cleanup-tui-v` and use an
installer receipt keyed to that name. They cannot be assumed to discover or install a `git-manager`
release. Document a manual migration: install Git Manager using its new installer, verify it runs,
and remove the old executable separately if desired. Old releases and their tags remain untouched.
The first renamed release must not claim that `update` upgrades an old binary.

Retain the existing `omni-scripts` cache directory and PR cache to avoid losing state or introducing
concurrent cache migration. The renamed application uses its own update-notice cache file in the
same directory. Retain the existing token and update-notice environment variable names for
compatibility. The installer's host override names follow the new package name. New installs and
updates use the new package name and installer receipt.

## Versioning and distribution

- Preserve the changelog and existing tags. The first new-name stable release is `0.10.0`, following
  the current `0.9.0`; its `git-manager-v0.10.0` tag is a new release identity, not a rewritten tag.
  Add a hand-written changelog entry so the first release does not regenerate the entire history.
- Update automatic release path filters, Cargo lockfile handling and package paths. Verify that
  release planning bootstraps the first new-name tag and later releases calculate from it.
- Update dist workspace discovery, Nix source filtering, CI, Dependabot, audit, formatting and tests
  for the root Cargo layout. Preserve all five release targets and installer channels.
- The smoke test must not mistake an old-name release for an upgradeable `git-manager` release. It
  continues to test the previous release once two releases with the new name exist.

## Repository rename

The PR does not rename the live GitHub repository. Until a maintainer performs that action, use the
working `EmilIvanichkovv/omni-scripts` repository URL for releases, Cargo metadata and updater
requests. After merging and verifying the first release, rename the repository in GitHub settings,
update canonical URLs and updater constants in a follow-up, and verify redirects, installers,
release feeds, API access, GitHub App permissions, branch rules and external integrations. Do not
rely on redirects as a substitute for testing old binaries.

## Acceptance checks

1. Root Cargo formatting, workspace tests and builds pass; existing branch safety tests remain
   intact.
2. Nix evaluates/builds the new default, new output, legacy alias and Bash output where available.
3. Dist plans the new package and release automation identifies `git-manager-v0.10.0` as its first
   release; later version calculations use new-name tags.
4. Installation and update instructions distinguish existing users from fresh installs, and all
   active links and commands refer to the new layout and actual repository URL.
5. Cross-platform release and updater smoke tests cover the new name and retain their existing
   platform matrix. GitHub-hosted tests and publication require the PR/merge workflows.
