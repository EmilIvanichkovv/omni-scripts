#!/usr/bin/env bash
# Publish the release decided by next-version.sh: commit the version bump, tag it and push. The
# tag push starts .github/workflows/release.yml (dist), which builds and publishes the release.
#
# Stable releases push the release commit and tag to main atomically, retrying when main moved
# meanwhile. Prereleases push only the tag: their commit is never added to a branch.
#
# Usage: publish.sh [next-version.sh options]   (run from a checkout with full history and tags)
# Writes release, version and tag to $GITHUB_OUTPUT when set.
set -euo pipefail

APP=${APP:-local-git-branch-cleanup-tui}
export APP

prerelease=false
for arg in "$@"; do
    [[ "$arg" == --prerelease ]] && prerelease=true
done
[[ " $* " == *" --dry-run "* ]] && {
    echo "use next-version.sh --dry-run for a dry run" >&2
    exit 2
}

cd "$(git rev-parse --show-toplevel)"
here=$(dirname "$0")

for attempt in 1 2 3; do
    if ((attempt > 1)); then
        echo "main moved during the release, retrying ($attempt/3)"
        git fetch -q origin main
        git reset -q --hard origin/main
    fi

    out=$("$here/next-version.sh" "$@")
    echo "$out"
    release=$(sed -n 's/^release=//p' <<<"$out")
    version=$(sed -n 's/^version=//p' <<<"$out")
    tag=$(sed -n 's/^tag=//p' <<<"$out")
    [[ -n "${GITHUB_OUTPUT:-}" ]] &&
        printf 'release=%s\nversion=%s\ntag=%s\n' "$release" "$version" "$tag" >>"$GITHUB_OUTPUT"
    [[ "$release" == true ]] || exit 0

    # A prerelease commit lives only under its tag, never on a branch.
    if $prerelease; then
        git checkout -q --detach
    fi
    # Nothing to commit when the Cargo.toml version and changelog entry are already in place
    # (first release, or a version set by hand in a PR): tag the current commit.
    if ! git diff --quiet; then
        git commit -q -a -m "chore(release): $APP v$version"
    fi
    git tag -a "$tag" -m "$APP $version"

    if $prerelease; then
        git push -q origin "refs/tags/$tag"
        echo "pushed prerelease tag $tag"
        exit 0
    fi
    if git push -q --atomic origin HEAD:refs/heads/main "refs/tags/$tag"; then
        echo "pushed release commit and tag $tag to main"
        exit 0
    fi
    git tag -d "$tag" >/dev/null
done

echo "could not push the release after 3 attempts" >&2
exit 1
