#!/usr/bin/env bash
# Decide a tool's next release from the commits since its last stable tag, and write the new
# version into the tree (Cargo.toml, Cargo.lock, CHANGELOG.md). Committing, tagging and pushing
# are done by scripts/release/publish.sh. Rules: docs/specs/RELEASE_DISTRIBUTION.md, section 6.
#
# Usage: next-version.sh [--bump auto|patch|minor|major] [--prerelease LABEL] [--dry-run]
#   --bump        override the bump computed from commit types (auto)
#   --prerelease  publish X.Y.Z-LABEL.N instead of a stable version
#   --dry-run     print the decision and the changelog entry, change nothing
#
# Env: APP (Cargo package and binary name), APP_DIR (its directory).
# Prints key=value lines on stdout: release, version, tag, previous, reason. The changelog entry
# goes to stderr. Exits 0 with release=false when there is nothing to release.
set -euo pipefail

APP=${APP:-local-git-branch-cleanup-tui}
APP_DIR=${APP_DIR:-rust/local-git-branch-cleanup-tui}

# Commit types that change the shipped binary. Keep in sync with commit_parsers in cliff.toml.
RELEASABLE='feat|fix|perf|refactor|build|revert'

bump=auto
prerelease=""
dry_run=false
while [[ $# -gt 0 ]]; do
    case "$1" in
    --bump)
        bump=$2
        shift 2
        ;;
    --prerelease)
        prerelease=$2
        shift 2
        ;;
    --dry-run)
        dry_run=true
        shift
        ;;
    *)
        echo "unknown argument: $1" >&2
        exit 2
        ;;
    esac
done
case "$bump" in auto | patch | minor | major) ;; *)
    echo "--bump must be auto, patch, minor or major" >&2
    exit 2
    ;;
esac
if [[ -n "$prerelease" && ! "$prerelease" =~ ^[a-z]+$ ]]; then
    echo "--prerelease must be lowercase letters, e.g. rc or beta" >&2
    exit 2
fi

cd "$(git rev-parse --show-toplevel)"

cargo_toml="$APP_DIR/Cargo.toml"
changelog="$APP_DIR/CHANGELOG.md"
# Paths whose changes can affect the shipped binary.
paths=("$APP_DIR" rust/Cargo.toml rust/Cargo.lock dist-workspace.toml rust-toolchain.toml
    ":(exclude)$APP_DIR/docs" ":(exclude)$changelog")

current=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$cargo_toml" | head -n 1)
[[ "$current" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
    echo "$cargo_toml: version '$current' is not X.Y.Z" >&2
    exit 1
}
previous=$(git tag --list "$APP-v*" --merged HEAD |
    grep -E "^$APP-v[0-9]+\.[0-9]+\.[0-9]+$" | sort -V | tail -n 1 || true)

finish() { # release version reason
    local tag=""
    [[ "$1" == true ]] && tag="$APP-v$2"
    printf 'release=%s\nversion=%s\ntag=%s\nprevious=%s\nreason=%s\n' \
        "$1" "$2" "$tag" "$previous" "$3"
}

# Highest bump the commits ask for: 0 none, 1 patch, 2 minor, 3 major.
commit_level() {
    local level=0 sha subject
    while read -r sha; do
        [[ -n "$sha" ]] || continue
        subject=$(git log -1 --format=%s "$sha")
        if [[ "$subject" =~ ^[a-z]+(\([a-z0-9-]+\))?!: ]] ||
            git log -1 --format=%b "$sha" | grep -qE '^BREAKING[ -]CHANGE:'; then
            level=3
        elif [[ "$subject" =~ ^feat(\([a-z0-9-]+\))?: ]]; then
            level=$((level > 2 ? level : 2))
        elif [[ "$subject" =~ ^($RELEASABLE)(\([a-z0-9-]+\))?: ]]; then
            level=$((level > 1 ? level : 1))
        fi
    done < <(git rev-list --no-merges "$1" -- "${paths[@]}")
    echo "$level"
}

if ! git rev-parse -q --verify "refs/tags/$APP-v$current" >/dev/null; then
    # First release of this version (bootstrap, or a version set by hand in a PR): ship it as is.
    next=$current
    reason="no tag for $current yet, releasing Cargo.toml version"
else
    [[ -n "$previous" ]] || {
        echo "tag $APP-v$current exists but is not an ancestor of HEAD" >&2
        exit 1
    }
    # Count from the last stable tag, not Cargo.toml, in case the two ever disagree.
    IFS=. read -r major minor patch <<<"${previous##*-v}"
    case "$bump" in
    patch) level=1 ;;
    minor) level=2 ;;
    major) level=3 ;;
    auto)
        level=$(commit_level "$previous..HEAD")
        # Before 1.0, breaking changes bump the minor version.
        if ((major == 0 && level == 3)); then level=2; fi
        ;;
    esac
    # An experiment should always be possible, even with only docs/test/chore commits.
    if [[ -n "$prerelease" ]] && ((level == 0)); then level=1; fi
    if ((level == 0)); then
        finish false "$current" "no releasable commits since $previous ($RELEASABLE)"
        exit 0
    fi
    case "$level" in
    1) next="$major.$minor.$((patch + 1))" ;;
    2) next="$major.$((minor + 1)).0" ;;
    3) next="$((major + 1)).0.0" ;;
    esac
    reason="bump=$bump level=$level since $previous"
fi

version=$next
if [[ -n "$prerelease" ]]; then
    n=$(git tag --list "$APP-v$next-$prerelease.*" | sed -n "s/^.*-$prerelease\.\([0-9]*\)$/\1/p" |
        sort -n | tail -n 1)
    version="$next-$prerelease.$((${n:-0} + 1))"
fi

# Changelog entry: keep a hand-written one, otherwise render the commits since the last release.
entry=""
if ! grep -qE "^## \[?$version\]?( |$)" "$changelog"; then
    range=${previous:+$previous..}HEAD
    include=()
    for p in "${paths[@]}"; do
        [[ "$p" == :* ]] || include+=(--include-path "$p" --include-path "$p/**")
    done
    entry=$(git cliff --config cliff.toml "${include[@]}" --exclude-path "$APP_DIR/docs/**" \
        --tag "$APP-v$version" --strip all "$range")
    [[ "$entry" == *"###"* ]] || entry+=$'\n\nNo user-facing changes.'
fi

if $dry_run; then
    [[ -n "$entry" ]] && printf '%s\n' "$entry" >&2
    finish true "$version" "$reason (dry run)"
    exit 0
fi

if [[ "$version" != "$current" ]]; then
    sed -i "0,/^version = \".*\"$/s//version = \"$version\"/" "$cargo_toml"
    # The package's own entry in Cargo.lock: the version line right after its name.
    awk -v app="$APP" -v v="$version" '
        $0 == "name = \"" app "\"" { hit = 1; print; next }
        hit && /^version = / { print "version = \"" v "\""; hit = 0; next }
        { hit = 0; print }' rust/Cargo.lock >rust/Cargo.lock.tmp
    mv rust/Cargo.lock.tmp rust/Cargo.lock
fi
if [[ -n "$entry" ]]; then
    # Insert above the newest entry, below the file's title and intro.
    ENTRY=$entry awk '
        !done && /^## / { print ENVIRON["ENTRY"]; print ""; done = 1 }
        { print }
        END { if (!done) { print ""; print ENVIRON["ENTRY"] } }' "$changelog" >"$changelog.tmp"
    mv "$changelog.tmp" "$changelog"
fi

finish true "$version" "$reason"
